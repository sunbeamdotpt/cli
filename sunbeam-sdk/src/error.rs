//! Unified error tree for the sunbeam CLI.
//!
//! Every module returns `Result<T, SunbeamError>`. Errors bubble up to `main`,
//! which maps them to exit codes and log output.

/// Exit codes for the sunbeam CLI.
#[allow(dead_code)]
pub mod exit {
/// Success.
    pub const SUCCESS: i32 = 0;
/// General.
    pub const GENERAL: i32 = 1;
/// Usage.
    pub const USAGE: i32 = 2;
/// Kube.
    pub const KUBE: i32 = 3;
/// Config.
    pub const CONFIG: i32 = 4;
/// Network.
    pub const NETWORK: i32 = 5;
/// Secrets.
    pub const SECRETS: i32 = 6;
/// Build.
    pub const BUILD: i32 = 7;
/// Identity.
    pub const IDENTITY: i32 = 8;
/// External tool.
    pub const EXTERNAL_TOOL: i32 = 9;
}

/// Top-level error type for the sunbeam CLI.
///
/// Each variant maps to a logical error category with its own exit code.
/// Leaf errors (io, json, yaml, kube, reqwest, etc.) are converted via `From` impls.
#[derive(Debug, thiserror::Error)]
pub enum SunbeamError {
    /// Kubernetes API or cluster-related error.
    #[error("{context}")]
    Kube {
        context: String,
        #[source]
        source: Option<Box<kube::Error>>,
    },

    /// Configuration error (missing config, invalid config, bad arguments).
    #[error("{0}")]
    Config(String),

    /// Network/HTTP error.
    #[error("{context}")]
    Network {
        context: String,
        #[source]
        source: Option<reqwest::Error>,
    },

    /// OpenBao / Vault error.
    #[error("{0}")]
    Secrets(String),

    /// Image build error.
    #[error("{0}")]
    Build(String),

    /// Identity / user management error (Kratos, Hydra).
    #[error("{0}")]
    Identity(String),

    /// External tool error (kustomize, linkerd, buildctl, yarn, etc.).
    #[error("{tool}: {detail}")]
    ExternalTool { tool: String, detail: String },

    /// IO error.
    #[error("{context}: {source}")]
    Io {
        context: String,
        source: std::io::Error,
    },

    /// JSON serialization/deserialization error.
    #[error("{0}")]
    Json(#[from] serde_json::Error),

    /// YAML serialization/deserialization error.
    #[error("{0}")]
    Yaml(#[from] serde_yaml::Error),

    /// Catch-all for errors that don't fit a specific category.
    #[error("{0}")]
    Other(String),
}

/// Convenience type alias used throughout the codebase.
pub type Result<T> = std::result::Result<T, SunbeamError>;

impl SunbeamError {
    /// Map this error to a process exit code.
    pub fn exit_code(&self) -> i32 {
        match self {
            SunbeamError::Config(_) => exit::CONFIG,
            SunbeamError::Kube { .. } => exit::KUBE,
            SunbeamError::Network { .. } => exit::NETWORK,
            SunbeamError::Secrets(_) => exit::SECRETS,
            SunbeamError::Build(_) => exit::BUILD,
            SunbeamError::Identity(_) => exit::IDENTITY,
            SunbeamError::ExternalTool { .. } => exit::EXTERNAL_TOOL,
            SunbeamError::Io { .. } => exit::GENERAL,
            SunbeamError::Json(_) => exit::GENERAL,
            SunbeamError::Yaml(_) => exit::GENERAL,
            SunbeamError::Other(_) => exit::GENERAL,
        }
    }
}

// ---------------------------------------------------------------------------
// From impls for automatic conversion
// ---------------------------------------------------------------------------

impl From<kube::Error> for SunbeamError {
    fn from(e: kube::Error) -> Self {
        SunbeamError::Kube {
            context: e.to_string(),
            source: Some(Box::new(e)),
        }
    }
}

impl From<reqwest::Error> for SunbeamError {
    fn from(e: reqwest::Error) -> Self {
        SunbeamError::Network {
            context: e.to_string(),
            source: Some(e),
        }
    }
}

impl From<std::io::Error> for SunbeamError {
    fn from(e: std::io::Error) -> Self {
        SunbeamError::Io {
            context: "IO error".into(),
            source: e,
        }
    }
}

impl From<lettre::transport::smtp::Error> for SunbeamError {
    fn from(e: lettre::transport::smtp::Error) -> Self {
        SunbeamError::Network {
            context: format!("SMTP error: {e}"),
            source: None,
        }
    }
}

impl From<lettre::error::Error> for SunbeamError {
    fn from(e: lettre::error::Error) -> Self {
        SunbeamError::Other(format!("Email error: {e}"))
    }
}

impl From<base64::DecodeError> for SunbeamError {
    fn from(e: base64::DecodeError) -> Self {
        SunbeamError::Other(format!("Base64 decode error: {e}"))
    }
}

impl From<std::string::FromUtf8Error> for SunbeamError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        SunbeamError::Other(format!("UTF-8 error: {e}"))
    }
}

