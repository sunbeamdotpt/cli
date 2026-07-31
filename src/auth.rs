//! OAuth2 Device Authorization Grant for CLI authentication against the sso-gateway.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use chrono::Utc;
use sdk::auth::{AuthClient, v1 as iam};
use sdk::config::AuthTokens;
use sdk::error::{Result, ResultExt, SunbeamError};
use sdk::kanban::prelude::buffa::MessageField;
use sdk::kanban::prelude::{connectrpc, sunbeam_g2v};
use sdk::reqwest;
use serde::Deserialize;

// Public OAuth2 client ID of the Sunbeam CLI, registered with the
// sso-gateway (`ApplicationService`) by the platform seeding.
//
// Client registration:
//   client_name: "Sunbeam CLI"
//   token_endpoint_auth_method: "none" (public client, no secret)
//   grant_types: authorization_code, refresh_token, urn:ietf:params:oauth:grant-type:device_code
// The OAuth2 client ID is NOT committed to this repo — it is provisioned
// per-platform by sbbb (a public client named "Sunbeam CLI"; grants:
// authorization_code, refresh_token, device_code; loopback redirect URIs
// 9876-9880). Release builds bake it in at compile time via
// SUNBEAM_SSO_CLIENT_ID (option_env! in resolve_client_id).
//
//   response_types: ["code"]
//   scope: "openid email profile offline_access identity:read identity:admin"
//   redirect_uris: http://localhost:9876-9880/callback, http://127.0.0.1:9876-9880/callback
//   post_logout_redirect_uris: http://localhost:9876/callback, http://127.0.0.1:9876/callback

/// Environment override for the sso-gateway base URL. Used by integration
/// tests and local development against a non-standard gateway address.
pub(crate) const SSO_URL_ENV: &str = "SUNBEAM_SSO_URL";

/// Environment variable carrying the CLI's public OAuth2 client ID.
pub(crate) const SSO_CLIENT_ID_ENV: &str = "SUNBEAM_SSO_CLIENT_ID";

/// Base URL of the sso-gateway for a domain.
///
/// Follows the platform's subdomain convention (`https://auth.{domain}` —
/// confirmed by sbbb; the gateway fronts the legacy Ory stack). The
/// [`SSO_URL_ENV`] variable overrides the derivation.
pub(crate) fn sso_base_url_for(domain: &str) -> Result<String> {
    if let Ok(u) = std::env::var(SSO_URL_ENV)
        && !u.is_empty()
    {
        return Ok(u.trim_end_matches('/').to_string());
    }
    if domain.is_empty() {
        return Err(SunbeamError::config(
            "no domain configured; set one with `sunbeam config set --domain ...`",
        ));
    }
    Ok(format!("https://auth.{domain}"))
}

/// Build an [`AuthClient`] for the sso-gateway at `base_url`.
///
/// When `token` is present the client injects `Authorization: Bearer <token>`
/// on every request. The tenant is resolved server-side from the token
/// subject, so no `x-tenant-id` header is needed.
pub(crate) fn build_auth_client(base_url: &str, token: Option<&str>) -> Result<AuthClient> {
    let builder = AuthClient::builder(base_url);
    let builder = match token {
        Some(t) => builder.auth(sunbeam_g2v::client::BearerToken::new(t)),
        None => builder,
    };
    let g2v = builder
        .build()
        .map_err(|e| SunbeamError::network(format!("failed to build auth client: {e}")))?;
    let uri = base_url
        .parse()
        .map_err(|e| SunbeamError::config(format!("invalid sso-gateway URL {base_url}: {e}")))?;
    AuthClient::new(g2v, uri)
        .map_err(|e| SunbeamError::network(format!("failed to build auth client: {e}")))
}

/// Authenticated [`AuthClient`] for the active context, using the logged-in
/// SSO token (see [`get_token`]).
pub(crate) async fn authenticated_auth_client() -> Result<AuthClient> {
    // Only genuine auth failures get the re-login hint — transport errors
    // already carry their own connectivity context.
    let token = get_token().await.map_err(|e| match e {
        SunbeamError::Identity(msg) => {
            SunbeamError::identity(format!("run `sunbeam auth login` first: {msg}"))
        }
        other => other,
    })?;
    let base_url = sso_base_url_for(sdk::config::domain())?;
    build_auth_client(&base_url, Some(&token))
}

// ---------------------------------------------------------------------------
// OIDC discovery
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
struct OidcDiscovery {
    token_endpoint: String,
    #[serde(default)]
    device_authorization_endpoint: Option<String>,
}

/// Resolve the domain for authentication, trying multiple sources.
async fn resolve_domain(explicit: Option<&str>) -> Result<String> {
    // 1. Explicit --domain flag
    if let Some(d) = explicit
        && !d.is_empty()
    {
        return Ok(d.to_string());
    }

    // 2. Active context domain (set by cli::dispatch from config)
    let ctx_domain = sdk::config::domain();
    if !ctx_domain.is_empty() {
        return Ok(ctx_domain.to_string());
    }

    // 3. Cached token domain (already logged in)
    let cfg = sdk::config::load_config();
    if let Some(domain) = cfg.auth.keys().find(|k| !k.is_empty()) {
        tracing::info!("Using cached domain: {domain}");
        return Ok(domain.clone());
    }

    // 4. Try cluster discovery (may fail if not connected)
    match sdk::kube::get_domain().await {
        Ok(d) if !d.is_empty() && !d.starts_with('.') => return Ok(d),
        _ => {}
    }

    Err(SunbeamError::config(
        "Could not determine domain. Use --domain flag, or configure with:\n  \
         sunbeam config set --host user@your-server.example.com",
    ))
}

/// Total attempts for the OIDC discovery fetch (initial try + retries).
/// Discovery is the first network hop of every login and token refresh, so a
/// single dropped packet on a flaky link must not fail the whole command.
const DISCOVERY_ATTEMPTS: u32 = 4;

/// Process-local cache of OIDC discovery documents per base URL. The
/// document changes ~never, so one successful fetch per process is enough.
fn discovery_cache() -> &'static Mutex<HashMap<String, OidcDiscovery>> {
    static CACHE: OnceLock<Mutex<HashMap<String, OidcDiscovery>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Lock the discovery cache, recovering from poisoning (a panicking writer
/// left no consistent state to protect here — entries are insert-only).
fn lock_discovery_cache() -> std::sync::MutexGuard<'static, HashMap<String, OidcDiscovery>> {
    discovery_cache().lock().unwrap_or_else(|p| p.into_inner())
}

