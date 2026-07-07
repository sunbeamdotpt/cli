#![warn(missing_docs)]
#![deny(unused_mut)]
#![deny(clippy::missing_safety_doc)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![cfg_attr(not(test), deny(clippy::expect_used))]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
// just keeps syntax consistent
#![deny(clippy::needless_borrow)]
//! sunbeam-sdk — SDK for Sunbeam CLI operations, workspace management,
//! Kubernetes manifests, VPN, and workflow orchestration.
#[macro_use]
/// Error types and result aliases for sunbeam-sdk.
pub mod error;

/// OAuth2 / SSO authentication commands.
pub mod auth;
/// Health check runners for cluster services.
pub mod checks;
/// Cluster topology and node discovery.
pub mod cluster;
/// Context-based configuration file I/O.
pub mod config;
/// Shared constants (paths, ports, timeouts).
pub mod constants;
/// kubectl describe wrappers.
pub mod describe;
/// Service discovery via cluster annotations.
pub mod discovery;
/// Connectivity and sanity diagnostics.
pub mod doctor;
/// Cluster tear-down commands.
pub mod down;
/// Pod exec and interactive shell helpers.
pub mod exec;
/// Kanban project management via gRPC.
pub mod kanban;
/// Kubernetes client setup and manifest operations.
pub mod kube;
/// Structured logger with inherited fields.
pub mod logger;
/// Unified tracing-based logging subsystem.
pub mod logging;
/// Runtime manifest parameter discovery and override application.
pub mod manifest_params;
/// Kustomize build, apply, and namespace filtering.
pub mod manifests;
/// OpenBao (HashiCorp Vault fork) API client.
pub mod openbao;
/// Workspace-level operations (compose, stack).
pub mod operations;
/// CLI output helpers (tables, JSON, YAML, step banners).
pub mod output;
/// Manifest-anchored profile override system.
pub mod profiles;

/// Kubernetes port-forward utilities.
pub mod port_forward;
/// Per-project build, test, and deployment commands.
pub mod project;
/// Local proxy and port-forward helpers.
pub mod proxy;
/// Infrastructure manifest registry and namespace discovery.
pub mod registry;
/// OpenBao secret reading and seeding.
pub mod secrets;
/// Service listing and status queries.
pub mod services;
/// Embedded binary extraction (kustomize, helm).
pub mod tools;
/// Topological sort for workspace project graphs.
pub mod topo;
/// Self-update from Gitea CI artifacts.
pub mod update;
/// Identity management (Kratos user CRUD).
pub mod users;
/// Vault transit keystore operations.
pub mod vault_keystore;
/// Version control commands (git).
pub mod vcs;
/// VPN connect/disconnect/status commands.
pub mod vpn_cmds;
/// VPN daemon socket and environment detection.
pub mod vpn_env;
/// Workflow engine remote control (list, run, logs, etc.).
pub mod wfectl;
/// Local workflow definitions and step primitives.
pub mod workflows;
