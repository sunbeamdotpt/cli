//! OAuth2 Authorization Code flow with PKCE for CLI authentication against Hydra.

use crate::error::{Result, ResultExt, SunbeamError};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Token cache data
// ---------------------------------------------------------------------------

/// Cached OAuth2 tokens persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    pub domain: String,
}

/// Default client ID when the K8s secret is unavailable.
const DEFAULT_CLIENT_ID: &str = "sunbeam-cli";

// ---------------------------------------------------------------------------
// Cache file helpers
// ---------------------------------------------------------------------------

fn cache_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local/share")
        })
        .join("sunbeam")
        .join("auth.json")
}

fn read_cache() -> Result<AuthTokens> {
    let path = cache_path();
    let content = std::fs::read_to_string(&path).map_err(|e| {
        SunbeamError::Identity(format!("No cached auth tokens ({}): {e}", path.display()))
    })?;
    let tokens: AuthTokens = serde_json::from_str(&content)
        .ctx("Failed to parse cached auth tokens")?;
    Ok(tokens)
}

fn write_cache(tokens: &AuthTokens) -> Result<()> {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_ctx(|| format!("Failed to create auth cache dir: {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(tokens)?;
    std::fs::write(&path, &content)
        .with_ctx(|| format!("Failed to write auth cache to {}", path.display()))?;

    // Set 0600 permissions on unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms)
            .with_ctx(|| format!("Failed to set permissions on {}", path.display()))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// PKCE
// ---------------------------------------------------------------------------

/// Generate a PKCE code_verifier and code_challenge (S256).
fn generate_pkce() -> (String, String) {
    let verifier_bytes: [u8; 32] = rand::random();
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);
    let challenge = {
        let hash = Sha256::digest(verifier.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
    };
    (verifier, challenge)
}

/// Generate a random state parameter for OAuth2.
fn generate_state() -> String {
    let bytes: [u8; 16] = rand::random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

// ---------------------------------------------------------------------------
// OIDC discovery
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OidcDiscovery {
    authorization_endpoint: String,
    token_endpoint: String,
}

/// Resolve the domain for authentication, trying multiple sources.
async fn resolve_domain(explicit: Option<&str>) -> Result<String> {
    // 1. Explicit --domain flag
    if let Some(d) = explicit {
        if !d.is_empty() {
            return Ok(d.to_string());
        }
    }

    // 2. Cached token domain (already logged in to a domain)
    if let Ok(tokens) = read_cache() {
        if !tokens.domain.is_empty() {
            crate::output::ok(&format!("Using cached domain: {}", tokens.domain));
            return Ok(tokens.domain);
        }
    }

    // 3. Config: derive from production_host
    let config = crate::config::load_config();
    if !config.production_host.is_empty() {
        let host = &config.production_host;
        let raw = host.split('@').last().unwrap_or(host);
        let raw = raw.split(':').next().unwrap_or(raw);
        // Take the last 2+ segments as the domain (e.g. admin.sunbeam.pt -> sunbeam.pt)
        let parts: Vec<&str> = raw.split('.').collect();
        if parts.len() >= 2 {
            let domain = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
            return Ok(domain);
        }
    }

    // 4. Try cluster discovery (may fail if not connected)
    match crate::kube::get_domain().await {
        Ok(d) if !d.is_empty() && !d.starts_with(".") => return Ok(d),
        _ => {}
    }

    Err(SunbeamError::config(
        "Could not determine domain. Use --domain flag, or configure with:\n  \
         sunbeam config set --host user@your-server.example.com"
    ))
}

async fn discover_oidc(domain: &str) -> Result<OidcDiscovery> {
    let url = format!("https://auth.{domain}/.well-known/openid-configuration");
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .send()
        .await
        .with_ctx(|| format!("Failed to fetch OIDC discovery from {url}"))?;

    if !resp.status().is_success() {
        return Err(SunbeamError::network(format!(
            "OIDC discovery returned HTTP {}",
            resp.status()
        )));
    }

    let discovery: OidcDiscovery = resp
        .json()
        .await
        .ctx("Failed to parse OIDC discovery response")?;
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

async fn exchange_code(
    token_endpoint: &str,
    code: &str,
    redirect_uri: &str,
    client_id: &str,
    code_verifier: &str,
) -> Result<TokenResponse> {
    let client = reqwest::Client::new();
    let resp = client
        .post(token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .ctx("Failed to exchange authorization code")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(SunbeamError::identity(format!(
            "Token exchange failed (HTTP {status}): {body}"
        )));
    }

    let token_resp: TokenResponse = resp.json().await.ctx("Failed to parse token response")?;
    Ok(token_resp)
}

/// Refresh an access token using a refresh token.
async fn refresh_token(cached: &AuthTokens) -> Result<AuthTokens> {
    let discovery = discover_oidc(&cached.domain).await?;

    // Try to get client_id from K8s, fall back to default
    let client_id = resolve_client_id().await;

    let client = reqwest::Client::new();
    let resp = client
        .post(&discovery.token_endpoint)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &cached.refresh_token),
            ("client_id", &client_id),
        ])
        .send()
        .await
        .ctx("Failed to refresh token")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(SunbeamError::identity(format!(
            "Token refresh failed (HTTP {status}): {body}"
        )));
    }

    let token_resp: TokenResponse = resp
        .json()
        .await
        .ctx("Failed to parse refresh token response")?;

    let expires_at = Utc::now()
        + chrono::Duration::seconds(token_resp.expires_in.unwrap_or(3600));

    let new_tokens = AuthTokens {
        access_token: token_resp.access_token,
        refresh_token: token_resp
            .refresh_token
            .unwrap_or_else(|| cached.refresh_token.clone()),
        expires_at,
        id_token: token_resp.id_token.or_else(|| cached.id_token.clone()),
        domain: cached.domain.clone(),
    };

    write_cache(&new_tokens)?;
    Ok(new_tokens)
}

// ---------------------------------------------------------------------------
// Client ID resolution
// ---------------------------------------------------------------------------

/// Try to read the client_id from K8s secret `oidc-sunbeam-cli` in `ory` namespace.
/// Falls back to the default client ID.
async fn resolve_client_id() -> String {
    match crate::kube::kube_get_secret_field("ory", "oidc-sunbeam-cli", "client_id").await {
        Ok(id) if !id.is_empty() => id,
        _ => DEFAULT_CLIENT_ID.to_string(),
    }
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
    let payload = decode_jwt_payload(id_token).ok()?;
    payload
        .get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// HTTP callback server
// ---------------------------------------------------------------------------

/// Parsed callback parameters from the OAuth2 redirect.
struct CallbackParams {
    code: String,
    #[allow(dead_code)]
    state: String,
}

/// Bind a TCP listener for the OAuth2 callback, preferring ports 9876-9880.
async fn bind_callback_listener() -> Result<(tokio::net::TcpListener, u16)> {
    for port in 9876..=9880 {
        if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            return Ok((listener, port));
        }
    }
    // Fall back to ephemeral port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .ctx("Failed to bind callback listener")?;
    let port = listener.local_addr().ctx("No local address")?.port();
    Ok((listener, port))
}

/// Wait for a single HTTP callback request, extract code and state, send HTML response.
async fn wait_for_callback(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<CallbackParams> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut stream, _) = listener.accept().await.ctx("Failed to accept callback connection")?;

    let mut buf = vec![0u8; 4096];
    let n = stream
        .read(&mut buf)
        .await
        .ctx("Failed to read callback request")?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse the GET request line: "GET /callback?code=...&state=... HTTP/1.1"
    let request_line = request
        .lines()
        .next()
        .ctx("Empty callback request")?;

    let path = request_line
        .split_whitespace()
        .nth(1)
        .ctx("No path in callback request")?;

    // Parse query params
    let query = path
        .split('?')
        .nth(1)
        .ctx("No query params in callback")?;

    let mut code = None;
    let mut state = None;

    for param in query.split('&') {
        let mut kv = param.splitn(2, '=');
        match (kv.next(), kv.next()) {
            (Some("code"), Some(v)) => code = Some(v.to_string()),
            (Some("state"), Some(v)) => state = Some(v.to_string()),
            _ => {}
        }
    }

    let code = code.ok_or_else(|| SunbeamError::identity("No 'code' in callback"))?;
    let state = state.ok_or_else(|| SunbeamError::identity("No 'state' in callback"))?;

    if state != expected_state {
        return Err(SunbeamError::identity(
            "OAuth2 state mismatch -- possible CSRF attack",
        ));
    }

    // Send success response
    let html = concat!(
        "<!DOCTYPE html><html><body>",
        "<h2>Authentication successful</h2>",
        "<p>You can close this tab and return to the terminal.</p>",
        "</body></html>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;

    Ok(CallbackParams { code, state })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get a valid access token, refreshing if needed.
///
/// Returns the access token string ready for use in Authorization headers.
/// If no cached token exists or refresh fails, returns an error prompting
/// the user to run `sunbeam auth login`.
pub async fn get_token() -> Result<String> {
    let cached = match read_cache() {
        Ok(tokens) => tokens,
        Err(_) => {
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
        match refresh_token(&cached).await {
            Ok(new_tokens) => return Ok(new_tokens.access_token),
            Err(e) => {
                crate::output::warn(&format!("Token refresh failed: {e}"));
            }
        }
    }

    Err(SunbeamError::identity(
        "Session expired. Run `sunbeam auth login` to re-authenticate.",
    ))
}

/// Interactive browser-based OAuth2 login.
pub async fn cmd_auth_login(domain_override: Option<&str>) -> Result<()> {
    crate::output::step("Authenticating with Hydra");

    // Resolve domain: explicit flag > cached token domain > config > cluster discovery
    let domain = resolve_domain(domain_override).await?;

    crate::output::ok(&format!("Domain: {domain}"));

    // OIDC discovery
    let discovery = discover_oidc(&domain).await?;

    // Resolve client_id
    let client_id = resolve_client_id().await;

    // Generate PKCE
    let (code_verifier, code_challenge) = generate_pkce();

    // Generate state
    let state = generate_state();

    // Bind callback listener
    let (listener, port) = bind_callback_listener().await?;
    let redirect_uri = format!("http://localhost:{port}/callback");

    // Build authorization URL
    let auth_url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        discovery.authorization_endpoint,
        urlencoding(&client_id),
        urlencoding(&redirect_uri),
        "openid%20email%20profile%20offline_access",
        code_challenge,
        state,
    );

    crate::output::ok("Opening browser for login...");
    println!("\n    {auth_url}\n");

    // Try to open the browser
    let _open_result = open_browser(&auth_url);

    // Wait for callback
    crate::output::ok("Waiting for authentication callback...");
    let callback = wait_for_callback(listener, &state).await?;

    // Exchange code for tokens
    crate::output::ok("Exchanging authorization code for tokens...");
    let token_resp = exchange_code(
        &discovery.token_endpoint,
        &callback.code,
        &redirect_uri,
        &client_id,
        &code_verifier,
    )
    .await?;

    let expires_at = Utc::now()
        + chrono::Duration::seconds(token_resp.expires_in.unwrap_or(3600));

    let tokens = AuthTokens {
        access_token: token_resp.access_token,
        refresh_token: token_resp.refresh_token.unwrap_or_default(),
        expires_at,
        id_token: token_resp.id_token.clone(),
        domain: domain.clone(),
    };

    write_cache(&tokens)?;

    // Print success with email if available
    if let Some(ref id_token) = tokens.id_token {
        if let Some(email) = extract_email(id_token) {
            crate::output::ok(&format!("Logged in as {email}"));
        } else {
            crate::output::ok("Logged in successfully");
        }
    } else {
        crate::output::ok("Logged in successfully");
    }

    Ok(())
}

/// Remove cached auth tokens.
pub async fn cmd_auth_logout() -> Result<()> {
    let path = cache_path();
    if path.exists() {
        std::fs::remove_file(&path)
            .with_ctx(|| format!("Failed to remove {}", path.display()))?;
        crate::output::ok("Logged out (cached tokens removed)");
    } else {
        crate::output::ok("Not logged in (no cached tokens to remove)");
    }
    Ok(())
}

/// Print current auth status.
pub async fn cmd_auth_status() -> Result<()> {
    match read_cache() {
        Ok(tokens) => {
            let now = Utc::now();
            let expired = tokens.expires_at <= now;

            // Try to get email from id_token
            let identity = tokens
                .id_token
                .as_deref()
                .and_then(extract_email)
                .unwrap_or_else(|| "unknown".to_string());

            if expired {
                crate::output::ok(&format!(
                    "Logged in as {identity} (token expired at {})",
                    tokens.expires_at.format("%Y-%m-%d %H:%M:%S UTC")
                ));
                if !tokens.refresh_token.is_empty() {
                    crate::output::ok("Token can be refreshed automatically on next use");
                }
            } else {
                crate::output::ok(&format!(
                    "Logged in as {identity} (token valid until {})",
                    tokens.expires_at.format("%Y-%m-%d %H:%M:%S UTC")
                ));
            }
            crate::output::ok(&format!("Domain: {}", tokens.domain));
        }
        Err(_) => {
            crate::output::ok("Not logged in. Run `sunbeam auth login` to authenticate.");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

/// Minimal percent-encoding for URL query parameters.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

/// Try to open a URL in the default browser.
fn open_browser(url: &str) -> std::result::Result<(), std::io::Error> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
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
    fn test_pkce_generation() {
        let (verifier, challenge) = generate_pkce();

        // Verifier should be base64url-encoded 32 bytes -> 43 chars
        assert_eq!(verifier.len(), 43);

        // Challenge should be base64url-encoded SHA256 -> 43 chars
        assert_eq!(challenge.len(), 43);

        // Verify the challenge matches the verifier
        let expected_hash = Sha256::digest(verifier.as_bytes());
        let expected_challenge =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(expected_hash);
        assert_eq!(challenge, expected_challenge);

        // Two calls should produce different values
        let (v2, c2) = generate_pkce();
        assert_ne!(verifier, v2);
        assert_ne!(challenge, c2);
    }

    #[test]
    fn test_token_cache_roundtrip() {
        let tokens = AuthTokens {
            access_token: "access_abc".to_string(),
            refresh_token: "refresh_xyz".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
            id_token: Some("eyJhbGciOiJSUzI1NiJ9.eyJlbWFpbCI6InRlc3RAZXhhbXBsZS5jb20ifQ.sig".to_string()),
            domain: "sunbeam.pt".to_string(),
        };

        let json = serde_json::to_string_pretty(&tokens).unwrap();
        let deserialized: AuthTokens = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.access_token, "access_abc");
        assert_eq!(deserialized.refresh_token, "refresh_xyz");
        assert_eq!(deserialized.domain, "sunbeam.pt");
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
            domain: "example.com".to_string(),
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
            domain: "example.com".to_string(),
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
            domain: "example.com".to_string(),
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
            domain: "example.com".to_string(),
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

        assert_eq!(extract_email(&fake_jwt), Some("alice@sunbeam.pt".to_string()));
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
    fn test_urlencoding() {
        assert_eq!(urlencoding("hello"), "hello");
        assert_eq!(urlencoding("hello world"), "hello%20world");
        assert_eq!(
            urlencoding("http://localhost:9876/callback"),
            "http%3A%2F%2Flocalhost%3A9876%2Fcallback"
        );
    }

    #[test]
    fn test_generate_state() {
        let s1 = generate_state();
        let s2 = generate_state();
        assert_ne!(s1, s2);
        // 16 bytes base64url -> 22 chars
        assert_eq!(s1.len(), 22);
    }

    #[test]
    fn test_cache_path_is_under_sunbeam() {
        let path = cache_path();
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("sunbeam"));
        assert!(path_str.ends_with("auth.json"));
    }
}
