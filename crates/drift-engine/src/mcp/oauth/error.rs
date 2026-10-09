#[derive(Debug, thiserror::Error)]
pub enum SignInError {
    #[error(transparent)]
    Store(#[from] rusqlite::Error),
    #[error(transparent)]
    Credentials(#[from] crate::llm::credentials::CredentialError),
    #[error(transparent)]
    CallbackIo(#[from] std::io::Error),
    #[error(transparent)]
    Callback(#[from] crate::llm::OAuthError),
    #[error(transparent)]
    Auth(#[from] rmcp::transport::auth::AuthError),
    #[error("no MCP server named {0}")]
    MissingServer(String),
    #[error("a server on stdio has no sign-in")]
    Stdio,
    #[error("could not start signing in to {server}: {source}")]
    Start {
        server: String,
        source: rmcp::transport::auth::AuthError,
    },
    #[error("no answer from the browser within {minutes} minutes")]
    BrowserTimeout { minutes: u64 },
    #[error("{0}")]
    Refused(String),
    #[error("the server sent no code")]
    NoCode,
}
