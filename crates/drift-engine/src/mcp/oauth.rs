//! Signing in to remote MCP servers that require OAuth. rmcp does the protocol (metadata discovery,
//! client registration, PKCE, token exchange and refresh); Drift keeps the tokens in the keychain under
//! `mcp:<server>` and serves the one-shot loopback callback the browser returns to.

use std::sync::Arc;
use std::time::Duration;

use rmcp::transport::auth::{AuthClient, AuthError, AuthorizationManager, AuthorizationRequest, CredentialStore, OAuthState, StoredCredentials};

use super::ServerConfig;
use crate::llm::credentials::Credentials;

/// How long a sign-in waits for the browser before giving up.
const SIGN_IN_WAIT: Duration = Duration::from_secs(10 * 60);

/// One server's sign-in in the keychain, as rmcp reads, saves and refreshes it.
pub struct KeychainStore {
    credentials: Arc<Credentials>,
    key: String,
}

impl KeychainStore {
    pub fn new(credentials: Arc<Credentials>, server: &str) -> Self {
        Self { credentials, key: key(server) }
    }
}

fn key(server: &str) -> String {
    format!("mcp:{server}")
}

fn store_error(error: String) -> AuthError {
    AuthError::InternalError(error)
}

#[async_trait::async_trait]
impl CredentialStore for KeychainStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        Ok(self.credentials.secret(&self.key).and_then(|json| serde_json::from_str(&json).ok()))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let json = serde_json::to_string(&credentials).map_err(|e| store_error(e.to_string()))?;
        self.credentials.set_secret(&self.key, &json).map_err(store_error)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.credentials.remove_secret(&self.key).map_err(store_error)
    }
}

/// A client for a server signed in before, refreshing its token as needed; `None` when it never was.
pub async fn signed_in_client(credentials: &Arc<Credentials>, server: &str, url: &str) -> Option<AuthClient<reqwest::Client>> {
    credentials.secret(&key(server))?;
    let mut manager = AuthorizationManager::new(url).await.ok()?;
    manager.with_client(crate::llm::http::client()).ok()?;
    manager.set_credential_store(KeychainStore::new(credentials.clone(), server));
    manager.initialize_from_store().await.ok()?.then(|| AuthClient::new(crate::llm::http::client(), manager))
}

pub fn has_sign_in(credentials: &Credentials, server: &str) -> bool {
    credentials.secret(&key(server)).is_some()
}

/// Forgets a server's sign-in.
pub fn forget(credentials: &Credentials, server: &str) -> Result<(), String> {
    credentials.remove_secret(&key(server))
}

/// Keeps a renamed server's sign-in under its new name.
pub fn move_sign_in(credentials: &Credentials, from: &str, to: &str) {
    if let Some(kept) = credentials.secret(&key(from)) {
        if credentials.set_secret(&key(to), &kept).is_ok() {
            let _ = forget(credentials, from);
        }
    }
}

/// Forgets a sign-in when a save points the server at another URL, so its tokens never reach a different host.
pub fn forget_if_moved(credentials: &Credentials, server: &str, before: &ServerConfig, after: &ServerConfig) {
    let url = |config: &ServerConfig| match config {
        ServerConfig::Http { url, .. } => Some(url.clone()),
        _ => None,
    };
    if url(before) != url(after) {
        let _ = forget(credentials, server);
    }
}

/// Whether a connect failed because the server wants the user to sign in.
pub fn wants_sign_in(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    ["auth required", "authorization required", "401", "unauthorized"].iter().any(|said| error.contains(said))
}

impl crate::Engine {
    /// Starts signing in to a remote server: returns the page to open in the browser. When the browser
    /// comes back the tokens are stored and the server connects.
    pub async fn sign_in_mcp(self: &Arc<Self>, name: &str) -> Result<String, String> {
        let row = self.store.mcp_server(name).map_err(|e| e.to_string())?.ok_or_else(|| format!("no MCP server named {name}"))?;
        let ServerConfig::Http { url, .. } = &row.config else { return Err("only servers on streamable HTTP sign in".into()) };
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.map_err(|e| e.to_string())?;
        let redirect = format!("http://127.0.0.1:{}/callback", listener.local_addr().map_err(|e| e.to_string())?.port());
        let mut manager = AuthorizationManager::new(url.as_str()).await.map_err(|e| e.to_string())?;
        manager.with_client(crate::llm::http::client()).map_err(|e| e.to_string())?;
        manager.set_credential_store(KeychainStore::new(self.credentials.clone(), name));
        let mut state = OAuthState::Unauthorized(manager);
        state.start_authorization(AuthorizationRequest::new(redirect).with_client_name("Drift")).await.map_err(|e| format!("could not start signing in to {name}: {e}"))?;
        let page = state.get_authorization_url().await.map_err(|e| e.to_string())?;
        let engine = Arc::downgrade(self);
        let server = name.to_string();
        tokio::spawn(async move {
            let finished = tokio::time::timeout(SIGN_IN_WAIT, finish(&listener, &mut state)).await;
            let Some(engine) = engine.upgrade() else { return };
            if let Ok(Ok(())) = finished {
                let _ = engine.connect_mcp(&server).await;
            }
        });
        Ok(page)
    }

    /// Forgets a server's sign-in and reconnects it without one, so it is plain what it can do signed out.
    pub async fn sign_out_mcp(self: &Arc<Self>, name: &str) -> Result<(), String> {
        forget(&self.credentials, name)?;
        self.mcp.disconnect(name, &self.store, &self.hub).await;
        let _ = self.connect_mcp(name).await;
        Ok(())
    }
}

/// Waits for the browser's return and trades its code for tokens, which the store keeps.
async fn finish(listener: &tokio::net::TcpListener, state: &mut OAuthState) -> Result<(), String> {
    let (mut socket, params) = crate::llm::openai::oauth::next_callback(listener, "/callback").await?;
    let (Some(code), Some(csrf)) = (params.get("code"), params.get("state")) else {
        crate::llm::openai::oauth::respond(&mut socket, 400, "Sign-in failed: the server sent no code.").await;
        return Err("no code".into());
    };
    match state.handle_callback(code, csrf).await {
        Ok(()) => {
            crate::llm::openai::oauth::respond(&mut socket, 200, "Drift is signed in to the MCP server. You can close this tab and go back to the app.").await;
            Ok(())
        }
        Err(error) => {
            crate::llm::openai::oauth::respond(&mut socket, 400, &format!("Sign-in failed: {error}")).await;
            Err(error.to_string())
        }
    }
}
