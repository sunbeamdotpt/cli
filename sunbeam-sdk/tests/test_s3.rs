#![cfg(feature = "integration")]
mod helpers;
use helpers::*;

use sunbeam_sdk::client::{AuthMethod, ServiceClient};
use sunbeam_sdk::storage::S3Client;
#[allow(unused_imports)]
use sunbeam_sdk::storage::types::*;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helper: build an S3Client pointed at the mock server
// ---------------------------------------------------------------------------

fn mock_client(server: &MockServer) -> S3Client {
    S3Client::from_parts(server.uri(), AuthMethod::None)
}

// ===========================================================================
// Health & connectivity (real MinIO)
// ===========================================================================

const MINIO_URL: &str = "http://localhost:9000";
const MINIO_HEALTH: &str = "http://localhost:9000/minio/health/live";

#[tokio::test]
async fn minio_is_healthy() {
    wait_for_healthy(MINIO_HEALTH, TIMEOUT).await;
    let resp = reqwest::get(MINIO_HEALTH).await.unwrap();
    assert!(resp.status().is_success());
}

// ===========================================================================
// Bucket operations
// ===========================================================================

// 1. create_bucket — PUT /{bucket}
#[tokio::test]
async fn create_bucket_success() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    client.create_bucket("test-bucket").await.unwrap();
}

// 2. delete_bucket — DELETE /{bucket}
#[tokio::test]
async fn delete_bucket_success() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/test-bucket"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    client.delete_bucket("test-bucket").await.unwrap();
}

// 3. list_buckets — GET /
#[tokio::test]
async fn list_buckets_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Buckets": [
                    {"Name": "bucket-a", "CreationDate": "2025-01-01T00:00:00Z"},
                    {"Name": "bucket-b", "CreationDate": "2025-06-15T12:00:00Z"}
                ],
                "Owner": {"ID": "owner-1", "DisplayName": "TestOwner"}
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let resp = client.list_buckets().await.unwrap();
    assert_eq!(resp.buckets.len(), 2);
    assert_eq!(resp.buckets[0].name, "bucket-a");
    assert_eq!(resp.buckets[1].name, "bucket-b");
    let owner = resp.owner.unwrap();
    assert_eq!(owner.display_name, Some("TestOwner".to_string()));
}

#[tokio::test]
async fn list_buckets_error_500() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client.list_buckets().await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("500"), "expected 500 in error, got: {msg}");
}

#[tokio::test]
async fn list_buckets_empty() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Buckets": [],
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let resp = client.list_buckets().await.unwrap();
    assert!(resp.buckets.is_empty());
}

// 4. head_bucket — HEAD /{bucket}
#[tokio::test]
async fn head_bucket_exists() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/test-bucket"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    assert!(client.head_bucket("test-bucket").await.unwrap());
}

#[tokio::test]
async fn head_bucket_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/no-such-bucket"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    assert!(!client.head_bucket("no-such-bucket").await.unwrap());
}

#[tokio::test]
async fn head_bucket_403_returns_false() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/forbidden-bucket"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    assert!(!client.head_bucket("forbidden-bucket").await.unwrap());
}

// 5. set_versioning — PUT /{bucket}?versioning
#[tokio::test]
async fn set_versioning_success() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket"))
        .and(query_param("versioning", ""))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let body = serde_json::json!({"Status": "Enabled"});
    client.set_versioning("test-bucket", &body).await.unwrap();
}

// 6. set_lifecycle — PUT /{bucket}?lifecycle
#[tokio::test]
async fn set_lifecycle_success() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket"))
        .and(query_param("lifecycle", ""))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let body = serde_json::json!({"Rules": [{"Status": "Enabled"}]});
    client.set_lifecycle("test-bucket", &body).await.unwrap();
}

// 7. set_cors — PUT /{bucket}?cors
#[tokio::test]
async fn set_cors_success() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket"))
        .and(query_param("cors", ""))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let body = serde_json::json!({"CORSRules": [{"AllowedOrigins": ["*"]}]});
    client.set_cors("test-bucket", &body).await.unwrap();
}

// 8. get_acl — GET /{bucket}?acl
#[tokio::test]
async fn get_acl_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/test-bucket"))
        .and(query_param("acl", ""))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Owner": {"ID": "owner-1"},
                "Grants": [{"Permission": "FULL_CONTROL"}]
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let acl = client.get_acl("test-bucket").await.unwrap();
    assert_eq!(acl["Owner"]["ID"], "owner-1");
    assert_eq!(acl["Grants"][0]["Permission"], "FULL_CONTROL");
}

