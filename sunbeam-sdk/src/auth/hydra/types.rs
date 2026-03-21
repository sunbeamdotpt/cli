//! Hydra admin API types.

use serde::{Deserialize, Serialize};

/// An OAuth2 client registration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuth2Client {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uris: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_types: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_types: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// Token lifespan configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenLifespans {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_code_grant_access_token_lifespan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_code_grant_id_token_lifespan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_code_grant_refresh_token_lifespan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_credentials_grant_access_token_lifespan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implicit_grant_access_token_lifespan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implicit_grant_id_token_lifespan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwt_bearer_grant_access_token_lifespan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_grant_access_token_lifespan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_grant_id_token_lifespan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_grant_refresh_token_lifespan: Option<String>,
}

/// Login request details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    #[serde(default)]
    pub challenge: String,
    #[serde(default)]
    pub requested_scope: Vec<String>,
    #[serde(default)]
    pub requested_access_token_audience: Vec<String>,
    #[serde(default)]
    pub skip: bool,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub client: Option<OAuth2Client>,
    #[serde(default)]
    pub request_url: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub oidc_context: Option<serde_json::Value>,
}

/// Body for accepting a login request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptLoginBody {
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remember: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remember_for: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amr: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_subject_identifier: Option<String>,
}

/// Consent request details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentRequest {
    #[serde(default)]
    pub challenge: String,
    #[serde(default)]
    pub requested_scope: Vec<String>,
    #[serde(default)]
    pub requested_access_token_audience: Vec<String>,
    #[serde(default)]
    pub skip: bool,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub client: Option<OAuth2Client>,
    #[serde(default)]
    pub request_url: String,
    #[serde(default)]
    pub login_challenge: Option<String>,
    #[serde(default)]
    pub login_session_id: Option<String>,
    #[serde(default)]
    pub acr: Option<String>,
    #[serde(default)]
    pub amr: Option<Vec<String>>,
    #[serde(default)]
    pub context: Option<serde_json::Value>,
    #[serde(default)]
    pub oidc_context: Option<serde_json::Value>,
}

/// Body for accepting a consent request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptConsentBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_scope: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_access_token_audience: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<ConsentSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remember: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remember_for: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handled_at: Option<String>,
}

/// Consent session data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsentSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_request: Option<Box<ConsentRequest>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_scope: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_access_token_audience: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Box<ConsentSessionData>>,
}

/// Inner session data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsentSessionData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<serde_json::Value>,
}

/// Body for rejecting a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_debug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<i64>,
}

/// Redirect response from accept/reject operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectResponse {
    pub redirect_to: String,
}

/// Logout request details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogoutRequest {
    #[serde(default)]
    pub challenge: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub request_url: Option<String>,
    #[serde(default)]
    pub rp_initiated: bool,
    #[serde(default)]
    pub client: Option<OAuth2Client>,
}

/// A JWK set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JwkSet {
    #[serde(default)]
    pub keys: Vec<serde_json::Value>,
}

/// Body for creating a JWK set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateJwkBody {
    pub alg: String,
    pub kid: String,
    #[serde(rename = "use")]
    pub use_: String,
}

/// A trusted JWT issuer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedJwtIssuer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub issuer: String,
    pub subject: String,
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<TrustedIssuerKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// Public key info for a trusted issuer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedIssuerKey {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
}

/// Token introspection result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntrospectResult {
    pub active: bool,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub sub: Option<String>,
    #[serde(default)]
    pub exp: Option<i64>,
    #[serde(default)]
    pub iat: Option<i64>,
    #[serde(default)]
    pub nbf: Option<i64>,
    #[serde(default)]
    pub aud: Option<Vec<String>>,
    #[serde(default)]
    pub iss: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub token_use: Option<String>,
    #[serde(default, rename = "ext")]
    pub extra: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth2_client_roundtrip() {
        let client = OAuth2Client {
            client_id: Some("sunbeam-cli".into()),
            client_name: Some("Sunbeam CLI".into()),
            redirect_uris: Some(vec!["http://localhost:9876/callback".into()]),
            ..Default::default()
        };
        let json = serde_json::to_value(&client).unwrap();
        assert_eq!(json["client_id"], "sunbeam-cli");
        assert!(json.get("client_secret").is_none());
    }

    #[test]
    fn test_redirect_response() {
        let json = serde_json::json!({"redirect_to": "https://example.com/callback"});
        let r: RedirectResponse = serde_json::from_value(json).unwrap();
        assert_eq!(r.redirect_to, "https://example.com/callback");
    }

    #[test]
    fn test_introspect_active() {
        let json = serde_json::json!({"active": true, "scope": "openid", "sub": "user-123"});
        let r: IntrospectResult = serde_json::from_value(json).unwrap();
        assert!(r.active);
        assert_eq!(r.sub, Some("user-123".into()));
    }
}
