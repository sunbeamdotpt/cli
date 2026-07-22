//! `sunbeam.yaml` types and parsing.
//!
//! Each project directory contains a `sunbeam.yaml` manifest describing
//! its name, dependencies on other workspace projects, the commands
//! (or workflows) that implement the standard build verbs, and
//! optional tenant/output metadata.
//!
//! The schema is versioned. Schema `1` is the initial release; unknown
//! schema versions fail parsing rather than silently degrading.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use sdk::error::{Result, ResultExt, SunbeamError};

/// Schema version supported by this CLI.
pub const SCHEMA_VERSION: u32 = 1;

/// The nine standard build verbs. Every project defines some subset of these
/// in its `targets` map; unlisted verbs default to [`Target::Skip`].
///
/// These match `sunbeam project <verb>` subcommands. Custom verbs (e.g. `seed`,
/// `coverage`) are also allowed in `targets:` and run via `sunbeam project run <verb>`.
pub const STANDARD_VERBS: &[&str] = &[
    "build", "test", "lint", "fmt", "package", "deploy", "dev", "clean", "doc",
];

/// True if `verb` matches one of the nine [`STANDARD_VERBS`].
pub fn is_standard_verb(verb: &str) -> bool {
    STANDARD_VERBS.contains(&verb)
}

/// Top-level `sunbeam.yaml` document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectConfig {
    /// Schema version. Must match [`SCHEMA_VERSION`].
    pub schema: u32,

    /// Project identity and metadata.
    pub project: ProjectMeta,

    /// Build verbs → target definitions. Missing verbs default to `Skip`.
    #[serde(default)]
    pub targets: BTreeMap<String, Target>,

    /// Cross-project dependency info.
    #[serde(default)]
    pub deps: Deps,

    /// Tenant / namespace / deployment metadata. Optional.
    #[serde(default)]
    pub tenant: Option<Tenant>,

    /// Build output paths (files/dirs) for caching and packaging. Optional.
    #[serde(default)]
    pub outputs: Vec<String>,
}

/// Project identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectMeta {
    /// Project name. Must be unique within the workspace.
    pub name: String,

    /// Free-form kind hint (`rust-lib`, `rust-bin`, `node-ui`, `kustomize`, …).
    /// Not interpreted by the CLI, but useful for filtering and docs.
    #[serde(default)]
    pub kind: Option<String>,

    /// Optional human description.
    #[serde(default)]
    pub description: Option<String>,
}

/// A build target: what happens when you run `sunbeam project <verb>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Target {
    /// Explicit skip (`<verb>: skip`). Behaves the same as omitting the verb,
    /// but is useful for documenting intent ("this project has no tests").
    Skip(SkipMarker),

    /// Run a workflow via the embedded `wfe` engine.
    Workflow(WorkflowTarget),

    /// Run a shell command or exec array.
    Exec(ExecTarget),
}

/// The string `"skip"` used as a target value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SkipMarker {
    /// Skip.
    Skip,
}

/// Shell exec target. Accepts either a single command string or an argv array.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecTarget {
    /// The command to run. Single string → `sh -c`. Array → exec-style argv.
    pub exec: ExecCommand,

    /// Working directory, relative to the project root. Defaults to project root.
    #[serde(default)]
    pub cwd: Option<String>,

    /// Extra environment variables to set for this command.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Command payload for an `Exec` target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ExecCommand {
    /// `cargo build` — runs through `sh -c`, so shell features (pipes, globs) work.
    Shell(String),

    /// `[cargo, build]` — argv form, runs the binary directly without a shell.
    Argv(Vec<String>),
}

/// Workflow target: defers to the embedded `wfe` engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowTarget {
    /// Path to a `wfe` YAML workflow, relative to the project root.
    pub workflow: String,

    /// Inputs passed to the workflow as `wfe` inputs.
    #[serde(default)]
    pub inputs: BTreeMap<String, serde_yaml::Value>,
}