/// Fetch the OIDC discovery document, retrying transient failures with
/// exponential backoff (250ms, 500ms, 1s) and caching the result per base URL.
async fn discover_oidc(base_url: &str) -> Result<OidcDiscovery> {
    if let Some(cached) = lock_discovery_cache().get(base_url) {
        return Ok(cached.clone());
    }

    let url = format!("{base_url}/.well-known/openid-configuration");
    let client = reqwest::Client::new();
    let mut attempt = 0;
    loop {
        attempt += 1;
        match fetch_discovery(&client, &url).await {
            Ok(discovery) => {
                lock_discovery_cache().insert(base_url.to_string(), discovery.clone());
                return Ok(discovery);
            }
            Err(DiscoveryFetchError::Transient(e)) if attempt < DISCOVERY_ATTEMPTS => {
                let backoff = Duration::from_millis(250 << (attempt - 1));
                tracing::warn!(
                    "OIDC discovery failed ({e}); retrying in {}ms (attempt {attempt}/{DISCOVERY_ATTEMPTS})",
                    backoff.as_millis()
                );
                tokio::time::sleep(backoff).await;
            }
            Err(DiscoveryFetchError::Transient(e)) | Err(DiscoveryFetchError::Permanent(e)) => {
                return Err(e);
            }
        }
    }
}

/// A failed discovery fetch, classified by whether a retry could help.
enum DiscoveryFetchError {
    /// Transport failure or HTTP 5xx — worth retrying.
    Transient(SunbeamError),
    /// HTTP 4xx or an unparseable body — retrying won't change the outcome.
    Permanent(SunbeamError),
}

/// One shot at fetching and parsing the discovery document.
async fn fetch_discovery(
    client: &reqwest::Client,
    url: &str,
) -> std::result::Result<OidcDiscovery, DiscoveryFetchError> {
    let resp = client.get(url).send().await.map_err(|e| {
        DiscoveryFetchError::Transient(SunbeamError::Network {
            context: format!("Failed to fetch OIDC discovery from {url}"),
            source: Some(e),
        })
    })?;

    if !resp.status().is_success() {
        let err = SunbeamError::network(format!(
            "OIDC discovery at {url} returned HTTP {}",
            resp.status()
        ));
        return Err(if resp.status().is_server_error() {
            DiscoveryFetchError::Transient(err)
        } else {
            DiscoveryFetchError::Permanent(err)
        });
    }

    resp.json().await.map_err(|e| {
        DiscoveryFetchError::Permanent(SunbeamError::Network {
            context: format!("Failed to parse OIDC discovery response from {url}"),
            source: Some(e),
        })
    })
}

// ---------------------------------------------------------------------------
// Token exchange / refresh
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    id_token: Option<String>,
}

/// Refresh an access token using a refresh token.
async fn refresh_token(domain: &str, cached: &AuthTokens) -> Result<AuthTokens> {
    let base_url = sso_base_url_for(domain)?;
    let discovery = match discover_oidc(&base_url).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };

    let client_id = resolve_client_id()?;

    let client = reqwest::Client::new();
    let resp = match client
        .post(&discovery.token_endpoint)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &cached.refresh_token),
            ("client_id", &client_id),
        ])
        .send()
        .await
        .ctx("Failed to refresh token")
    {
        Ok(resp) => resp,
        Err(e) => return Err(e),
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        // Only 4xx means the session itself was rejected (e.g. invalid_grant)
        // — 5xx is a server-side problem, not an expired session.
        return Err(if status.is_client_error() {
            SunbeamError::identity(format!("Token refresh failed (HTTP {status}): {body}"))
        } else {
            SunbeamError::network(format!("Token refresh failed (HTTP {status}): {body}"))
        });
    }

    let token_resp: TokenResponse = match resp
        .json()
        .await
        .ctx("Failed to parse refresh token response")
    {
        Ok(t) => t,
        Err(e) => return Err(e),
    };

    let expires_at = Utc::now() + chrono::Duration::seconds(token_resp.expires_in.unwrap_or(3600));

    let new_tokens = AuthTokens {
        access_token: token_resp.access_token,
        refresh_token: token_resp
            .refresh_token
            .unwrap_or_else(|| cached.refresh_token.clone()),
        expires_at,
        id_token: token_resp.id_token.or_else(|| cached.id_token.clone()),
    };

    match sdk::config::set_auth_tokens(domain, &new_tokens) {
        Ok(()) => Ok(new_tokens),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Device Authorization Grant (RFC 8628)
// ---------------------------------------------------------------------------
//
// The flow runs against the sso-gateway's public protocol endpoints
// (`/oauth2/device/auth`, `/oauth2/token`), discovered via OIDC. The
// ConnectRPC `iam.v1.OAuth2DeviceService` is NOT usable here: its methods
// require a caller token with `tenant:admin`/`application:admin` scope, which
// a CLI that is logging in does not have yet. The public endpoints are the
// same Hydra device grant the gateway wraps.

#[derive(Debug, Deserialize)]
struct DeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default = "default_device_interval")]
    interval: u64,
    #[serde(default = "default_device_expires")]
    expires_in: u64,
}

fn default_device_interval() -> u64 {
    5
}

fn default_device_expires() -> u64 {
    1800
}

#[derive(Debug, Deserialize)]
struct OAuth2ErrorResponse {
    error: String,
    #[allow(dead_code)]
    #[serde(default)]
    error_description: Option<String>,
}

fn device_authorization_endpoint(discovery: &OidcDiscovery, base_url: &str) -> String {
    discovery
        .device_authorization_endpoint
        .clone()
        .unwrap_or_else(|| format!("{base_url}/oauth2/device/auth"))
}

async fn request_device_code(
    endpoint: &str,
    client_id: &str,
) -> Result<DeviceAuthorizationResponse> {
    let client = reqwest::Client::new();
    let resp = match client
        .post(endpoint)
        .form(&[
            ("client_id", client_id),
            (
                "scope",
                "openid email profile offline_access identity:read identity:admin",
            ),
        ])
        .send()
        .await
        .with_ctx(|| format!("Failed to request device code from {endpoint}"))
    {
        Ok(resp) => resp,
        Err(e) => return Err(e),
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(SunbeamError::identity(format!(
            "Device authorization request failed (HTTP {status}): {body}"
        )));
    }

    let body = match resp
        .bytes()
        .await
        .ctx("Failed to read device authorization response")
    {
        Ok(b) => b,
        Err(e) => return Err(e),
    };
    serde_json::from_slice::<DeviceAuthorizationResponse>(&body)
        .ctx("Failed to parse device authorization response")
}

async fn poll_device_token(
    token_endpoint: &str,
    client_id: &str,
    device_code: &str,
    mut interval_secs: u64,
    expires_secs: u64,
) -> Result<TokenResponse> {
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();
    let expires = std::time::Duration::from_secs(expires_secs);

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;

        let resp = match client
            .post(token_endpoint)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
                ("client_id", client_id),
            ])
            .send()
            .await
            .ctx("Failed to poll device token endpoint")
        {
            Ok(resp) => resp,
            Err(e) => return Err(e),
        };

        if resp.status().is_success() {
            let body = match resp
                .bytes()
                .await
                .ctx("Failed to read device token response")
            {
                Ok(b) => b,
                Err(e) => return Err(e),
            };
            return serde_json::from_slice::<TokenResponse>(&body)
                .ctx("Failed to parse device token response");
        }

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let err = serde_json::from_str::<OAuth2ErrorResponse>(&body)
            .map(|e| e.error)
            .unwrap_or_else(|_| body.clone());

        match err.as_str() {
            "authorization_pending" => {}
            // WORKAROUND (upstream COE-2026-004): the gateway currently masks
            // `authorization_pending` as `server_error` on /oauth2/token
            // polls. Treat it as pending; the device-code expiry check below
            // still bounds the loop. Remove once the gateway is fixed.
            "server_error" => {}
            "slow_down" => {
                interval_secs += 5;
            }
            _ => {
                return Err(SunbeamError::identity(format!(
                    "Device token request failed (HTTP {status}): {body}"
                )));
            }
        }

        if start.elapsed() >= expires {
            return Err(SunbeamError::identity(
                "Device login timed out. Run `sunbeam auth login` to try again.",
            ));
        }
    }
}

