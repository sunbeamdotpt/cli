//! Thin wrapper around `docker compose` that materializes a compose file
//! from the workspace manifest and drives `docker compose up/down/ps/logs`.

use std::path::{Path, PathBuf};

use tokio::process::Command;

use crate::error::{Result, ResultExt, SunbeamError};
use crate::operations::WorkspaceConfig;

/// Options shared across compose sub-commands.
#[derive(Debug, Clone)]
pub struct ComposeOptions {
    /// Detach after starting (`-d` on `up`).
    pub detach: bool,
    /// Wait for healthy on start (`--wait`). Implies detach when combined.
    pub wait: bool,
    /// Remove named volumes on `down`.
    pub volumes: bool,
    /// Compose project name (`-p`). Defaults to `workspace.name`.
    pub project_name: Option<String>,
}

impl Default for ComposeOptions {
    fn default() -> Self {
        Self {
            detach: true,
            wait: true,
            volumes: false,
            project_name: None,
        }
    }
}

/// Status of a single compose service as reported by `docker compose ps`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceStatus {
    /// Name.
    pub name: String,
    /// State.
    pub state: String,
    /// Health.
    pub health: Option<String>,
    /// Ports.
    pub ports: Option<String>,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn compose_file_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".sunbeam")
        .join("compose")
        .join("docker-compose.yaml")
}

fn project_name<'a>(ws: &'a WorkspaceConfig, opts: &'a ComposeOptions) -> &'a str {
    opts.project_name
        .as_deref()
        .unwrap_or(&ws.workspace.name)
}

/// Build the base `docker compose -f <path> -p <project>` argument prefix.
fn base_args(compose_path: &Path, project: &str) -> Vec<String> {
    vec![
        "compose".into(),
        "-f".into(),
        compose_path.to_string_lossy().into_owned(),
        "-p".into(),
        project.into(),
    ]
}