// 9. set_policy — PUT /{bucket}?policy
#[tokio::test]
async fn set_policy_success() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket"))
        .and(query_param("policy", ""))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let body = serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Principal": "*", "Action": "s3:GetObject"}]
    });
    client.set_policy("test-bucket", &body).await.unwrap();
}

// ===========================================================================
// Object operations
// ===========================================================================

// 10. put_object — PUT /{bucket}/{key}
#[tokio::test]
async fn put_object_success() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket/hello.txt"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    client
        .put_object("test-bucket", "hello.txt", "text/plain", bytes::Bytes::from("hello world"))
        .await
        .unwrap();
}

#[tokio::test]
async fn put_object_error_403() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket/secret.txt"))
        .respond_with(ResponseTemplate::new(403).set_body_string("Access Denied"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client
        .put_object("test-bucket", "secret.txt", "text/plain", bytes::Bytes::from("nope"))
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("403"), "expected 403 in error, got: {msg}");
}

#[tokio::test]
async fn put_object_error_500() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket/fail.txt"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client
        .put_object("test-bucket", "fail.txt", "text/plain", bytes::Bytes::from("data"))
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("500"), "expected 500 in error, got: {msg}");
}

// 11. get_object — GET /{bucket}/{key}
#[tokio::test]
async fn get_object_success() {
    let server = MockServer::start().await;
    let payload = b"file contents here";
    Mock::given(method("GET"))
        .and(path("/test-bucket/hello.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let data = client.get_object("test-bucket", "hello.txt").await.unwrap();
    assert_eq!(data.as_ref(), payload);
}

#[tokio::test]
async fn get_object_error_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/test-bucket/missing.txt"))
        .respond_with(ResponseTemplate::new(404).set_body_string("NoSuchKey"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client.get_object("test-bucket", "missing.txt").await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("404"), "expected 404 in error, got: {msg}");
}

#[tokio::test]
async fn get_object_error_500() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/test-bucket/broken.txt"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client.get_object("test-bucket", "broken.txt").await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("500"), "expected 500 in error, got: {msg}");
}

// 12. head_object — HEAD /{bucket}/{key}
#[tokio::test]
async fn head_object_exists() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/test-bucket/hello.txt"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    assert!(client.head_object("test-bucket", "hello.txt").await.unwrap());
}

#[tokio::test]
async fn head_object_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/test-bucket/missing.txt"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    assert!(!client.head_object("test-bucket", "missing.txt").await.unwrap());
}

// 13. delete_object — DELETE /{bucket}/{key}
#[tokio::test]
async fn delete_object_success() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/test-bucket/hello.txt"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    client.delete_object("test-bucket", "hello.txt").await.unwrap();
}

