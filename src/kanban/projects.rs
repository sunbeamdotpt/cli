//! Kanban project commands.

use clap::Subcommand;
use sdk::error::{Result, SunbeamError};
use sdk::kanban::KanbanClient;
use sdk::kanban::prelude::buffa_types;
use sdk::kanban::v1;
use serde::Serialize;

use super::{fmt_ts, mutating_options, new_idempotency_key, object_id_options, required};
use crate::output::{OutputFormat, render, render_list};

/// Project actions.
#[derive(Debug, Subcommand)]
pub enum ProjectAction {
    /// List projects.
    List,
    /// Get a project.
    Get {
        /// Project ID or name.
        project_id: String,
    },
    /// Create a project.
    Create {
        /// Project name.
        #[arg(short, long)]
        name: String,
        /// Short uppercase prefix.
        #[arg(short, long)]
        prefix: String,
        /// Icon identifier.
        #[arg(short, long)]
        icon: Option<String>,
        /// Color token.
        #[arg(short, long)]
        color: Option<String>,
        /// Description.
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Update a project.
    Update {
        /// Project ID or name.
        project_id: String,
        /// New name.
        #[arg(short, long)]
        name: Option<String>,
        /// New icon.
        #[arg(short, long)]
        icon: Option<String>,
        /// New color.
        #[arg(short, long)]
        color: Option<String>,
        /// New description.
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Delete a project.
    Delete {
        /// Project ID or name.
        project_id: String,
    },
    /// Member management.
    Member {
        /// Member subcommand to run.
        #[command(subcommand)]
        action: MemberAction,
    },
}

/// Project member actions.
#[derive(Debug, Subcommand)]
pub enum MemberAction {
    /// List members.
    List {
        /// Project ID or name.
        project_id: String,
    },
    /// Add a member.
    Add {
        /// Project ID or name.
        project_id: String,
        /// Member email address.
        subject: String,
        /// Relation.
        #[arg(short, long, default_value = "view")]
        relation: String,
    },
    /// Remove a member.
    Remove {
        /// Project ID or name.
        project_id: String,
        /// Member email address.
        subject: String,
    },
}

/// Serializable project for output.
#[derive(Serialize)]
struct ProjectOut {
    id: String,
    name: String,
    prefix: String,
    icon: String,
    color: String,
    description: String,
    created_at: String,
    updated_at: String,
    member_count: i32,
}

impl From<v1::Project> for ProjectOut {
    fn from(p: v1::Project) -> Self {
        Self {
            id: p.id,
            name: p.name,
            prefix: p.prefix,
            icon: p.icon,
            color: p.color,
            description: p.description,
            created_at: fmt_ts(&p.created_at),
            updated_at: fmt_ts(&p.updated_at),
            member_count: p.member_count,
        }
    }
}

/// Serializable project member for output.
#[derive(Serialize)]
struct MemberOut {
    project_id: String,
    subject: String,
    relation: String,
    display_name: String,
    email: String,
    added_at: String,
}

impl From<v1::ProjectMember> for MemberOut {
    fn from(m: v1::ProjectMember) -> Self {
        Self {
            project_id: m.project_id,
            subject: m.subject,
            relation: m.relation,
            display_name: m.display_name,
            email: m.email,
            added_at: fmt_ts(&m.added_at),
        }
    }
}

/// Resolve a member identifier to an SSO subject.
///
/// Only email addresses are accepted; they are resolved through the
/// sso-gateway IdentityService with the logged-in SSO token.
async fn resolve_member_subject(subject: &str) -> Result<String> {
    if !subject.contains('@') {
        return Err(SunbeamError::identity(
            "member identifier must be an email address",
        ));
    }
    #[cfg(test)]
    if subject.ends_with("@test") {
        let local = subject.split('@').next().unwrap_or(subject);
        return Ok(format!("user:{local}"));
    }
    crate::auth::resolve_subject_for_email(subject).await
}

/// Run a project command.
pub(crate) async fn run(
    cmd: ProjectAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        ProjectAction::List => {
            let resp = client
                .projects()
                .list_projects(v1::ListProjectsRequest::default())
                .await?
                .into_owned();
            let projects: Vec<ProjectOut> = resp.projects.into_iter().map(Into::into).collect();
            render_list(
                &projects,
                &["NAME", "PREFIX", "DESCRIPTION", "MEMBERS", "ID"],
                |p| {
                    vec![
                        p.name.clone(),
                        p.prefix.clone(),
                        p.description.clone(),
                        p.member_count.to_string(),
                        p.id.clone(),
                    ]
                },
                format,
            )
        }
        ProjectAction::Get { project_id } => {
            let project_id = super::resolve::NameResolver::new(client)
                .project(&project_id)
                .await?;
            let resp = client
                .projects()
                .get_project_with_options(
                    v1::GetProjectRequest {
                        project_id: project_id.clone(),
                        ..Default::default()
                    },
                    object_id_options(&project_id),
                )
                .await?
                .into_owned();
            render(
                &ProjectOut::from(required(resp.project, "project")?),
                format,
            )
        }
        ProjectAction::Create {
            name,
            prefix,
            icon,
            color,
            description,
        } => {
            let resp = client
                .projects()
                .create_project(v1::CreateProjectRequest {
                    name,
                    prefix,
                    icon: icon.unwrap_or_default(),
                    color: color.unwrap_or_default(),
                    description: description.unwrap_or_default(),
                    idempotency_key: new_idempotency_key(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            render(
                &ProjectOut::from(required(resp.project, "project")?),
                format,
            )
        }
        ProjectAction::Update {
            project_id,
            name,
            icon,
            color,
            description,
        } => {
            let project_id = super::resolve::NameResolver::new(client)
                .project(&project_id)
                .await?;
            let mut paths = Vec::new();
            if name.is_some() {
                paths.push("name".to_string());
            }
            if icon.is_some() {
                paths.push("icon".to_string());
            }
            if color.is_some() {
                paths.push("color".to_string());
            }
            if description.is_some() {
                paths.push("description".to_string());
            }
            let resp = client
                .projects()
                .update_project_with_options(
                    v1::UpdateProjectRequest {
                        project_id: project_id.clone(),
                        project: v1::Project {
                            id: project_id.clone(),
                            name: name.unwrap_or_default(),
                            icon: icon.unwrap_or_default(),
                            color: color.unwrap_or_default(),
                            description: description.unwrap_or_default(),
                            ..Default::default()
                        }
                        .into(),
                        update_mask: Some(buffa_types::google::protobuf::FieldMask {
                            paths,
                            ..Default::default()
                        })
                        .into(),
                        ..Default::default()
                    },
                    object_id_options(&project_id),
                )
                .await?
                .into_owned();
            render(
                &ProjectOut::from(required(resp.project, "project")?),
                format,
            )
        }
        ProjectAction::Delete { project_id } => {
            let project_id = super::resolve::NameResolver::new(client)
                .project(&project_id)
                .await?;
            client
                .projects()
                .delete_project_with_options(
                    v1::DeleteProjectRequest {
                        project_id: project_id.clone(),
                        ..Default::default()
                    },
                    object_id_options(&project_id),
                )
                .await?;
            render(
                &serde_json::json!({"deleted": true, "project_id": project_id}),
                format,
            )
        }
        ProjectAction::Member { action } => match action {
            MemberAction::List { project_id } => {
                let project_id = super::resolve::NameResolver::new(client)
                    .project(&project_id)
                    .await?;
                let resp = client
                    .projects()
                    .list_members_with_options(
                        v1::ListMembersRequest {
                            project_id: project_id.clone(),
                            ..Default::default()
                        },
                        object_id_options(&project_id),
                    )
                    .await?
                    .into_owned();
                let members: Vec<MemberOut> = resp.members.into_iter().map(Into::into).collect();
                render_list(
                    &members,
                    &["EMAIL", "RELATION", "DISPLAY NAME", "PROJECT ID"],
                    |m| {
                        vec![
                            if m.email.is_empty() {
                                m.subject.clone()
                            } else {
                                m.email.clone()
                            },
                            m.relation.clone(),
                            m.display_name.clone(),
                            m.project_id.clone(),
                        ]
                    },
                    format,
                )
            }
            MemberAction::Add {
                project_id,
                subject,
                relation,
            } => {
                let project_id = super::resolve::NameResolver::new(client)
                    .project(&project_id)
                    .await?;
                let subject = resolve_member_subject(&subject).await?;
                client
                    .projects()
                    .add_member_with_options(
                        v1::AddMemberRequest {
                            project_id: project_id.clone(),
                            subject: subject.clone(),
                            relation: relation.clone(),
                            ..Default::default()
                        },
                        mutating_options(&project_id),
                    )
                    .await?;
                render(
                    &serde_json::json!({
                        "added": true,
                        "project_id": project_id,
                        "subject": subject,
                        "relation": relation,
                    }),
                    format,
                )
            }
            MemberAction::Remove {
                project_id,
                subject,
            } => {
                let project_id = super::resolve::NameResolver::new(client)
                    .project(&project_id)
                    .await?;
                let subject = resolve_member_subject(&subject).await?;
                client
                    .projects()
                    .remove_member_with_options(
                        v1::RemoveMemberRequest {
                            project_id: project_id.clone(),
                            subject: subject.clone(),
                            ..Default::default()
                        },
                        mutating_options(&project_id),
                    )
                    .await?;
                render(
                    &serde_json::json!({
                        "removed": true,
                        "project_id": project_id,
                        "subject": subject,
                    }),
                    format,
                )
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanban::testutil;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    fn project(id: &str, name: &str) -> v1::Project {
        v1::Project {
            id: id.to_string(),
            name: name.to_string(),
            prefix: "BEAM".into(),
            description: "desc".into(),
            member_count: 3,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn list_projects_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/sunbeam.kanban.v1.ProjectService/ListProjects"))
                .respond_with(testutil::proto_response(&v1::ListProjectsResponse {
                    projects: vec![project("proj_1", "Sunbeam")],
                    ..Default::default()
                }))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(ProjectAction::List, format, &client).await.unwrap();
        }
    }

    #[tokio::test]
    async fn get_project_by_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/GetProject"))
            .respond_with(testutil::proto_response(&v1::GetProjectResponse {
                project: project("proj_1", "Sunbeam").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            ProjectAction::Get {
                project_id: "proj_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn get_project_resolves_name_first() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/ListProjects"))
            .respond_with(testutil::proto_response(&v1::ListProjectsResponse {
                projects: vec![project("proj_1", "Sunbeam")],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/GetProject"))
            .respond_with(testutil::proto_response(&v1::GetProjectResponse {
                project: project("proj_1", "Sunbeam").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            ProjectAction::Get {
                project_id: "sunbeam".into(),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_project() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/CreateProject"))
            .respond_with(testutil::proto_response(&v1::CreateProjectResponse {
                project: project("proj_new", "New").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            ProjectAction::Create {
                name: "New".into(),
                prefix: "NEW".into(),
                icon: Some("rocket".into()),
                color: None,
                description: Some("d".into()),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn update_project() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/UpdateProject"))
            .respond_with(testutil::proto_response(&v1::UpdateProjectResponse {
                project: project("proj_1", "Renamed").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            ProjectAction::Update {
                project_id: "proj_1".into(),
                name: Some("Renamed".into()),
                icon: None,
                color: Some("blue".into()),
                description: None,
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_project() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/DeleteProject"))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            ProjectAction::Delete {
                project_id: "proj_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn member_list() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/ListMembers"))
            .respond_with(testutil::proto_response(&v1::ListMembersResponse {
                members: vec![
                    v1::ProjectMember {
                        project_id: "proj_1".into(),
                        subject: "user:alice".into(),
                        relation: "owner".into(),
                        email: "alice@sunbeam.pt".into(),
                        display_name: "Alice".into(),
                        ..Default::default()
                    },
                    v1::ProjectMember {
                        project_id: "proj_1".into(),
                        subject: "user:bob".into(),
                        relation: "view".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            ProjectAction::Member {
                action: MemberAction::List {
                    project_id: "proj_1".into(),
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn member_add_and_remove() {
        let server = MockServer::start().await;
        for rpc in ["AddMember", "RemoveMember"] {
            Mock::given(method("POST"))
                .and(path(format!("/sunbeam.kanban.v1.ProjectService/{rpc}")))
                .respond_with(testutil::proto_response(
                    &buffa_types::google::protobuf::Empty::default(),
                ))
                .mount(&server)
                .await;
        }

        let client = testutil::client_for(&server.uri());
        run(
            ProjectAction::Member {
                action: MemberAction::Add {
                    project_id: "proj_1".into(),
                    subject: "alice@test".into(),
                    relation: "edit".into(),
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            ProjectAction::Member {
                action: MemberAction::Remove {
                    project_id: "proj_1".into(),
                    subject: "alice@test".into(),
                },
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn member_add_rejects_non_email() {
        let client = testutil::client_for("http://127.0.0.1:1");
        let err = run(
            ProjectAction::Member {
                action: MemberAction::Add {
                    project_id: "proj_1".into(),
                    subject: "not-an-email".into(),
                    relation: "view".into(),
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("email address"));
    }

    #[tokio::test]
    async fn rpc_error_is_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/ListProjects"))
            .respond_with(testutil::connect_error(
                401,
                "unauthenticated",
                "token expired",
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(ProjectAction::List, OutputFormat::Table, &client)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("token expired"));
    }
}
