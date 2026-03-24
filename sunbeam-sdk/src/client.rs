//! ServiceClient trait, HttpTransport, AuthMethod, and SunbeamClient factory.
//!
//! Every service client implements [`ServiceClient`] and uses [`HttpTransport`]
//! for shared HTTP plumbing. [`SunbeamClient`] lazily constructs per-service
//! clients from the active config context.

use crate::error::{Result, ResultExt, SunbeamError};
use reqwest::Method;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::OnceCell;

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
/// context. Each accessor resolves auth and returns a `&Client` reference,
/// constructing on first call via [`OnceCell`] (async-aware).
///
/// Auth is resolved per-client:
/// - SSO bearer (`get_token()`) — admin APIs, Matrix, La Suite, OpenSearch
/// - Gitea PAT (`get_gitea_token()`) — Gitea
/// - None — Prometheus, Loki, S3, LiveKit
pub struct SunbeamClient {
    ctx: crate::config::Context,
    domain: String,
    #[cfg(feature = "identity")]
    kratos: OnceCell<crate::identity::KratosClient>,
    #[cfg(feature = "identity")]
    hydra: OnceCell<crate::auth::hydra::HydraClient>,
    #[cfg(feature = "gitea")]
    gitea: OnceCell<crate::gitea::GiteaClient>,
    #[cfg(feature = "matrix")]
    matrix: OnceCell<crate::matrix::MatrixClient>,
    #[cfg(feature = "opensearch")]
    opensearch: OnceCell<crate::search::OpenSearchClient>,
    #[cfg(feature = "s3")]
    s3: OnceCell<crate::storage::S3Client>,
    #[cfg(feature = "livekit")]
    livekit: OnceCell<crate::media::LiveKitClient>,
    #[cfg(feature = "monitoring")]
    prometheus: OnceCell<crate::monitoring::PrometheusClient>,
    #[cfg(feature = "monitoring")]
    loki: OnceCell<crate::monitoring::LokiClient>,
    #[cfg(feature = "monitoring")]
    grafana: OnceCell<crate::monitoring::GrafanaClient>,
    #[cfg(feature = "lasuite")]
    people: OnceCell<crate::lasuite::PeopleClient>,
    #[cfg(feature = "lasuite")]
    docs: OnceCell<crate::lasuite::DocsClient>,
    #[cfg(feature = "lasuite")]
    meet: OnceCell<crate::lasuite::MeetClient>,
    #[cfg(feature = "lasuite")]
    drive: OnceCell<crate::lasuite::DriveClient>,
    #[cfg(feature = "lasuite")]
    messages: OnceCell<crate::lasuite::MessagesClient>,
    #[cfg(feature = "lasuite")]
    calendars: OnceCell<crate::lasuite::CalendarsClient>,
    #[cfg(feature = "lasuite")]
    find: OnceCell<crate::lasuite::FindClient>,
    bao: OnceCell<crate::openbao::BaoClient>,
}