/// Dependency info.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Deps {
    /// Workspace projects this one depends on. Used to compute build order.
    #[serde(default)]
    pub projects: Vec<String>,

    /// Shared services this project needs running for `dev`/`test`
    /// (keys from the workspace `services:` map).
    #[serde(default)]
    pub services: Vec<String>,
}

/// Tenant / deployment metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tenant {
    /// Kubernetes namespace this project deploys into (prod).
    #[serde(default)]
    pub namespace: Option<String>,

    /// Public-facing host (e.g., `sol.sunbeam.pt`).
    #[serde(default)]
    pub host: Option<String>,

    /// Arbitrary extra fields — kept as-is so future schema additions don't break parsing.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

impl ProjectConfig {
    /// Load and validate a `sunbeam.yaml` from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_ctx(|| format!("reading {}", path.display()))?;
        Self::from_yaml(&text).with_ctx(|| format!("parsing {}", path.display()))
    }

    /// Parse and validate from a YAML string.
    pub fn from_yaml(text: &str) -> Result<Self> {
        let cfg: ProjectConfig = serde_yaml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Lookup a verb, returning `Target::Skip` for unknown/missing entries.
    pub fn target(&self, verb: &str) -> Target {
        self.targets
            .get(verb)
            .cloned()
            .unwrap_or(Target::Skip(SkipMarker::Skip))
    }

    /// Returns true if `verb` has an explicit non-skip entry.
    pub fn has_target(&self, verb: &str) -> bool {
        matches!(
            self.targets.get(verb),
            Some(Target::Exec(_)) | Some(Target::Workflow(_))
        )
    }

    fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA_VERSION {
            return Err(SunbeamError::Config(format!(
                "unsupported sunbeam.yaml schema {}; this CLI supports schema {SCHEMA_VERSION}",
                self.schema
            )));
        }
        if self.project.name.is_empty() {
            return Err(SunbeamError::Config(
                "project.name must not be empty".into(),
            ));
        }
        for verb in self.targets.keys() {
            if verb.is_empty() {
                return Err(SunbeamError::Config(
                    "target verb name must not be empty".into(),
                ));
            }
            if !verb
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(SunbeamError::Config(format!(
                    "invalid target verb {verb:?}; allowed: ASCII letters, digits, '-', '_'"
                )));
            }
        }
        for (verb, target) in &self.targets {
            if let Target::Exec(ExecTarget { exec, .. }) = target
                && exec_is_empty(exec)
            {
                return Err(SunbeamError::Config(format!(
                    "target {verb:?} has an empty exec command"
                )));
            }
            if let Target::Workflow(WorkflowTarget { workflow, .. }) = target
                && workflow.is_empty()
            {
                return Err(SunbeamError::Config(format!(
                    "target {verb:?} has an empty workflow path"
                )));
            }
        }
        Ok(())
    }
}