// ---------------------------------------------------------------------------
// Context extension trait (replaces anyhow's .context())
// ---------------------------------------------------------------------------

/// Extension trait that adds `.ctx()` to `Result<T, E>` for adding context strings.
/// Replaces `anyhow::Context`.
pub trait ResultExt<T> {
    /// Add context to an error, converting it to `SunbeamError`.
    fn ctx(self, context: &str) -> Result<T>;

    /// Add lazy context to an error.
    fn with_ctx<F: FnOnce() -> String>(self, f: F) -> Result<T>;
}

impl<T, E: Into<SunbeamError>> ResultExt<T> for std::result::Result<T, E> {
    fn ctx(self, context: &str) -> Result<T> {
        self.map_err(|e| {
            let inner = e.into();
            match inner {
                SunbeamError::Kube { source, .. } => SunbeamError::Kube {
                    context: context.to_string(),
                    source,
                },
                SunbeamError::Network { source, .. } => SunbeamError::Network {
                    context: context.to_string(),
                    source,
                },
                SunbeamError::Io { source, .. } => SunbeamError::Io {
                    context: context.to_string(),
                    source,
                },
                SunbeamError::Secrets(msg) => SunbeamError::Secrets(format!("{context}: {msg}")),
                SunbeamError::Config(msg) => SunbeamError::Config(format!("{context}: {msg}")),
                SunbeamError::Build(msg) => SunbeamError::Build(format!("{context}: {msg}")),
                SunbeamError::Identity(msg) => SunbeamError::Identity(format!("{context}: {msg}")),
                SunbeamError::ExternalTool { tool, detail } => SunbeamError::ExternalTool {
                    tool,
                    detail: format!("{context}: {detail}"),
                },
                other => SunbeamError::Other(format!("{context}: {other}")),
            }
        })
    }

    fn with_ctx<F: FnOnce() -> String>(self, f: F) -> Result<T> {
        self.map_err(|e| {
            let context = f();
            let inner = e.into();
            match inner {
                SunbeamError::Kube { source, .. } => SunbeamError::Kube { context, source },
                SunbeamError::Network { source, .. } => SunbeamError::Network { context, source },
                SunbeamError::Io { source, .. } => SunbeamError::Io { context, source },
                SunbeamError::Secrets(msg) => SunbeamError::Secrets(format!("{context}: {msg}")),
                SunbeamError::Config(msg) => SunbeamError::Config(format!("{context}: {msg}")),
                SunbeamError::Build(msg) => SunbeamError::Build(format!("{context}: {msg}")),
                SunbeamError::Identity(msg) => SunbeamError::Identity(format!("{context}: {msg}")),
                SunbeamError::ExternalTool { tool, detail } => SunbeamError::ExternalTool {
                    tool,
                    detail: format!("{context}: {detail}"),
                },
                other => SunbeamError::Other(format!("{context}: {other}")),
            }
        })
    }
}

