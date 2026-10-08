use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] rusqlite::Error),
    #[error(transparent)]
    Service(rmcp::service::ServiceError),
    #[error("tools/list failed: {0}")]
    Tools(rmcp::service::ServiceError),
    #[error("no such server")]
    NotFound,
    #[error("{name} has no saved value to keep")]
    MissingSavedValue { name: String },
    #[error("a stdio server runs in a workspace; open one and connect it there")]
    NeedsWorkspace,
    #[error("a remote server has one shared connection")]
    SharedRemote,
    #[error("server definition changed")]
    DefinitionChanged,
    #[error("server definition changed during connect")]
    ConnectChanged,
    #[error("already connected or connecting")]
    Busy,
    #[error("it failed to connect; connect it again in Settings")]
    RetryInSettings,
    #[error("the user disconnected it")]
    Disconnected,
    #[error("it is off in this workspace")]
    OffWorkspace,
    #[error("server is disabled")]
    Disabled,
    #[error("the {server} MCP server is not connected")]
    NotConnected { server: String },
    #[error("the server did not {what} within {limit:?}")]
    Timeout { what: String, limit: Duration },
    #[error("{message}")]
    Connect { message: String, needs_sign_in: bool },
}
