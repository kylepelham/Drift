//! Signing in to remote MCP servers that require OAuth. rmcp does the protocol (metadata discovery,
//! client registration, PKCE, token exchange and refresh); Drift keeps the tokens in the keychain under
//! `mcp:<server>` and serves the one-shot loopback callback the browser returns to.

use std::sync::Arc;
use std::time::Duration;

use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationManager, AuthorizationRequest, CredentialStore, OAuthClientConfig, OAuthState,
    StoredCredentials,
};

use super::{OAuthClient, ServerConfig};
use crate::llm::credentials::{CredentialError, Credentials};

/// How long a sign-in waits for the browser before giving up.
const SIGN_IN_WAIT: Duration = Duration::from_secs(10 * 60);

mod error;
pub use error::SignInError;

/// One server's sign-in in the keychain, as rmcp reads, saves and refreshes it.
pub(super) struct KeychainStore {
    credentials: Arc<Credentials>,
    key: String,
}

impl KeychainStore {
    pub(super) fn new(credentials: Arc<Credentials>, server: &str) -> Self {
        Self {
            credentials,
            key: key(server),
        }
    }
}

fn key(server: &str) -> String {
    format!("mcp:{server}")
}

fn store_error(error: impl ToString) -> AuthError {
    AuthError::InternalError(error.to_string())
}

#[async_trait::async_trait]
impl CredentialStore for KeychainStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        Ok(self
            .credentials
            .secret(&self.key)
            .and_then(|json| serde_json::from_str(&json).ok()))
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
async fn signed_in(
    credentials: &Arc<Credentials>,
    server: &str,
    url: &str,
    app: Option<&OAuthClient>,
) -> Option<AuthorizationManager> {
    credentials.secret(&key(server))?;
    let mut manager = AuthorizationManager::new(url).await.ok()?;
    manager.with_client(crate::llm::http::client()).ok()?;
    manager.set_credential_store(KeychainStore::new(credentials.clone(), server));
    if !manager.initialize_from_store().await.ok()? {
        return None;
    }

    // The store keeps the app's id but not its secret, which a confidential app needs to refresh.
    if let Some(OAuthClient {
        client_id,
        client_secret: Some(secret),
        scopes,
    }) = app
    {
        let config = OAuthClientConfig::new(client_id, "http://127.0.0.1/callback")
            .with_client_secret(secret)
            .with_scopes(scopes.clone());
        manager.configure_client(config).ok()?;
    }

    Some(manager)
}

/// A client for a server signed in before, refreshing its token as needed; `None` when it never was.
pub(super) async fn signed_in_client(
    credentials: &Arc<Credentials>,
    server: &str,
    url: &str,
    app: Option<&OAuthClient>,
) -> Option<AuthClient<reqwest::Client>> {
    signed_in(credentials, server, url, app)
        .await
        .map(|manager| AuthClient::new(crate::llm::http::client(), manager))
}

/// A signed-in server's access token, refreshed first when it is due.
pub(super) async fn signed_in_token(
    credentials: &Arc<Credentials>,
    server: &str,
    url: &str,
    app: Option<&OAuthClient>,
) -> Option<String> {
    signed_in(credentials, server, url, app)
        .await?
        .get_access_token()
        .await
        .ok()
}

pub(super) fn has_sign_in(credentials: &Credentials, server: &str) -> bool {
    credentials.secret(&key(server)).is_some()
}

/// Forgets a server's sign-in.
pub fn forget(credentials: &Credentials, server: &str) -> Result<(), CredentialError> {
    credentials.remove_secret(&key(server))
}

/// Keeps a renamed server's sign-in under its new name.
pub fn move_sign_in(credentials: &Credentials, from: &str, to: &str) {
    if let Some(kept) = credentials.secret(&key(from))
        && credentials.set_secret(&key(to), &kept).is_ok()
    {
        let _ = forget(credentials, from);
    }
}

/// Who a sign-in's tokens are for: the server's URL and the app it signs in as.
fn identity(config: &ServerConfig) -> Option<(String, Option<String>)> {
    config
        .remote()
        .map(|(url, app)| (url.to_string(), app.map(|app| app.client_id.clone())))
}

/// Forgets saved tokens when a server changes URL or app identity, keeping them away from other hosts or clients.
pub fn forget_if_moved(credentials: &Credentials, server: &str, before: &ServerConfig, after: &ServerConfig) {
    if identity(before) != identity(after) {
        let _ = forget(credentials, server);
    }
}

