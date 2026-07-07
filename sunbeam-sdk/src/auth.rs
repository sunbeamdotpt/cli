//! OAuth2 Device Authorization Grant for CLI authentication against Hydra.

use crate::config::AuthTokens;
use crate::error::{Result, ResultExt, SunbeamError};
use base64::Engine;
use chrono::Utc;
use serde::Deserialize;

/// Hydra OAuth2 client ID for the Sunbeam CLI public client.
///
/// Client registration:
///   client_name: "Sunbeam CLI"
///   token_endpoint_auth_method: "none" (public client, no secret)
///   grant_types: authorization_code, refresh_token, urn:ietf:params:oauth:grant-type:device_code
///   response_types: ["code"]
///   scope: "openid email profile offline_access"
///   redirect_uris: http://localhost:9876-9880/callback, http://127.0.0.1:9876-9880/callback
///   post_logout_redirect_uris: http://localhost:9876/callback, http://127.0.0.1:9876/callback
const DEFAULT_CLIENT_ID: &str = "62c878f8-4229-4bf9-a73c-1e3aae0ae425";

// ---------------------------------------------------------------------------
// OIDC discovery
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
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
    let ctx_domain = crate::config::domain();
    if !ctx_domain.is_empty() {
        return Ok(ctx_domain.to_string());
    }

    // 3. Cached token domain (already logged in)
    let cfg = crate::config::load_config();
    if let Some(domain) = cfg.auth.keys().find(|k| !k.is_empty()) {
        tracing::info!("Using cached domain: {domain}");
        return Ok(domain.clone());
    }

    // 4. Try cluster discovery (may fail if not connected)
    match crate::kube::get_domain().await {
        Ok(d) if !d.is_empty() && !d.starts_with('.') => return Ok(d),
        _ => {}
    }

    Err(SunbeamError::config(
        "Could not determine domain. Use --domain flag, or configure with:\n  \
         sunbeam config set --host user@your-server.example.com",
    ))
}

