//! Signing in to remote MCP servers that require OAuth. rmcp does the protocol (metadata discovery,
//! client registration, PKCE, token exchange and refresh); Drift keeps the tokens in the keychain under
//! `mcp:<server>` and serves the one-shot loopback callback the browser returns to.

use std::sync::Arc;
use std::time::Duration;

use rmcp::transport::auth::{AuthClient, AuthError, AuthorizationManager, AuthorizationRequest, CredentialStore, OAuthClientConfig, OAuthState, StoredCredentials};

use super::{OAuthClient, ServerConfig};
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

/// The sign-in kept for a server, ready to refresh; `None` when it never signed in.
async fn signed_in(credentials: &Arc<Credentials>, server: &str, url: &str, app: Option<&OAuthClient>) -> Option<AuthorizationManager> {
    credentials.secret(&key(server))?;
    let mut manager = AuthorizationManager::new(url).await.ok()?;
    manager.with_client(crate::llm::http::client()).ok()?;
    manager.set_credential_store(KeychainStore::new(credentials.clone(), server));
    if !manager.initialize_from_store().await.ok()? {
        return None;
    }
    // The store keeps the app's id but not its secret, which a confidential app needs to refresh.
    if let Some(OAuthClient { client_id, client_secret: Some(secret), scopes }) = app {
        let config = OAuthClientConfig::new(client_id, "http://127.0.0.1/callback").with_client_secret(secret).with_scopes(scopes.clone());
        manager.configure_client(config).ok()?;
    }
    Some(manager)
}

/// A client for a server signed in before, refreshing its token as needed; `None` when it never was.
pub async fn signed_in_client(credentials: &Arc<Credentials>, server: &str, url: &str, app: Option<&OAuthClient>) -> Option<AuthClient<reqwest::Client>> {
    signed_in(credentials, server, url, app).await.map(|manager| AuthClient::new(crate::llm::http::client(), manager))
}

/// A signed-in server's access token, refreshed first when it is due.
pub async fn signed_in_token(credentials: &Arc<Credentials>, server: &str, url: &str, app: Option<&OAuthClient>) -> Option<String> {
    signed_in(credentials, server, url, app).await?.get_access_token().await.ok()
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
    if let Some(kept) = credentials.secret(&key(from))
        && credentials.set_secret(&key(to), &kept).is_ok() {
            let _ = forget(credentials, from);
        }
}

/// Forgets a sign-in when a save points the server at another URL or app, so its tokens never reach a different host or client.
pub fn forget_if_moved(credentials: &Credentials, server: &str, before: &ServerConfig, after: &ServerConfig) {
    let identity = |config: &ServerConfig| config.remote().map(|(url, app)| (url.to_string(), app.map(|app| app.client_id.clone())));
    if identity(before) != identity(after) {
        let _ = forget(credentials, server);
    }
}

impl crate::Engine {
    /// Starts signing in to a remote server: returns the page to open in the browser. When the browser
    /// comes back the tokens are stored and the server connects.
    pub async fn sign_in_mcp(self: &Arc<Self>, name: &str) -> Result<String, String> {
        let row = self.store.mcp_server(name).map_err(|e| e.to_string())?.ok_or_else(|| format!("no MCP server named {name}"))?;
        let Some((url, app)) = row.config.remote() else { return Err("a server on stdio has no sign-in".into()) };
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.map_err(|e| e.to_string())?;
        let redirect = format!("http://127.0.0.1:{}/callback", listener.local_addr().map_err(|e| e.to_string())?.port());
        let mut manager = AuthorizationManager::new(url).await.map_err(|e| e.to_string())?;
        manager.with_client(crate::llm::http::client()).map_err(|e| e.to_string())?;
        manager.set_credential_store(KeychainStore::new(self.credentials.clone(), name));
        let mut state = OAuthState::Unauthorized(manager);
        state.start_authorization(request(redirect, app)).await.map_err(|e| format!("could not start signing in to {name}: {e}"))?;
        let page = state.get_authorization_url().await.map_err(|e| e.to_string())?;
        let engine = Arc::downgrade(self);
        let server = name.to_string();
        tokio::spawn(async move {
            let finished = tokio::time::timeout(SIGN_IN_WAIT, finish(&listener, &mut state)).await.unwrap_or_else(|_| Err(format!("no answer from the browser within {} minutes", SIGN_IN_WAIT.as_secs() / 60)));
            let Some(engine) = engine.upgrade() else { return };
            match finished {
                Ok(()) => drop(engine.connect_mcp(&server).await),
                Err(why) => engine.mcp.sign_in_failed(&server, &engine.store, &engine.hub, &why),
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

/// The sign-in to ask for: Drift registering itself, or the app the config names.
fn request(redirect: String, app: Option<&OAuthClient>) -> AuthorizationRequest {
    let request = AuthorizationRequest::new(redirect).with_client_name("Drift");
    let Some(app) = app else { return request };
    let request = request.with_preregistered_client(&app.client_id).with_scopes(app.scopes.clone());
    match &app.client_secret {
        Some(secret) => request.with_client_secret(secret),
        None => request,
    }
}

/// Waits for the browser's return and trades its code for tokens, which the store keeps.
async fn finish(listener: &tokio::net::TcpListener, state: &mut OAuthState) -> Result<(), String> {
    let (mut socket, params) = crate::llm::openai::oauth::next_callback(listener, "/callback").await?;
    let (Some(code), Some(csrf)) = (params.get("code"), params.get("state")) else {
        // The authorization server's own reason, when it gave one (`access_denied` and its description).
        let why = match (params.get("error"), params.get("error_description")) {
            (Some(error), Some(description)) => format!("{error}: {description}"),
            (Some(error), None) => error.clone(),
            _ => "the server sent no code".into(),
        };
        crate::llm::openai::oauth::respond(&mut socket, 400, &format!("Sign-in failed: {why}")).await;
        return Err(why);
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
