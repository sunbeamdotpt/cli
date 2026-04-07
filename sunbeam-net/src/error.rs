/// Errors produced by sunbeam-net.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("noise handshake failed: {0}")]
    Noise(String),

    #[error("control protocol error: {0}")]
    Control(String),

    #[error("wireguard error: {0}")]
    WireGuard(String),

    #[error("DERP relay error: {0}")]
    Derp(String),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("daemon error: {0}")]
    Daemon(String),

    #[error("IPC error: {0}")]
    Ipc(String),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Json(#[from] serde_json::Error),

    #[error("connection closed")]
    ConnectionClosed,

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<h2::Error> for Error {
    fn from(e: h2::Error) -> Self {
        Error::Control(e.to_string())
    }
}

/// Extension trait for adding context to `Result` types.
pub trait ResultExt<T> {
    fn ctx(self, context: &str) -> Result<T>;
    fn with_ctx<F: FnOnce() -> String>(self, f: F) -> Result<T>;
}

impl<T> ResultExt<T> for std::result::Result<T, std::io::Error> {
    fn ctx(self, context: &str) -> Result<T> {
        self.map_err(|source| Error::Io {
            context: context.to_string(),
            source,
        })
    }

    fn with_ctx<F: FnOnce() -> String>(self, f: F) -> Result<T> {
        self.map_err(|source| Error::Io {
            context: f(),
            source,
        })
    }
}

impl<T> ResultExt<T> for Result<T> {
    fn ctx(self, context: &str) -> Result<T> {
        self.map_err(|e| Error::Other(format!("{context}: {e}")))
    }

    fn with_ctx<F: FnOnce() -> String>(self, f: F) -> Result<T> {
        self.map_err(|e| Error::Other(format!("{}: {e}", f())))
    }
}
