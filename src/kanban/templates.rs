//! Kanban board template commands.

use clap::Subcommand;
use sdk::error::Result;
use sdk::kanban::KanbanClient;
use sdk::kanban::prelude::buffa_types;
use sdk::kanban::v1;
use serde::Serialize;

use super::{fmt_ts, mutating_options, required};
use crate::output::{OutputFormat, render, render_list};

/// Board template actions.
#[derive(Debug, Clone, Subcommand)]
pub enum TemplateAction {
    /// List templates.
    List {
        /// Project ID (omit for global templates).
        #[arg(short, long)]
        project: Option<String>,
    },
    /// Get a template.
    Get {
        /// Template ID or name.
        template_id: String,
    },
    /// Create a template.
    Create {
        /// Project ID (omit for global).
        #[arg(short, long)]
        project: Option<String>,
        /// Template name.
        #[arg(short, long)]
        name: String,
        /// Description.
        #[arg(short, long)]
        description: Option<String>,
        /// Template columns: comma-separated `title[:accent][!]` entries
        /// (position by order, `!` marks a completion lane), e.g.
        /// --columns "todo:blue,in progress:amber,review:purple,done:green!"
        #[arg(short, long)]
        columns: Option<String>,
    },
    /// Update a template.
    Update {
        /// Template ID or name.
        template_id: String,
        /// New name.
        #[arg(short, long)]
        name: Option<String>,
        /// New description.
        #[arg(short, long)]
        description: Option<String>,
        /// Replace the template's columns (same spec as create --columns).
        #[arg(short, long)]
        columns: Option<String>,
    },
    /// Delete a template.
    Delete {
        /// Template ID or name.
        template_id: String,
    },
}

/// Serializable column preset for output.
#[derive(Serialize)]
struct TemplateColumnOut {
    title: String,
    position: i32,
    accent: String,
    is_done: bool,
}

impl From<v1::TemplateColumn> for TemplateColumnOut {
    fn from(c: v1::TemplateColumn) -> Self {
        Self {
            title: c.title,
            position: c.position,
            accent: c.accent,
            is_done: c.is_done,
        }
    }
}

/// Map parsed `--columns` entries to template column protos.
fn template_columns(specs: &[super::ColumnSpec]) -> Vec<v1::TemplateColumn> {
    specs
        .iter()
        .map(|s| v1::TemplateColumn {
            title: s.title.clone(),
            position: s.position,
            accent: s.accent.clone(),
            is_done: s.is_done,
            ..Default::default()
        })
        .collect()
}

/// Serializable board template for output.
#[derive(Serialize)]
struct BoardTemplateOut {
    id: String,
    project_id: String,
    name: String,
    description: String,
    columns: Vec<TemplateColumnOut>,
    is_global: bool,
    created_at: String,
    updated_at: String,
}

impl From<v1::BoardTemplate> for BoardTemplateOut {
    fn from(t: v1::BoardTemplate) -> Self {
        Self {
            id: t.id,
            project_id: t.project_id,
            name: t.name,
            description: t.description,
            columns: t.columns.into_iter().map(Into::into).collect(),
            is_global: t.is_global,
            created_at: fmt_ts(&t.created_at),
            updated_at: fmt_ts(&t.updated_at),
        }
    }
}

