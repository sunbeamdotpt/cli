//! S3-compatible storage client.

pub mod types;

#[cfg(feature = "cli")]
pub mod cli;

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::{Result, ResultExt, SunbeamError};
use bytes::Bytes;
use reqwest::Method;
use types::*;

/// Client for S3-compatible object storage.
pub struct S3Client {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for S3Client {
    fn service_name(&self) -> &'static str {
        "s3"
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

impl S3Client {
    /// Build an S3Client from domain (e.g. `https://s3.{domain}`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://s3.{domain}");
        Self::from_parts(base_url, AuthMethod::None)
    }

    /// Replace the auth method (e.g. after obtaining credentials).
    pub fn set_auth(&mut self, auth: AuthMethod) {
        self.transport.set_auth(auth);
    }

    // -- Buckets ------------------------------------------------------------

    /// Create a bucket.
    pub async fn create_bucket(&self, bucket: &str) -> Result<()> {
        self.transport
            .send(Method::PUT, bucket, Option::<&()>::None, "s3 create bucket")
            .await
    }

    /// Delete a bucket.
    pub async fn delete_bucket(&self, bucket: &str) -> Result<()> {
        self.transport
            .send(Method::DELETE, bucket, Option::<&()>::None, "s3 delete bucket")
            .await
    }

    /// List all buckets.
    pub async fn list_buckets(&self) -> Result<ListBucketsResponse> {
        self.transport
            .json(Method::GET, "/", Option::<&()>::None, "s3 list buckets")
            .await
    }

    /// Check if a bucket exists (HEAD request, returns true/false).
    pub async fn head_bucket(&self, bucket: &str) -> Result<bool> {
        let resp = self
            .transport
            .request(Method::HEAD, bucket)
            .send()
            .await
            .with_ctx(|| format!("s3 head bucket {bucket}: request failed"))?;
        Ok(resp.status().is_success())
    }

