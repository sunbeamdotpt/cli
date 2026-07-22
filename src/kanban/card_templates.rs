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
#[derive(Debug, Subcommand)]
pub enum CardTemplateAction {
    /// List card templates.
    List {
        /// Project ID or name (omit for global templates).
        #[arg(short, long)]
        project: Option<String>,
    },
    /// Get a card template.
    Get {
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
    },
    /// Update a card template.
    Update {
        /// Template ID or name.
        template_id: String,
        /// New name.
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Delete a card template.
    Delete {
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
        CardTemplateAction::Get { template_id } => {
            let template_id = super::resolve::NameResolver::new(client)
                .card_template(&template_id)
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
        CardTemplateAction::Create { project, name } => {
            let object_id = project.clone().unwrap_or_else(|| "global".to_string());
            let resp = client
                .templates()
                .create_card_template_with_options(
                    v1::CreateCardTemplateRequest {
                        project_id: project.unwrap_or_default(),
                        name,
                        description: String::new(),
                        title: String::new(),
                        default_description: String::new(),
                        label_names: Vec::new(),
                        checklist_items: Vec::new(),
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
        CardTemplateAction::Update { template_id, name } => {
            let template_id = super::resolve::NameResolver::new(client)
                .card_template(&template_id)
                .await?;
            let mut paths = Vec::new();
            if name.is_some() {
                paths.push("name".to_string());
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
                        description: String::new(),
                        title: String::new(),
                        default_description: String::new(),
                        label_names: Vec::new(),
                        checklist_items: Vec::new(),
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
        CardTemplateAction::Delete { template_id } => {
            let template_id = super::resolve::NameResolver::new(client)
                .card_template(&template_id)
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
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            CardTemplateAction::Update {
                template_id: "ctmpl_1".into(),
                name: Some("Renamed".into()),
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
        run(
            CardTemplateAction::Delete {
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
                template_id: "ctmpl_1".into(),
                name: None,
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
