//! Kratos identity types.

use serde::{Deserialize, Serialize};

/// A Kratos identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub schema_id: String,
    #[serde(default)]
    pub schema_url: String,
    pub traits: serde_json::Value,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub metadata_public: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata_admin: Option<serde_json::Value>,
    #[serde(default)]
    pub verifiable_addresses: Option<Vec<VerifiableAddress>>,
    #[serde(default)]
    pub recovery_addresses: Option<Vec<RecoveryAddress>>,
    #[serde(default)]
    pub credentials: Option<serde_json::Value>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub state_changed_at: Option<String>,
}

/// A verifiable address (e.g. email).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiableAddress {
    pub id: String,
    pub value: String,
    pub via: String,
    pub status: String,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub verified_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A recovery address.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryAddress {
    pub id: String,
    pub value: String,
    pub via: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Body for creating an identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIdentityBody {
    pub schema_id: String,
    pub traits: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_public: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_admin: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifiable_addresses: Option<Vec<VerifiableAddress>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_addresses: Option<Vec<RecoveryAddress>>,
}

/// Body for updating an identity (PUT).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateIdentityBody {
    pub schema_id: String,
    pub traits: serde_json::Value,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_public: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_admin: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<serde_json::Value>,
}

/// Body for batch patching identities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchPatchIdentitiesBody {
    pub identities: Vec<BatchPatchEntry>,
}

/// A single entry in a batch patch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchPatchEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<CreateIdentityBody>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_id: Option<String>,
}

/// Result of a batch patch operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchPatchResult {
    #[serde(default)]
    pub identities: Vec<BatchPatchResultEntry>,
}

/// A single entry in batch patch results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchPatchResultEntry {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub identity: Option<String>,
    #[serde(default)]
    pub patch_id: Option<String>,
}

/// A Kratos session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub authenticated_at: Option<String>,
    #[serde(default)]
    pub authenticator_assurance_level: Option<String>,
    #[serde(default)]
    pub identity: Option<Identity>,
    #[serde(default)]
    pub devices: Option<Vec<SessionDevice>>,
}

/// Device info attached to a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDevice {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub ip_address: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
}

/// Recovery code creation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryCodeResult {
    #[serde(default)]
    pub recovery_link: String,
    #[serde(default)]
    pub recovery_code: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// Recovery link creation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryLinkResult {
    #[serde(default)]
    pub recovery_link: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// An identity schema definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentitySchema {
    pub id: String,
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
}

/// A courier message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CourierMessage {
    pub id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub recipient: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Health check response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_roundtrip() {
        let json = serde_json::json!({
            "id": "abc-123",
            "schema_id": "employee",
            "traits": { "email": "test@example.com" },
            "state": "active"
        });
        let identity: Identity = serde_json::from_value(json).unwrap();
        assert_eq!(identity.id, "abc-123");
        assert_eq!(identity.schema_id, "employee");
        assert_eq!(identity.state, Some("active".to_string()));
    }

    #[test]
    fn test_create_identity_body() {
        let body = CreateIdentityBody {
            schema_id: "default".into(),
            traits: serde_json::json!({"email": "new@example.com"}),
            state: Some("active".into()),
            metadata_public: None,
            metadata_admin: None,
            credentials: None,
            verifiable_addresses: None,
            recovery_addresses: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["schema_id"], "default");
        assert!(json.get("metadata_public").is_none());
    }

    #[test]
    fn test_health_status() {
        let json = serde_json::json!({"status": "ok"});
        let h: HealthStatus = serde_json::from_value(json).unwrap();
        assert_eq!(h.status, "ok");
    }

    #[test]
    fn test_recovery_code_result() {
        let json = serde_json::json!({
            "recovery_link": "https://example.com/recover",
            "recovery_code": "abc123"
        });
        let r: RecoveryCodeResult = serde_json::from_value(json).unwrap();
        assert_eq!(r.recovery_code, "abc123");
    }
}
