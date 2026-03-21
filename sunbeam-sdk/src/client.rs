//! ServiceClient trait, HttpTransport, AuthMethod, and SunbeamClient factory.
//!
//! Every service client implements [`ServiceClient`] and uses [`HttpTransport`]
//! for shared HTTP plumbing. [`SunbeamClient`] lazily constructs per-service
//! clients from the active config context.

use crate::error::{Result, ResultExt, SunbeamError};
use reqwest::Method;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// AuthMethod
// ---------------------------------------------------------------------------

/// Authentication credential for API clients.
#[derive(Debug, Clone)]
pub enum AuthMethod {
    /// No authentication.
    None,
    /// Bearer token (`Authorization: Bearer <token>`).
    Bearer(String),
    /// Custom header (e.g. `X-Vault-Token`).
    Header { name: &'static str, value: String },
    /// Gitea-style PAT (`Authorization: token <pat>`).
    Token(String),
}

// ---------------------------------------------------------------------------
// ServiceClient trait
// ---------------------------------------------------------------------------

/// Base trait all service clients implement.
pub trait ServiceClient: Send + Sync {
    /// Human-readable service name (e.g. "kratos", "matrix").
    fn service_name(&self) -> &'static str;

    /// The base URL this client is configured against.
    fn base_url(&self) -> &str;

    /// Construct from base URL and auth.
    fn from_parts(base_url: String, auth: AuthMethod) -> Self
    where
        Self: Sized;
}

// ---------------------------------------------------------------------------
// HttpTransport
// ---------------------------------------------------------------------------

/// Shared HTTP helpers any ServiceClient can use.
///
/// Wraps a base URL, reqwest client, and auth method. Provides `json()`,
/// `json_opt()`, `send()`, `bytes()`, and `request()` to eliminate
/// per-client boilerplate.
#[derive(Debug, Clone)]
pub struct HttpTransport {
    pub base_url: String,
    pub http: reqwest::Client,
    pub auth: AuthMethod,
}

impl HttpTransport {
    /// Create a new transport with the given base URL and auth.
    pub fn new(base_url: &str, auth: AuthMethod) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
            auth,
        }
    }

    /// Build a request with auth headers applied.
    pub fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
        let mut req = self.http.request(method, &url);
        match &self.auth {
            AuthMethod::None => {}
            AuthMethod::Bearer(token) => {
                req = req.bearer_auth(token);
            }
            AuthMethod::Header { name, value } => {
                req = req.header(*name, value);
            }
            AuthMethod::Token(pat) => {
                req = req.header("Authorization", format!("token {pat}"));
            }
        }
        req
    }

    /// Make a request, parse the response as JSON type `T`.
    ///
    /// `ctx` is a human-readable description for error messages.
    pub async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&(impl Serialize + Sync)>,
        ctx: &str,
    ) -> Result<T> {
        let mut req = self.request(method, path);
        if let Some(b) = body {
            req = req.json(b);
        }

        let resp = req.send().await.with_ctx(|| format!("{ctx}: request failed"))?;
        let status = resp.status();

        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(SunbeamError::network(format!(
                "{ctx}: HTTP {status}: {body_text}"
            )));
        }

        resp.json::<T>()
            .await
            .with_ctx(|| format!("{ctx}: failed to parse response"))
    }

    /// Like [`json()`] but returns `None` on 404 instead of an error.
    pub async fn json_opt<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&(impl Serialize + Sync)>,
        ctx: &str,
    ) -> Result<Option<T>> {
        let mut req = self.request(method, path);
        if let Some(b) = body {
            req = req.json(b);
        }

        let resp = req.send().await.with_ctx(|| format!("{ctx}: request failed"))?;
        let status = resp.status();

        if status.as_u16() == 404 {
            return Ok(None);
        }

        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(SunbeamError::network(format!(
                "{ctx}: HTTP {status}: {body_text}"
            )));
        }

        let val = resp
            .json::<T>()
            .await
            .with_ctx(|| format!("{ctx}: failed to parse response"))?;
        Ok(Some(val))
    }

    /// Make a request, discard the response body (assert success).
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&(impl Serialize + Sync)>,
        ctx: &str,
    ) -> Result<()> {
        let mut req = self.request(method, path);
        if let Some(b) = body {
            req = req.json(b);
        }

        let resp = req.send().await.with_ctx(|| format!("{ctx}: request failed"))?;
        let status = resp.status();

        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(SunbeamError::network(format!(
                "{ctx}: HTTP {status}: {body_text}"
            )));
        }
        Ok(())
    }

    /// Make a request, return the raw response bytes.
    pub async fn bytes(
        &self,
        method: Method,
        path: &str,
        ctx: &str,
    ) -> Result<bytes::Bytes> {
        let resp = self
            .request(method, path)
            .send()
            .await
            .with_ctx(|| format!("{ctx}: request failed"))?;
        let status = resp.status();

        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(SunbeamError::network(format!(
                "{ctx}: HTTP {status}: {body_text}"
            )));
        }

        resp.bytes()
            .await
            .with_ctx(|| format!("{ctx}: failed to read response bytes"))
    }

    /// Replace the auth method (e.g. after token exchange).
    pub fn set_auth(&mut self, auth: AuthMethod) {
        self.auth = auth;
    }
}

