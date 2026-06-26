//! Kanban attachment commands.

use crate::error::{Result, ResultExt, SunbeamError};
use crate::kanban::client::{self, AttachmentServiceClient};
use crate::logger::Logger;
use crate::output::{OutputFormat, render, render_list};
use async_trait::async_trait;
use clap::Subcommand;
use serde::Serialize;
use tonic::metadata::MetadataValue;

/// Attachment actions.
#[derive(Debug, Subcommand)]
pub enum AttachmentAction {
    /// List attachments for a card.
    List {
        /// Card ID.
        card_id: String,
    },
    /// Upload a file to a card.
    Upload {
        /// Card ID.
        card_id: String,
        /// Local file path.
        file: String,
    },
    /// Download an attachment.
    Download {
        /// Attachment ID.
        attachment_id: String,
        /// Destination path.
        path: String,
    },
    /// Delete an attachment.
    Delete {
        /// Attachment ID.
        attachment_id: String,
    },
}

/// Serializable attachment record.
#[derive(Serialize)]
struct AttachmentOut {
    id: String,
    card_id: String,
    s3_key: String,
    filename: String,
    mime_type: String,
    size_bytes: i64,
    uploaded_by: String,
    uploaded_at: Option<String>,
}

/// Build a mutating request with object-id and idempotency headers.
fn mutating_request<T>(msg: T, object_id: &str) -> Result<tonic::Request<T>> {
    let mut req = client::request_with_object_id(msg, object_id)?;
    let key = crate::kanban::new_idempotency_key();
    let value = MetadataValue::try_from(key)
        .with_ctx(|| "invalid idempotency key (cannot encode as header)".to_string())?;
    req.metadata_mut()
        .insert("x-sunbeam-idempotency-key", value);
    Ok(req)
}

/// Format a Prost timestamp as RFC 3339.
fn fmt_timestamp(ts: &Option<prost_types::Timestamp>) -> Option<String> {
    ts.as_ref().and_then(|t| {
        chrono::DateTime::from_timestamp(t.seconds, t.nanos.max(0) as u32).map(|dt| dt.to_rfc3339())
    })
}

/// Convert a generated attachment into a serializable view.
fn attachment_out(a: client::Attachment) -> AttachmentOut {
    AttachmentOut {
        id: a.id,
        card_id: a.card_id,
        s3_key: a.s3_key,
        filename: a.filename,
        mime_type: a.mime_type,
        size_bytes: a.size_bytes,
        uploaded_by: a.uploaded_by,
        uploaded_at: fmt_timestamp(&a.uploaded_at),
    }
}