fn exec_is_empty(exec: &ExecCommand) -> bool {
    match exec {
        ExecCommand::Shell(s) => s.trim().is_empty(),
        ExecCommand::Argv(v) => v.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_project() {
        let yaml = r"
schema: 1
project:
  name: wfe
";
        let cfg = ProjectConfig::from_yaml(yaml).unwrap();
        assert_eq!(cfg.project.name, "wfe");
        assert!(cfg.targets.is_empty());
        assert!(cfg.deps.projects.is_empty());
    }

    #[test]
    fn parses_full_project() {
        let yaml = r#"
schema: 1
project:
  name: sol
  kind: rust-bin
  description: "Terminal agent."
targets:
  build:
    exec: cargo build --release
  test:
    exec: [cargo, nextest, run]
    env:
      RUST_LOG: debug
  lint:
    exec: cargo clippy -- -D warnings
  package:
    workflow: .sunbeam/workflows/package.yaml
    inputs:
      tag: latest
  deploy: skip
deps:
  projects: [wfe, cli]
  services: [postgres, valkey]
tenant:
  namespace: sunbeam-sol
  host: sol.sunbeam.pt
outputs:
  - target/release/sol
"#;
        let cfg = ProjectConfig::from_yaml(yaml).unwrap();
        assert_eq!(cfg.project.name, "sol");
        assert_eq!(cfg.project.kind.as_deref(), Some("rust-bin"));
        assert_eq!(cfg.deps.projects, vec!["wfe", "cli"]);
        assert_eq!(cfg.deps.services, vec!["postgres", "valkey"]);
        assert!(cfg.has_target("build"));
        assert!(cfg.has_target("test"));
        assert!(!cfg.has_target("deploy"));
        assert!(!cfg.has_target("dev"));
        assert_eq!(cfg.outputs, vec!["target/release/sol"]);

        match cfg.target("build") {
            Target::Exec(ExecTarget {
                exec: ExecCommand::Shell(s),
                ..
            }) => {
                assert_eq!(s, "cargo build --release");
            }
            other => panic!("expected shell exec, got {other:?}"),
        }
        match cfg.target("test") {
            Target::Exec(ExecTarget {
                exec: ExecCommand::Argv(v),
                env,
                ..
            }) => {
                assert_eq!(v, vec!["cargo", "nextest", "run"]);
                assert_eq!(env.get("RUST_LOG").map(String::as_str), Some("debug"));
            }
            other => panic!("expected argv exec, got {other:?}"),
        }
        match cfg.target("package") {
            Target::Workflow(WorkflowTarget { workflow, inputs }) => {
                assert_eq!(workflow, ".sunbeam/workflows/package.yaml");
                assert_eq!(inputs.get("tag").and_then(|v| v.as_str()), Some("latest"));
            }
            other => panic!("expected workflow, got {other:?}"),
        }
        assert!(matches!(cfg.target("deploy"), Target::Skip(_)));
    }

    #[test]
    fn missing_verbs_default_to_skip() {
        let yaml = r"
schema: 1
project:
  name: x
";
        let cfg = ProjectConfig::from_yaml(yaml).unwrap();
        for v in STANDARD_VERBS {
            assert!(matches!(cfg.target(v), Target::Skip(_)));
            assert!(!cfg.has_target(v));
        }
    }

    #[test]
    fn unknown_schema_rejected() {
        let yaml = r"
schema: 2
project:
  name: x
";
        let err = ProjectConfig::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("schema"));
    }

    #[test]
    fn custom_verb_accepted() {
        let yaml = r"
schema: 1
project:
  name: x
targets:
  seed:
    exec: ./bootstrap.sh
  coverage:
    exec: [cargo, llvm-cov]
";
        let cfg = ProjectConfig::from_yaml(yaml).unwrap();
        assert!(cfg.has_target("seed"));
        assert!(cfg.has_target("coverage"));
        assert!(!is_standard_verb("seed"));
        assert!(is_standard_verb("build"));
    }

    #[test]
    fn invalid_verb_name_rejected() {
        let yaml = r"
schema: 1
project:
  name: x
targets:
  bad verb:
    exec: do-it
";
        let err = ProjectConfig::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("invalid target verb"));
    }

    #[test]
    fn empty_exec_rejected() {
        let yaml = r#"
schema: 1
project:
  name: x
targets:
  build:
    exec: ""
"#;
        let err = ProjectConfig::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("empty exec"));
    }

    #[test]
    fn empty_name_rejected() {
        let yaml = r#"
schema: 1
project:
  name: ""
"#;
        let err = ProjectConfig::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn tenant_extra_fields_preserved() {
        let yaml = r"
schema: 1
project:
  name: x
tenant:
  namespace: foo
  replicas: 3
  image: ghcr.io/x/y
";
        let cfg = ProjectConfig::from_yaml(yaml).unwrap();
        let t = cfg.tenant.unwrap();
        assert_eq!(t.namespace.as_deref(), Some("foo"));
        assert_eq!(t.extra.get("replicas").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(
            t.extra.get("image").and_then(|v| v.as_str()),
            Some("ghcr.io/x/y")
        );
    }
}
