//! `sunbeam.workspace.yaml` types and parsing.
//!
//! The workspace manifest enumerates repos (owned / 3p / forks / research /
//! retired), shared dev services (docker compose spec fragment), and pinned
//! stack SHAs. `sunbeam ops` commands read and mutate this file.
//!
//! The services map is kept as `serde_yaml::Value` because the CLI treats
//! it as an opaque compose fragment: it gets merged with generated bits
//! (network, cwd, healthcheck tweaks) and handed to `docker compose`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Result, ResultExt, SunbeamError};

/// Schema version for `sunbeam.workspace.yaml`.
pub const SCHEMA_VERSION: u32 = 1;

/// Top-level `sunbeam.workspace.yaml` document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceConfig {
    /// Schema.
    pub schema: u32,

    /// Workspace.
    pub workspace: WorkspaceMeta,

    /// Repos bucketed by kind. Every bucket is optional; missing buckets
    /// are treated as empty.
    #[serde(default)]
    pub repos: Repos,

    /// Docker compose-style services. Opaque YAML — we don't model the
    /// compose schema ourselves.
    #[serde(default)]
    pub services: BTreeMap<String, serde_yaml::Value>,

    /// Named docker volumes used by services.
    #[serde(default)]
    pub volumes: BTreeMap<String, serde_yaml::Value>,

    /// Pinned stack snapshots (populated by `sunbeam ops stack pin`).
    #[serde(default)]
    pub stacks: BTreeMap<String, Stack>,
}

/// Workspace identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceMeta {
    /// Name.
    pub name: String,

    /// Root path, relative to the directory containing the manifest.
    /// Usually just `.`.
    #[serde(default = "default_root")]
    pub root: String,
}

fn default_root() -> String {
    ".".into()
}

/// Repo buckets. Each bucket is a `name → Repo` map.
///
/// Buckets are kept separate (rather than one flat map with a `kind` field)
/// because the manifest is often read by humans; the structure carries
/// meaning that a single flat list would lose.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Repos {
    #[serde(default)]
    /// Owned.
    pub owned: BTreeMap<String, Repo>,

    /// 3P libraries we've forked (`3p:` in YAML).
    #[serde(rename = "3p", default)]
    pub third_party: BTreeMap<String, Repo>,

    #[serde(default)]
    /// Forks.
    pub forks: BTreeMap<String, Repo>,

    #[serde(default)]
    /// Research.
    pub research: BTreeMap<String, Repo>,

    #[serde(default)]
    /// Retired.
    pub retired: BTreeMap<String, Repo>,
}

/// Single repo entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Repo {
    /// Path relative to workspace root.
    pub path: String,

    /// `owned`, `owned-fork`, `owned-research`, or absent (defaults based on bucket).
    #[serde(default)]
    pub kind: Option<String>,

    /// Upstream slug (e.g., `tokio-rs/tokio`, `codeberg.org/forgejo/forgejo`).
    #[serde(default)]
    pub upstream: Option<String>,

    /// Upstream branch to track.
    #[serde(default)]
    pub tracking: Option<String>,

    /// Extra fields for forward-compat.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

/// Pinned stack snapshot. Populated by `sunbeam ops stack pin <name>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Stack {
    /// Human-readable description of this stack.
    #[serde(default)]
    pub description: Option<String>,

    /// Project name → pinned SHA (or version ref).
    #[serde(default)]
    pub projects: BTreeMap<String, String>,

    /// Timestamp when the stack was pinned (ISO 8601).
    #[serde(default)]
    pub pinned_at: Option<String>,

    /// Extra fields for forward-compat.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

/// A single repo entry merged with its bucket name, for flat iteration.
#[derive(Debug, Clone)]
pub struct RepoEntry<'a> {
    /// Bucket.
    pub bucket: RepoBucket,
    /// Name.
    pub name: &'a str,
    /// Repo.
    pub repo: &'a Repo,
}

/// Which top-level bucket a repo lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoBucket {
    /// Owned.
    Owned,
    /// Thirdparty.
    ThirdParty,
    /// Forks.
    Forks,
    /// Research.
    Research,
    /// Retired.
    Retired,
}

impl RepoBucket {
    pub fn as_str(self) -> &'static str {
        match self {
            RepoBucket::Owned => "owned",
            RepoBucket::ThirdParty => "3p",
            RepoBucket::Forks => "forks",
            RepoBucket::Research => "research",
            RepoBucket::Retired => "retired",
        }
    }
}

