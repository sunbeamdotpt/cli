//! Drive service client — files, folders, shares, permissions.

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use super::types::*;

/// Client for the La Suite Drive API.
#[derive(Clone)]
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

    // -- Items --------------------------------------------------------------

    /// List items with optional pagination and type filter.
    pub async fn list_items(
        &self,
        page: Option<u32>,
        item_type: Option<&str>,
    ) -> Result<DRFPage<DriveFile>> {
        let mut path = String::from("items/?");
        if let Some(p) = page {
            path.push_str(&format!("page={p}&"));
        }
        if let Some(t) = item_type {
            path.push_str(&format!("type={t}&"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "drive list items")
            .await
    }

    /// List files (items with type=file).
    pub async fn list_files(&self, page: Option<u32>) -> Result<DRFPage<DriveFile>> {
        self.list_items(page, Some("file")).await
    }

    /// List folders (items with type=folder).
    pub async fn list_folders(&self, page: Option<u32>) -> Result<DRFPage<DriveFolder>> {
        let mut path = String::from("items/?type=folder&");
        if let Some(p) = page {
            path.push_str(&format!("page={p}&"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "drive list folders")
            .await
    }

    /// Get a single item by ID.
    pub async fn get_file(&self, id: &str) -> Result<DriveFile> {
        self.transport
            .json(
                Method::GET,
                &format!("items/{id}/"),
                Option::<&()>::None,
                "drive get item",
            )
            .await
    }

    /// Create a new item (file or folder) at the root level.
    pub async fn upload_file(&self, body: &serde_json::Value) -> Result<DriveFile> {
        self.transport
            .json(Method::POST, "items/", Some(body), "drive create item")
            .await
    }

    /// Delete an item.
    pub async fn delete_file(&self, id: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("items/{id}/"),
                Option::<&()>::None,
                "drive delete item",
            )
            .await
    }

    /// Create a new folder at the root level.
    pub async fn create_folder(&self, body: &serde_json::Value) -> Result<DriveFolder> {
        self.transport
            .json(Method::POST, "items/", Some(body), "drive create folder")
            .await
    }

    // -- Items (children API) ------------------------------------------------

    /// Create a child item under a parent folder.
    /// Returns the created item including its upload_url for files.
    pub async fn create_child(
        &self,
        parent_id: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.transport
            .json(
                Method::POST,
                &format!("items/{parent_id}/children/"),
                Some(body),
                "drive create child",
            )
            .await
    }

    /// List children of an item (folder).
    pub async fn list_children(
        &self,
        parent_id: &str,
        page: Option<u32>,
    ) -> Result<DRFPage<serde_json::Value>> {
        let path = match page {
            Some(p) => format!("items/{parent_id}/children/?page={p}"),
            None => format!("items/{parent_id}/children/"),
        };
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "drive list children")
            .await
    }

    /// Notify Drive that a file upload to S3 is complete.
    pub async fn upload_ended(&self, item_id: &str) -> Result<serde_json::Value> {
        self.transport
            .json(
                Method::POST,
                &format!("items/{item_id}/upload-ended/"),
                Option::<&()>::None,
                "drive upload ended",
            )
            .await
    }

    /// Upload file bytes directly to a presigned S3 URL.
    /// The presigned URL's SigV4 signature covers host + x-amz-acl headers.
    pub async fn upload_to_s3(&self, presigned_url: &str, data: bytes::Bytes) -> Result<()> {
        let resp = reqwest::Client::new()
            .put(presigned_url)
            .header("x-amz-acl", "private")
            .body(data)
            .send()
            .await
            .map_err(|e| crate::error::SunbeamError::network(format!("S3 upload: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(crate::error::SunbeamError::network(format!(
                "S3 upload: HTTP {status}: {body}"
            )));
        }
        Ok(())
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
