//! `sunbeam project` — per-project `sunbeam.yaml` parsing, validation, and
//! target execution.
//!
//! This module is organized into submodules so that parallel work can
//! proceed on disjoint files:
//!
//! - [`config`] — YAML schema types, parsing, validation.
//! - [`runner`] (added later) — execute verbs (exec + workflow dispatch).

pub mod cli;
pub mod config;
pub mod runner;

pub use config::{
    Deps, ExecCommand, ExecTarget, ProjectConfig, ProjectMeta, SkipMarker, Target, Tenant,
    WorkflowTarget, SCHEMA_VERSION, STANDARD_VERBS,
};