impl crate::Engine {
    /// Starts signing in to a remote server: returns the page to open in the browser. When the browser
    /// comes back the tokens are stored and the server connects.
    pub async fn sign_in_mcp(self: &Arc<Self>, name: &str) -> Result<String, SignInError> {
        let row = self
            .store
            .mcp_server(name)?
            .ok_or_else(|| SignInError::MissingServer(name.to_owned()))?;
        let Some((url, app)) = row.config.remote() else {
            return Err(SignInError::Stdio);
        };

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let redirect = format!("http://127.0.0.1:{}/callback", listener.local_addr()?.port());

        let mut manager = AuthorizationManager::new(url).await?;
        manager.with_client(crate::llm::http::client())?;
        manager.set_credential_store(KeychainStore::new(self.credentials.clone(), name));
        let mut state = OAuthState::Unauthorized(manager);
        state
            .start_authorization(request(redirect, app))
            .await
            .map_err(|source| SignInError::Start {
                server: name.to_owned(),
                source,
            })?;

        let page = state.get_authorization_url().await?;
        let engine = Arc::downgrade(self);
        let server = name.to_string();
        let started = identity(&row.config);
        let stopped = tokio_util::sync::CancellationToken::new();
        if let Some(earlier) = self
            .mcp
            .signing_in
            .lock()
            .unwrap()
            .insert(server.clone(), stopped.clone())
        {
            earlier.cancel();
        }

        tokio::spawn(async move {
            let waited = tokio::time::timeout(SIGN_IN_WAIT, finish(&listener, &mut state));
            let finished = tokio::select! {
                waited = waited => waited.unwrap_or_else(|_| {
                    Err(SignInError::BrowserTimeout {
                        minutes: SIGN_IN_WAIT.as_secs() / 60,
                    })
                }),
                () = stopped.cancelled() => return,
            };
            let Some(engine) = engine.upgrade() else { return };
            engine.finish_sign_in(&server, &stopped, started, finished).await;
        });

        Ok(page)
    }

    /// Connects with what a finished sign-in stored, unless it was stopped or the server changed while
    /// it waited: tokens issued for another host or app, or after a sign-out, are forgotten.
    async fn finish_sign_in(
        self: &Arc<Self>,
        server: &str,
        stopped: &tokio_util::sync::CancellationToken,
        started: Option<(String, Option<String>)>,
        finished: Result<(), SignInError>,
    ) {
        let current = {
            let mut flows = self.mcp.signing_in.lock().unwrap();
            let current = !stopped.is_cancelled();
            if current {
                flows.remove(server);
            }
            current
        };
        let config = self.store.mcp_server(server).ok().flatten().map(|row| row.config);
        let same = config.as_ref().map(identity) == Some(started);

        match finished {
            Ok(()) if current && same => drop(self.connect_mcp(server).await),
            Ok(()) => {
                let _ = forget(&self.credentials, server);
                if current {
                    let why = "the server changed while signing in; sign in again";
                    self.mcp.sign_in_failed(server, &self.store, &self.hub, why);
                }
            }
            Err(why) if current => self
                .mcp
                .sign_in_failed(server, &self.store, &self.hub, &why.to_string()),
            Err(_) => {}
        }
    }

    /// Forgets a server's sign-in and reconnects it without one, so it is plain what it can do signed out.
    pub async fn sign_out_mcp(self: &Arc<Self>, name: &str) -> Result<(), SignInError> {
        if let Some(waiting) = self.mcp.signing_in.lock().unwrap().remove(name) {
            waiting.cancel();
        }
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

    let request = request
        .with_preregistered_client(&app.client_id)
        .with_scopes(app.scopes.clone());
    match &app.client_secret {
        Some(secret) => request.with_client_secret(secret),
        None => request,
    }
}

/// Waits for the browser's return and trades its code for tokens, which the store keeps.
async fn finish(listener: &tokio::net::TcpListener, state: &mut OAuthState) -> Result<(), SignInError> {
    let (mut socket, params) = crate::llm::openai::oauth::next_callback(listener, "/callback").await?;
    let (Some(code), Some(csrf)) = (params.get("code"), params.get("state")) else {
        // The authorization server's own reason, when it gave one (`access_denied` and its description).
        let why = match (params.get("error"), params.get("error_description")) {
            (Some(error), Some(description)) => SignInError::Refused(format!("{error}: {description}")),
            (Some(error), None) => SignInError::Refused(error.clone()),
            _ => SignInError::NoCode,
        };

        crate::llm::openai::oauth::respond(&mut socket, 400, &format!("Sign-in failed: {why}")).await;
        return Err(why);
    };

    match state.handle_callback(code, csrf).await {
        Ok(()) => {
            crate::llm::openai::oauth::respond(
                &mut socket,
                200,
                "Drift is signed in to the MCP server. You can close this tab and go back to the app.",
            )
            .await;
            Ok(())
        }
        Err(error) => {
            crate::llm::openai::oauth::respond(&mut socket, 400, &format!("Sign-in failed: {error}")).await;
            Err(SignInError::Auth(error))
        }
    }
}