async fn discover_oidc(domain: &str) -> Result<OidcDiscovery> {
    let url = format!("https://auth.{domain}/.well-known/openid-configuration");
    let client = reqwest::Client::new();
    let resp = match client
        .get(&url)
        .send()
        .await
        .with_ctx(|| format!("Failed to fetch OIDC discovery from {url}"))
    {
        Ok(resp) => resp,
        Err(e) => return Err(e),
    };

    if !resp.status().is_success() {
        return Err(SunbeamError::network(format!(
            "OIDC discovery returned HTTP {}",
            resp.status()
        )));
    }

    let discovery: OidcDiscovery = match resp
        .json()
        .await
        .ctx("Failed to parse OIDC discovery response")
    {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    Ok(discovery)
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
    let discovery = match discover_oidc(domain).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };

    // Try to get client_id from K8s, fall back to default
    let client_id = resolve_client_id().await;

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
        return Err(SunbeamError::identity(format!(
            "Token refresh failed (HTTP {status}): {body}"
        )));
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

    match crate::config::set_auth_tokens(domain, &new_tokens) {
        Ok(()) => Ok(new_tokens),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Device Authorization Grant (RFC 8628)
// ---------------------------------------------------------------------------

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

fn device_authorization_endpoint(discovery: &OidcDiscovery, domain: &str) -> String {
    discovery
        .device_authorization_endpoint
        .clone()
        .unwrap_or_else(|| format!("https://auth.{domain}/oauth2/device/auth"))
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
            ("scope", "openid email profile offline_access"),
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
    tracing::info!("Authenticating with Hydra via device code");

    let domain = match resolve_domain(domain_override).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    let discovery = match discover_oidc(&domain).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    let client_id = resolve_client_id().await;

    let device_endpoint = device_authorization_endpoint(&discovery, &domain);
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

    match crate::config::set_auth_tokens(&domain, &tokens) {
        Ok(()) => Ok(()),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Client ID resolution
// ---------------------------------------------------------------------------

/// Resolve the OAuth2 client ID for device login.
///
/// The CLI is a public Hydra client (no secret). The client_id is hardcoded
/// to match the pre-registered Sunbeam CLI client.
async fn resolve_client_id() -> String {
    DEFAULT_CLIENT_ID.to_string()
}

// ---------------------------------------------------------------------------
// JWT payload decoding (minimal, no verification)
// ---------------------------------------------------------------------------

/// Decode the payload of a JWT (middle segment) without verification.
/// Returns the parsed JSON value.
fn decode_jwt_payload(token: &str) -> Result<serde_json::Value> {
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
// Kratos identity resolution
// ---------------------------------------------------------------------------

/// Return the configured Kratos admin API base URL.
///
/// Reads `kratos_admin_url` from the active context. The URL must be
/// configured explicitly; no domain-based default is assumed.
pub fn kratos_admin_base_url() -> Result<String> {
    let ctx = crate::config::active_context();
    let url = ctx.kratos_admin_url.trim_end_matches('/').to_string();
    if url.is_empty() {
        return Err(SunbeamError::config(
            "kratos-admin-url is not set in the active context",
        ));
    }
    Ok(url)
}

/// Parse a kanban OIDC subject string, returning the Kratos identity ID if present.
fn identity_id_from_subject(subject: &str) -> Option<&str> {
    if let Some(id) = subject.strip_prefix("user:") {
        return Some(id);
    }
    // If it already looks like a UUID, treat it as the identity ID directly.
    if subject.len() == 36 && subject.chars().filter(|&c| c == '-').count() == 4 {
        return Some(subject);
    }
    None
}

/// Format a Kratos identity ID as the kanban OIDC subject.
fn subject_from_identity_id(id: &str) -> String {
    format!("user:{id}")
}

/// Find identity by UUID or email search. Returns the identity JSON.
async fn find_identity(base_url: &str, target: &str) -> Result<Option<serde_json::Value>> {
    let client = reqwest::Client::new();

    // Looks like a UUID? Try direct lookup.
    if target.len() == 36 && target.chars().filter(|&c| c == '-').count() == 4 {
        let url = format!("{base_url}/admin/identities/{target}");
        let resp = client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .with_ctx(|| format!("HTTP GET {url} failed"))?;
        let status = resp.status();
        if status.is_success() {
            let val: serde_json::Value =
                resp.json().await.ctx("Failed to parse identity response")?;
            return Ok(Some(val));
        }
        if status.as_u16() == 404 {
            return Ok(None);
        }
        let err_text = resp.text().await.unwrap_or_default();
        return Err(SunbeamError::identity(format!(
            "Kratos API error {status}: {err_text}"
        )));
    }

    // Search by email
    let url = format!("{base_url}/admin/identities?credentials_identifier={target}&page_size=1");
    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .with_ctx(|| format!("HTTP GET {url} failed"))?;
    let status = resp.status();
    if !status.is_success() {
        let err_text = resp.text().await.unwrap_or_default();
        return Err(SunbeamError::identity(format!(
            "Kratos API error {status}: {err_text}"
        )));
    }
    let val: serde_json::Value = resp
        .json()
        .await
        .ctx("Failed to parse identity list response")?;
    if let Some(serde_json::Value::Array(arr)) = &val.get("identities")
        && let Some(first) = arr.first()
    {
        return Ok(Some(first.clone()));
    }
    Ok(None)
}

/// Extract the email address from a Kratos identity JSON value.
fn identity_email(identity: &serde_json::Value) -> String {
    identity
        .get("traits")
        .and_then(|t| t.get("email"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Get identity ID as a string from a JSON value.
fn identity_id(identity: &serde_json::Value) -> Result<String> {
    identity
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| SunbeamError::identity("Identity missing 'id' field"))
}

/// Resolve an email address to the OIDC subject used by the kanban backend.
///
/// Calls the Kratos admin `/admin/identities` API directly. Configure the
/// endpoint with the `kratos-admin-url` field in the active context.
pub async fn resolve_subject_for_email(email: &str) -> Result<String> {
    let base_url = kratos_admin_base_url()?;
    let identity = find_identity(&base_url, email)
        .await?
        .ok_or_else(|| SunbeamError::identity(format!("Identity not found: {email}")))?;
    let id = identity_id(&identity)?;
    Ok(subject_from_identity_id(&id))
}

/// Resolve an OIDC subject to the user's email address.
///
/// Calls the Kratos admin `/admin/identities/{id}` API directly. Configure the
/// endpoint with the `kratos-admin-url` field in the active context.
pub async fn resolve_email_for_subject(subject: &str) -> Result<String> {
    let id = identity_id_from_subject(subject)
        .ok_or_else(|| SunbeamError::identity(format!("Unrecognised subject format: {subject}")))?;
    let base_url = kratos_admin_base_url()?;
    let identity = find_identity(&base_url, id)
        .await?
        .ok_or_else(|| SunbeamError::identity(format!("Identity not found: {subject}")))?;
    Ok(identity_email(&identity))
}

/// Resolve multiple SSO subjects to email addresses in one batched request.
///
/// Calls the Kratos admin `/admin/identities` API for each unique subject.
/// Returns a map of subject → email. Unresolvable subjects are omitted.
pub async fn resolve_emails_for_subjects(
    subjects: &[&str],
) -> Result<std::collections::HashMap<String, String>> {
    let base_url = kratos_admin_base_url()?;
    let mut map = std::collections::HashMap::new();
    for subject in subjects {
        if let Some(id) = identity_id_from_subject(subject)
            && let Ok(Some(identity)) = find_identity(&base_url, id).await
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
/// Calls the Kratos admin `/admin/identities` API for each unique email.
/// Returns a map of email → subject. Unresolvable emails are omitted.
pub async fn resolve_subjects_for_emails(
    emails: &[&str],
) -> Result<std::collections::HashMap<String, String>> {
    let base_url = kratos_admin_base_url()?;
    let mut map = std::collections::HashMap::new();
    for email in emails {
        if let Ok(Some(identity)) = find_identity(&base_url, email).await
            && let Ok(id) = identity_id(&identity)
        {
            map.insert(email.to_string(), subject_from_identity_id(&id));
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
    let domain = crate::config::domain();
    if domain.is_empty() {
        return Err(SunbeamError::config(
            "No domain configured; set one with `sunbeam config set --domain ...`",
        ));
    }

    let cached = match crate::config::get_auth_tokens(domain) {
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
            Err(e) => {
                tracing::error!("Token refresh failed: {e}");
            }
        }
    }

    Err(SunbeamError::identity(
        "Session expired. Run `sunbeam auth login` to re-authenticate.",
    ))
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
    let domain = crate::config::domain();
    if domain.is_empty() {
        return Err(SunbeamError::config(
            "No domain configured; set one with `sunbeam config set --domain ...`",
        ));
    }

    if crate::config::get_auth_tokens(domain).is_some() {
        match crate::config::remove_auth_tokens(domain) {
            Ok(()) => tracing::info!("Logged out (cached tokens removed)"),
            Err(e) => return Err(e),
        }
    } else {
        tracing::info!("Not logged in (no cached tokens to remove)");
    }
    Ok(())
}

/// Print current auth status.
#[tracing::instrument]
pub async fn cmd_auth_status() -> Result<()> {
    let domain = crate::config::domain();
    if domain.is_empty() {
        return Err(SunbeamError::config(
            "No domain configured; set one with `sunbeam config set --domain ...`",
        ));
    }

    match crate::config::get_auth_tokens(domain) {
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
                tracing::info!(
                    "Logged in as {identity} (token expired at {})",
                    tokens.expires_at.format("%Y-%m-%d %H:%M:%S UTC")
                );
                if !tokens.refresh_token.is_empty() {
                    tracing::info!("Token can be refreshed automatically on next use");
                }
            } else {
                tracing::info!(
                    "Logged in as {identity} (token valid until {})",
                    tokens.expires_at.format("%Y-%m-%d %H:%M:%S UTC")
                );
            }
            tracing::info!("Domain: {domain}");
        }
        None => {
            tracing::info!("Not logged in. Run `sunbeam auth login` to authenticate.");
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
            "verification_uri": "https://auth.example.com/device",
            "verification_uri_complete": "https://auth.example.com/device?user_code=ABCD-EFGH",
            "interval": 5,
            "expires_in": 600
        }"#;
        let resp: DeviceAuthorizationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.device_code, "device_123");
        assert_eq!(resp.user_code, "ABCD-EFGH");
        assert_eq!(resp.verification_uri, "https://auth.example.com/device");
        assert_eq!(
            resp.verification_uri_complete,
            Some("https://auth.example.com/device?user_code=ABCD-EFGH".to_string())
        );
        assert_eq!(resp.interval, 5);
        assert_eq!(resp.expires_in, 600);
    }

    #[test]
    fn test_device_authorization_response_uses_defaults() {
        let json = r#"{
            "device_code": "device_123",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://auth.example.com/device"
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
            .and(path("/device/auth"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "dev_123",
                "user_code": "USER-CODE",
                "verification_uri": "https://auth.example.com/device",
                "verification_uri_complete": "https://auth.example.com/device?user_code=USER-CODE",
                "interval": 1,
                "expires_in": 300
            })))
            .mount(&server)
            .await;

        let resp = request_device_code(&format!("{}/device/auth", server.uri()), "sunbeam-cli")
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
            .and(path("/token"))
            .respond_with(PendingThenSuccess(Arc::new(AtomicUsize::new(0))))
            .expect(3)
            .mount(&server)
            .await;

        let token = poll_device_token(
            &format!("{}/token", server.uri()),
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
            .and(path("/token"))
            .respond_with(SlowDownThenSuccess(Arc::new(AtomicUsize::new(0))))
            .expect(2)
            .mount(&server)
            .await;

        let start = std::time::Instant::now();
        let token = poll_device_token(
            &format!("{}/token", server.uri()),
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
}
