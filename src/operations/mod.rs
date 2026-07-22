//! Workspace configuration — `sunbeam.workspace.yaml` schema, parsing, validation.
//!
//! Only the config schema is kept here: the `compose` and `stack` command
//! layers were removed with the `operations` CLI verb. Workflow steps use
//! these types to locate and order workspace projects.

/// Config.
pub mod config;