/// Run a board template command.
pub(crate) async fn run(
    cmd: TemplateAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        TemplateAction::List { project } => {
            let resp = client
                .templates()
                .list_templates(v1::ListTemplatesRequest {
                    project_id: project.unwrap_or_default(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            let templates: Vec<BoardTemplateOut> =
                resp.templates.into_iter().map(Into::into).collect();
            render_list(
                &templates,
                &["NAME", "DESCRIPTION", "GLOBAL", "COLUMNS", "PROJECT", "ID"],
                |t| {
                    vec![
                        t.name.clone(),
                        t.description.clone(),
                        t.is_global.to_string(),
                        t.columns.len().to_string(),
                        t.project_id.clone(),
                        t.id.clone(),
                    ]
                },
                format,
            )
        }
        TemplateAction::Get { template_id } => {
            let template_id = super::resolve::NameResolver::new(client)
                .template(None, &template_id)
                .await?;
            let resp = client
                .templates()
                .get_template(v1::GetTemplateRequest {
                    template_id: template_id.clone(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            render(
                &BoardTemplateOut::from(required(resp.template, "template")?),
                format,
            )
        }
        TemplateAction::Create {
            project,
            name,
            description,
            columns,
        } => {
            let object_id = project.clone().unwrap_or_else(|| "global".to_string());
            let columns = columns
                .as_deref()
                .map(super::parse_columns_spec)
                .transpose()?;
            let resp = client
                .templates()
                .create_template_with_options(
                    v1::CreateTemplateRequest {
                        project_id: project.unwrap_or_default(),
                        name,
                        description: description.unwrap_or_default(),
                        columns: columns.as_deref().map(template_columns).unwrap_or_default(),
                        ..Default::default()
                    },
                    mutating_options(&object_id),
                )
                .await?
                .into_owned();
            render(
                &BoardTemplateOut::from(required(resp.template, "template")?),
                format,
            )
        }
        TemplateAction::Update {
            template_id,
            name,
            description,
            columns,
        } => {
            let template_id = super::resolve::NameResolver::new(client)
                .template(None, &template_id)
                .await?;
            let mut paths = Vec::new();
            if name.is_some() {
                paths.push("name".to_string());
            }
            if description.is_some() {
                paths.push("description".to_string());
            }
            let columns = columns
                .as_deref()
                .map(super::parse_columns_spec)
                .transpose()?;
            if columns.is_some() {
                paths.push("columns".to_string());
            }
            let resp = client
                .templates()
                .update_template_with_options(
                    v1::UpdateTemplateRequest {
                        template_id: template_id.clone(),
                        update_mask: Some(buffa_types::google::protobuf::FieldMask {
                            paths,
                            ..Default::default()
                        })
                        .into(),
                        name: name.unwrap_or_default(),
                        description: description.unwrap_or_default(),
                        columns: columns.as_deref().map(template_columns).unwrap_or_default(),
                        ..Default::default()
                    },
                    mutating_options(&template_id),
                )
                .await?
                .into_owned();
            render(
                &BoardTemplateOut::from(required(resp.template, "template")?),
                format,
            )
        }
        TemplateAction::Delete { template_id } => {
            let template_id = super::resolve::NameResolver::new(client)
                .template(None, &template_id)
                .await?;
            client
                .templates()
                .delete_template_with_options(
                    v1::DeleteTemplateRequest {
                        template_id: template_id.clone(),
                        ..Default::default()
                    },
                    mutating_options(&template_id),
                )
                .await?;
            render(&serde_json::json!({ "deleted": template_id }), format)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanban::testutil;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    fn template(id: &str, name: &str) -> v1::BoardTemplate {
        v1::BoardTemplate {
            id: id.to_string(),
            name: name.to_string(),
            description: "std layout".into(),
            is_global: true,
            columns: vec![
                v1::TemplateColumn {
                    title: "Todo".into(),
                    position: 0,
                    accent: "blue".into(),
                    ..Default::default()
                },
                v1::TemplateColumn {
                    title: "Done".into(),
                    position: 1,
                    accent: "green".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn list_templates_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/sunbeam.kanban.v1.TemplatesService/ListTemplates"))
                .respond_with(testutil::proto_response(&v1::ListTemplatesResponse {
                    templates: vec![template("tmpl_1", "Standard")],
                    ..Default::default()
                }))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(TemplateAction::List { project: None }, format, &client)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn get_template_resolves_name() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.TemplatesService/ListTemplates"))
            .respond_with(testutil::proto_response(&v1::ListTemplatesResponse {
                templates: vec![template("tmpl_1", "Standard")],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.TemplatesService/GetTemplate"))
            .respond_with(testutil::proto_response(&v1::GetTemplateResponse {
                template: template("tmpl_1", "Standard").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            TemplateAction::Get {
                template_id: "standard".into(),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_update_delete_template() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.TemplatesService/CreateTemplate"))
            .respond_with(testutil::proto_response(&v1::CreateTemplateResponse {
                template: template("tmpl_new", "New").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.TemplatesService/UpdateTemplate"))
            .respond_with(testutil::proto_response(&v1::UpdateTemplateResponse {
                template: template("tmpl_1", "Renamed").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.TemplatesService/DeleteTemplate"))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            TemplateAction::Create {
                project: Some("proj_1".into()),
                name: "New".into(),
                description: Some("d".into()),
                columns: None,
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            TemplateAction::Create {
                project: None,
                name: "Global".into(),
                description: None,
                columns: Some("todo:blue,in progress:amber,done:green!".into()),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            TemplateAction::Update {
                template_id: "tmpl_1".into(),
                name: Some("Renamed".into()),
                description: None,
                columns: Some("backlog,done!".into()),
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
        run(
            TemplateAction::Delete {
                template_id: "tmpl_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();

        // The --columns spec lands on the wire (CLI-024).
        use sdk::kanban::prelude::buffa::Message;
        let requests = server.received_requests().await.unwrap();
        let creates: Vec<_> = requests
            .iter()
            .filter(|r| r.url.path().ends_with("/CreateTemplate"))
            .collect();
        let with_columns =
            v1::CreateTemplateRequest::decode(&mut creates[1].body.as_slice()).unwrap();
        assert_eq!(with_columns.columns.len(), 3);
        assert_eq!(with_columns.columns[0].title, "todo");
        assert_eq!(with_columns.columns[0].accent, "blue");
        assert_eq!(with_columns.columns[0].position, 1);
        assert!(!with_columns.columns[0].is_done);
        assert_eq!(with_columns.columns[2].title, "done");
        assert!(with_columns.columns[2].is_done);

        let update = requests
            .iter()
            .find(|r| r.url.path().ends_with("/UpdateTemplate"))
            .expect("UpdateTemplate request");
        let updated = v1::UpdateTemplateRequest::decode(&mut update.body.as_slice()).unwrap();
        assert_eq!(updated.columns.len(), 2);
        assert!(updated.columns[1].is_done);
        let mask = updated.update_mask.as_option().expect("update mask");
        assert!(mask.paths.contains(&"columns".to_string()));
    }

    #[tokio::test]
    async fn rpc_error_is_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.TemplatesService/ListTemplates"))
            .respond_with(testutil::connect_error(500, "internal", "db down"))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            TemplateAction::List { project: None },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("db down"));
    }
}
