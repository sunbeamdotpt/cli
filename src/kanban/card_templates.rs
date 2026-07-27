//! Kanban card template commands.

use clap::Subcommand;
use sdk::error::Result;
use sdk::kanban::KanbanClient;
use sdk::kanban::prelude::buffa_types;
use sdk::kanban::v1;
use serde::Serialize;

use super::{fmt_ts, mutating_options, required};
use crate::output::{OutputFormat, render, render_list};

/// Card template actions.
#[derive(Debug, Clone, Subcommand)]
pub enum CardTemplateAction {
    /// List card templates.
    List {
        /// Project ID or name (omit for global templates).
        #[arg(short, long)]
        project: Option<String>,
    },
    /// Get a card template.
    Get {
        /// Project ID or name (omit to search global and all projects).
        #[arg(short, long)]
        project: Option<String>,
        /// Template ID or name.
        template_id: String,
    },
    /// Create a card template.
    Create {
        /// Project ID or name (omit for global).
        #[arg(short, long)]
        project: Option<String>,
        /// Template name.
        #[arg(short, long)]
        name: String,
        /// Template description.
        #[arg(short, long)]
        description: Option<String>,
        /// Default card title.
        #[arg(long)]
        title: Option<String>,
        /// Default card description.
        #[arg(long)]
        default_description: Option<String>,
        /// Label applied to created cards (repeatable).
        #[arg(long)]
        label: Vec<String>,
        /// Checklist item for created cards (repeatable).
        #[arg(long)]
        checklist: Vec<String>,
    },
    /// Update a card template.
    Update {
        /// Project ID or name (omit to search global and all projects).
        #[arg(short, long)]
        project: Option<String>,
        /// Template ID or name.
        template_id: String,
        /// New name.
        #[arg(short, long)]
        name: Option<String>,
        /// New description.
        #[arg(short, long)]
        description: Option<String>,
        /// New default card title.
        #[arg(long)]
        title: Option<String>,
        /// New default card description.
        #[arg(long)]
        default_description: Option<String>,
        /// Replace the label set wholesale (repeatable).
        #[arg(long)]
        label: Option<Vec<String>>,
        /// Replace the checklist wholesale (repeatable).
        #[arg(long)]
        checklist: Option<Vec<String>>,
    },
    /// Delete a card template.
    Delete {
        /// Project ID or name (omit to search global and all projects).
        #[arg(short, long)]
        project: Option<String>,
        /// Template ID or name.
        template_id: String,
    },
}

/// Serializable checklist item for output.
#[derive(Serialize)]
struct TemplateChecklistItemOut {
    title: String,
}

impl From<v1::TemplateChecklistItem> for TemplateChecklistItemOut {
    fn from(i: v1::TemplateChecklistItem) -> Self {
        Self { title: i.title }
    }
}

/// Serializable card template for output.
#[derive(Serialize)]
struct CardTemplateOut {
    id: String,
    project_id: String,
    name: String,
    description: String,
    title: String,
    default_description: String,
    label_names: Vec<String>,
    checklist_items: Vec<TemplateChecklistItemOut>,
    is_global: bool,
    created_at: String,
    updated_at: String,
}

impl From<v1::CardTemplate> for CardTemplateOut {
    fn from(t: v1::CardTemplate) -> Self {
        Self {
            id: t.id,
            project_id: t.project_id,
            name: t.name,
            description: t.description,
            title: t.title,
            default_description: t.default_description,
            label_names: t.label_names,
            checklist_items: t.checklist_items.into_iter().map(Into::into).collect(),
            is_global: t.is_global,
            created_at: fmt_ts(&t.created_at),
            updated_at: fmt_ts(&t.updated_at),
        }
    }
}

