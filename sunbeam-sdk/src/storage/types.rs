//! S3 storage types.

use serde::{Deserialize, Serialize};

/// Response from ListBuckets (GET /).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListBucketsResponse {
    #[serde(default, rename = "Buckets")]
    pub buckets: Vec<Bucket>,
    #[serde(default, rename = "Owner")]
    pub owner: Option<Owner>,
}

/// A single S3 bucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bucket {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(default, rename = "CreationDate")]
    pub creation_date: Option<String>,
}

/// Bucket owner info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Owner {
    #[serde(default, rename = "ID")]
    pub id: Option<String>,
    #[serde(default, rename = "DisplayName")]
    pub display_name: Option<String>,
}

/// Response from ListObjectsV2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListObjectsResponse {
    #[serde(default, rename = "Name")]
    pub name: String,
    #[serde(default, rename = "Prefix")]
    pub prefix: Option<String>,
    #[serde(default, rename = "MaxKeys")]
    pub max_keys: Option<u32>,
    #[serde(default, rename = "IsTruncated")]
    pub is_truncated: Option<bool>,
    #[serde(default, rename = "Contents")]
    pub contents: Vec<Object>,
    #[serde(default, rename = "NextContinuationToken")]
    pub next_continuation_token: Option<String>,
}

/// A single S3 object in a listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Object {
    #[serde(rename = "Key")]
    pub key: String,
    #[serde(default, rename = "LastModified")]
    pub last_modified: Option<String>,
    #[serde(default, rename = "ETag")]
    pub etag: Option<String>,
    #[serde(default, rename = "Size")]
    pub size: Option<u64>,
    #[serde(default, rename = "StorageClass")]
    pub storage_class: Option<String>,
}

/// Response from InitiateMultipartUpload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitiateMultipartResponse {
    #[serde(rename = "Bucket")]
    pub bucket: String,
    #[serde(rename = "Key")]
    pub key: String,
    #[serde(rename = "UploadId")]
    pub upload_id: String,
}

/// Response from UploadPart (extracted from headers).
#[derive(Debug, Clone)]
pub struct UploadPartResponse {
    pub etag: String,
    pub part_number: u32,
}

/// Response from CompleteMultipartUpload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteMultipartResponse {
    #[serde(default, rename = "Location")]
    pub location: Option<String>,
    #[serde(rename = "Bucket")]
    pub bucket: String,
    #[serde(rename = "Key")]
    pub key: String,
    #[serde(default, rename = "ETag")]
    pub etag: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_buckets_response_roundtrip() {
        let json = serde_json::json!({
            "Buckets": [
                {"Name": "my-bucket", "CreationDate": "2024-01-01T00:00:00Z"}
            ],
            "Owner": {"ID": "owner-id", "DisplayName": "Owner"}
        });
        let resp: ListBucketsResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.buckets.len(), 1);
        assert_eq!(resp.buckets[0].name, "my-bucket");
        assert_eq!(resp.owner.unwrap().display_name, Some("Owner".to_string()));
    }

    #[test]
    fn test_list_objects_response_roundtrip() {
        let json = serde_json::json!({
            "Name": "my-bucket",
            "Prefix": "docs/",
            "MaxKeys": 1000,
            "IsTruncated": false,
            "Contents": [
                {"Key": "docs/readme.md", "Size": 1024, "ETag": "\"abc\""}
            ]
        });
        let resp: ListObjectsResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.name, "my-bucket");
        assert_eq!(resp.contents.len(), 1);
        assert_eq!(resp.contents[0].key, "docs/readme.md");
    }

    #[test]
    fn test_initiate_multipart_response() {
        let json = serde_json::json!({
            "Bucket": "my-bucket",
            "Key": "large-file.bin",
            "UploadId": "upload-123"
        });
        let resp: InitiateMultipartResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.upload_id, "upload-123");
    }

    #[test]
    fn test_complete_multipart_response() {
        let json = serde_json::json!({
            "Bucket": "my-bucket",
            "Key": "large-file.bin",
            "Location": "https://s3.example.com/my-bucket/large-file.bin",
            "ETag": "\"final-etag\""
        });
        let resp: CompleteMultipartResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.bucket, "my-bucket");
        assert_eq!(resp.location, Some("https://s3.example.com/my-bucket/large-file.bin".to_string()));
    }

    #[test]
    fn test_empty_list_buckets() {
        let json = serde_json::json!({});
        let resp: ListBucketsResponse = serde_json::from_value(json).unwrap();
        assert!(resp.buckets.is_empty());
        assert!(resp.owner.is_none());
    }
}
