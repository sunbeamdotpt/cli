//! Kanban milestone commands.

use clap::Subcommand;
use sdk::error::{Result, SunbeamError};
use sdk::kanban::KanbanClient;
use sdk::kanban::prelude::buffa_types;
use sdk::kanban::v1;
use serde::Serialize;

use super::{fmt_ts, mutating_options, object_id_options, required};
use crate::output::{OutputFormat, render, render_list};

/// Milestone actions.
#[derive(Debug, Clone, Subcommand)]
pub enum MilestoneAction {
    /// List a project's milestones (with completion stats).
    List {
        /// Project ID, key, or name.
        #[arg(short, long)]
        project: String,
    },
    /// Get a milestone.
    Get {
        /// Milestone ID or title (a title needs --project for resolution).
        milestone: String,
        /// Project ID, key, or name (for title resolution).
        #[arg(short, long)]
        project: Option<String>,
    },
    /// Create a milestone.
    Create {
        /// Project ID, key, or name.
        #[arg(short, long)]
        project: String,
        /// Milestone title.
        #[arg(short, long)]
        title: String,
        /// Due date (RFC 3339 or YYYY-MM-DD).
        #[arg(long)]
        due: Option<String>,
    },
    /// Update a milestone.
    Update {
        /// Milestone ID or title (a title needs --project for resolution).
        milestone: String,
        /// Project ID, key, or name (for title resolution).
        #[arg(short, long)]
        project: Option<String>,
        /// New title.
        #[arg(long)]
        title: Option<String>,
        /// New due date (RFC 3339 or YYYY-MM-DD).
        #[arg(long)]
        due: Option<String>,
    },
    /// Delete a milestone (cards referencing it are cleared).
    Delete {
        /// Milestone ID or title (a title needs --project for resolution).
        milestone: String,
        /// Project ID, key, or name (for title resolution).
        #[arg(short, long)]
        project: Option<String>,
    },
}

/// Serializable milestone for output.
#[derive(Serialize)]
struct MilestoneOut {
    id: String,
    project_id: String,
    title: String,
    due: String,
    total_cards: i32,
    completed_cards: i32,
    created_at: String,
    updated_at: String,
}

impl From<v1::Milestone> for MilestoneOut {
    fn from(m: v1::Milestone) -> Self {
        Self {
            id: m.id,
            project_id: m.project_id,
            title: m.title,
            due: fmt_ts(&m.due),
            total_cards: m.total_cards,
            completed_cards: m.completed_cards,
            created_at: fmt_ts(&m.created_at),
            updated_at: fmt_ts(&m.updated_at),
        }
    }
}

/// Parse a due date (RFC 3339 or YYYY-MM-DD) into a protobuf Timestamp.
fn parse_due(raw: &str) -> Result<buffa_types::google::protobuf::Timestamp> {
    let seconds = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        dt.timestamp()
    } else if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        date.and_hms_opt(0, 0, 0)
            .map(|dt| dt.and_utc().timestamp())
            .ok_or_else(|| SunbeamError::Other(format!("invalid due date {raw:?}")))?
    } else {
        return Err(SunbeamError::Other(format!(
            "invalid due date {raw:?} (want RFC 3339 or YYYY-MM-DD)"
        )));
    };
    Ok(buffa_types::google::protobuf::Timestamp {
        seconds,
        nanos: 0,
        ..Default::default()
    })
}

/// Resolve a milestone argument to a ULID.
///
/// ID-shaped input passes through untouched; anything else resolves by title
/// against the project's milestones, so `--project` is required for titles.
async fn resolve_milestone(
    client: &KanbanClient,
    project: Option<&str>,
    raw: &str,
) -> Result<String> {
    if super::resolve::looks_like_id(raw) {
        return Ok(raw.to_string());
    }
    let Some(project) = project else {
        return Err(SunbeamError::Other(format!(
            "required: --project <ID|name> to resolve milestone title {raw:?}"
        )));
    };
    super::resolve::NameResolver::new(client)
        .milestone(project, raw)
        .await
}