/// Run a card template command.
pub(crate) async fn run(
    cmd: CardTemplateAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        CardTemplateAction::List { project } => {
            let resp = client
                .templates()
                .list_card_templates(v1::ListCardTemplatesRequest {
                    project_id: project.unwrap_or_default(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            let templates: Vec<CardTemplateOut> =
                resp.templates.into_iter().map(Into::into).collect();
            render_list(
                &templates,
                &[
                    "NAME",
                    "TITLE",
                    "GLOBAL",
                    "LABELS",
                    "CHECKLIST",
                    "PROJECT",
                    "ID",
                ],
                |t| {
                    vec![
                        t.name.clone(),
                        t.title.clone(),
                        t.is_global.to_string(),
                        t.label_names.len().to_string(),
                        t.checklist_items.len().to_string(),
                        t.project_id.clone(),
                        t.id.clone(),
                    ]
                },
                format,
            )
        }
        CardTemplateAction::Get {
            project,
            template_id,
        } => {
            let template_id = super::resolve::NameResolver::new(client)
                .card_template(project.as_deref(), &template_id)
                .await?;
            let resp = client
                .templates()
                .get_card_template(v1::GetCardTemplateRequest {
                    template_id,
                    ..Default::default()
                })
                .await?
                .into_owned();
            render(
                &CardTemplateOut::from(required(resp.template, "card template")?),
                format,
            )
        }
        CardTemplateAction::Create {
            project,
            name,
            description,
            title,
            default_description,
            label,
            checklist,
        } => {
            let object_id = project.clone().unwrap_or_else(|| "global".to_string());
            let resp = client
                .templates()
                .create_card_template_with_options(
                    v1::CreateCardTemplateRequest {
                        project_id: project.unwrap_or_default(),
                        name,
                        description: description.unwrap_or_default(),
                        title: title.unwrap_or_default(),
                        default_description: default_description.unwrap_or_default(),
                        label_names: label,
                        checklist_items: checklist
                            .into_iter()
                            .map(|title| v1::TemplateChecklistItem {
                                title,
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    },
                    mutating_options(&object_id),
                )
                .await?
                .into_owned();
            render(
                &CardTemplateOut::from(required(resp.template, "card template")?),
                format,
            )
        }
        CardTemplateAction::Update {
            project,
            template_id,
            name,
            description,
            title,
            default_description,
            label,
            checklist,
        } => {
            let template_id = super::resolve::NameResolver::new(client)
                .card_template(project.as_deref(), &template_id)
                .await?;
            let mut paths = Vec::new();
            if name.is_some() {
                paths.push("name".to_string());
            }
            if description.is_some() {
                paths.push("description".to_string());
            }
            if title.is_some() {
                paths.push("title".to_string());
            }
            if default_description.is_some() {
                paths.push("default_description".to_string());
            }
            if label.is_some() {
                paths.push("label_names".to_string());
            }
            if checklist.is_some() {
                paths.push("checklist_items".to_string());
            }
            let resp = client
                .templates()
                .update_card_template_with_options(
                    v1::UpdateCardTemplateRequest {
                        template_id: template_id.clone(),
                        update_mask: Some(buffa_types::google::protobuf::FieldMask {
                            paths,
                            ..Default::default()
                        })
                        .into(),
                        name: name.unwrap_or_default(),
                        description: description.unwrap_or_default(),
                        title: title.unwrap_or_default(),
                        default_description: default_description.unwrap_or_default(),
                        label_names: label.unwrap_or_default(),
                        checklist_items: checklist
                            .unwrap_or_default()
                            .into_iter()
                            .map(|title| v1::TemplateChecklistItem {
                                title,
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    },
                    mutating_options(&template_id),
                )
                .await?
                .into_owned();
            render(
                &CardTemplateOut::from(required(resp.template, "card template")?),
                format,
            )
        }
        CardTemplateAction::Delete {
            project,
            template_id,
        } => {
            let template_id = super::resolve::NameResolver::new(client)
                .card_template(project.as_deref(), &template_id)
                .await?;
            client
                .templates()
                .delete_card_template_with_options(
                    v1::DeleteCardTemplateRequest {
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

    fn card_template(id: &str, name: &str) -> v1::CardTemplate {
        v1::CardTemplate {
            id: id.to_string(),
            project_id: "proj_1".into(),
            name: name.to_string(),
            description: "Bug card template".into(),
            title: "[BUG] ".into(),
            default_description: "Describe the bug".into(),
            label_names: vec!["bug".into()],
            checklist_items: vec![v1::TemplateChecklistItem {
                title: "Reproduce".into(),
                ..Default::default()
            }],
            is_global: false,
            created_at: buffa_types::google::protobuf::Timestamp {
                seconds: 1_700_000_000,
                nanos: 0,
                ..Default::default()
            }
            .into(),
            updated_at: buffa_types::google::protobuf::Timestamp {
                seconds: 1_700_000_000,
                nanos: 0,
                ..Default::default()
            }
            .into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn list_card_templates_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(
                    "/sunbeam.kanban.v1.TemplatesService/ListCardTemplates",
                ))
                .respond_with(testutil::proto_response(&v1::ListCardTemplatesResponse {
                    templates: vec![card_template("ctmpl_1", "Bug")],
                    ..Default::default()
                }))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(
                CardTemplateAction::List {
                    project: Some("proj_1".into()),
                },
                format,
                &client,
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn list_global_card_templates() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.TemplatesService/ListCardTemplates",
            ))
            .respond_with(testutil::proto_response(&v1::ListCardTemplatesResponse {
                templates: vec![card_template("ctmpl_1", "Bug")],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardTemplateAction::List { project: None },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn get_card_template_resolves_name() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.TemplatesService/ListCardTemplates",
            ))
            .respond_with(testutil::proto_response(&v1::ListCardTemplatesResponse {
                templates: vec![card_template("ctmpl_1", "Bug")],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/ListProjects"))
            .respond_with(testutil::proto_response(&v1::ListProjectsResponse {
                projects: vec![v1::Project {
                    id: "proj_1".into(),
                    name: "One".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.TemplatesService/GetCardTemplate"))
            .respond_with(testutil::proto_response(&v1::GetCardTemplateResponse {
                template: card_template("ctmpl_1", "Bug").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardTemplateAction::Get {
                project: None,
                template_id: "bug".into(),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn get_card_template_resolves_name_scoped_to_project() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.TemplatesService/ListCardTemplates",
            ))
            .respond_with(testutil::proto_response(&v1::ListCardTemplatesResponse {
                templates: vec![card_template("ctmpl_1", "Bug")],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.TemplatesService/GetCardTemplate"))
            .respond_with(testutil::proto_response(&v1::GetCardTemplateResponse {
                template: card_template("ctmpl_1", "Bug").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardTemplateAction::Get {
                project: Some("proj_1".into()),
                template_id: "bug".into(),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_update_delete_card_template() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.TemplatesService/CreateCardTemplate",
            ))
            .respond_with(testutil::proto_response(&v1::CreateCardTemplateResponse {
                template: card_template("ctmpl_new", "New").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.TemplatesService/UpdateCardTemplate",
            ))
            .respond_with(testutil::proto_response(&v1::UpdateCardTemplateResponse {
                template: card_template("ctmpl_1", "Renamed").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.TemplatesService/DeleteCardTemplate",
            ))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardTemplateAction::Create {
                project: Some("proj_1".into()),
                name: "New".into(),
                description: Some("Bug card template".into()),
                title: Some("[BUG] ".into()),
                default_description: Some("Describe the bug".into()),
                label: vec!["bug".into()],
                checklist: vec!["Reproduce".into()],
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            CardTemplateAction::Create {
                project: None,
                name: "Global".into(),
                description: None,
                title: None,
                default_description: None,
                label: Vec::new(),
                checklist: Vec::new(),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            CardTemplateAction::Update {
                project: None,
                template_id: "ctmpl_1".into(),
                name: Some("Renamed".into()),
                description: Some("Updated".into()),
                title: Some("[ISSUE] ".into()),
                default_description: None,
                label: Some(vec!["bug".into(), "triage".into()]),
                checklist: Some(vec!["Reproduce".into(), "Bisect".into()]),
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
        run(
            CardTemplateAction::Delete {
                project: None,
                template_id: "ctmpl_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn update_card_template_without_name_sends_empty_mask() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.TemplatesService/UpdateCardTemplate",
            ))
            .respond_with(testutil::proto_response(&v1::UpdateCardTemplateResponse {
                template: card_template("ctmpl_1", "Bug").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardTemplateAction::Update {
                project: None,
                template_id: "ctmpl_1".into(),
                name: None,
                description: None,
                title: None,
                default_description: None,
                label: None,
                checklist: None,
            },
            OutputFormat::Json,
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
                "/sunbeam.kanban.v1.TemplatesService/ListCardTemplates",
            ))
            .respond_with(testutil::connect_error(500, "internal", "db down"))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            CardTemplateAction::List { project: None },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("db down"));
    }
}