impl<T> ResultExt<T> for Option<T> {
    fn ctx(self, context: &str) -> Result<T> {
        self.ok_or_else(|| SunbeamError::Other(context.to_string()))
    }

    fn with_ctx<F: FnOnce() -> String>(self, f: F) -> Result<T> {
        self.ok_or_else(|| SunbeamError::Other(f()))
    }
}

// ---------------------------------------------------------------------------
// Convenience constructors
// ---------------------------------------------------------------------------

impl SunbeamError {
    pub fn kube(context: impl Into<String>) -> Self {
        SunbeamError::Kube {
            context: context.into(),
            source: None,
        }
    }

    pub fn config(msg: impl Into<String>) -> Self {
        SunbeamError::Config(msg.into())
    }

    pub fn network(context: impl Into<String>) -> Self {
        SunbeamError::Network {
            context: context.into(),
            source: None,
        }
    }

    pub fn secrets(msg: impl Into<String>) -> Self {
        SunbeamError::Secrets(msg.into())
    }

    pub fn build(msg: impl Into<String>) -> Self {
        SunbeamError::Build(msg.into())
    }

    pub fn identity(msg: impl Into<String>) -> Self {
        SunbeamError::Identity(msg.into())
    }

    pub fn tool(tool: impl Into<String>, detail: impl Into<String>) -> Self {
        SunbeamError::ExternalTool {
            tool: tool.into(),
            detail: detail.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// bail! macro replacement
// ---------------------------------------------------------------------------

/// Like anyhow::bail! but produces a SunbeamError::Other.
#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err($crate::error::SunbeamError::Other(format!($($arg)*)))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit_codes() {
        assert_eq!(SunbeamError::config("bad").exit_code(), exit::CONFIG);
        assert_eq!(SunbeamError::kube("fail").exit_code(), exit::KUBE);
        assert_eq!(SunbeamError::network("fail").exit_code(), exit::NETWORK);
        assert_eq!(SunbeamError::secrets("fail").exit_code(), exit::SECRETS);
        assert_eq!(SunbeamError::build("fail").exit_code(), exit::BUILD);
        assert_eq!(SunbeamError::identity("fail").exit_code(), exit::IDENTITY);
        assert_eq!(
            SunbeamError::tool("kustomize", "not found").exit_code(),
            exit::EXTERNAL_TOOL
        );
        assert_eq!(
            SunbeamError::Other("oops".into()).exit_code(),
            exit::GENERAL
        );
    }

    #[test]
    fn test_display_formatting() {
        let e = SunbeamError::tool("kustomize", "build failed");
        assert_eq!(e.to_string(), "kustomize: build failed");

        let e = SunbeamError::config("missing --domain");
        assert_eq!(e.to_string(), "missing --domain");
    }

    #[test]
    fn test_kube_from() {
        // Just verify the From impl compiles and categorizes correctly
        let e = SunbeamError::kube("test");
        assert!(matches!(e, SunbeamError::Kube { .. }));
    }

    #[test]
    fn test_context_extension() {
        let result: std::result::Result<(), std::io::Error> =
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        let mapped = result.ctx("reading config");
        assert!(mapped.is_err());
        let e = mapped.unwrap_err();
        assert!(e.to_string().starts_with("reading config"));
        assert_eq!(e.exit_code(), exit::GENERAL); // IO maps to general
    }

    #[test]
    fn test_option_context() {
        let val: Option<i32> = None;
        let result = val.ctx("value not found");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "value not found");
    }

    #[test]
    fn test_bail_macro() {
        fn failing() -> Result<()> {
            bail!("something went wrong: {}", 42);
        }
        let e = failing().unwrap_err();
        assert_eq!(e.to_string(), "something went wrong: 42");
    }
}