// 14. copy_object — PUT /{bucket}/{key} with x-amz-copy-source header
#[tokio::test]
async fn copy_object_success() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/dest-bucket/dest-key"))
        .and(wiremock::matchers::header("x-amz-copy-source", "/src-bucket/src-key"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    client
        .copy_object("dest-bucket", "dest-key", "/src-bucket/src-key")
        .await
        .unwrap();
}

#[tokio::test]
async fn copy_object_error_403() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/dest-bucket/dest-key"))
        .respond_with(ResponseTemplate::new(403).set_body_string("Access Denied"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client
        .copy_object("dest-bucket", "dest-key", "/src-bucket/src-key")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("403"), "expected 403 in error, got: {msg}");
}

// 15. list_objects_v2 — GET /{bucket}?list-type=2
#[tokio::test]
async fn list_objects_v2_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/test-bucket"))
        .and(query_param("list-type", "2"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Name": "test-bucket",
                "Prefix": null,
                "MaxKeys": 1000,
                "IsTruncated": false,
                "Contents": [
                    {
                        "Key": "file1.txt",
                        "LastModified": "2025-01-01T00:00:00Z",
                        "ETag": "\"abc123\"",
                        "Size": 1024,
                        "StorageClass": "STANDARD"
                    },
                    {
                        "Key": "file2.txt",
                        "Size": 2048
                    }
                ]
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let resp = client.list_objects_v2("test-bucket", None, None).await.unwrap();
    assert_eq!(resp.name, "test-bucket");
    assert_eq!(resp.contents.len(), 2);
    assert_eq!(resp.contents[0].key, "file1.txt");
    assert_eq!(resp.contents[0].size, Some(1024));
    assert_eq!(resp.contents[1].key, "file2.txt");
    assert_eq!(resp.is_truncated, Some(false));
}

#[tokio::test]
async fn list_objects_v2_with_prefix_and_max_keys() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/test-bucket"))
        .and(query_param("list-type", "2"))
        .and(query_param("prefix", "docs/"))
        .and(query_param("max-keys", "10"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Name": "test-bucket",
                "Prefix": "docs/",
                "MaxKeys": 10,
                "IsTruncated": false,
                "Contents": [
                    {"Key": "docs/readme.md", "Size": 512}
                ]
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let resp = client
        .list_objects_v2("test-bucket", Some("docs/"), Some(10))
        .await
        .unwrap();
    assert_eq!(resp.prefix, Some("docs/".to_string()));
    assert_eq!(resp.max_keys, Some(10));
    assert_eq!(resp.contents.len(), 1);
    assert_eq!(resp.contents[0].key, "docs/readme.md");
}

// 16. set_tags — PUT /{bucket}/{key}?tagging
#[tokio::test]
async fn set_tags_success() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket/hello.txt"))
        .and(query_param("tagging", ""))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let body = serde_json::json!({"TagSet": [{"Key": "env", "Value": "prod"}]});
    client.set_tags("test-bucket", "hello.txt", &body).await.unwrap();
}

// 17. get_tags — GET /{bucket}/{key}?tagging
#[tokio::test]
async fn get_tags_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/test-bucket/hello.txt"))
        .and(query_param("tagging", ""))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "TagSet": [{"Key": "env", "Value": "prod"}]
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let tags = client.get_tags("test-bucket", "hello.txt").await.unwrap();
    assert_eq!(tags["TagSet"][0]["Key"], "env");
    assert_eq!(tags["TagSet"][0]["Value"], "prod");
}

// ===========================================================================
// Multipart operations
// ===========================================================================

// 18. initiate_multipart — POST /{bucket}/{key}?uploads
#[tokio::test]
async fn initiate_multipart_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/test-bucket/large-file.bin"))
        .and(query_param("uploads", ""))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Bucket": "test-bucket",
                "Key": "large-file.bin",
                "UploadId": "upload-abc-123"
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let resp = client.initiate_multipart("test-bucket", "large-file.bin").await.unwrap();
    assert_eq!(resp.bucket, "test-bucket");
    assert_eq!(resp.key, "large-file.bin");
    assert_eq!(resp.upload_id, "upload-abc-123");
}