fn tool_error(subcmd: &str, stderr: &[u8]) -> SunbeamError {
    let detail = String::from_utf8_lossy(stderr).trim().to_string();
    SunbeamError::tool(format!("docker compose {subcmd}"), detail)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a docker-compose YAML document from the workspace manifest.
///
/// Only the services and volumes declared in the manifest are emitted —
/// nothing is injected.
pub fn render(ws: &WorkspaceConfig) -> Result<String> {
    let mut doc = serde_yaml::Mapping::new();

    doc.insert(
        serde_yaml::Value::String("version".into()),
        serde_yaml::Value::String("3.9".into()),
    );

    let services_val = if ws.services.is_empty() {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    } else {
        serde_yaml::to_value(&ws.services)?
    };
    doc.insert(
        serde_yaml::Value::String("services".into()),
        services_val,
    );

    let volumes_val = if ws.volumes.is_empty() {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    } else {
        serde_yaml::to_value(&ws.volumes)?
    };
    doc.insert(
        serde_yaml::Value::String("volumes".into()),
        volumes_val,
    );

    Ok(serde_yaml::to_string(&serde_yaml::Value::Mapping(doc))?)
}

/// Write the rendered compose file to `<workspace_root>/.sunbeam/compose/docker-compose.yaml`.
///
/// Creates the directory tree if it does not exist. Returns the path written.
pub fn materialize(ws: &WorkspaceConfig, workspace_root: &Path) -> Result<PathBuf> {
    let path = compose_file_path(workspace_root);
    let parent = path.parent().expect("path always has a parent");
    std::fs::create_dir_all(parent).with_ctx(|| {
        format!("creating compose directory {}", parent.display())
    })?;
    let content = render(ws)?;
    std::fs::write(&path, &content)
        .with_ctx(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Run `docker compose up` for the given services (empty slice = all services).
pub async fn up(
    ws: &WorkspaceConfig,
    workspace_root: &Path,
    services: &[String],
    opts: &ComposeOptions,
) -> Result<()> {
    let compose_path = materialize(ws, workspace_root)?;
    let project = project_name(ws, opts).to_string();

    let mut args = base_args(&compose_path, &project);
    args.push("up".into());

    if opts.wait {
        args.push("--wait".into());
    } else if opts.detach {
        args.push("-d".into());
    }

    for svc in services {
        args.push(svc.clone());
    }

    let status = Command::new("docker")
        .args(&args)
        .status()
        .await
        .with_ctx(|| "spawning docker compose up".to_string())?;

    if !status.success() {
        return Err(SunbeamError::tool(
            "docker compose up",
            format!("exited with status {status}"),
        ));
    }
    Ok(())
}

/// Run `docker compose down` (all services).
pub async fn down(
    ws: &WorkspaceConfig,
    workspace_root: &Path,
    opts: &ComposeOptions,
) -> Result<()> {
    let compose_path = materialize(ws, workspace_root)?;
    let project = project_name(ws, opts).to_string();

    let mut args = base_args(&compose_path, &project);
    args.push("down".into());

    if opts.volumes {
        args.push("-v".into());
    }

    let status = Command::new("docker")
        .args(&args)
        .status()
        .await
        .with_ctx(|| "spawning docker compose down".to_string())?;

    if !status.success() {
        return Err(SunbeamError::tool(
            "docker compose down",
            format!("exited with status {status}"),
        ));
    }
    Ok(())
}

/// Run `docker compose ps --format json` and parse the NDJSON output.
pub async fn ps(
    ws: &WorkspaceConfig,
    workspace_root: &Path,
    opts: &ComposeOptions,
) -> Result<Vec<ServiceStatus>> {
    let compose_path = materialize(ws, workspace_root)?;
    let project = project_name(ws, opts).to_string();

    let mut args = base_args(&compose_path, &project);
    args.extend(["ps".into(), "--format".into(), "json".into()]);

    let output = Command::new("docker")
        .args(&args)
        .output()
        .await
        .with_ctx(|| "spawning docker compose ps".to_string())?;

    if !output.status.success() {
        return Err(tool_error("ps", &output.stderr));
    }

    parse_ps_output(&output.stdout)
}

/// Run `docker compose logs <service>`, streaming to stdout/stderr.
pub async fn logs(
    ws: &WorkspaceConfig,
    workspace_root: &Path,
    service: &str,
    follow: bool,
    opts: &ComposeOptions,
) -> Result<()> {
    let compose_path = materialize(ws, workspace_root)?;
    let project = project_name(ws, opts).to_string();

    let mut args = base_args(&compose_path, &project);
    args.push("logs".into());
    if follow {
        args.push("-f".into());
    }
    args.push(service.to_string());

    let status = Command::new("docker")
        .args(&args)
        .status()
        .await
        .with_ctx(|| format!("spawning docker compose logs {service}"))?;

    if !status.success() {
        return Err(SunbeamError::tool(
            "docker compose logs",
            format!("exited with status {status}"),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// NDJSON parser for `docker compose ps --format json`
// ---------------------------------------------------------------------------

/// `docker compose ps --format json` emits one JSON object per line.
/// Fields are PascalCase. We're lenient about missing/unknown fields.
fn parse_ps_output(raw: &[u8]) -> Result<Vec<ServiceStatus>> {
    let text = std::str::from_utf8(raw)
        .map_err(|e| SunbeamError::Other(format!("docker compose ps output is not utf-8: {e}")))?;

    let mut results = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line)?;
        let status = ServiceStatus {
            name: string_field(&v, &["Name", "name"]),
            state: string_field(&v, &["State", "state"]),
            health: optional_string_field(&v, &["Health", "health"]),
            ports: optional_string_field(&v, &["Ports", "ports"]),
        };
        results.push(status);
    }
    Ok(results)
}

fn string_field(v: &serde_json::Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(s) = v.get(key).and_then(|f| f.as_str()) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn optional_string_field(v: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = v.get(key).and_then(|f| f.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::TempDir;

    use super::*;
    use crate::operations::config::{WorkspaceMeta, WorkspaceConfig};

    fn minimal_ws() -> WorkspaceConfig {
        WorkspaceConfig {
            schema: 1,
            workspace: WorkspaceMeta {
                name: "test".into(),
                root: ".".into(),
            },
            repos: Default::default(),
            services: BTreeMap::new(),
            volumes: BTreeMap::new(),
            stacks: BTreeMap::new(),
        }
    }

    #[test]
    fn render_minimal() {
        let ws = minimal_ws();
        let out = render(&ws).unwrap();
        // Must have version, services: {}, volumes: {}
        assert!(out.contains("version:"), "missing version in:\n{out}");
        assert!(out.contains("services:"), "missing services in:\n{out}");
        assert!(out.contains("volumes:"), "missing volumes in:\n{out}");

        // Re-parse and confirm keys are empty mappings
        let v: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
        let services = v.get("services").unwrap();
        assert!(
            services.as_mapping().map(|m| m.is_empty()).unwrap_or(false),
            "expected empty services mapping"
        );
        let volumes = v.get("volumes").unwrap();
        assert!(
            volumes.as_mapping().map(|m| m.is_empty()).unwrap_or(false),
            "expected empty volumes mapping"
        );
    }

    #[test]
    fn render_preserves_services() {
        let yaml = r#"
schema: 1
workspace:
  name: sunbeam
services:
  postgres:
    image: postgres:17
    ports:
      - "5432:5432"
  valkey:
    image: valkey/valkey:8
    ports:
      - "6379:6379"
volumes:
  sunbeam-pg-data:
"#;
        let ws = WorkspaceConfig::from_str(yaml).unwrap();
        let rendered = render(&ws).unwrap();
        let reparsed: serde_yaml::Value = serde_yaml::from_str(&rendered).unwrap();

        let services = reparsed.get("services").unwrap().as_mapping().unwrap();
        assert!(services.contains_key("postgres"), "missing postgres");
        assert!(services.contains_key("valkey"), "missing valkey");

        let pg = &services["postgres"];
        assert_eq!(
            pg.get("image").and_then(|v| v.as_str()),
            Some("postgres:17")
        );

        let volumes = reparsed.get("volumes").unwrap().as_mapping().unwrap();
        assert!(volumes.contains_key("sunbeam-pg-data"), "missing volume");
    }

    #[test]
    fn materialize_writes_file() {
        let tmp = TempDir::new().unwrap();
        let ws = minimal_ws();
        let path = materialize(&ws, tmp.path()).unwrap();

        let expected = tmp.path().join(".sunbeam/compose/docker-compose.yaml");
        assert_eq!(path, expected);
        assert!(path.exists(), "compose file was not created");

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("services:"));
        assert!(content.contains("volumes:"));
    }

    #[test]
    fn service_status_parses_ndjson() {
        let ndjson = r#"{"Name":"sunbeam_postgres_1","Command":"docker-entrypoint.sh postgres","Project":"sunbeam","Service":"postgres","State":"running","Health":"healthy","Ports":"0.0.0.0:5432->5432/tcp"}
{"Name":"sunbeam_valkey_1","Command":"docker-entrypoint.sh valkey-server","Project":"sunbeam","Service":"valkey","State":"running","Health":"","Ports":"0.0.0.0:6379->6379/tcp"}
{"Name":"sunbeam_opensearch_1","Command":"/usr/share/opensearch/opensearch-docker-entrypoint.sh","Project":"sunbeam","Service":"opensearch","State":"exited","Health":"","Ports":""}
"#;

        let statuses = parse_ps_output(ndjson.as_bytes()).unwrap();
        assert_eq!(statuses.len(), 3);

        assert_eq!(statuses[0].name, "sunbeam_postgres_1");
        assert_eq!(statuses[0].state, "running");
        assert_eq!(statuses[0].health.as_deref(), Some("healthy"));
        assert_eq!(
            statuses[0].ports.as_deref(),
            Some("0.0.0.0:5432->5432/tcp")
        );

        assert_eq!(statuses[1].name, "sunbeam_valkey_1");
        assert_eq!(statuses[1].state, "running");
        // empty health string collapses to None
        assert!(statuses[1].health.is_none());

        assert_eq!(statuses[2].state, "exited");
        // empty ports string collapses to None
        assert!(statuses[2].ports.is_none());
    }
}
