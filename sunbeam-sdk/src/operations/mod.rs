//! `sunbeam operations` (alias `ops`) — workspace-level commands.
//!
//! Organized into submodules so parallel work can proceed on disjoint files:
//!
//! - [`config`]   — `sunbeam.workspace.yaml` schema, parsing, validation.
//! - [`compose`]  (added later) — `ops compose up/down` (docker compose wrapper).
//! - [`stack`]    (added later) — `ops stack pin/apply` for pinned snapshots.

pub mod cli;
pub mod compose;
pub mod config;
pub mod stack;

pub use config::{
    Repo, RepoBucket, RepoEntry, Repos, Stack, WorkspaceConfig, WorkspaceMeta, SCHEMA_VERSION,
};