impl SunbeamClient {
    /// Create a factory from a resolved context.
    pub fn from_context(ctx: &crate::config::Context) -> Self {
        Self {
            domain: ctx.domain.clone(),
            ctx: ctx.clone(),
            #[cfg(feature = "identity")]
            kratos: OnceCell::new(),
            #[cfg(feature = "identity")]
            hydra: OnceCell::new(),
            #[cfg(feature = "gitea")]
            gitea: OnceCell::new(),
            #[cfg(feature = "matrix")]
            matrix: OnceCell::new(),
            #[cfg(feature = "opensearch")]
            opensearch: OnceCell::new(),
            #[cfg(feature = "s3")]
            s3: OnceCell::new(),
            #[cfg(feature = "livekit")]
            livekit: OnceCell::new(),
            #[cfg(feature = "monitoring")]
            prometheus: OnceCell::new(),
            #[cfg(feature = "monitoring")]
            loki: OnceCell::new(),
            #[cfg(feature = "monitoring")]
            grafana: OnceCell::new(),
            #[cfg(feature = "lasuite")]
            people: OnceCell::new(),
            #[cfg(feature = "lasuite")]
            docs: OnceCell::new(),
            #[cfg(feature = "lasuite")]
            meet: OnceCell::new(),
            #[cfg(feature = "lasuite")]
            drive: OnceCell::new(),
            #[cfg(feature = "lasuite")]
            messages: OnceCell::new(),
            #[cfg(feature = "lasuite")]
            calendars: OnceCell::new(),
            #[cfg(feature = "lasuite")]
            find: OnceCell::new(),
            bao: OnceCell::new(),
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

    // -- Auth helpers --------------------------------------------------------

    /// Get cached SSO bearer token (from `sunbeam auth sso`).
    async fn sso_token(&self) -> Result<String> {
        crate::auth::get_token().await
    }

    /// Get cached Gitea PAT (from `sunbeam auth git`).
    fn gitea_token(&self) -> Result<String> {
        crate::auth::get_gitea_token()
    }

    /// Get cached OIDC id_token (JWT with claims including admin flag).
    fn id_token(&self) -> Result<String> {
        crate::auth::get_id_token()
    }

    // -- Lazy async accessors (each feature-gated) ---------------------------
    //
    // Each accessor resolves the appropriate auth and constructs the client
    // with from_parts(url, auth). Cached after first call.

    #[cfg(feature = "identity")]
    pub async fn kratos(&self) -> Result<&crate::identity::KratosClient> {
        self.kratos.get_or_try_init(|| async {
            let token = self.sso_token().await?;
            let url = format!("https://id.{}", self.domain);
            Ok(crate::identity::KratosClient::from_parts(url, AuthMethod::Bearer(token)))
        }).await
    }

    #[cfg(feature = "identity")]
    pub async fn hydra(&self) -> Result<&crate::auth::hydra::HydraClient> {
        self.hydra.get_or_try_init(|| async {
            let token = self.sso_token().await?;
            let url = format!("https://hydra.{}", self.domain);
            Ok(crate::auth::hydra::HydraClient::from_parts(url, AuthMethod::Bearer(token)))
        }).await
    }

    #[cfg(feature = "gitea")]
    pub async fn gitea(&self) -> Result<&crate::gitea::GiteaClient> {
        self.gitea.get_or_try_init(|| async {
            let token = self.gitea_token()?;
            let url = format!("https://src.{}/api/v1", self.domain);
            Ok(crate::gitea::GiteaClient::from_parts(url, AuthMethod::Token(token)))
        }).await
    }

    #[cfg(feature = "matrix")]
    pub async fn matrix(&self) -> Result<&crate::matrix::MatrixClient> {
        self.matrix.get_or_try_init(|| async {
            let token = self.sso_token().await?;
            let url = format!("https://messages.{}/_matrix", self.domain);
            Ok(crate::matrix::MatrixClient::from_parts(url, AuthMethod::Bearer(token)))
        }).await
    }

    #[cfg(feature = "opensearch")]
    pub async fn opensearch(&self) -> Result<&crate::search::OpenSearchClient> {
        self.opensearch.get_or_try_init(|| async {
            let token = self.sso_token().await?;
            let url = format!("https://search.{}", self.domain);
            Ok(crate::search::OpenSearchClient::from_parts(url, AuthMethod::Bearer(token)))
        }).await
    }

    #[cfg(feature = "s3")]
    pub async fn s3(&self) -> Result<&crate::storage::S3Client> {
        self.s3.get_or_try_init(|| async {
            Ok(crate::storage::S3Client::connect(&self.domain))
        }).await
    }

    #[cfg(feature = "livekit")]
    pub async fn livekit(&self) -> Result<&crate::media::LiveKitClient> {
        self.livekit.get_or_try_init(|| async {
            Ok(crate::media::LiveKitClient::connect(&self.domain))
        }).await
    }

    #[cfg(feature = "monitoring")]
    pub async fn prometheus(&self) -> Result<&crate::monitoring::PrometheusClient> {
        self.prometheus.get_or_try_init(|| async {
            Ok(crate::monitoring::PrometheusClient::connect(&self.domain))
        }).await
    }

    #[cfg(feature = "monitoring")]
    pub async fn loki(&self) -> Result<&crate::monitoring::LokiClient> {
        self.loki.get_or_try_init(|| async {
            Ok(crate::monitoring::LokiClient::connect(&self.domain))
        }).await
    }

    #[cfg(feature = "monitoring")]
    pub async fn grafana(&self) -> Result<&crate::monitoring::GrafanaClient> {
        self.grafana.get_or_try_init(|| async {
            Ok(crate::monitoring::GrafanaClient::connect(&self.domain))
        }).await
    }

    #[cfg(feature = "lasuite")]
    pub async fn people(&self) -> Result<&crate::lasuite::PeopleClient> {
        self.people.get_or_try_init(|| async {
            let token = self.sso_token().await?;
            let url = format!("https://people.{}/external_api/v1.0", self.domain);
            Ok(crate::lasuite::PeopleClient::from_parts(url, AuthMethod::Bearer(token)))
        }).await
    }

    #[cfg(feature = "lasuite")]
    pub async fn docs(&self) -> Result<&crate::lasuite::DocsClient> {
        self.docs.get_or_try_init(|| async {
            let token = self.sso_token().await?;
            let url = format!("https://docs.{}/external_api/v1.0", self.domain);
            Ok(crate::lasuite::DocsClient::from_parts(url, AuthMethod::Bearer(token)))
        }).await
    }

    #[cfg(feature = "lasuite")]
    pub async fn meet(&self) -> Result<&crate::lasuite::MeetClient> {
        self.meet.get_or_try_init(|| async {
            let token = self.sso_token().await?;
            let url = format!("https://meet.{}/external_api/v1.0", self.domain);
            Ok(crate::lasuite::MeetClient::from_parts(url, AuthMethod::Bearer(token)))
        }).await
    }

    #[cfg(feature = "lasuite")]
    pub async fn drive(&self) -> Result<&crate::lasuite::DriveClient> {
        self.drive.get_or_try_init(|| async {
            let token = self.sso_token().await?;
            let url = format!("https://drive.{}/external_api/v1.0", self.domain);
            Ok(crate::lasuite::DriveClient::from_parts(url, AuthMethod::Bearer(token)))
        }).await
    }

    #[cfg(feature = "lasuite")]
    pub async fn messages(&self) -> Result<&crate::lasuite::MessagesClient> {
        self.messages.get_or_try_init(|| async {
            let token = self.sso_token().await?;
            let url = format!("https://mail.{}/external_api/v1.0", self.domain);
            Ok(crate::lasuite::MessagesClient::from_parts(url, AuthMethod::Bearer(token)))
        }).await
    }

    #[cfg(feature = "lasuite")]
    pub async fn calendars(&self) -> Result<&crate::lasuite::CalendarsClient> {
        self.calendars.get_or_try_init(|| async {
            let token = self.sso_token().await?;
            let url = format!("https://calendar.{}/external_api/v1.0", self.domain);
            Ok(crate::lasuite::CalendarsClient::from_parts(url, AuthMethod::Bearer(token)))
        }).await
    }

    #[cfg(feature = "lasuite")]
    pub async fn find(&self) -> Result<&crate::lasuite::FindClient> {
        self.find.get_or_try_init(|| async {
            let token = self.sso_token().await?;
            let url = format!("https://find.{}/external_api/v1.0", self.domain);
            Ok(crate::lasuite::FindClient::from_parts(url, AuthMethod::Bearer(token)))
        }).await
    }

    pub async fn bao(&self) -> Result<&crate::openbao::BaoClient> {
        self.bao.get_or_try_init(|| async {
            let url = format!("https://vault.{}", self.domain);
            let id_token = self.id_token()?;
            let bearer = self.sso_token().await?;

            // Authenticate to OpenBao via JWT auth method using the OIDC id_token.
            // Try admin role first (for users with admin: true), fall back to reader.
            let http = reqwest::Client::new();
            let vault_token = {
                let mut token = None;
                for role in &["cli-admin", "cli-reader"] {
                    let resp = http
                        .post(format!("{url}/v1/auth/jwt/login"))
                        .bearer_auth(&bearer)
                        .json(&serde_json::json!({ "jwt": id_token, "role": role }))
                        .send()
                        .await;
                    match resp {
                        Ok(r) => {
                            let status = r.status();
                            if status.is_success() {
                                if let Ok(body) = r.json::<serde_json::Value>().await {
                                    if let Some(t) = body["auth"]["client_token"].as_str() {
                                        tracing::debug!("vault JWT login ok (role={role})");
                                        token = Some(t.to_string());
                                        break;
                                    }
                                }
                            } else {
                                let body = r.text().await.unwrap_or_default();
                                tracing::debug!("vault JWT login {status} (role={role}): {body}");
                            }
                        }
                        Err(e) => {
                            tracing::debug!("vault JWT login request failed (role={role}): {e}");
                        }
                    }
                }
                match token {
                    Some(t) => t,
                    None => {
                        tracing::debug!("vault JWT auth failed, falling back to local keystore");
                        match crate::vault_keystore::load_keystore(&self.domain) {
                            Ok(ks) => ks.root_token,
                            Err(_) => return Err(SunbeamError::secrets(
                                "Vault auth failed: no valid JWT role and no local keystore. Run `sunbeam auth sso` and retry."
                            )),
                        }
                    }
                }
            };

            Ok(crate::openbao::BaoClient::with_proxy_auth(&url, &vault_token, &bearer))
        }).await
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