// 19. upload_part — PUT /{bucket}/{key}?partNumber=N&uploadId=xxx
#[tokio::test]
async fn upload_part_success() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket/large-file.bin"))
        .and(query_param("partNumber", "1"))
        .and(query_param("uploadId", "upload-abc-123"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("ETag", "\"part-etag-1\""),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let resp = client
        .upload_part(
            "test-bucket",
            "large-file.bin",
            "upload-abc-123",
            1,
            bytes::Bytes::from("part data chunk 1"),
        )
        .await
        .unwrap();
    assert_eq!(resp.etag, "\"part-etag-1\"");
    assert_eq!(resp.part_number, 1);
}

#[tokio::test]
async fn upload_part_error_500() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket/large-file.bin"))
        .and(query_param("partNumber", "2"))
        .and(query_param("uploadId", "upload-abc-123"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client
        .upload_part(
            "test-bucket",
            "large-file.bin",
            "upload-abc-123",
            2,
            bytes::Bytes::from("data"),
        )
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("500"), "expected 500 in error, got: {msg}");
}

#[tokio::test]
async fn upload_part_no_etag_header() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket/large-file.bin"))
        .and(query_param("partNumber", "3"))
        .and(query_param("uploadId", "upload-abc-123"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let resp = client
        .upload_part(
            "test-bucket",
            "large-file.bin",
            "upload-abc-123",
            3,
            bytes::Bytes::from("data"),
        )
        .await
        .unwrap();
    // No ETag header → empty string fallback
    assert_eq!(resp.etag, "");
    assert_eq!(resp.part_number, 3);
}

// 20. complete_multipart — POST /{bucket}/{key}?uploadId=xxx
#[tokio::test]
async fn complete_multipart_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/test-bucket/large-file.bin"))
        .and(query_param("uploadId", "upload-abc-123"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Location": "https://s3.example.com/test-bucket/large-file.bin",
                "Bucket": "test-bucket",
                "Key": "large-file.bin",
                "ETag": "\"final-etag-xyz\""
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let parts = serde_json::json!({
        "Parts": [
            {"PartNumber": 1, "ETag": "\"part-etag-1\""},
            {"PartNumber": 2, "ETag": "\"part-etag-2\""}
        ]
    });
    let resp = client
        .complete_multipart("test-bucket", "large-file.bin", "upload-abc-123", &parts)
        .await
        .unwrap();
    assert_eq!(resp.bucket, "test-bucket");
    assert_eq!(resp.key, "large-file.bin");
    assert_eq!(resp.etag, Some("\"final-etag-xyz\"".to_string()));
    assert_eq!(
        resp.location,
        Some("https://s3.example.com/test-bucket/large-file.bin".to_string())
    );
}

// 21. abort_multipart — DELETE /{bucket}/{key}?uploadId=xxx
#[tokio::test]
async fn abort_multipart_success() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/test-bucket/large-file.bin"))
        .and(query_param("uploadId", "upload-abc-123"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    client
        .abort_multipart("test-bucket", "large-file.bin", "upload-abc-123")
        .await
        .unwrap();
}

// ===========================================================================
// Additional error paths
// ===========================================================================

#[tokio::test]
async fn create_bucket_error_409_conflict() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/existing-bucket"))
        .respond_with(ResponseTemplate::new(409).set_body_string("BucketAlreadyExists"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client.create_bucket("existing-bucket").await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("409"), "expected 409 in error, got: {msg}");
}

#[tokio::test]
async fn delete_bucket_error_404() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/no-such-bucket"))
        .respond_with(ResponseTemplate::new(404).set_body_string("NoSuchBucket"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client.delete_bucket("no-such-bucket").await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("404"), "expected 404 in error, got: {msg}");
}

#[tokio::test]
async fn delete_object_error_403() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/test-bucket/protected.txt"))
        .respond_with(ResponseTemplate::new(403).set_body_string("Access Denied"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client.delete_object("test-bucket", "protected.txt").await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("403"), "expected 403 in error, got: {msg}");
}

#[tokio::test]
async fn get_acl_error_403() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/test-bucket"))
        .and(query_param("acl", ""))
        .respond_with(ResponseTemplate::new(403).set_body_string("Access Denied"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client.get_acl("test-bucket").await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("403"), "expected 403 in error, got: {msg}");
}

#[tokio::test]
async fn set_versioning_error_403() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/test-bucket"))
        .and(query_param("versioning", ""))
        .respond_with(ResponseTemplate::new(403).set_body_string("Access Denied"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let body = serde_json::json!({"Status": "Enabled"});
    let err = client.set_versioning("test-bucket", &body).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("403"), "expected 403 in error, got: {msg}");
}

#[tokio::test]
async fn copy_object_error_500() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/dest-bucket/dest-key"))
        .respond_with(ResponseTemplate::new(500).set_body_string("InternalError"))
        .expect(1)
        .mount(&server)
        .await;

    let client = mock_client(&server);
    let err = client
        .copy_object("dest-bucket", "dest-key", "/src-bucket/src-key")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("500"), "expected 500 in error, got: {msg}");
}

// ===========================================================================
// Client construction (kept from original)
// ===========================================================================

#[tokio::test]
async fn client_from_parts() {
    let client = S3Client::from_parts(MINIO_URL.into(), AuthMethod::None);
    assert_eq!(client.base_url(), MINIO_URL);
    assert_eq!(client.service_name(), "s3");
}

#[tokio::test]
async fn client_connect_builds_url() {
    let client = S3Client::connect("example.com");
    assert_eq!(client.base_url(), "https://s3.example.com");
}

#[tokio::test]
async fn client_set_auth_does_not_panic() {
    let mut client = S3Client::from_parts(MINIO_URL.into(), AuthMethod::None);
    client.set_auth(AuthMethod::Bearer("tok".into()));
    assert_eq!(client.base_url(), MINIO_URL);
}