// ---------------------------------------------------------------------------
// SunbeamClient factory
// ---------------------------------------------------------------------------

/// Unified entry point for all service clients.
///
/// Lazily constructs and caches per-service clients from the active config
/// context. Each accessor returns a `&Client` reference, constructing on
/// first call via [`OnceLock`].
pub struct SunbeamClient {
    ctx: crate::config::Context,
    domain: String,
    // Phase 1
    #[cfg(feature = "identity")]
    kratos: OnceLock<crate::identity::KratosClient>,
    #[cfg(feature = "identity")]
    hydra: OnceLock<crate::auth::hydra::HydraClient>,
    // Phase 2
    #[cfg(feature = "gitea")]
    gitea: OnceLock<crate::gitea::GiteaClient>,
    // Phase 3
    #[cfg(feature = "matrix")]
    matrix: OnceLock<crate::matrix::MatrixClient>,
    #[cfg(feature = "opensearch")]
    opensearch: OnceLock<crate::search::OpenSearchClient>,
    #[cfg(feature = "s3")]
    s3: OnceLock<crate::storage::S3Client>,
    #[cfg(feature = "livekit")]
    livekit: OnceLock<crate::media::LiveKitClient>,
    #[cfg(feature = "monitoring")]
    prometheus: OnceLock<crate::monitoring::PrometheusClient>,
    #[cfg(feature = "monitoring")]
    loki: OnceLock<crate::monitoring::LokiClient>,
    #[cfg(feature = "monitoring")]
    grafana: OnceLock<crate::monitoring::GrafanaClient>,
    // Phase 4
    #[cfg(feature = "lasuite")]
    people: OnceLock<crate::lasuite::PeopleClient>,
    #[cfg(feature = "lasuite")]
    docs: OnceLock<crate::lasuite::DocsClient>,
    #[cfg(feature = "lasuite")]
    meet: OnceLock<crate::lasuite::MeetClient>,
    #[cfg(feature = "lasuite")]
    drive: OnceLock<crate::lasuite::DriveClient>,
    #[cfg(feature = "lasuite")]
    messages: OnceLock<crate::lasuite::MessagesClient>,
    #[cfg(feature = "lasuite")]
    calendars: OnceLock<crate::lasuite::CalendarsClient>,
    #[cfg(feature = "lasuite")]
    find: OnceLock<crate::lasuite::FindClient>,
    // Bao/Planka stay in their existing modules
    bao: OnceLock<crate::openbao::BaoClient>,
}

impl SunbeamClient {
    /// Create a factory from a resolved context.
    pub fn from_context(ctx: &crate::config::Context) -> Self {
        Self {
            domain: ctx.domain.clone(),
            ctx: ctx.clone(),
            #[cfg(feature = "identity")]
            kratos: OnceLock::new(),
            #[cfg(feature = "identity")]
            hydra: OnceLock::new(),
            #[cfg(feature = "gitea")]
            gitea: OnceLock::new(),
            #[cfg(feature = "matrix")]
            matrix: OnceLock::new(),
            #[cfg(feature = "opensearch")]
            opensearch: OnceLock::new(),
            #[cfg(feature = "s3")]
            s3: OnceLock::new(),
            #[cfg(feature = "livekit")]
            livekit: OnceLock::new(),
            #[cfg(feature = "monitoring")]
            prometheus: OnceLock::new(),
            #[cfg(feature = "monitoring")]
            loki: OnceLock::new(),
            #[cfg(feature = "monitoring")]
            grafana: OnceLock::new(),
            #[cfg(feature = "lasuite")]
            people: OnceLock::new(),
            #[cfg(feature = "lasuite")]
            docs: OnceLock::new(),
            #[cfg(feature = "lasuite")]
            meet: OnceLock::new(),
            #[cfg(feature = "lasuite")]
            drive: OnceLock::new(),
            #[cfg(feature = "lasuite")]
            messages: OnceLock::new(),
            #[cfg(feature = "lasuite")]
            calendars: OnceLock::new(),
            #[cfg(feature = "lasuite")]
            find: OnceLock::new(),
            bao: OnceLock::new(),
        }
    }

    /// The domain this client targets.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// The underlying config context.
    pub fn context(&self) -> &crate::config::Context {
        &self.ctx
    }

    // -- Lazy accessors (each feature-gated) --------------------------------

