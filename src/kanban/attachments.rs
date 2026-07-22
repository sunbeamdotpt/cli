//! Kanban attachment commands.

use clap::Subcommand;
use sdk::error::{Result, ResultExt, SunbeamError};
use sdk::kanban::KanbanClient;
use sdk::kanban::v1;
use sdk::reqwest;
use serde::Serialize;

use super::{fmt_ts, mutating_options, object_id_options, required};
use crate::output::{OutputFormat, render, render_list};

/// Attachment actions.
#[derive(Debug, Clone, Subcommand)]
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
        /// Card ID.
        card: String,
        /// Attachment ID.
        attachment_id: String,
        /// Destination path.
        path: String,
    },
    /// Delete an attachment.
    Delete {
        /// Card ID.
        card: String,
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
    uploaded_at: String,
}

impl From<v1::Attachment> for AttachmentOut {
    fn from(a: v1::Attachment) -> Self {
        Self {
            id: a.id,
            card_id: a.card_id,
            s3_key: a.s3_key,
            filename: a.filename,
            mime_type: a.mime_type,
            size_bytes: a.size_bytes,
            uploaded_by: a.uploaded_by,
            uploaded_at: fmt_ts(&a.uploaded_at),
        }
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

/// Resolve attachment uploader subjects to email addresses through the
/// sso-gateway IdentityService. Failures are non-fatal: subjects are left
/// as-is.
async fn resolve_uploader_emails(attachments: &mut [AttachmentOut]) {
    let subjects: Vec<&str> = attachments
        .iter()
        .map(|a| a.uploaded_by.as_str())
        .filter(|s| !s.is_empty())
        .collect();

    if subjects.is_empty() {
        return;
    }

    let email_map = match crate::auth::resolve_emails_for_subjects(&subjects).await {
        Ok(m) => m,
        Err(_) => return,
    };

    for a in attachments.iter_mut() {
        if a.uploaded_by.is_empty() {
            continue;
        }
        if let Some(email) = email_map.get(&a.uploaded_by) {
            a.uploaded_by = email.clone();
        }
    }
}

/// Run an attachment command.
pub(crate) async fn run(
    cmd: AttachmentAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        AttachmentAction::List { card_id } => {
            let resp = client
                .attachments()
                .list_attachments_by_card_with_options(
                    v1::ListAttachmentsByCardRequest {
                        card_id: card_id.clone(),
                        ..Default::default()
                    },
                    object_id_options(&card_id),
                )
                .await?
                .into_owned();
            let mut attachments: Vec<_> = resp
                .attachments
                .into_iter()
                .map(AttachmentOut::from)
                .collect();
            resolve_uploader_emails(&mut attachments).await;
            render_list(
                &attachments,
                &[
                    "FILENAME",
                    "MIME TYPE",
                    "SIZE",
                    "UPLOADED AT",
                    "UPLOADED BY",
                    "ID",
                ],
                |a| {
                    vec![
                        a.filename.clone(),
                        a.mime_type.clone(),
                        a.size_bytes.to_string(),
                        a.uploaded_at.clone(),
                        a.uploaded_by.clone(),
                        a.id.clone(),
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

            let init = client
                .attachments()
                .request_presigned_upload_with_options(
                    v1::RequestPresignedUploadRequest {
                        card_id: card_id.clone(),
                        filename,
                        mime_type: mime_type.clone(),
                        size_bytes,
                        ..Default::default()
                    },
                    mutating_options(&card_id),
                )
                .await?
                .into_owned();

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
                return Err(SunbeamError::network(format!(
                    "upload to presigned URL failed: {status} {body}"
                )));
            }

            let confirmed = client
                .attachments()
                .confirm_upload_with_options(
                    v1::ConfirmUploadRequest {
                        attachment_id: init.attachment_id.clone(),
                        ..Default::default()
                    },
                    mutating_options(&card_id),
                )
                .await?
                .into_owned();

            let mut out = AttachmentOut::from(required(confirmed.attachment, "attachment")?);
            resolve_uploader_emails(std::slice::from_mut(&mut out)).await;
            render(&out, format)
        }
        AttachmentAction::Download {
            card,
            attachment_id,
            path,
        } => {
            let dl = client
                .attachments()
                .request_presigned_download_with_options(
                    v1::RequestPresignedDownloadRequest {
                        attachment_id: attachment_id.clone(),
                        ..Default::default()
                    },
                    object_id_options(&card),
                )
                .await?
                .into_owned();

            let http = reqwest::Client::new();
            let get_resp = http
                .get(&dl.presigned_url)
                .send()
                .await
                .with_ctx(|| "failed to GET attachment from presigned URL".to_string())?;
            if !get_resp.status().is_success() {
                let status = get_resp.status();
                let body = get_resp.text().await.unwrap_or_default();
                return Err(SunbeamError::network(format!(
                    "download from presigned URL failed: {status} {body}"
                )));
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
        AttachmentAction::Delete {
            card,
            attachment_id,
        } => {
            client
                .attachments()
                .delete_attachment_with_options(
                    v1::DeleteAttachmentRequest {
                        attachment_id: attachment_id.clone(),
                        ..Default::default()
                    },
                    mutating_options(&card),
                )
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
    use crate::kanban::testutil;
    use sdk::kanban::prelude::buffa_types;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn attachment(id: &str) -> v1::Attachment {
        v1::Attachment {
            id: id.to_string(),
            card_id: "card_1".into(),
            s3_key: format!("kanban/cards/card_1/{id}/notes.txt"),
            filename: "notes.txt".into(),
            mime_type: "text/plain".into(),
            size_bytes: 11,
            uploaded_by: "user:alice".into(),
            ..Default::default()
        }
    }

    #[test]
    fn guess_mime_type_covers_extensions() {
        assert_eq!(guess_mime_type("a.txt"), "text/plain");
        assert_eq!(guess_mime_type("a.MD"), "text/markdown");
        assert_eq!(guess_mime_type("a.png"), "image/png");
        assert_eq!(guess_mime_type("a.pdf"), "application/pdf");
        assert_eq!(guess_mime_type("a.bin"), "application/octet-stream");
        assert_eq!(guess_mime_type("noext"), "application/octet-stream");
    }

    #[tokio::test]
    async fn list_attachments_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(
                    "/sunbeam.kanban.v1.AttachmentService/ListAttachmentsByCard",
                ))
                .respond_with(testutil::proto_response(
                    &v1::ListAttachmentsByCardResponse {
                        attachments: vec![attachment("att_1")],
                        ..Default::default()
                    },
                ))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(
                AttachmentAction::List {
                    card_id: "card_1".into(),
                },
                format,
                &client,
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn upload_flow_puts_bytes_and_confirms() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.AttachmentService/RequestPresignedUpload",
            ))
            .respond_with(testutil::proto_response(
                &v1::RequestPresignedUploadResponse {
                    presigned_url: format!("{}/upload", server.uri()),
                    s3_key: "kanban/cards/card_1/att_1/notes.txt".into(),
                    attachment_id: "att_1".into(),
                    ..Default::default()
                },
            ))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.AttachmentService/ConfirmUpload"))
            .respond_with(testutil::proto_response(&v1::ConfirmUploadResponse {
                attachment: attachment("att_1").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, b"hello world").unwrap();

        let client = testutil::client_for(&server.uri());
        run(
            AttachmentAction::Upload {
                card_id: "card_1".into(),
                file: file.to_string_lossy().to_string(),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn upload_fails_when_file_missing() {
        let client = testutil::client_for("http://127.0.0.1:1");
        let err = run(
            AttachmentAction::Upload {
                card_id: "card_1".into(),
                file: "/nonexistent/nope.txt".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("failed to read file"));
    }

    #[tokio::test]
    async fn download_flow_writes_file() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.AttachmentService/RequestPresignedDownload",
            ))
            .respond_with(testutil::proto_response(
                &v1::RequestPresignedDownloadResponse {
                    presigned_url: format!("{}/download", server.uri()),
                    ..Default::default()
                },
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"file bytes".to_vec()))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.txt");

        let client = testutil::client_for(&server.uri());
        run(
            AttachmentAction::Download {
                card: "card_1".into(),
                attachment_id: "att_1".into(),
                path: dest.to_string_lossy().to_string(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"file bytes");
    }

    #[tokio::test]
    async fn download_surfaces_http_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.AttachmentService/RequestPresignedDownload",
            ))
            .respond_with(testutil::proto_response(
                &v1::RequestPresignedDownloadResponse {
                    presigned_url: format!("{}/download", server.uri()),
                    ..Default::default()
                },
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download"))
            .respond_with(ResponseTemplate::new(404).set_body_string("gone"))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.txt");
        let client = testutil::client_for(&server.uri());
        let err = run(
            AttachmentAction::Download {
                card: "card_1".into(),
                attachment_id: "att_1".into(),
                path: dest.to_string_lossy().to_string(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("download from presigned URL failed")
        );
    }

    #[tokio::test]
    async fn delete_attachment() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.AttachmentService/DeleteAttachment",
            ))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            AttachmentAction::Delete {
                card: "card_1".into(),
                attachment_id: "att_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rpc_error_is_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.AttachmentService/ListAttachmentsByCard",
            ))
            .respond_with(testutil::connect_error(404, "not_found", "card missing"))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            AttachmentAction::List {
                card_id: "card_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("card missing"));
    }
}
