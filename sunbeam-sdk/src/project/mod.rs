//! `sunbeam project` — per-project `sunbeam.yaml` parsing, validation, and
//! target execution.
//!
//! This module is organized into submodules so that parallel work can
//! proceed on disjoint files:
//!
//! - [`config`] — YAML schema types, parsing, validation.
//! - [`runner`] (added later) — execute verbs (exec + workflow dispatch).

/// Config.
pub mod config;
/// Runner.
pub mod runner;

pub use config::{
    Deps, ExecCommand, ExecTarget, ProjectConfig, ProjectMeta, SCHEMA_VERSION, STANDARD_VERBS,
    SkipMarker, Target, Tenant, WorkflowTarget,
};
