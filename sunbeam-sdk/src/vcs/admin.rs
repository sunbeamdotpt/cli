//! `sunbeam vcs admin …` — org administration surface.
//!
//! M-1 verb surface: create org, add/remove member, set/list relations.
//! Admin RPCs require the caller to hold a system-admin or org-admin tuple
//! in Keto; the server enforces this — the CLI is a thin marshalling layer.

use buffa::MessageField;
use crate::error::Result;
use crate::output::{OutputFormat, render, render_list};
use crate::vcs::client::{connect_admin_client, map_status, resolve_token};
use gitserv_proto::pb::{
    AddMemberRequest, AddSshCaRequest, CreateOrgRequest, ListRelationsRequest,
    ListSshCasRequest, OrgId, RemoveMemberRequest, RevokeSshCaRequest, SetRelationRequest,
};
use serde::Serialize;

#[derive(Debug, clap::Args)]
pub struct AdminArgs {
    #[command(subcommand)]
    pub command: AdminCmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum AdminCmd {
    /// Org lifecycle (create) and membership management.
    Org(OrgArgs),
    /// SSH CA trust-anchor management (system-admin only).
    SshCa(SshCaArgs),
}

#[derive(Debug, clap::Args)]
pub struct SshCaArgs {
    #[command(subcommand)]
    pub command: SshCaCmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum SshCaCmd {
    /// Add (or update comment on) a CA trust anchor. Public-key path is read
    /// as a single OpenSSH-formatted line ("ssh-ed25519 AAAA... [comment]").
    Add {
        /// Path to a `.pub` file (or `-` to read from stdin).
        public_key_path: String,
        /// Operator-supplied audit comment.
        #[arg(long, default_value = "")]
        comment: String,
    },
    /// Soft-revoke a CA by row id. Existing user certs signed by this CA stop
    /// authenticating immediately on the next handler refresh.
    Revoke {
        /// `id` field from `list`.
        id: i64,
    },
    /// List trusted CAs. Defaults to active only; pass `--all` to include
    /// revoked rows for audit history.
    List {
        #[arg(long)]
        all: bool,
    },
}

#[derive(Debug, clap::Args)]
pub struct OrgArgs {
    #[command(subcommand)]
    pub command: OrgCmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum OrgCmd {
    /// Create a new org (requires system-admin tuple in Keto).
    Create {
        /// Org slug (lowercase, hyphens OK, max 64 chars).
        slug: String,
    },
    /// Add a member to an org with a role.
    AddMember {
        /// Org ULID.
        org_id: String,
        /// Subject identifier (e.g. `user:<ulid>`).
        user: String,
        /// Role: owner | admin | writer | reader.
        #[arg(long, default_value = "reader")]
        role: String,
    },
    /// Remove a member from an org.
    RemoveMember {
        /// Org ULID.
        org_id: String,
        /// Subject identifier.
        user: String,
    },
    /// Write a raw Keto relation tuple on an object you admin.
    SetRelation {
        /// Object string (e.g. `org:<ulid>` or `repo:<ulid>`).
        object: String,
        /// Relation name (e.g. `admin`, `reader`).
        relation: String,
        /// Subject identifier.
        subject: String,
    },
    /// List Keto relation tuples for an object you admin.
    ListRelations {
        /// Object string (e.g. `org:<ulid>`).
        object: String,
    },
}

#[derive(Serialize)]
struct OrgRow {
    id: String,
    slug: String,
}

#[derive(Serialize)]
struct RelationRow {
    object: String,
    relation: String,
    subject: String,
}

pub async fn run(args: AdminArgs, endpoint: &str, format: OutputFormat) -> Result<()> {
    let domain = crate::config::domain().to_string();
    let token = resolve_token(&domain).await?;
    let mut client = connect_admin_client(endpoint, &token).await?;

    match args.command {
        AdminCmd::Org(OrgArgs { command }) => match command {
            OrgCmd::Create { slug } => {
                let resp = client
                    .create_org(CreateOrgRequest { slug: slug.clone(), ..Default::default() })
                    .await
                    .map_err(map_status)?
                    .into_owned();
                render(
                    &OrgRow { id: resp.ulid, slug },
                    format,
                )
            }

            OrgCmd::AddMember { org_id, user, role } => {
                client
                    .add_member(AddMemberRequest {
                        org: MessageField::some(OrgId { ulid: org_id, ..Default::default() }),
                        user,
                        role,
                        ..Default::default()
                    })
                    .await
                    .map_err(map_status)?;
                Ok(())
            }

            OrgCmd::RemoveMember { org_id, user } => {
                client
                    .remove_member(RemoveMemberRequest {
                        org: MessageField::some(OrgId { ulid: org_id, ..Default::default() }),
                        user,
                        ..Default::default()
                    })
                    .await
                    .map_err(map_status)?;
                Ok(())
            }

            OrgCmd::SetRelation { object, relation, subject } => {
                client
                    .set_relation(SetRelationRequest {
                        object,
                        relation,
                        subject,
                        ..Default::default()
                    })
                    .await
                    .map_err(map_status)?;
                Ok(())
            }

            OrgCmd::ListRelations { object } => {
                let resp = client
                    .list_relations(ListRelationsRequest { object, ..Default::default() })
                    .await
                    .map_err(map_status)?
                    .into_owned();
                let rows: Vec<RelationRow> = resp
                    .relations
                    .into_iter()
                    .map(|r| RelationRow {
                        object: r.object,
                        relation: r.relation,
                        subject: r.subject,
                    })
                    .collect();
                render_list(
                    &rows,
                    &["OBJECT", "RELATION", "SUBJECT"],
                    |r| vec![r.object.clone(), r.relation.clone(), r.subject.clone()],
                    format,
                )
            }
        },
        AdminCmd::SshCa(SshCaArgs { command }) => match command {
            SshCaCmd::Add { public_key_path, comment } => {
                let public_key = if public_key_path == "-" {
                    use std::io::Read as _;
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_to_string(&mut buf)
                        .map_err(|e| crate::error::SunbeamError::Other(format!("stdin: {e}")))?;
                    buf.trim().to_owned()
                } else {
                    std::fs::read_to_string(&public_key_path)
                        .map_err(|e| crate::error::SunbeamError::Other(
                            format!("read {public_key_path}: {e}"),
                        ))?
                        .trim()
                        .to_owned()
                };
                let resp = client
                    .add_ssh_ca(AddSshCaRequest { public_key, comment, ..Default::default() })
                    .await
                    .map_err(map_status)?
                    .into_owned();
                render(&SshCaRow::from_pb(resp), format)
            }
            SshCaCmd::Revoke { id } => {
                client
                    .revoke_ssh_ca(RevokeSshCaRequest { id, ..Default::default() })
                    .await
                    .map_err(map_status)?;
                Ok(())
            }
            SshCaCmd::List { all } => {
                let resp = client
                    .list_ssh_cas(ListSshCasRequest { include_revoked: all, ..Default::default() })
                    .await
                    .map_err(map_status)?
                    .into_owned();
                let rows: Vec<SshCaRow> =
                    resp.cas.into_iter().map(SshCaRow::from_pb).collect();
                render_list(
                    &rows,
                    &["ID", "FINGERPRINT", "COMMENT", "REVOKED"],
                    |r| {
                        vec![
                            r.id.to_string(),
                            r.fingerprint.clone(),
                            r.comment.clone(),
                            if r.revoked_at == 0 { "no".into() } else { "yes".into() },
                        ]
                    },
                    format,
                )
            }
        },
    }
}

#[derive(Debug, Serialize)]
struct SshCaRow {
    id: i64,
    public_key: String,
    fingerprint: String,
    comment: String,
    created_at: i64,
    revoked_at: i64,
}

impl SshCaRow {
    fn from_pb(c: gitserv_proto::pb::SshCa) -> Self {
        Self {
            id: c.id,
            public_key: c.public_key,
            fingerprint: c.fingerprint,
            comment: c.comment,
            created_at: c.created_at,
            revoked_at: c.revoked_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_row_serializes_to_json() {
        let row = OrgRow {
            id: "01K000000000000000000000A".into(),
            slug: "acme".into(),
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("\"slug\":\"acme\""), "{json}");
        assert!(json.contains("\"id\":"), "{json}");
    }

    #[test]
    fn relation_row_serializes_to_json() {
        let row = RelationRow {
            object: "org:01K000000000000000000000A".into(),
            relation: "admin".into(),
            subject: "user:01K000000000000000000000B".into(),
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("\"relation\":\"admin\""), "{json}");
    }
}
