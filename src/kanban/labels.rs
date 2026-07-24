//! Kanban label catalog commands.

use clap::Subcommand;
use sdk::error::{Result, SunbeamError};
use sdk::kanban::KanbanClient;
use sdk::kanban::prelude::buffa_types;
use sdk::kanban::v1;
use serde::Serialize;

use super::{mutating_options, object_id_options, required};
use crate::output::{OutputFormat, render, render_list};

/// Label actions.
#[derive(Debug, Clone, Subcommand)]
pub enum LabelAction {
    /// List the labels visible to a project (global + project labels).
    List {
        /// Project ID, key, or name.
        #[arg(short, long)]
        project: String,
    },
    /// Create a label.
    Create {
        /// Project ID, key, or name (omit for a global label).
        #[arg(short, long)]
        project: Option<String>,
        /// Label name.
        #[arg(short, long)]
        name: String,
        /// Style token (beam-ui token name, never a hex code).
        #[arg(short, long)]
        style: Option<String>,
    },
    /// Update a label.
    Update {
        /// Label ID or name (a name needs --project for resolution).
        label: String,
        /// Project ID, key, or name (for name resolution).
        #[arg(short, long)]
        project: Option<String>,
        /// New name.
        #[arg(short, long)]
        name: Option<String>,
        /// New style token.
        #[arg(short, long)]
        style: Option<String>,
    },
    /// Delete a label (card assignments are removed via cascade).
    Delete {
        /// Label ID or name (a name needs --project for resolution).
        label: String,
        /// Project ID, key, or name (for name resolution).
        #[arg(short, long)]
        project: Option<String>,
    },
}

/// Serializable label for output.
#[derive(Serialize)]
struct LabelOut {
    id: String,
    project_id: String,
    name: String,
    style: String,
}

impl From<v1::Label> for LabelOut {
    fn from(l: v1::Label) -> Self {
        Self {
            id: l.id,
            project_id: l.project_id,
            name: l.name,
            style: l.style,
        }
    }
}

/// Resolve a label argument to a catalog ULID.
///
/// ID-shaped input passes through untouched; anything else resolves by name
/// against the project's catalog, so `--project` is required for names.
async fn resolve_label(client: &KanbanClient, project: Option<&str>, raw: &str) -> Result<String> {
    if super::resolve::looks_like_id(raw) {
        return Ok(raw.to_string());
    }
    let Some(project) = project else {
        return Err(SunbeamError::Other(format!(
            "required: --project <ID|name> to resolve label name {raw:?}"
        )));
    };
    super::resolve::NameResolver::new(client)
        .label(project, raw)
        .await
}

/// Run a label command.
pub(crate) async fn run(
    cmd: LabelAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        LabelAction::List { project } => {
            let resp = client
                .labels()
                .list_labels_with_options(
                    v1::ListLabelsRequest {
                        project_id: project.clone(),
                        ..Default::default()
                    },
                    object_id_options(&project),
                )
                .await?
                .into_owned();
            let labels: Vec<LabelOut> = resp.labels.into_iter().map(Into::into).collect();
            render_list(
                &labels,
                &["NAME", "STYLE", "PROJECT", "ID"],
                |l| {
                    vec![
                        l.name.clone(),
                        l.style.clone(),
                        l.project_id.clone(),
                        l.id.clone(),
                    ]
                },
                format,
            )
        }
        LabelAction::Create {
            project,
            name,
            style,
        } => {
            let object_id = project.clone().unwrap_or_else(|| "global".to_string());
            let resp = client
                .labels()
                .create_label_with_options(
                    v1::CreateLabelRequest {
                        project_id: project.unwrap_or_default(),
                        name,
                        style: style.unwrap_or_default(),
                        ..Default::default()
                    },
                    mutating_options(&object_id),
                )
                .await?
                .into_owned();
            render(&LabelOut::from(required(resp.label, "label")?), format)
        }
        LabelAction::Update {
            label,
            project,
            name,
            style,
        } => {
            let label_id = resolve_label(client, project.as_deref(), &label).await?;
            let mut paths = Vec::new();
            if name.is_some() {
                paths.push("name".to_string());
            }
            if style.is_some() {
                paths.push("style".to_string());
            }
            let resp = client
                .labels()
                .update_label_with_options(
                    v1::UpdateLabelRequest {
                        label_id: label_id.clone(),
                        update_mask: Some(buffa_types::google::protobuf::FieldMask {
                            paths,
                            ..Default::default()
                        })
                        .into(),
                        name: name.unwrap_or_default(),
                        style: style.unwrap_or_default(),
                        ..Default::default()
                    },
                    mutating_options(&label_id),
                )
                .await?
                .into_owned();
            render(&LabelOut::from(required(resp.label, "label")?), format)
        }
        LabelAction::Delete { label, project } => {
            let label_id = resolve_label(client, project.as_deref(), &label).await?;
            client
                .labels()
                .delete_label_with_options(
                    v1::DeleteLabelRequest {
                        label_id: label_id.clone(),
                        ..Default::default()
                    },
                    mutating_options(&label_id),
                )
                .await?;
            render(&serde_json::json!({ "deleted": label_id }), format)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanban::testutil;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    fn label(id: &str, name: &str) -> v1::Label {
        v1::Label {
            id: id.to_string(),
            project_id: "proj_1".into(),
            name: name.to_string(),
            style: "red".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn list_labels_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/sunbeam.kanban.v1.LabelService/ListLabels"))
                .respond_with(testutil::proto_response(&v1::ListLabelsResponse {
                    labels: vec![label("label_1", "bug")],
                    ..Default::default()
                }))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(
                LabelAction::List {
                    project: "proj_1".into(),
                },
                format,
                &client,
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn create_update_delete_label() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.LabelService/CreateLabel"))
            .respond_with(testutil::proto_response(&v1::CreateLabelResponse {
                label: label("label_new", "backend").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.LabelService/UpdateLabel"))
            .respond_with(testutil::proto_response(&v1::UpdateLabelResponse {
                label: label("label_1", "renamed").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.LabelService/DeleteLabel"))
            .respond_with(testutil::proto_response(&v1::DeleteLabelResponse::default()))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            LabelAction::Create {
                project: Some("proj_1".into()),
                name: "backend".into(),
                style: Some("blue".into()),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            LabelAction::Create {
                project: None,
                name: "global".into(),
                style: None,
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            LabelAction::Update {
                label: "label_1".into(),
                project: None,
                name: Some("renamed".into()),
                style: Some("green".into()),
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
        run(
            LabelAction::Delete {
                label: "label_1".into(),
                project: None,
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn update_resolves_name_with_project() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.LabelService/ListLabels"))
            .respond_with(testutil::proto_response(&v1::ListLabelsResponse {
                labels: vec![label("label_1", "bug")],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.LabelService/UpdateLabel"))
            .respond_with(testutil::proto_response(&v1::UpdateLabelResponse {
                label: label("label_1", "bug").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            LabelAction::Update {
                label: "BUG".into(),
                project: Some("proj_1".into()),
                name: None,
                style: Some("orange".into()),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn name_without_project_errors() {
        let client = testutil::client_for("http://127.0.0.1:1");
        let err = run(
            LabelAction::Delete {
                label: "bug".into(),
                project: None,
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("--project"), "{err}");
    }

    #[tokio::test]
    async fn rpc_error_is_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.LabelService/ListLabels"))
            .respond_with(testutil::connect_error(500, "internal", "db down"))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            LabelAction::List {
                project: "proj_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("db down"));
    }
}