    /// Set versioning configuration on a bucket.
    pub async fn set_versioning(&self, bucket: &str, body: &(impl serde::Serialize + Sync)) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("{bucket}?versioning"),
                Some(body),
                "s3 set versioning",
            )
            .await
    }

    /// Set lifecycle configuration on a bucket.
    pub async fn set_lifecycle(&self, bucket: &str, body: &(impl serde::Serialize + Sync)) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("{bucket}?lifecycle"),
                Some(body),
                "s3 set lifecycle",
            )
            .await
    }

    /// Set CORS configuration on a bucket.
    pub async fn set_cors(&self, bucket: &str, body: &(impl serde::Serialize + Sync)) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("{bucket}?cors"),
                Some(body),
                "s3 set cors",
            )
            .await
    }

    /// Get the ACL for a bucket.
    pub async fn get_acl(&self, bucket: &str) -> Result<serde_json::Value> {
        self.transport
            .json(
                Method::GET,
                &format!("{bucket}?acl"),
                Option::<&()>::None,
                "s3 get acl",
            )
            .await
    }

    /// Set a bucket policy.
    pub async fn set_policy(&self, bucket: &str, body: &(impl serde::Serialize + Sync)) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("{bucket}?policy"),
                Some(body),
                "s3 set policy",
            )
            .await
    }

    // -- Objects -------------------------------------------------------------

    /// Upload an object.
    pub async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        data: Bytes,
    ) -> Result<()> {
        let path = format!("{bucket}/{key}");
        let resp = self
            .transport
            .request(Method::PUT, &path)
            .header("Content-Type", content_type)
            .body(data)
            .send()
            .await
            .with_ctx(|| format!("s3 put object {path}: request failed"))?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(SunbeamError::network(format!(
                "s3 put object: HTTP {status}: {body_text}"
            )));
        }
        Ok(())
    }

    /// Download an object.
    pub async fn get_object(&self, bucket: &str, key: &str) -> Result<Bytes> {
        self.transport
            .bytes(Method::GET, &format!("{bucket}/{key}"), "s3 get object")
            .await
    }

    /// Check if an object exists (HEAD request, returns true/false).
    pub async fn head_object(&self, bucket: &str, key: &str) -> Result<bool> {
        let path = format!("{bucket}/{key}");
        let resp = self
            .transport
            .request(Method::HEAD, &path)
            .send()
            .await
            .with_ctx(|| format!("s3 head object {path}: request failed"))?;
        Ok(resp.status().is_success())
    }

    /// Delete an object.
    pub async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("{bucket}/{key}"),
                Option::<&()>::None,
                "s3 delete object",
            )
            .await
    }

    /// Copy an object from `source` to `{bucket}/{key}`.
    pub async fn copy_object(&self, bucket: &str, key: &str, source: &str) -> Result<()> {
        let path = format!("{bucket}/{key}");
        let resp = self
            .transport
            .request(Method::PUT, &path)
            .header("x-amz-copy-source", source)
            .send()
            .await
            .with_ctx(|| format!("s3 copy object {path}: request failed"))?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(SunbeamError::network(format!(
                "s3 copy object: HTTP {status}: {body_text}"
            )));
        }
        Ok(())
    }

    /// List objects in a bucket (v2).
    pub async fn list_objects_v2(
        &self,
        bucket: &str,
        prefix: Option<&str>,
        max_keys: Option<u32>,
    ) -> Result<ListObjectsResponse> {
        let mut path = format!("{bucket}?list-type=2");
        if let Some(p) = prefix {
            path.push_str(&format!("&prefix={p}"));
        }
        if let Some(m) = max_keys {
            path.push_str(&format!("&max-keys={m}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "s3 list objects v2")
            .await
    }

    /// Set tags on an object.
    pub async fn set_tags(
        &self,
        bucket: &str,
        key: &str,
        body: &(impl serde::Serialize + Sync),
    ) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("{bucket}/{key}?tagging"),
                Some(body),
                "s3 set tags",
            )
            .await
    }

    /// Get tags for an object.
    pub async fn get_tags(&self, bucket: &str, key: &str) -> Result<serde_json::Value> {
        self.transport
            .json(
                Method::GET,
                &format!("{bucket}/{key}?tagging"),
                Option::<&()>::None,
                "s3 get tags",
            )
            .await
    }

    // -- Multipart -----------------------------------------------------------

    /// Initiate a multipart upload.
    pub async fn initiate_multipart(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<InitiateMultipartResponse> {
        self.transport
            .json(
                Method::POST,
                &format!("{bucket}/{key}?uploads"),
                Option::<&()>::None,
                "s3 initiate multipart",
            )
            .await
    }

    /// Upload a single part of a multipart upload.
    pub async fn upload_part(
        &self,
        bucket: &str,
        key: &str,
        upload_id: &str,
        part_number: u32,
        data: Bytes,
    ) -> Result<UploadPartResponse> {
        let path = format!("{bucket}/{key}?partNumber={part_number}&uploadId={upload_id}");
        let resp = self
            .transport
            .request(Method::PUT, &path)
            .body(data)
            .send()
            .await
            .with_ctx(|| format!("s3 upload part {path}: request failed"))?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(SunbeamError::network(format!(
                "s3 upload part: HTTP {status}: {body_text}"
            )));
        }
        let etag = resp
            .headers()
            .get("ETag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        Ok(UploadPartResponse { etag, part_number })
    }

    /// Complete a multipart upload.
    pub async fn complete_multipart(
        &self,
        bucket: &str,
        key: &str,
        upload_id: &str,
        body: &(impl serde::Serialize + Sync),
    ) -> Result<CompleteMultipartResponse> {
        self.transport
            .json(
                Method::POST,
                &format!("{bucket}/{key}?uploadId={upload_id}"),
                Some(body),
                "s3 complete multipart",
            )
            .await
    }

    /// Abort a multipart upload.
    pub async fn abort_multipart(
        &self,
        bucket: &str,
        key: &str,
        upload_id: &str,
    ) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("{bucket}/{key}?uploadId={upload_id}"),
                Option::<&()>::None,
                "s3 abort multipart",
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = S3Client::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://s3.sunbeam.pt");
        assert_eq!(c.service_name(), "s3");
    }

    #[test]
    fn test_from_parts() {
        let c = S3Client::from_parts("http://localhost:9000".into(), AuthMethod::None);
        assert_eq!(c.base_url(), "http://localhost:9000");
    }

    #[test]
    fn test_set_auth() {
        let mut c = S3Client::connect("sunbeam.pt");
        assert!(matches!(c.transport.auth, AuthMethod::None));
        c.set_auth(AuthMethod::Bearer("tok".into()));
        assert!(matches!(c.transport.auth, AuthMethod::Bearer(ref s) if s == "tok"));
    }

    #[tokio::test]
    async fn test_head_bucket_unreachable() {
        let c = S3Client::from_parts("http://127.0.0.1:19998".into(), AuthMethod::None);
        let result = c.head_bucket("test-bucket").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_head_object_unreachable() {
        let c = S3Client::from_parts("http://127.0.0.1:19998".into(), AuthMethod::None);
        let result = c.head_object("bucket", "key").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_buckets_unreachable() {
        let c = S3Client::from_parts("http://127.0.0.1:19998".into(), AuthMethod::None);
        let result = c.list_buckets().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_put_object_unreachable() {
        let c = S3Client::from_parts("http://127.0.0.1:19998".into(), AuthMethod::None);
        let result = c
            .put_object("bucket", "key", "text/plain", Bytes::from("hello"))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_copy_object_unreachable() {
        let c = S3Client::from_parts("http://127.0.0.1:19998".into(), AuthMethod::None);
        let result = c.copy_object("bucket", "dest", "/src-bucket/src-key").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_upload_part_unreachable() {
        let c = S3Client::from_parts("http://127.0.0.1:19998".into(), AuthMethod::None);
        let result = c
            .upload_part("bucket", "key", "upload-id", 1, Bytes::from("data"))
            .await;
        assert!(result.is_err());
    }
}
