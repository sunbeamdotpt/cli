//! Docs service client — documents, templates, versions, invitations.

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use super::types::*;

/// Client for the La Suite Docs API.
pub struct DocsClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for DocsClient {
    fn service_name(&self) -> &'static str {
        "docs"
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

impl DocsClient {
    /// Build a DocsClient from domain (e.g. `https://docs.{domain}/api/v1.0`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://docs.{domain}/api/v1.0");
        Self::from_parts(base_url, AuthMethod::Bearer(String::new()))
    }

    /// Set the bearer token for authentication.
    pub fn with_token(mut self, token: &str) -> Self {
        self.transport.set_auth(AuthMethod::Bearer(token.to_string()));
        self
    }

    // -- Documents ----------------------------------------------------------

    /// List documents with optional pagination.
    pub async fn list_documents(&self, page: Option<u32>) -> Result<DRFPage<Document>> {
        let path = match page {
            Some(p) => format!("documents/?page={p}"),
            None => "documents/".to_string(),
        };
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "docs list documents")
            .await
    }

    /// Get a single document by ID.
    pub async fn get_document(&self, id: &str) -> Result<Document> {
        self.transport
            .json(
                Method::GET,
                &format!("documents/{id}/"),
                Option::<&()>::None,
                "docs get document",
            )
            .await
    }

    /// Create a new document.
    pub async fn create_document(&self, body: &serde_json::Value) -> Result<Document> {
        self.transport
            .json(Method::POST, "documents/", Some(body), "docs create document")
            .await
    }

    /// Update a document (partial).
    pub async fn update_document(&self, id: &str, body: &serde_json::Value) -> Result<Document> {
        self.transport
            .json(
                Method::PATCH,
                &format!("documents/{id}/"),
                Some(body),
                "docs update document",
            )
            .await
    }

    /// Delete a document.
    pub async fn delete_document(&self, id: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("documents/{id}/"),
                Option::<&()>::None,
                "docs delete document",
            )
            .await
    }

    // -- Templates ----------------------------------------------------------

    /// List templates with optional pagination.
    pub async fn list_templates(&self, page: Option<u32>) -> Result<DRFPage<Template>> {
        let path = match page {
            Some(p) => format!("templates/?page={p}"),
            None => "templates/".to_string(),
        };
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "docs list templates")
            .await
    }

    /// Create a new template.
    pub async fn create_template(&self, body: &serde_json::Value) -> Result<Template> {
        self.transport
            .json(Method::POST, "templates/", Some(body), "docs create template")
            .await
    }

    // -- Versions -----------------------------------------------------------

    /// List versions of a document.
    pub async fn list_versions(&self, doc_id: &str) -> Result<DRFPage<DocVersion>> {
        self.transport
            .json(
                Method::GET,
                &format!("documents/{doc_id}/versions/"),
                Option::<&()>::None,
                "docs list versions",
            )
            .await
    }

    // -- Invitations --------------------------------------------------------

    /// Invite a user to collaborate on a document.
    pub async fn invite_user(
        &self,
        doc_id: &str,
        body: &serde_json::Value,
    ) -> Result<Invitation> {
        self.transport
            .json(
                Method::POST,
                &format!("documents/{doc_id}/invitations/"),
                Some(body),
                "docs invite user",
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = DocsClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://docs.sunbeam.pt/api/v1.0");
        assert_eq!(c.service_name(), "docs");
    }

    #[test]
    fn test_from_parts() {
        let c = DocsClient::from_parts(
            "http://localhost:8000/api/v1.0".into(),
            AuthMethod::Bearer("tok".into()),
        );
        assert_eq!(c.base_url(), "http://localhost:8000/api/v1.0");
    }
}
