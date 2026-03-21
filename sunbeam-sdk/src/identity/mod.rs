//! Kratos identity management client.

pub mod types;

#[cfg(feature = "cli")]
pub mod cli;

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use types::*;

/// Client for the Ory Kratos Admin API.
pub struct KratosClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for KratosClient {
    fn service_name(&self) -> &'static str {
        "kratos"
    }

    fn base_url(&self) -> &str {
        &self.transport.base_url
    }

    fn from_parts(base_url: String, auth: AuthMethod) -> Self {
        Self {
            transport: HttpTransport::new(&base_url, auth),
        }
    }
}

impl KratosClient {
    /// Build a KratosClient from domain (e.g. `https://id.{domain}`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://id.{domain}");
        Self::from_parts(base_url, AuthMethod::None)
    }

    // -- Identities ---------------------------------------------------------

    /// List identities with optional pagination.
    pub async fn list_identities(
        &self,
        page: Option<u32>,
        page_size: Option<u32>,
    ) -> Result<Vec<Identity>> {
        let page = page.unwrap_or(1);
        let size = page_size.unwrap_or(20);
        self.transport
            .json(
                Method::GET,
                &format!("admin/identities?page={page}&page_size={size}"),
                Option::<&()>::None,
                "kratos list identities",
            )
            .await
    }

    /// Create a new identity.
    pub async fn create_identity(&self, body: &CreateIdentityBody) -> Result<Identity> {
        self.transport
            .json(Method::POST, "admin/identities", Some(body), "kratos create identity")
            .await
    }

    /// Get a single identity by ID.
    pub async fn get_identity(&self, id: &str) -> Result<Identity> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/identities/{id}"),
                Option::<&()>::None,
                "kratos get identity",
            )
            .await
    }

    /// Update an identity (full replace).
    pub async fn update_identity(&self, id: &str, body: &UpdateIdentityBody) -> Result<Identity> {
        self.transport
            .json(
                Method::PUT,
                &format!("admin/identities/{id}"),
                Some(body),
                "kratos update identity",
            )
            .await
    }

    /// Patch an identity (partial update).
    pub async fn patch_identity(
        &self,
        id: &str,
        patches: &[serde_json::Value],
    ) -> Result<Identity> {
        self.transport
            .json(
                Method::PATCH,
                &format!("admin/identities/{id}"),
                Some(&patches),
                "kratos patch identity",
            )
            .await
    }

    /// Delete an identity.
    pub async fn delete_identity(&self, id: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("admin/identities/{id}"),
                Option::<&()>::None,
                "kratos delete identity",
            )
            .await
    }

    /// Batch patch identities.
    pub async fn batch_patch_identities(
        &self,
        body: &BatchPatchIdentitiesBody,
    ) -> Result<BatchPatchResult> {
        self.transport
            .json(Method::PATCH, "admin/identities", Some(body), "kratos batch patch")
            .await
    }

    /// Get identity by external credential identifier (e.g. email).
    pub async fn get_by_credential_identifier(&self, identifier: &str) -> Result<Vec<Identity>> {
        self.transport
            .json(
                Method::GET,
                &format!(
                    "admin/identities?credentials_identifier={}&page_size=1",
                    identifier
                ),
                Option::<&()>::None,
                "kratos get by credential",
            )
            .await
    }

    /// Delete a specific credential from an identity.
    pub async fn delete_credential(
        &self,
        id: &str,
        credential_type: &str,
    ) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("admin/identities/{id}/credentials/{credential_type}"),
                Option::<&()>::None,
                "kratos delete credential",
            )
            .await
    }

    // -- Sessions -----------------------------------------------------------

    /// List all sessions across identities.
    pub async fn list_sessions(
        &self,
        page_size: Option<u32>,
        page_token: Option<&str>,
        active: Option<bool>,
    ) -> Result<Vec<Session>> {
        let mut path = format!(
            "admin/sessions?page_size={}",
            page_size.unwrap_or(20)
        );
        if let Some(token) = page_token {
            path.push_str(&format!("&page_token={token}"));
        }
        if let Some(active) = active {
            path.push_str(&format!("&active={active}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "kratos list sessions")
            .await
    }

    /// Get a specific session.
    pub async fn get_session(&self, id: &str) -> Result<Session> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/sessions/{id}"),
                Option::<&()>::None,
                "kratos get session",
            )
            .await
    }

    /// Extend a session.
    pub async fn extend_session(&self, id: &str) -> Result<Session> {
        self.transport
            .json(
                Method::PATCH,
                &format!("admin/sessions/{id}/extend"),
                Option::<&()>::None,
                "kratos extend session",
            )
            .await
    }

    /// Disable (revoke) a session.
    pub async fn disable_session(&self, id: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("admin/sessions/{id}"),
                Option::<&()>::None,
                "kratos disable session",
            )
            .await
    }

    /// List sessions for a specific identity.
    pub async fn list_identity_sessions(&self, identity_id: &str) -> Result<Vec<Session>> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/identities/{identity_id}/sessions"),
                Option::<&()>::None,
                "kratos list identity sessions",
            )
            .await
    }

    /// Delete all sessions for a specific identity.
    pub async fn delete_identity_sessions(&self, identity_id: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("admin/identities/{identity_id}/sessions"),
                Option::<&()>::None,
                "kratos delete identity sessions",
            )
            .await
    }

    // -- Recovery -----------------------------------------------------------

    /// Create a recovery code for an identity.
    pub async fn create_recovery_code(
        &self,
        identity_id: &str,
        expires_in: Option<&str>,
    ) -> Result<RecoveryCodeResult> {
        let body = serde_json::json!({
            "identity_id": identity_id,
            "expires_in": expires_in.unwrap_or("24h"),
        });
        self.transport
            .json(Method::POST, "admin/recovery/code", Some(&body), "kratos recovery code")
            .await
    }

    /// Create a recovery link for an identity.
    pub async fn create_recovery_link(
        &self,
        identity_id: &str,
        expires_in: Option<&str>,
    ) -> Result<RecoveryLinkResult> {
        let body = serde_json::json!({
            "identity_id": identity_id,
            "expires_in": expires_in.unwrap_or("24h"),
        });
        self.transport
            .json(Method::POST, "admin/recovery/link", Some(&body), "kratos recovery link")
            .await
    }

    // -- Schemas ------------------------------------------------------------

    /// List identity schemas.
    pub async fn list_schemas(&self) -> Result<Vec<IdentitySchema>> {
        self.transport
            .json(
                Method::GET,
                "schemas",
                Option::<&()>::None,
                "kratos list schemas",
            )
            .await
    }

    /// Get a specific identity schema.
    pub async fn get_schema(&self, id: &str) -> Result<serde_json::Value> {
        self.transport
            .json(
                Method::GET,
                &format!("schemas/{id}"),
                Option::<&()>::None,
                "kratos get schema",
            )
            .await
    }

    // -- Courier messages ---------------------------------------------------

    /// List courier messages.
    pub async fn list_courier_messages(
        &self,
        page_size: Option<u32>,
        page_token: Option<&str>,
    ) -> Result<Vec<CourierMessage>> {
        let mut path = format!(
            "admin/courier/messages?page_size={}",
            page_size.unwrap_or(20)
        );
        if let Some(token) = page_token {
            path.push_str(&format!("&page_token={token}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "kratos list courier")
            .await
    }

    /// Get a specific courier message.
    pub async fn get_courier_message(&self, id: &str) -> Result<CourierMessage> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/courier/messages/{id}"),
                Option::<&()>::None,
                "kratos get courier message",
            )
            .await
    }

    // -- Health -------------------------------------------------------------

    /// Alive health check.
    pub async fn alive(&self) -> Result<HealthStatus> {
        self.transport
            .json(
                Method::GET,
                "health/alive",
                Option::<&()>::None,
                "kratos alive",
            )
            .await
    }

    /// Ready health check.
    pub async fn ready(&self) -> Result<HealthStatus> {
        self.transport
            .json(
                Method::GET,
                "health/ready",
                Option::<&()>::None,
                "kratos ready",
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = KratosClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://id.sunbeam.pt");
        assert_eq!(c.service_name(), "kratos");
    }

    #[test]
    fn test_from_parts() {
        let c = KratosClient::from_parts(
            "http://localhost:4434".into(),
            AuthMethod::None,
        );
        assert_eq!(c.base_url(), "http://localhost:4434");
    }
}