/// Device login — OAuth2 Device Authorization Grant.
///
/// Prints a user code and verification URL, then polls the token endpoint until
/// the user authorizes the device. Tokens are cached so `sunbeam auth token`
/// and `crate::auth::get_token()` work identically for upstream API calls.
#[tracing::instrument(skip(domain_override))]
pub async fn cmd_auth_login(domain_override: Option<&str>) -> Result<()> {
    tracing::info!("Authenticating with the sso-gateway via device code");

    let domain = match resolve_domain(domain_override).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    let base_url = sso_base_url_for(&domain)?;
    let discovery = match discover_oidc(&base_url).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    let client_id = resolve_client_id()?;

    let device_endpoint = device_authorization_endpoint(&discovery, &base_url);
    let device_resp = match request_device_code(&device_endpoint, &client_id).await {
        Ok(r) => r,
        Err(e) => return Err(e),
    };

    println!("\n    Device code: {}\n", device_resp.user_code);
    println!(
        "    Open this URL in your browser: {}\n",
        device_resp.verification_uri
    );

    // Try to open the browser using the complete URI when available.
    let browser_url = device_resp
        .verification_uri_complete
        .as_ref()
        .unwrap_or(&device_resp.verification_uri);
    let _open_result = open_browser(browser_url);

    tracing::info!("Waiting for device authorization...");
    let token_resp = match poll_device_token(
        &discovery.token_endpoint,
        &client_id,
        &device_resp.device_code,
        device_resp.interval,
        device_resp.expires_in,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return Err(e),
    };

    let expires_at = Utc::now() + chrono::Duration::seconds(token_resp.expires_in.unwrap_or(3600));

    let tokens = AuthTokens {
        access_token: token_resp.access_token,
        refresh_token: token_resp.refresh_token.unwrap_or_default(),
        expires_at,
        id_token: token_resp.id_token.clone(),
    };

    let email = tokens.id_token.as_ref().and_then(|t| extract_email(t));
    if let Some(ref email) = email {
        tracing::info!("Logged in as {email}");
    } else {
        tracing::info!("Logged in successfully");
    }

    match sdk::config::set_auth_tokens(&domain, &tokens) {
        Ok(()) => Ok(()),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Client ID resolution
// ---------------------------------------------------------------------------

/// Resolve the OAuth2 client ID for device login.
///
/// The CLI is a public sso-gateway client (no secret), but the client ID is
/// platform-provisioned and deliberately not committed to the repo. Release
/// builds bake it in at compile time via `SUNBEAM_SSO_CLIENT_ID`
/// (`option_env!` — the same pattern AWS/Azure CLIs use for their public
/// client IDs); the same-named runtime env var overrides it for dev/tests.
/// (A `sdk::config` field has been requested as a third source.)
fn resolve_client_id() -> Result<String> {
    if let Ok(id) = std::env::var(SSO_CLIENT_ID_ENV)
        && !id.is_empty()
    {
        return Ok(id);
    }
    if let Some(id) = option_env!("SUNBEAM_SSO_CLIENT_ID")
        && !id.is_empty()
    {
        return Ok(id.to_string());
    }
    Err(SunbeamError::config(format!(
        "no SSO client ID configured; set {SSO_CLIENT_ID_ENV} to the \
         public client ID provisioned by your platform admin"
    )))
}

// ---------------------------------------------------------------------------
// JWT payload decoding (minimal, no verification)
// ---------------------------------------------------------------------------

/// Decode the payload of a JWT (middle segment) without verification.
/// Returns the parsed JSON value.
pub(crate) fn decode_jwt_payload(token: &str) -> Result<serde_json::Value> {
    let parts: Vec<&str> = token.splitn(3, '.').collect();
    if parts.len() < 2 {
        return Err(SunbeamError::identity("Invalid JWT: not enough segments"));
    }
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .ctx("Failed to base64-decode JWT payload")?;
    let payload: serde_json::Value =
        serde_json::from_slice(&payload_bytes).ctx("Failed to parse JWT payload as JSON")?;
    Ok(payload)
}

/// Extract the email claim from an id_token.
fn extract_email(id_token: &str) -> Option<String> {
    let payload = match decode_jwt_payload(id_token) {
        Ok(p) => p,
        Err(_) => return None,
    };
    payload
        .get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// sso-gateway identity resolution
// ---------------------------------------------------------------------------

/// Parse a kanban OIDC subject string, returning the sso-gateway identity ID
/// if present.
///
/// Accepts the `user:<id>` prefix, a bare gateway identity id (ULID), or a
/// legacy Kratos UUID for subjects minted before the sso-gateway migration.
fn identity_id_from_subject(subject: &str) -> Option<&str> {
    if let Some(id) = subject.strip_prefix("user:") {
        return if id.is_empty() { None } else { Some(id) };
    }
    if ulid::Ulid::from_string(subject).is_ok() || looks_like_uuid(subject) {
        return Some(subject);
    }
    None
}

fn looks_like_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().filter(|&c| c == '-').count() == 4
}

/// Format an sso-gateway identity ID as the kanban OIDC subject.
fn subject_from_identity_id(id: &str) -> String {
    format!("user:{id}")
}

/// Extract the email address from an identity's traits.
pub(crate) fn identity_email(identity: &iam::Identity) -> String {
    identity
        .traits
        .as_option()
        .and_then(|t| serde_json::to_value(t).ok())
        .and_then(|v| {
            v.get("email")
                .and_then(|e| e.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default()
}

/// List every identity in the caller's tenant, following pagination.
///
/// NOTE: `ListIdentities` has no server-side email filter (the Kratos
/// `credentials_identifier` lookup has no sso-gateway equivalent), so email
/// lookups page the full directory and filter client-side.
pub(crate) async fn list_all_identities(client: &AuthClient) -> Result<Vec<iam::Identity>> {
    let mut out = Vec::new();
    let mut page_token = String::new();
    loop {
        let resp = client
            .identity()
            .list_identities(iam::ListIdentitiesRequest {
                page: MessageField::some(iam::PageRequest {
                    page_size: 200,
                    page_token: page_token.clone(),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await?
            .into_owned();
        out.extend(resp.identities);
        let next = resp
            .page
            .as_option()
            .map(|p| p.next_page_token.clone())
            .unwrap_or_default();
        if next.is_empty() {
            return Ok(out);
        }
        page_token = next;
    }
}

/// Find an identity by gateway ID, ULID, or email.
pub(crate) async fn find_identity(
    client: &AuthClient,
    target: &str,
) -> Result<Option<iam::Identity>> {
    // Looks like an ID? Try direct lookup.
    if ulid::Ulid::from_string(target).is_ok() || looks_like_uuid(target) {
        return match client
            .identity()
            .get_identity(iam::GetIdentityRequest {
                id: target.to_string(),
                ..Default::default()
            })
            .await
        {
            Ok(resp) => Ok(Some(resp.into_owned())),
            Err(e) if e.code == connectrpc::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(SunbeamError::from(e)),
        };
    }

    // Otherwise treat as an email and filter client-side (see
    // list_all_identities NOTE).
    let identities = list_all_identities(client).await?;
    Ok(identities
        .into_iter()
        .find(|i| identity_email(i).eq_ignore_ascii_case(target)))
}

/// Resolve an email address to the OIDC subject used by the kanban backend.
///
/// Calls the sso-gateway `IdentityService` with the logged-in SSO token.
pub async fn resolve_subject_for_email(email: &str) -> Result<String> {
    let client = authenticated_auth_client().await?;
    let identity = find_identity(&client, email)
        .await?
        .ok_or_else(|| SunbeamError::identity(format!("Identity not found: {email}")))?;
    Ok(subject_from_identity_id(&identity.id))
}

/// Resolve an identity ULID (or `user:<ulid>` subject) to the OIDC subject
/// used by the kanban backend, verifying the identity exists via GetIdentity.
///
/// Anything that is not ULID/UUID-shaped is rejected locally. Calls the
/// sso-gateway `IdentityService` with the logged-in SSO token.
pub async fn resolve_verified_subject(raw: &str) -> Result<String> {
    let id = raw.strip_prefix("user:").unwrap_or(raw);
    if ulid::Ulid::from_string(id).is_err() && !looks_like_uuid(id) {
        return Err(SunbeamError::identity(format!(
            "unknown user '{raw}' — pass an email address or identity ULID"
        )));
    }
    let client = authenticated_auth_client().await?;
    let identity = find_identity(&client, id)
        .await?
        .ok_or_else(|| SunbeamError::identity(format!("Identity not found: {raw}")))?;
    Ok(subject_from_identity_id(&identity.id))
}

/// Resolve an OIDC subject to the user's email address.
///
/// Calls the sso-gateway `IdentityService` with the logged-in SSO token.
#[allow(dead_code)]
pub async fn resolve_email_for_subject(subject: &str) -> Result<String> {
    let id = identity_id_from_subject(subject)
        .ok_or_else(|| SunbeamError::identity(format!("Unrecognised subject format: {subject}")))?;
    let client = authenticated_auth_client().await?;
    let identity = find_identity(&client, id)
        .await?
        .ok_or_else(|| SunbeamError::identity(format!("Identity not found: {subject}")))?;
    Ok(identity_email(&identity))
}

/// Resolve multiple SSO subjects to email addresses.
///
/// Calls the sso-gateway `IdentityService` for each unique subject. Returns a
/// map of subject → email. Unresolvable subjects are omitted.
pub async fn resolve_emails_for_subjects(
    subjects: &[&str],
) -> Result<std::collections::HashMap<String, String>> {
    let client = authenticated_auth_client().await?;
    let mut map = std::collections::HashMap::new();
    for subject in subjects {
        if let Some(id) = identity_id_from_subject(subject)
            && let Ok(Some(identity)) = find_identity(&client, id).await
        {
            let email = identity_email(&identity);
            if !email.is_empty() {
                map.insert(subject.to_string(), email);
            }
        }
    }
    Ok(map)
}

/// Resolve multiple email addresses to SSO subjects in one batched request.
///
/// Lists the tenant directory once and filters client-side. Returns a map of
/// email → subject. Unresolvable emails are omitted.
#[allow(dead_code)]
pub async fn resolve_subjects_for_emails(
    emails: &[&str],
) -> Result<std::collections::HashMap<String, String>> {
    let client = authenticated_auth_client().await?;
    let identities = list_all_identities(&client).await?;
    let mut map = std::collections::HashMap::new();
    for email in emails {
        if let Some(identity) = identities
            .iter()
            .find(|i| identity_email(i).eq_ignore_ascii_case(email))
        {
            map.insert(email.to_string(), subject_from_identity_id(&identity.id));
        }
    }
    Ok(map)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get a valid access token, refreshing if needed.
///
/// Returns the access token string ready for use in Authorization headers.
/// If no cached token exists or refresh fails, returns an error prompting
/// the user to run `sunbeam auth login`.
#[tracing::instrument]
pub async fn get_token() -> Result<String> {
    let domain = sdk::config::domain();
    if domain.is_empty() {
        return Err(SunbeamError::config(
            "No domain configured; set one with `sunbeam config set --domain ...`",
        ));
    }

    let cached = match sdk::config::get_auth_tokens(domain) {
        Some(tokens) => tokens,
        None => {
            return Err(SunbeamError::identity(
                "Not logged in. Run `sunbeam auth login` to authenticate.",
            ));
        }
    };

    // Check if access token is still valid (>60s remaining)
    let now = Utc::now();
    if cached.expires_at > now + chrono::Duration::seconds(60) {
        return Ok(cached.access_token);
    }

    // Try to refresh
    if !cached.refresh_token.is_empty() {
        match refresh_token(domain, &cached).await {
            Ok(new_tokens) => return Ok(new_tokens.access_token),
            Err(SunbeamError::Network { context, source }) => {
                // Transport failure: the session was never evaluated, so don't
                // claim it expired — report the connectivity problem instead.
                return Err(SunbeamError::Network {
                    context: format!(
                        "{context} (sso-gateway unreachable; check your connection and try again)"
                    ),
                    source,
                });
            }
            Err(e @ SunbeamError::Identity(_)) => {
                tracing::error!("Token refresh failed: {e}");
            }
            Err(e) => return Err(e),
        }
    }

    Err(SunbeamError::identity(
        "Session expired. Run `sunbeam auth login` to re-authenticate.",
    ))
}

/// Force a token refresh regardless of the cached access token's remaining
/// lifetime, persisting the result like a normal refresh.
///
/// Recovery path for a server-side `unauthenticated` against a token the
/// local expiry check still considered valid (CLI-021).
pub(crate) async fn force_refresh_token() -> Result<String> {
    let domain = sdk::config::domain();
    if domain.is_empty() {
        return Err(SunbeamError::config(
            "No domain configured; set one with `sunbeam config set --domain ...`",
        ));
    }
    let cached = sdk::config::get_auth_tokens(domain).ok_or_else(|| {
        SunbeamError::identity("Not logged in. Run `sunbeam auth login` to authenticate.")
    })?;
    if cached.refresh_token.is_empty() {
        return Err(SunbeamError::identity(
            "Session expired. Run `sunbeam auth login` to re-authenticate.",
        ));
    }
    let new_tokens = refresh_token(domain, &cached).await?;
    Ok(new_tokens.access_token)
}

/// Print the current access token as a JSON headers object.
/// Designed for use as a Claude Code MCP `headersHelper`.
/// Output: {"Authorization": "Bearer <token>"}
#[tracing::instrument]
pub async fn cmd_auth_token() -> Result<()> {
    let token = match get_token().await {
        Ok(t) => t,
        Err(e) => return Err(e),
    };
    println!("{{\"Authorization\": \"Bearer {token}\"}}");
    Ok(())
}

/// Remove cached auth tokens.
#[tracing::instrument]
pub async fn cmd_auth_logout() -> Result<()> {
    let domain = sdk::config::domain();
    if domain.is_empty() {
        return Err(SunbeamError::config(
            "No domain configured; set one with `sunbeam config set --domain ...`",
        ));
    }

    if sdk::config::get_auth_tokens(domain).is_some() {
        match sdk::config::remove_auth_tokens(domain) {
            Ok(()) => tracing::info!("Logged out (cached tokens removed)"),
            Err(e) => return Err(e),
        }
    } else {
        tracing::info!("Not logged in (no cached tokens to remove)");
    }
    Ok(())
}

/// Print current auth status.
///
/// The status lines are the command's data output, so they go to stdout via
/// `println!` rather than the log pipeline (hidden by default at WARN).
#[tracing::instrument]
pub async fn cmd_auth_status() -> Result<()> {
    let domain = sdk::config::domain();
    if domain.is_empty() {
        return Err(SunbeamError::config(
            "No domain configured; set one with `sunbeam config set --domain ...`",
        ));
    }

    match sdk::config::get_auth_tokens(domain) {
        Some(tokens) => {
            let now = Utc::now();
            let expired = tokens.expires_at <= now;

            // Try to get email from id_token
            let identity = tokens
                .id_token
                .as_deref()
                .and_then(extract_email)
                .unwrap_or_else(|| "unknown".to_string());

            if expired {
                println!(
                    "Logged in as {identity} (token expired at {})",
                    tokens.expires_at.format("%Y-%m-%d %H:%M:%S UTC")
                );
                if !tokens.refresh_token.is_empty() {
                    println!("Token can be refreshed automatically on next use");
                }
            } else {
                println!(
                    "Logged in as {identity} (token valid until {})",
                    tokens.expires_at.format("%Y-%m-%d %H:%M:%S UTC")
                );
            }
            println!("Domain: {domain}");
        }
        None => {
            println!("Not logged in. Run `sunbeam auth login` to authenticate.");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

/// Try to open a URL in the default browser.
fn open_browser(url: &str) -> std::result::Result<(), std::io::Error> {
    #[cfg(target_os = "macos")]
    {
        match std::process::Command::new("open").arg(url).spawn() {
            Ok(_) => {}
            Err(e) => return Err(e),
        }
    }
    #[cfg(target_os = "linux")]
    {
        match std::process::Command::new("xdg-open").arg(url).spawn() {
            Ok(_) => {}
            Err(e) => return Err(e),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = url;
        // No-op on unsupported platforms; URL is printed to the terminal.
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_token_cache_roundtrip() {
        let tokens = AuthTokens {
            access_token: "access_abc".to_string(),
            refresh_token: "refresh_xyz".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
            id_token: Some(
                "eyJhbGciOiJSUzI1NiJ9.eyJlbWFpbCI6InRlc3RAZXhhbXBsZS5jb20ifQ.sig".to_string(),
            ),
        };

        let json = serde_json::to_string_pretty(&tokens).unwrap();
        let deserialized: AuthTokens = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.access_token, "access_abc");
        assert_eq!(deserialized.refresh_token, "refresh_xyz");
        assert!(deserialized.id_token.is_some());

        // Verify expires_at survives roundtrip (within 1 second tolerance)
        let diff = (deserialized.expires_at - tokens.expires_at)
            .num_milliseconds()
            .abs();
        assert!(diff < 1000, "expires_at drift: {diff}ms");
    }

    #[test]
    fn test_token_cache_roundtrip_no_id_token() {
        let tokens = AuthTokens {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
            id_token: None,
        };

        let json = serde_json::to_string(&tokens).unwrap();
        // id_token should be absent from the JSON when None
        assert!(!json.contains("id_token"));

        let deserialized: AuthTokens = serde_json::from_str(&json).unwrap();
        assert!(deserialized.id_token.is_none());
    }

    #[test]
    fn test_token_expiry_check_valid() {
        let tokens = AuthTokens {
            access_token: "valid".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
            id_token: None,
        };

        let now = Utc::now();
        // Token is valid: more than 60 seconds until expiry
        assert!(tokens.expires_at > now + Duration::seconds(60));
    }

    #[test]
    fn test_token_expiry_check_expired() {
        let tokens = AuthTokens {
            access_token: "expired".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() - Duration::hours(1),
            id_token: None,
        };

        let now = Utc::now();
        // Token is expired
        assert!(tokens.expires_at <= now + Duration::seconds(60));
    }

    #[test]
    fn test_token_expiry_check_almost_expired() {
        let tokens = AuthTokens {
            access_token: "almost".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: Utc::now() + Duration::seconds(30),
            id_token: None,
        };

        let now = Utc::now();
        // Token expires in 30s, which is within the 60s threshold
        assert!(tokens.expires_at <= now + Duration::seconds(60));
    }

    #[test]
    fn test_jwt_payload_decode() {
        // Build a fake JWT: header.payload.signature
        let payload_json = r#"{"email":"user@example.com","sub":"12345"}"#;
        let encoded_payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let fake_jwt = format!("eyJhbGciOiJSUzI1NiJ9.{encoded_payload}.fakesig");

        let payload = decode_jwt_payload(&fake_jwt).unwrap();
        assert_eq!(payload["email"], "user@example.com");
        assert_eq!(payload["sub"], "12345");
    }

    #[test]
    fn test_jwt_payload_decode_rejects_short_token() {
        assert!(decode_jwt_payload("no-segments").is_err());
    }

    #[test]
    fn test_extract_email() {
        let payload_json = r#"{"email":"alice@sunbeam.pt","name":"Alice"}"#;
        let encoded_payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let fake_jwt = format!("eyJhbGciOiJSUzI1NiJ9.{encoded_payload}.fakesig");

        assert_eq!(
            extract_email(&fake_jwt),
            Some("alice@sunbeam.pt".to_string())
        );
    }

    #[test]
    fn test_extract_email_missing() {
        let payload_json = r#"{"sub":"12345","name":"Bob"}"#;
        let encoded_payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let fake_jwt = format!("eyJhbGciOiJSUzI1NiJ9.{encoded_payload}.fakesig");

        assert_eq!(extract_email(&fake_jwt), None);
    }

    #[test]
    fn test_device_authorization_response_parses() {
        let json = r#"{
            "device_code": "device_123",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://sso.example.com/device",
            "verification_uri_complete": "https://sso.example.com/device?user_code=ABCD-EFGH",
            "interval": 5,
            "expires_in": 600
        }"#;
        let resp: DeviceAuthorizationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.device_code, "device_123");
        assert_eq!(resp.user_code, "ABCD-EFGH");
        assert_eq!(resp.verification_uri, "https://sso.example.com/device");
        assert_eq!(
            resp.verification_uri_complete,
            Some("https://sso.example.com/device?user_code=ABCD-EFGH".to_string())
        );
        assert_eq!(resp.interval, 5);
        assert_eq!(resp.expires_in, 600);
    }

    #[test]
    fn test_device_authorization_response_uses_defaults() {
        let json = r#"{
            "device_code": "device_123",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://sso.example.com/device"
        }"#;
        let resp: DeviceAuthorizationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.interval, 5);
        assert_eq!(resp.expires_in, 1800);
        assert!(resp.verification_uri_complete.is_none());
    }

    #[tokio::test]
    async fn test_request_device_code_hits_endpoint() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device/auth"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "dev_123",
                "user_code": "USER-CODE",
                "verification_uri": "https://sso.example.com/device",
                "verification_uri_complete": "https://sso.example.com/device?user_code=USER-CODE",
                "interval": 1,
                "expires_in": 300
            })))
            .mount(&server)
            .await;

        let resp = request_device_code(
            &format!("{}/oauth2/device/auth", server.uri()),
            "sunbeam-cli",
        )
        .await
        .unwrap();
        assert_eq!(resp.user_code, "USER-CODE");
        assert_eq!(resp.device_code, "dev_123");
    }

    #[tokio::test]
    async fn test_poll_device_token_retries_pending() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

        struct PendingThenSuccess(Arc<AtomicUsize>);
        impl Respond for PendingThenSuccess {
            fn respond(&self, _request: &Request) -> ResponseTemplate {
                let count = self.0.fetch_add(1, Ordering::SeqCst);
                if count < 2 {
                    ResponseTemplate::new(400).set_body_json(serde_json::json!({
                        "error": "authorization_pending"
                    }))
                } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "access_token": "access_abc",
                        "refresh_token": "refresh_xyz",
                        "expires_in": 3600,
                        "id_token": "eyJhbGciOiJIUzI1NiJ9.eyJlbWFpbCI6ImFsaWNlQGV4YW1wbGUuY29tIn0.sig"
                    }))
                }
            }
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(PendingThenSuccess(Arc::new(AtomicUsize::new(0))))
            .expect(3)
            .mount(&server)
            .await;

        let token = poll_device_token(
            &format!("{}/oauth2/token", server.uri()),
            "sunbeam-cli",
            "dev_123",
            1,
            30,
        )
        .await
        .unwrap();

        assert_eq!(token.access_token, "access_abc");
        assert_eq!(token.refresh_token, Some("refresh_xyz".to_string()));
        assert!(token.id_token.is_some());
    }

    #[tokio::test]
    async fn test_poll_device_token_respects_slow_down() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

        struct SlowDownThenSuccess(Arc<AtomicUsize>);
        impl Respond for SlowDownThenSuccess {
            fn respond(&self, _request: &Request) -> ResponseTemplate {
                let count = self.0.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    ResponseTemplate::new(400).set_body_json(serde_json::json!({
                        "error": "slow_down"
                    }))
                } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "access_token": "access_slow",
                        "expires_in": 3600
                    }))
                }
            }
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(SlowDownThenSuccess(Arc::new(AtomicUsize::new(0))))
            .expect(2)
            .mount(&server)
            .await;

        let start = std::time::Instant::now();
        let token = poll_device_token(
            &format!("{}/oauth2/token", server.uri()),
            "sunbeam-cli",
            "dev_123",
            1,
            30,
        )
        .await
        .unwrap();

        assert_eq!(token.access_token, "access_slow");
        // slow_down should have added 5s to the interval, so the second poll waits 6s total.
        assert!(start.elapsed() >= std::time::Duration::from_secs(6));
    }

    #[tokio::test]
    async fn test_poll_device_token_fatal_error_bails() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "access_denied",
                "error_description": "user denied the request"
            })))
            .mount(&server)
            .await;

        let err = poll_device_token(
            &format!("{}/oauth2/token", server.uri()),
            "sunbeam-cli",
            "dev_123",
            1,
            30,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("access_denied"), "err: {err}");
    }

    #[tokio::test]
    async fn test_poll_device_token_treats_server_error_as_pending() {
        // WORKAROUND coverage for upstream COE-2026-004: the gateway masks
        // authorization_pending as server_error; polling must continue.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "server_error"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "access_after_mask",
                "refresh_token": "refresh",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        let token = poll_device_token(
            &format!("{}/oauth2/token", server.uri()),
            "sunbeam-cli",
            "dev_123",
            1,
            30,
        )
        .await
        .unwrap();
        assert_eq!(token.access_token, "access_after_mask");
    }

    // ---------------------------------------------------------------------
    // Client ID resolution
    // ---------------------------------------------------------------------

    #[test]
    fn test_resolve_client_id_from_env() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var(SSO_CLIENT_ID_ENV, "client-from-env");
        }
        assert_eq!(resolve_client_id().unwrap(), "client-from-env");
    }

    #[test]
    fn test_resolve_client_id_unset_errors_without_build_time_value() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::remove_var(SSO_CLIENT_ID_ENV);
        }
        // Dev builds don't bake a value in; release builds do (option_env!).
        if option_env!("SUNBEAM_SSO_CLIENT_ID").is_none() {
            let err = resolve_client_id().unwrap_err();
            assert!(
                err.to_string().contains("SUNBEAM_SSO_CLIENT_ID"),
                "err: {err}"
            );
        }
    }

    // ---------------------------------------------------------------------
    // sso-gateway base URL derivation
    // ---------------------------------------------------------------------

    #[test]
    fn test_sso_base_url_for_domain() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::remove_var(SSO_URL_ENV);
        }
        assert_eq!(
            sso_base_url_for("sunbeam.pt").unwrap(),
            "https://auth.sunbeam.pt"
        );
    }

    #[test]
    fn test_sso_base_url_env_override() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var(SSO_URL_ENV, "http://127.0.0.1:8080/");
        }
        assert_eq!(
            sso_base_url_for("sunbeam.pt").unwrap(),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn test_sso_base_url_empty_domain_errors() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::remove_var(SSO_URL_ENV);
        }
        let err = sso_base_url_for("").unwrap_err();
        assert!(err.to_string().contains("no domain configured"));
    }

    #[test]
    fn test_build_auth_client_with_and_without_token() {
        build_auth_client("http://127.0.0.1:1", Some("token")).unwrap();
        build_auth_client("http://127.0.0.1:1", None).unwrap();
    }

    #[test]
    fn test_build_auth_client_rejects_invalid_url() {
        let err = build_auth_client("not a valid url ::://", None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid sso-gateway URL") || msg.contains("failed to build auth client"),
            "unexpected error: {msg}"
        );
    }

    // ---------------------------------------------------------------------
    // sso-gateway identity resolution
    // ---------------------------------------------------------------------

    #[test]
    fn test_identity_id_from_subject_prefixed() {
        let id = "01HZY9JTKKHK3Y6XJJYHZ9Q5TV";
        assert_eq!(identity_id_from_subject(&format!("user:{id}")), Some(id));
    }

    #[test]
    fn test_identity_id_from_subject_bare_ulid() {
        let id = "01HZY9JTKKHK3Y6XJJYHZ9Q5TV";
        assert_eq!(identity_id_from_subject(id), Some(id));
    }

    #[test]
    fn test_identity_id_from_subject_bare_uuid() {
        let id = "12345678-abcd-1234-abcd-123456789012";
        assert_eq!(identity_id_from_subject(id), Some(id));
    }

    #[test]
    fn test_identity_id_from_subject_rejects_other_formats() {
        assert_eq!(identity_id_from_subject("alice@example.com"), None);
        assert_eq!(identity_id_from_subject("user:"), None);
        assert_eq!(identity_id_from_subject("too-short"), None);
    }

    #[test]
    fn test_subject_from_identity_id() {
        assert_eq!(subject_from_identity_id("abc"), "user:abc");
    }

    #[test]
    fn test_identity_email_extraction() {
        use sdk::kanban::prelude::buffa_types::google::protobuf::Struct;
        let traits: Struct = serde_json::from_value(serde_json::json!({
            "email": "a@b.com",
            "name": "Alice"
        }))
        .unwrap();
        let identity = iam::Identity {
            id: "id-1".to_string(),
            traits: MessageField::some(traits),
            ..Default::default()
        };
        assert_eq!(identity_email(&identity), "a@b.com");

        let no_traits = iam::Identity {
            id: "id-2".to_string(),
            ..Default::default()
        };
        assert_eq!(identity_email(&no_traits), "");
    }

    #[test]
    fn test_device_authorization_endpoint_fallback() {
        let discovery = OidcDiscovery {
            token_endpoint: "https://sso.example.com/oauth2/token".to_string(),
            device_authorization_endpoint: None,
        };
        assert_eq!(
            device_authorization_endpoint(&discovery, "https://sso.example.com"),
            "https://sso.example.com/oauth2/device/auth"
        );

        let discovered = OidcDiscovery {
            token_endpoint: "https://sso.example.com/oauth2/token".to_string(),
            device_authorization_endpoint: Some("https://custom/device".to_string()),
        };
        assert_eq!(
            device_authorization_endpoint(&discovered, "https://sso.example.com"),
            "https://custom/device"
        );
    }

    // ---------------------------------------------------------------------
    // ConnectRPC identity resolution (wiremock)
    // ---------------------------------------------------------------------

    use crate::kanban::testutil;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ULID: &str = "01HZY9JTKKHK3Y6XJJYHZ9Q5TV";

    fn identity_with_email(id: &str, email: &str) -> iam::Identity {
        use sdk::kanban::prelude::buffa_types::google::protobuf::Struct;
        let traits: Struct = serde_json::from_value(serde_json::json!({ "email": email })).unwrap();
        iam::Identity {
            id: id.to_string(),
            tenant_id: "tenant-1".to_string(),
            schema_id: "default".to_string(),
            traits: MessageField::some(traits),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_find_identity_by_id_direct() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/GetIdentity"))
            .respond_with(testutil::proto_response(&identity_with_email(
                ULID, "a@b.com",
            )))
            .mount(&server)
            .await;

        let client = build_auth_client(&server.uri(), Some("token")).unwrap();
        let out = find_identity(&client, ULID).await.unwrap();
        assert_eq!(out.unwrap().id, ULID);
    }

    #[tokio::test]
    async fn test_find_identity_id_not_found_returns_none() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/GetIdentity"))
            .respond_with(testutil::connect_error(
                404,
                "not_found",
                "identity not found",
            ))
            .mount(&server)
            .await;

        let client = build_auth_client(&server.uri(), Some("token")).unwrap();
        let out = find_identity(&client, ULID).await.unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn test_find_identity_id_server_error_bails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/GetIdentity"))
            .respond_with(testutil::connect_error(500, "internal", "boom"))
            .mount(&server)
            .await;

        let client = build_auth_client(&server.uri(), Some("token")).unwrap();
        let err = find_identity(&client, ULID).await.unwrap_err();
        assert!(err.to_string().contains("boom"), "err: {err}");
    }

    #[tokio::test]
    async fn test_find_identity_by_email_filters_client_side() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/ListIdentities"))
            .respond_with(testutil::proto_response(&iam::ListIdentitiesResponse {
                identities: vec![
                    identity_with_email("id-1", "someone@b.com"),
                    identity_with_email("id-2", "a@b.com"),
                ],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = build_auth_client(&server.uri(), Some("token")).unwrap();
        let out = find_identity(&client, "a@b.com").await.unwrap();
        assert_eq!(out.unwrap().id, "id-2");
    }

    #[tokio::test]
    async fn test_find_identity_by_email_empty_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/ListIdentities"))
            .respond_with(testutil::proto_response(&iam::ListIdentitiesResponse {
                identities: vec![],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = build_auth_client(&server.uri(), Some("token")).unwrap();
        let out = find_identity(&client, "ghost@b.com").await.unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn test_list_all_identities_follows_pagination() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::{Request, Respond};

        struct Paged(Arc<AtomicUsize>);
        impl Respond for Paged {
            fn respond(&self, _request: &Request) -> ResponseTemplate {
                let page = self.0.fetch_add(1, Ordering::SeqCst);
                let msg = if page == 0 {
                    iam::ListIdentitiesResponse {
                        identities: vec![identity_with_email("id-1", "a@b.com")],
                        page: MessageField::some(iam::PageResponse {
                            next_page_token: "page-2".to_string(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }
                } else {
                    iam::ListIdentitiesResponse {
                        identities: vec![identity_with_email("id-2", "c@d.com")],
                        ..Default::default()
                    }
                };
                testutil::proto_response(&msg)
            }
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/ListIdentities"))
            .respond_with(Paged(Arc::new(AtomicUsize::new(0))))
            .expect(2)
            .mount(&server)
            .await;

        let client = build_auth_client(&server.uri(), Some("token")).unwrap();
        let all = list_all_identities(&client).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "id-1");
        assert_eq!(all[1].id, "id-2");
    }

    // ---------------------------------------------------------------------
    // Token-cache commands with HOME redirected to a tempdir
    // ---------------------------------------------------------------------

    fn temp_home() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        dir
    }

    fn set_domain(domain: &str) {
        sdk::config::set_active_context(sdk::config::Context {
            domain: domain.to_string(),
            ..Default::default()
        });
    }

    fn cached_tokens(expires_in_secs: i64, with_refresh: bool) -> AuthTokens {
        AuthTokens {
            access_token: "cached-access".to_string(),
            refresh_token: if with_refresh {
                "cached-refresh".to_string()
            } else {
                String::new()
            },
            expires_at: Utc::now() + Duration::seconds(expires_in_secs),
            id_token: None,
        }
    }

    #[tokio::test]
    async fn test_get_token_without_domain_errors() {
        let _home = temp_home();
        let err = get_token().await.unwrap_err();
        assert!(err.to_string().contains("No domain"), "err: {err}");
    }

    #[tokio::test]
    async fn test_get_token_not_logged_in_errors() {
        let _home = temp_home();
        set_domain("example.com");
        let err = get_token().await.unwrap_err();
        assert!(err.to_string().contains("Not logged in"), "err: {err}");
    }

    #[tokio::test]
    async fn test_get_token_returns_valid_cached_token() {
        let _home = temp_home();
        set_domain("example.com");
        sdk::config::set_auth_tokens("example.com", &cached_tokens(3600, true)).unwrap();

        let token = get_token().await.unwrap();
        assert_eq!(token, "cached-access");

        // cmd_auth_token prints the Authorization header object.
        cmd_auth_token().await.unwrap();
    }

    #[tokio::test]
    async fn test_get_token_expired_without_refresh_errors() {
        let _home = temp_home();
        set_domain("example.com");
        sdk::config::set_auth_tokens("example.com", &cached_tokens(-3600, false)).unwrap();

        let err = get_token().await.unwrap_err();
        assert!(err.to_string().contains("Session expired"), "err: {err}");
    }

    #[tokio::test]
    async fn test_cmd_auth_logout_removes_and_tolerates_absence() {
        let _home = temp_home();
        set_domain("example.com");
        sdk::config::set_auth_tokens("example.com", &cached_tokens(3600, true)).unwrap();

        cmd_auth_logout().await.unwrap();
        assert!(sdk::config::get_auth_tokens("example.com").is_none());

        // Second logout: no tokens cached — still Ok.
        cmd_auth_logout().await.unwrap();
    }

    #[tokio::test]
    async fn test_cmd_auth_logout_without_domain_errors() {
        let _home = temp_home();
        let err = cmd_auth_logout().await.unwrap_err();
        assert!(err.to_string().contains("No domain"), "err: {err}");
    }

    #[tokio::test]
    async fn test_cmd_auth_status_paths() {
        let _home = temp_home();
        set_domain("example.com");

        // Not logged in.
        cmd_auth_status().await.unwrap();

        // Valid token.
        sdk::config::set_auth_tokens("example.com", &cached_tokens(3600, true)).unwrap();
        cmd_auth_status().await.unwrap();

        // Expired token with refresh available.
        sdk::config::set_auth_tokens("example.com", &cached_tokens(-3600, true)).unwrap();
        cmd_auth_status().await.unwrap();
    }

    // ---------------------------------------------------------------------
    // OIDC discovery: retry, cache, error classification (wiremock)
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn test_discover_oidc_retries_5xx_then_succeeds() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::{Request, Respond};

        struct FlakyThenOk(Arc<AtomicUsize>);
        impl Respond for FlakyThenOk {
            fn respond(&self, _request: &Request) -> ResponseTemplate {
                let n = self.0.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    ResponseTemplate::new(500)
                } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "token_endpoint": "https://sso.example.com/oauth2/token"
                    }))
                }
            }
        }

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(FlakyThenOk(Arc::new(AtomicUsize::new(0))))
            .expect(3)
            .mount(&server)
            .await;

        let discovery = discover_oidc(&server.uri()).await.unwrap();
        assert_eq!(
            discovery.token_endpoint,
            "https://sso.example.com/oauth2/token"
        );
    }

    #[tokio::test]
    async fn test_discover_oidc_caches_per_base_url() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token_endpoint": "https://sso.example.com/oauth2/token"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let first = discover_oidc(&server.uri()).await.unwrap();
        let second = discover_oidc(&server.uri()).await.unwrap();
        assert_eq!(first.token_endpoint, second.token_endpoint);
    }

    #[tokio::test]
    async fn test_discover_oidc_4xx_is_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let err = discover_oidc(&server.uri()).await.unwrap_err();
        assert!(matches!(err, SunbeamError::Network { .. }), "err: {err:?}");
        assert!(err.to_string().contains("HTTP 404"), "err: {err}");
    }

    #[tokio::test]
    async fn test_discover_oidc_transport_error_is_network_not_session_expired() {
        // Port 9 (discard) on loopback refuses connections fast.
        let err = discover_oidc("http://127.0.0.1:9").await.unwrap_err();
        assert!(matches!(err, SunbeamError::Network { .. }), "err: {err:?}");
        assert!(!err.to_string().contains("Session expired"), "err: {err}");
    }

    // ---------------------------------------------------------------------
    // get_token error mapping: transport vs genuine auth failure
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_token_network_failure_is_not_session_expired() {
        let _home = temp_home();
        set_domain("example.com");
        sdk::config::set_auth_tokens("example.com", &cached_tokens(-3600, true)).unwrap();
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var(SSO_URL_ENV, "http://127.0.0.1:9");
            std::env::set_var(SSO_CLIENT_ID_ENV, "test-client");
        }

        let err = get_token().await.unwrap_err();
        assert!(matches!(err, SunbeamError::Network { .. }), "err: {err:?}");
        assert!(!err.to_string().contains("Session expired"), "err: {err}");
        assert!(err.to_string().contains("unreachable"), "err: {err}");
    }

    #[tokio::test]
    async fn test_get_token_refresh_5xx_is_network_not_session_expired() {
        let _home = temp_home();
        set_domain("example.com");
        sdk::config::set_auth_tokens("example.com", &cached_tokens(-3600, true)).unwrap();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token_endpoint": format!("{}/oauth2/token", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var(SSO_URL_ENV, server.uri());
            std::env::set_var(SSO_CLIENT_ID_ENV, "test-client");
        }

        let err = get_token().await.unwrap_err();
        assert!(matches!(err, SunbeamError::Network { .. }), "err: {err:?}");
        assert!(!err.to_string().contains("Session expired"), "err: {err}");
    }

    #[tokio::test]
    async fn test_get_token_invalid_grant_keeps_session_expired_guidance() {
        let _home = temp_home();
        set_domain("example.com");
        sdk::config::set_auth_tokens("example.com", &cached_tokens(-3600, true)).unwrap();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token_endpoint": format!("{}/oauth2/token", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "refresh token revoked"
            })))
            .mount(&server)
            .await;

        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var(SSO_URL_ENV, server.uri());
            std::env::set_var(SSO_CLIENT_ID_ENV, "test-client");
        }

        let err = get_token().await.unwrap_err();
        assert!(matches!(err, SunbeamError::Identity(_)), "err: {err:?}");
        assert!(err.to_string().contains("Session expired"), "err: {err}");
    }

    #[tokio::test]
    async fn test_get_token_refresh_persists_new_tokens() {
        let _home = temp_home();
        set_domain("example.com");
        sdk::config::set_auth_tokens("example.com", &cached_tokens(-3600, true)).unwrap();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token_endpoint": format!("{}/oauth2/token", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh-access",
                "refresh_token": "fresh-refresh",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var(SSO_URL_ENV, server.uri());
            std::env::set_var(SSO_CLIENT_ID_ENV, "test-client");
        }

        // Expired cached token: refresh kicks in, returns the fresh token.
        let token = get_token().await.unwrap();
        assert_eq!(token, "fresh-access");

        // CLI-021 (a): the refreshed tokens must be persisted so a later
        // `auth status` (which reads the store) shows the new expiry, and
        // the next process does not refresh with a stale refresh token.
        let stored = sdk::config::get_auth_tokens("example.com").unwrap();
        assert_eq!(stored.access_token, "fresh-access");
        assert_eq!(stored.refresh_token, "fresh-refresh");
        assert!(stored.expires_at > Utc::now() + chrono::Duration::minutes(50));

        // force_refresh_token bypasses the still-valid cache and refreshes
        // again, persisting once more.
        let forced = force_refresh_token().await.unwrap();
        assert_eq!(forced, "fresh-access");
        let stored = sdk::config::get_auth_tokens("example.com").unwrap();
        assert_eq!(stored.access_token, "fresh-access");
    }
}