    #[cfg(feature = "identity")]
    pub fn kratos(&self) -> &crate::identity::KratosClient {
        self.kratos.get_or_init(|| {
            crate::identity::KratosClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "identity")]
    pub fn hydra(&self) -> &crate::auth::hydra::HydraClient {
        self.hydra.get_or_init(|| {
            crate::auth::hydra::HydraClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "gitea")]
    pub fn gitea(&self) -> &crate::gitea::GiteaClient {
        self.gitea.get_or_init(|| {
            crate::gitea::GiteaClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "matrix")]
    pub fn matrix(&self) -> &crate::matrix::MatrixClient {
        self.matrix.get_or_init(|| {
            crate::matrix::MatrixClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "opensearch")]
    pub fn opensearch(&self) -> &crate::search::OpenSearchClient {
        self.opensearch.get_or_init(|| {
            crate::search::OpenSearchClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "s3")]
    pub fn s3(&self) -> &crate::storage::S3Client {
        self.s3.get_or_init(|| {
            crate::storage::S3Client::connect(&self.domain)
        })
    }

    #[cfg(feature = "livekit")]
    pub fn livekit(&self) -> &crate::media::LiveKitClient {
        self.livekit.get_or_init(|| {
            crate::media::LiveKitClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "monitoring")]
    pub fn prometheus(&self) -> &crate::monitoring::PrometheusClient {
        self.prometheus.get_or_init(|| {
            crate::monitoring::PrometheusClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "monitoring")]
    pub fn loki(&self) -> &crate::monitoring::LokiClient {
        self.loki.get_or_init(|| {
            crate::monitoring::LokiClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "monitoring")]
    pub fn grafana(&self) -> &crate::monitoring::GrafanaClient {
        self.grafana.get_or_init(|| {
            crate::monitoring::GrafanaClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "lasuite")]
    pub fn people(&self) -> &crate::lasuite::PeopleClient {
        self.people.get_or_init(|| {
            crate::lasuite::PeopleClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "lasuite")]
    pub fn docs(&self) -> &crate::lasuite::DocsClient {
        self.docs.get_or_init(|| {
            crate::lasuite::DocsClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "lasuite")]
    pub fn meet(&self) -> &crate::lasuite::MeetClient {
        self.meet.get_or_init(|| {
            crate::lasuite::MeetClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "lasuite")]
    pub fn drive(&self) -> &crate::lasuite::DriveClient {
        self.drive.get_or_init(|| {
            crate::lasuite::DriveClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "lasuite")]
    pub fn messages(&self) -> &crate::lasuite::MessagesClient {
        self.messages.get_or_init(|| {
            crate::lasuite::MessagesClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "lasuite")]
    pub fn calendars(&self) -> &crate::lasuite::CalendarsClient {
        self.calendars.get_or_init(|| {
            crate::lasuite::CalendarsClient::connect(&self.domain)
        })
    }

    #[cfg(feature = "lasuite")]
    pub fn find(&self) -> &crate::lasuite::FindClient {
        self.find.get_or_init(|| {
            crate::lasuite::FindClient::connect(&self.domain)
        })
    }

    pub fn bao(&self, base_url: &str) -> &crate::openbao::BaoClient {
        self.bao.get_or_init(|| {
            crate::openbao::BaoClient::new(base_url)
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_transport_url_construction() {
        let t = HttpTransport::new("https://api.example.com/", AuthMethod::None);
        assert_eq!(t.base_url, "https://api.example.com");
    }

    #[test]
    fn test_http_transport_strips_trailing_slash() {
        let t = HttpTransport::new("https://api.example.com///", AuthMethod::None);
        assert_eq!(t.base_url, "https://api.example.com");
    }

    #[test]
    fn test_auth_method_variants() {
        let _none = AuthMethod::None;
        let _bearer = AuthMethod::Bearer("tok".into());
        let _header = AuthMethod::Header {
            name: "X-Api-Key",
            value: "abc".into(),
        };
        let _token = AuthMethod::Token("pat123".into());
    }

    #[test]
    fn test_set_auth() {
        let mut t = HttpTransport::new("https://example.com", AuthMethod::None);
        t.set_auth(AuthMethod::Bearer("new-token".into()));
        assert!(matches!(t.auth, AuthMethod::Bearer(ref s) if s == "new-token"));
    }

    #[test]
    fn test_sunbeam_client_from_context() {
        let ctx = crate::config::Context {
            domain: "sunbeam.pt".to_string(),
            ..Default::default()
        };
        let client = SunbeamClient::from_context(&ctx);
        assert_eq!(client.domain(), "sunbeam.pt");
    }

    #[tokio::test]
    async fn test_transport_json_error_on_unreachable() {
        let t = HttpTransport::new("http://127.0.0.1:19998", AuthMethod::None);
        let result = t
            .json::<serde_json::Value>(Method::GET, "/test", Option::<&()>::None, "test")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_transport_send_error_on_unreachable() {
        let t = HttpTransport::new("http://127.0.0.1:19998", AuthMethod::None);
        let result = t
            .send(Method::GET, "/test", Option::<&()>::None, "test")
            .await;
        assert!(result.is_err());
    }
}
