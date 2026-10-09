#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Resolve(String),
    #[error("plugin cache: {0}")]
    Cache(String),
    #[error("plugin runtime: {0}")]
    Runtime(String),
    #[error("{0}")]
    CompileTask(String),
    #[error("could not compile: {0}")]
    Compile(String),
    #[error("could not instantiate: {0}")]
    Instantiate(String),
    #[error("name(): {0}")]
    Name(String),
    #[error("name() returned nothing")]
    EmptyName,
    #[cfg(not(feature = "wasm-plugins"))]
    #[error("this build of Drift runs no plugins")]
    Unsupported,
}