/// Guess a MIME type from a file extension.
fn guess_mime_type(path: &str) -> String {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "txt" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Trait abstracting the Kanban attachment service for testability.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait AttachmentService {
    /// List attachments for a card.
    async fn list_attachments_by_card(
        &mut self,
        req: tonic::Request<client::ListAttachmentsByCardRequest>,
    ) -> Result<client::ListAttachmentsByCardResponse>;
    /// Request a presigned upload URL.
    async fn request_presigned_upload(
        &mut self,
        req: tonic::Request<client::RequestPresignedUploadRequest>,
    ) -> Result<client::RequestPresignedUploadResponse>;
    /// Confirm an upload after the client-side PUT succeeds.
    async fn confirm_upload(
        &mut self,
        req: tonic::Request<client::ConfirmUploadRequest>,
    ) -> Result<client::Attachment>;
    /// Request a presigned download URL.
    async fn request_presigned_download(
        &mut self,
        req: tonic::Request<client::RequestPresignedDownloadRequest>,
    ) -> Result<client::RequestPresignedDownloadResponse>;
    /// Delete an attachment.
    async fn delete_attachment(
        &mut self,
        req: tonic::Request<client::DeleteAttachmentRequest>,
    ) -> Result<()>;
}

/// Wrapper around the generated Tonic attachment client.
#[derive(Debug)]
pub struct AttachmentServiceClientWrapper {
    inner: AttachmentServiceClient<client::AuthChannel>,
}

impl AttachmentServiceClientWrapper {
    /// Build a wrapper from an authenticated channel.
    pub fn new(inner: AttachmentServiceClient<client::AuthChannel>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl AttachmentService for AttachmentServiceClientWrapper {
    async fn list_attachments_by_card(
        &mut self,
        req: tonic::Request<client::ListAttachmentsByCardRequest>,
    ) -> Result<client::ListAttachmentsByCardResponse> {
        let resp = self.inner.list_attachments_by_card(req).await?;
        Ok(resp.into_inner())
    }

    async fn request_presigned_upload(
        &mut self,
        req: tonic::Request<client::RequestPresignedUploadRequest>,
    ) -> Result<client::RequestPresignedUploadResponse> {
        let resp = self.inner.request_presigned_upload(req).await?;
        Ok(resp.into_inner())
    }

    async fn confirm_upload(
        &mut self,
        req: tonic::Request<client::ConfirmUploadRequest>,
    ) -> Result<client::Attachment> {
        let resp = self.inner.confirm_upload(req).await?;
        Ok(resp.into_inner())
    }

    async fn request_presigned_download(
        &mut self,
        req: tonic::Request<client::RequestPresignedDownloadRequest>,
    ) -> Result<client::RequestPresignedDownloadResponse> {
        let resp = self.inner.request_presigned_download(req).await?;
        Ok(resp.into_inner())
    }

    async fn delete_attachment(
        &mut self,
        req: tonic::Request<client::DeleteAttachmentRequest>,
    ) -> Result<()> {
        self.inner.delete_attachment(req).await?;
        Ok(())
    }
}

/// Build an attachment-service client wrapper for the given server and token.
pub async fn build_client(
    logger: &Logger,
    server: &str,
    token: &str,
) -> Result<AttachmentServiceClientWrapper> {
    let channel = client::build(logger, server, token).await?;
    Ok(AttachmentServiceClientWrapper::new(
        AttachmentServiceClient::new(channel),
    ))
}

/// Run an attachment command.
pub async fn run(
    cmd: AttachmentAction,
    format: OutputFormat,
    client: &mut dyn AttachmentService,
) -> Result<()> {
    match cmd {
        AttachmentAction::List { card_id } => {
            let req = client::ListAttachmentsByCardRequest {
                card_id: card_id.clone(),
            };
            let resp = client
                .list_attachments_by_card(client::request_with_object_id(req, &card_id)?)
                .await?;
            let attachments: Vec<_> = resp.attachments.into_iter().map(attachment_out).collect();
            render_list(
                &attachments,
                &[
                    "ID",
                    "FILENAME",
                    "MIME TYPE",
                    "SIZE",
                    "UPLOADED BY",
                    "UPLOADED AT",
                ],
                |a| {
                    vec![
                        a.id.clone(),
                        a.filename.clone(),
                        a.mime_type.clone(),
                        a.size_bytes.to_string(),
                        a.uploaded_by.clone(),
                        a.uploaded_at.clone().unwrap_or_default(),
                    ]
                },
                format,
            )
        }
        AttachmentAction::Upload { card_id, file } => {
            let bytes = tokio::fs::read(&file)
                .await
                .with_ctx(|| format!("failed to read file {file}"))?;
            let filename = std::path::Path::new(&file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&file)
                .to_string();
            let mime_type = guess_mime_type(&file);
            let size_bytes = bytes.len() as i64;

            let init_req = client::RequestPresignedUploadRequest {
                card_id: card_id.clone(),
                filename,
                mime_type: mime_type.clone(),
                size_bytes,
            };
            let init = client
                .request_presigned_upload(mutating_request(init_req, &card_id)?)
                .await?;

            let http = reqwest::Client::new();
            let put_resp = http
                .put(&init.presigned_url)
                .header(reqwest::header::CONTENT_TYPE, &mime_type)
                .body(bytes)
                .send()
                .await
                .with_ctx(|| "failed to PUT attachment to presigned URL".to_string())?;
            if !put_resp.status().is_success() {
                let status = put_resp.status();
                let body = put_resp.text().await.unwrap_or_default();
                return Err(SunbeamError::Network {
                    context: format!("upload to presigned URL failed: {status} {body}"),
                    source: None,
                });
            }

            let confirm_req = client::ConfirmUploadRequest {
                attachment_id: init.attachment_id.clone(),
            };
            let confirmed = client
                .confirm_upload(mutating_request(confirm_req, &init.attachment_id)?)
                .await?;

            render(&attachment_out(confirmed), format)
        }
        AttachmentAction::Download {
            attachment_id,
            path,
        } => {
            let req = client::RequestPresignedDownloadRequest {
                attachment_id: attachment_id.clone(),
            };
            let dl = client
                .request_presigned_download(client::request_with_object_id(req, &attachment_id)?)
                .await?;

            let http = reqwest::Client::new();
            let get_resp = http
                .get(&dl.presigned_url)
                .send()
                .await
                .with_ctx(|| "failed to GET attachment from presigned URL".to_string())?;
            if !get_resp.status().is_success() {
                let status = get_resp.status();
                let body = get_resp.text().await.unwrap_or_default();
                return Err(SunbeamError::Network {
                    context: format!("download from presigned URL failed: {status} {body}"),
                    source: None,
                });
            }
            let bytes = get_resp
                .bytes()
                .await
                .with_ctx(|| "failed to read attachment bytes".to_string())?;
            tokio::fs::write(&path, &bytes)
                .await
                .with_ctx(|| format!("failed to write attachment to {path}"))?;

            render(
                &serde_json::json!({
                    "attachment_id": attachment_id,
                    "path": path,
                    "bytes": bytes.len(),
                }),
                format,
            )
        }
        AttachmentAction::Delete { attachment_id } => {
            let req = client::DeleteAttachmentRequest {
                attachment_id: attachment_id.clone(),
            };
            client
                .delete_attachment(mutating_request(req, &attachment_id)?)
                .await?;
            render(
                &serde_json::json!({
                    "deleted": true,
                    "attachment_id": attachment_id,
                }),
                format,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_attachment(id: &str) -> client::Attachment {
        client::Attachment {
            id: id.into(),
            card_id: "card_1".into(),
            s3_key: format!("kanban/cards/card_1/{id}/file.txt"),
            filename: "file.txt".into(),
            mime_type: "text/plain".into(),
            size_bytes: 12,
            uploaded_by: "user_1".into(),
            uploaded_at: Some(prost_types::Timestamp {
                seconds: 1_700_000_000,
                nanos: 0,
            }),
        }
    }

    #[tokio::test]
    async fn list_renders_attachments() {
        let mut mock = MockAttachmentService::new();
        mock.expect_list_attachments_by_card()
            .withf(|req| req.get_ref().card_id == "card_1")
            .times(1)
            .returning(|_| {
                Ok(client::ListAttachmentsByCardResponse {
                    attachments: vec![sample_attachment("att_1")],
                })
            });

        run(
            AttachmentAction::List {
                card_id: "card_1".into(),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_renders_ok() {
        let mut mock = MockAttachmentService::new();
        mock.expect_delete_attachment()
            .withf(|req| req.get_ref().attachment_id == "att_1")
            .times(1)
            .returning(|_| Ok(()));

        run(
            AttachmentAction::Delete {
                attachment_id: "att_1".into(),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn upload_with_http_round_trip() {
        let mut mock = MockAttachmentService::new();

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let presigned_url = server.uri() + "/upload";
        mock.expect_request_presigned_upload()
            .withf(|req| {
                let r = req.get_ref();
                r.card_id == "card_1" && r.filename == "file.txt" && r.mime_type == "text/plain"
            })
            .times(1)
            .returning(move |_| {
                Ok(client::RequestPresignedUploadResponse {
                    presigned_url: presigned_url.clone(),
                    s3_key: "kanban/cards/card_1/att_1/file.txt".into(),
                    attachment_id: "att_1".into(),
                    expires_at: None,
                })
            });

        mock.expect_confirm_upload()
            .withf(|req| req.get_ref().attachment_id == "att_1")
            .times(1)
            .returning(|_| Ok(sample_attachment("att_1")));

        let tmp_dir = tempfile::tempdir().unwrap();
        let path = tmp_dir.path().join("file.txt");
        std::fs::write(&path, b"hello world\n").unwrap();

        run(
            AttachmentAction::Upload {
                card_id: "card_1".into(),
                file: path.to_str().unwrap().to_string(),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn download_with_http_round_trip() {
        let mut mock = MockAttachmentService::new();

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(b"hello world\n"))
            .mount(&server)
            .await;

        let presigned_url = server.uri() + "/download";
        mock.expect_request_presigned_download()
            .withf(|req| req.get_ref().attachment_id == "att_1")
            .times(1)
            .returning(move |_| {
                Ok(client::RequestPresignedDownloadResponse {
                    presigned_url: presigned_url.clone(),
                    expires_at: None,
                })
            });

        let tmp = tempfile::NamedTempFile::with_suffix(".txt").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();

        run(
            AttachmentAction::Download {
                attachment_id: "att_1".into(),
                path,
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_attachments_table_renders() {
        let mut mock = MockAttachmentService::new();
        mock.expect_list_attachments_by_card()
            .times(1)
            .returning(|_| {
                Ok(client::ListAttachmentsByCardResponse {
                    attachments: vec![sample_attachment("att_1")],
                })
            });

        run(
            AttachmentAction::List {
                card_id: "card_1".into(),
            },
            OutputFormat::Table,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[test]
    fn guess_mime_type_covers_branches() {
        assert_eq!(guess_mime_type("file.txt"), "text/plain");
        assert_eq!(guess_mime_type("file.md"), "text/markdown");
        assert_eq!(guess_mime_type("file.html"), "text/html");
        assert_eq!(guess_mime_type("file.yml"), "application/yaml");
        assert_eq!(guess_mime_type("file.png"), "image/png");
        assert_eq!(guess_mime_type("file.jpg"), "image/jpeg");
        assert_eq!(guess_mime_type("file.pdf"), "application/pdf");
        assert_eq!(guess_mime_type("file.mp4"), "video/mp4");
        assert_eq!(guess_mime_type("file.UNKNOWN"), "application/octet-stream");
        assert_eq!(guess_mime_type("no_extension"), "application/octet-stream");
    }

    #[test]
    fn fmt_timestamp_negative_nanos() {
        let ts = prost_types::Timestamp {
            seconds: 1,
            nanos: -1,
        };
        assert!(super::fmt_timestamp(&Some(ts)).is_some());
    }

    #[tokio::test]
    async fn build_client_rejects_invalid_url() {
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let err = build_client(&logger, ":::bad", "token").await.unwrap_err();
        assert!(err.to_string().contains("invalid kanban server URL"));
    }
}
