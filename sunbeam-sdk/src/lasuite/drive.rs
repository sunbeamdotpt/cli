//! Drive service client — files, folders, shares, permissions.

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use super::types::*;

/// Client for the La Suite Drive API.
pub struct DriveClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for DriveClient {
    fn service_name(&self) -> &'static str {
        "drive"
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

impl DriveClient {
    /// Build a DriveClient from domain (e.g. `https://drive.{domain}/api/v1.0`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://drive.{domain}/api/v1.0");
        Self::from_parts(base_url, AuthMethod::Bearer(String::new()))
    }

    /// Set the bearer token for authentication.
    pub fn with_token(mut self, token: &str) -> Self {
        self.transport.set_auth(AuthMethod::Bearer(token.to_string()));
        self
    }

    // -- Files --------------------------------------------------------------

    /// List files with optional pagination.
    pub async fn list_files(&self, page: Option<u32>) -> Result<DRFPage<DriveFile>> {
        let path = match page {
            Some(p) => format!("files/?page={p}"),
            None => "files/".to_string(),
        };
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "drive list files")
            .await
    }

    /// Get a single file by ID.
    pub async fn get_file(&self, id: &str) -> Result<DriveFile> {
        self.transport
            .json(
                Method::GET,
                &format!("files/{id}/"),
                Option::<&()>::None,
                "drive get file",
            )
            .await
    }

    /// Upload a new file.
    pub async fn upload_file(&self, body: &serde_json::Value) -> Result<DriveFile> {
        self.transport
            .json(Method::POST, "files/", Some(body), "drive upload file")
            .await
    }

    /// Delete a file.
    pub async fn delete_file(&self, id: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("files/{id}/"),
                Option::<&()>::None,
                "drive delete file",
            )
            .await
    }

    // -- Folders ------------------------------------------------------------

    /// List folders with optional pagination.
    pub async fn list_folders(&self, page: Option<u32>) -> Result<DRFPage<DriveFolder>> {
        let path = match page {
            Some(p) => format!("folders/?page={p}"),
            None => "folders/".to_string(),
        };
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "drive list folders")
            .await
    }

    /// Create a new folder.
    pub async fn create_folder(&self, body: &serde_json::Value) -> Result<DriveFolder> {
        self.transport
            .json(Method::POST, "folders/", Some(body), "drive create folder")
            .await
    }

    // -- Shares -------------------------------------------------------------

    /// Share a file with a user.
    pub async fn share_file(&self, id: &str, body: &serde_json::Value) -> Result<FileShare> {
        self.transport
            .json(
                Method::POST,
                &format!("files/{id}/shares/"),
                Some(body),
                "drive share file",
            )
            .await
    }

    // -- Permissions --------------------------------------------------------

    /// Get permissions for a file.
    pub async fn get_permissions(&self, id: &str) -> Result<DRFPage<FilePermission>> {
        self.transport
            .json(
                Method::GET,
                &format!("files/{id}/permissions/"),
                Option::<&()>::None,
                "drive get permissions",
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = DriveClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://drive.sunbeam.pt/api/v1.0");
        assert_eq!(c.service_name(), "drive");
    }

    #[test]
    fn test_from_parts() {
        let c = DriveClient::from_parts(
            "http://localhost:8000/api/v1.0".into(),
            AuthMethod::Bearer("tok".into()),
        );
        assert_eq!(c.base_url(), "http://localhost:8000/api/v1.0");
    }
}