/// Run a milestone command.
pub(crate) async fn run(
    cmd: MilestoneAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        MilestoneAction::List { project } => {
            let resp = client
                .milestones()
                .list_milestones_with_options(
                    v1::ListMilestonesRequest {
                        project_id: project.clone(),
                        ..Default::default()
                    },
                    object_id_options(&project),
                )
                .await?
                .into_owned();
            let milestones: Vec<MilestoneOut> =
                resp.milestones.into_iter().map(Into::into).collect();
            render_list(
                &milestones,
                &["TITLE", "DUE", "DONE", "TOTAL", "ID"],
                |m| {
                    vec![
                        m.title.clone(),
                        m.due.clone(),
                        m.completed_cards.to_string(),
                        m.total_cards.to_string(),
                        m.id.clone(),
                    ]
                },
                format,
            )
        }
        MilestoneAction::Get { milestone, project } => {
            let milestone_id = resolve_milestone(client, project.as_deref(), &milestone).await?;
            let resp = client
                .milestones()
                .get_milestone_with_options(
                    v1::GetMilestoneRequest {
                        milestone_id: milestone_id.clone(),
                        ..Default::default()
                    },
                    object_id_options(&milestone_id),
                )
                .await?
                .into_owned();
            render(
                &MilestoneOut::from(required(resp.milestone, "milestone")?),
                format,
            )
        }
        MilestoneAction::Create {
            project,
            title,
            due,
        } => {
            let due = due.map(|d| parse_due(&d)).transpose()?;
            let resp = client
                .milestones()
                .create_milestone_with_options(
                    v1::CreateMilestoneRequest {
                        project_id: project.clone(),
                        title,
                        due: due.into(),
                        ..Default::default()
                    },
                    mutating_options(&project),
                )
                .await?
                .into_owned();
            render(
                &MilestoneOut::from(required(resp.milestone, "milestone")?),
                format,
            )
        }
        MilestoneAction::Update {
            milestone,
            project,
            title,
            due,
        } => {
            let milestone_id = resolve_milestone(client, project.as_deref(), &milestone).await?;
            let due = due.map(|d| parse_due(&d)).transpose()?;
            let mut paths = Vec::new();
            if title.is_some() {
                paths.push("title".to_string());
            }
            if due.is_some() {
                paths.push("due".to_string());
            }
            let resp = client
                .milestones()
                .update_milestone_with_options(
                    v1::UpdateMilestoneRequest {
                        milestone_id: milestone_id.clone(),
                        update_mask: Some(buffa_types::google::protobuf::FieldMask {
                            paths,
                            ..Default::default()
                        })
                        .into(),
                        title: title.unwrap_or_default(),
                        due: due.into(),
                        ..Default::default()
                    },
                    mutating_options(&milestone_id),
                )
                .await?
                .into_owned();
            render(
                &MilestoneOut::from(required(resp.milestone, "milestone")?),
                format,
            )
        }
        MilestoneAction::Delete { milestone, project } => {
            let milestone_id = resolve_milestone(client, project.as_deref(), &milestone).await?;
            client
                .milestones()
                .delete_milestone_with_options(
                    v1::DeleteMilestoneRequest {
                        milestone_id: milestone_id.clone(),
                        ..Default::default()
                    },
                    mutating_options(&milestone_id),
                )
                .await?;
            render(&serde_json::json!({ "deleted": milestone_id }), format)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanban::testutil;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    fn milestone(id: &str, title: &str) -> v1::Milestone {
        v1::Milestone {
            id: id.to_string(),
            project_id: "proj_1".into(),
            title: title.to_string(),
            total_cards: 8,
            completed_cards: 6,
            ..Default::default()
        }
    }

    #[test]
    fn parse_due_accepts_rfc3339_and_date() {
        let ts = parse_due("2026-08-01T00:00:00Z").unwrap();
        assert_eq!(ts.seconds, 1_785_542_400);
        let ts = parse_due("2026-08-01").unwrap();
        assert_eq!(ts.seconds, 1_785_542_400);
        assert!(parse_due("next friday").is_err());
    }

    #[tokio::test]
    async fn list_milestones_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/sunbeam.kanban.v1.MilestoneService/ListMilestones"))
                .respond_with(testutil::proto_response(&v1::ListMilestonesResponse {
                    milestones: vec![milestone("ms_1", "3.2")],
                    ..Default::default()
                }))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(
                MilestoneAction::List {
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
    async fn create_get_update_delete_milestone() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.MilestoneService/CreateMilestone"))
            .respond_with(testutil::proto_response(&v1::CreateMilestoneResponse {
                milestone: milestone("ms_new", "3.3").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.MilestoneService/GetMilestone"))
            .respond_with(testutil::proto_response(&v1::GetMilestoneResponse {
                milestone: milestone("ms_1", "3.2").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.MilestoneService/UpdateMilestone"))
            .respond_with(testutil::proto_response(&v1::UpdateMilestoneResponse {
                milestone: milestone("ms_1", "3.2.1").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.MilestoneService/DeleteMilestone"))
            .respond_with(testutil::proto_response(
                &v1::DeleteMilestoneResponse::default(),
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            MilestoneAction::Create {
                project: "proj_1".into(),
                title: "3.3".into(),
                due: Some("2026-09-01".into()),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            MilestoneAction::Get {
                milestone: "ms_1".into(),
                project: None,
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            MilestoneAction::Update {
                milestone: "ms_1".into(),
                project: None,
                title: Some("3.2.1".into()),
                due: Some("2026-08-15T12:00:00Z".into()),
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
        run(
            MilestoneAction::Delete {
                milestone: "ms_1".into(),
                project: None,
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn get_resolves_title_with_project() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.MilestoneService/ListMilestones"))
            .respond_with(testutil::proto_response(&v1::ListMilestonesResponse {
                milestones: vec![milestone("ms_1", "3.2")],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.MilestoneService/GetMilestone"))
            .respond_with(testutil::proto_response(&v1::GetMilestoneResponse {
                milestone: milestone("ms_1", "3.2").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            MilestoneAction::Get {
                milestone: "3.2".into(),
                project: Some("proj_1".into()),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn title_without_project_errors() {
        let client = testutil::client_for("http://127.0.0.1:1");
        let err = run(
            MilestoneAction::Delete {
                milestone: "3.2".into(),
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
            .and(path("/sunbeam.kanban.v1.MilestoneService/ListMilestones"))
            .respond_with(testutil::connect_error(500, "internal", "db down"))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            MilestoneAction::List {
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
