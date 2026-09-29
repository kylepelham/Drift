//! The Drift engine: sessions, providers, tools and the API that serves them.

pub mod api;
pub mod event;
pub mod id;
pub mod llm;
pub mod session;
pub mod store;
pub mod tool;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use event::Hub;
use store::Store;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Events kept for cursor replay before a reconnecting client must hydrate instead.
const EVENT_HISTORY: usize = 4096;

#[derive(Debug)]
pub enum Error {
    Store(rusqlite::Error),
    Io(std::io::Error),
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => write!(f, "store: {error}"),
            Self::Io(error) => write!(f, "io: {error}"),
        }
    }
}

impl std::error::Error for Error {}

pub struct Engine {
    pub store: Arc<Store>,
    pub hub: Hub,
    /// Every request must present this; the shell hands it to the UI, remote clients get it via the gateway.
    pub token: String,
}

impl Engine {
    pub fn open(data_dir: &Path) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self {
            store: Arc::new(store::open(data_dir)?),
            hub: Hub::new(EVENT_HISTORY),
            token: random_hex(32),
        }))
    }
}

pub(crate) fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("system random source unavailable");
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub struct Server {
    pub addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn stop(&self) {
        self.task.abort();
    }
}

pub async fn listen(engine: Arc<Engine>, addr: SocketAddr) -> Result<Server, Error> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let router = api::router(engine);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(Server { addr, task })
}