impl WorkspaceConfig {
    /// Load and validate a `sunbeam.workspace.yaml`.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_ctx(|| format!("reading {}", path.display()))?;
        Self::from_str(&text).with_ctx(|| format!("parsing {}", path.display()))
    }

    /// Parse and validate from a YAML string.
    pub fn from_str(text: &str) -> Result<Self> {
        let cfg: WorkspaceConfig = serde_yaml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Iterate over every repo in every bucket.
    pub fn iter_repos(&self) -> impl Iterator<Item = RepoEntry<'_>> {
        use RepoBucket::*;
        let buckets = [
            (Owned, &self.repos.owned),
            (ThirdParty, &self.repos.third_party),
            (Forks, &self.repos.forks),
            (Research, &self.repos.research),
            (Retired, &self.repos.retired),
        ];
        buckets.into_iter().flat_map(|(bucket, m)| {
            m.iter().map(move |(name, repo)| RepoEntry {
                bucket,
                name: name.as_str(),
                repo,
            })
        })
    }

    /// Find a repo by name across all buckets. Returns the first match
    /// (names should be globally unique — `validate()` enforces that).
    pub fn find_repo(&self, name: &str) -> Option<RepoEntry<'_>> {
        self.iter_repos().find(|e| e.name == name)
    }

    fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA_VERSION {
            return Err(SunbeamError::Config(format!(
                "unsupported workspace schema {}; this CLI supports schema {SCHEMA_VERSION}",
                self.schema
            )));
        }
        if self.workspace.name.is_empty() {
            return Err(SunbeamError::Config(
                "workspace.name must not be empty".into(),
            ));
        }
        self.validate_unique_repo_names()?;
        self.validate_service_ports()?;
        self.validate_stacks()?;
        Ok(())
    }

    fn validate_unique_repo_names(&self) -> Result<()> {
        let mut seen: BTreeMap<&str, &'static str> = BTreeMap::new();
        for entry in self.iter_repos() {
            if let Some(prior) = seen.insert(entry.name, entry.bucket.as_str()) {
                return Err(SunbeamError::Config(format!(
                    "repo {:?} appears in both {} and {}",
                    entry.name,
                    prior,
                    entry.bucket.as_str()
                )));
            }
        }
        Ok(())
    }

    fn validate_service_ports(&self) -> Result<()> {
        let mut claimed: BTreeMap<u16, String> = BTreeMap::new();
        for (name, val) in &self.services {
            let Some(ports) = val.get("ports").and_then(|v| v.as_sequence()) else {
                continue;
            };
            for p in ports {
                let Some(s) = p.as_str() else { continue };
                let Some(host) = parse_host_port(s) else {
                    return Err(SunbeamError::Config(format!(
                        "service {name:?} has unparseable port mapping {s:?}"
                    )));
                };
                if let Some(prior) = claimed.insert(host, name.clone()) {
                    return Err(SunbeamError::Config(format!(
                        "port {host} is claimed by both {prior:?} and {name:?}"
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_stacks(&self) -> Result<()> {
        let known: BTreeSet<&str> = self.iter_repos().map(|e| e.name).collect();
        for (stack_name, stack) in &self.stacks {
            for project in stack.projects.keys() {
                if !known.contains(project.as_str()) {
                    return Err(SunbeamError::Config(format!(
                        "stack {stack_name:?} references unknown repo {project:?}"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Parse the host port from a docker compose port mapping like
/// `"5432:5432"` or `"127.0.0.1:5432:5432"`.
///
/// Returns `None` for unparseable strings. Container-only forms
/// (single port, no colon) return `None` since they don't claim a host port.
fn parse_host_port(s: &str) -> Option<u16> {
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    let host_part = match parts.as_slice() {
        [_only] => return None,
        [host, _container] => host,
        [_bind, host, _container] => host,
        _ => return None,
    };
    host_part.split('/').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_workspace() {
        let yaml = r"
schema: 1
workspace:
  name: sunbeam
";
        let cfg = WorkspaceConfig::from_str(yaml).unwrap();
        assert_eq!(cfg.workspace.name, "sunbeam");
        assert_eq!(cfg.workspace.root, ".");
        assert!(cfg.repos.owned.is_empty());
    }

    #[test]
    fn parses_repos_buckets() {
        let yaml = r"
schema: 1
workspace:
  name: sunbeam
repos:
  owned:
    sol:  { path: sol }
    wfe:  { path: wfe }
    tuwunel: { path: tuwunel, kind: owned-fork, upstream: matrix-construct/tuwunel, tracking: main }
  3p:
    tokio: { path: 3p/tokio, upstream: tokio-rs/tokio, tracking: main }
  forks:
    kratos: { path: forks/kratos, upstream: ory/kratos, tracking: master }
  research:
    DGGRID: { path: research/DGGRID }
  retired:
    docs: { path: retired/docs }
";
        let cfg = WorkspaceConfig::from_str(yaml).unwrap();
        assert_eq!(cfg.repos.owned.len(), 3);
        assert_eq!(cfg.repos.third_party.len(), 1);
        assert_eq!(cfg.repos.forks.len(), 1);
        assert_eq!(cfg.repos.research.len(), 1);
        assert_eq!(cfg.repos.retired.len(), 1);

        let tuwunel = &cfg.repos.owned["tuwunel"];
        assert_eq!(tuwunel.path, "tuwunel");
        assert_eq!(tuwunel.kind.as_deref(), Some("owned-fork"));
        assert_eq!(
            tuwunel.upstream.as_deref(),
            Some("matrix-construct/tuwunel")
        );
    }

    #[test]
    fn iter_and_find_repo() {
        let yaml = r"
schema: 1
workspace:
  name: sunbeam
repos:
  owned:
    sol: { path: sol }
  3p:
    tokio: { path: 3p/tokio, upstream: tokio-rs/tokio }
";
        let cfg = WorkspaceConfig::from_str(yaml).unwrap();
        let names: Vec<&str> = cfg.iter_repos().map(|e| e.name).collect();
        assert_eq!(names, vec!["sol", "tokio"]);

        let tokio = cfg.find_repo("tokio").unwrap();
        assert_eq!(tokio.bucket, RepoBucket::ThirdParty);
        assert_eq!(tokio.repo.path, "3p/tokio");

        assert!(cfg.find_repo("missing").is_none());
    }

    #[test]
    fn duplicate_repo_names_rejected() {
        let yaml = r"
schema: 1
workspace:
  name: sunbeam
repos:
  owned:
    foo: { path: a }
  forks:
    foo: { path: b }
";
        let err = WorkspaceConfig::from_str(yaml).unwrap_err();
        assert!(err.to_string().contains("foo"));
        assert!(err.to_string().contains("owned"));
        assert!(err.to_string().contains("forks"));
    }

    #[test]
    fn port_conflict_detected() {
        let yaml = r#"
schema: 1
workspace:
  name: sunbeam
services:
  a:
    image: x
    ports: ["5432:5432"]
  b:
    image: y
    ports: ["5432:9999"]
"#;
        let err = WorkspaceConfig::from_str(yaml).unwrap_err();
        assert!(err.to_string().contains("5432"));
    }

    #[test]
    fn distinct_ports_accepted() {
        let yaml = r#"
schema: 1
workspace:
  name: sunbeam
services:
  a:
    image: x
    ports: ["5432:5432", "127.0.0.1:5433:5432"]
  b:
    image: y
    ports: ["6379:6379"]
"#;
        WorkspaceConfig::from_str(yaml).unwrap();
    }

    #[test]
    fn stack_unknown_repo_rejected() {
        let yaml = r#"
schema: 1
workspace:
  name: sunbeam
repos:
  owned:
    sol: { path: sol }
stacks:
  rc1:
    projects:
      sol: abcd1234
      ghost: deadbeef
"#;
        let err = WorkspaceConfig::from_str(yaml).unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn stack_known_repo_ok() {
        let yaml = r#"
schema: 1
workspace:
  name: sunbeam
repos:
  owned:
    sol: { path: sol }
stacks:
  rc1:
    description: "release candidate 1"
    projects:
      sol: abcd1234
    pinned_at: "2026-04-16T12:00:00Z"
"#;
        let cfg = WorkspaceConfig::from_str(yaml).unwrap();
        assert_eq!(cfg.stacks["rc1"].projects["sol"], "abcd1234");
    }

    #[test]
    fn parse_host_port_variants() {
        assert_eq!(parse_host_port("5432:5432"), Some(5432));
        assert_eq!(parse_host_port("127.0.0.1:5432:5432"), Some(5432));
        assert_eq!(parse_host_port("8080/tcp:80"), Some(8080));
        assert_eq!(parse_host_port("5432"), None); // container-only
        assert_eq!(parse_host_port("abc:def"), None);
    }

    #[test]
    fn unknown_schema_rejected() {
        let yaml = r"
schema: 2
workspace:
  name: x
";
        let err = WorkspaceConfig::from_str(yaml).unwrap_err();
        assert!(err.to_string().contains("schema"));
    }
}
