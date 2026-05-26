//! Run a single verb against a [`ProjectConfig`].
//!
//! The runner resolves the verb to a [`Target`] and dispatches:
//!
//! - [`Target::Skip`] → no-op, returns [`RunOutcome::Skipped`].
//! - [`Target::Exec`] → spawn a subprocess (shell string or argv form).
//! - [`Target::Workflow`] → run a wfe workflow, blocking until complete.
//!
//! The runner inherits stdio by default so long-running verbs (`dev`, `test`)
//! stream their output live.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::error::{Result, SunbeamError};
use crate::project::{ProjectConfig, Target};
use crate::project::config::{ExecCommand, ExecTarget, WorkflowTarget};

/// Outcome of attempting to run a verb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// Target was [`Target::Skip`] (explicit `skip` or missing entry).
    Skipped,
    /// Target ran to completion successfully.
    Ran,
}

/// Options controlling how the runner behaves.
#[derive(Debug, Default, Clone)]
pub struct RunOptions {
    /// Extra env vars layered on top of the current process env. Target env
    /// from the YAML still wins over these — the layering order is:
    /// inherited → `extra_env` → target env.
    pub extra_env: BTreeMap<String, String>,
    /// Echo the command on stderr before running.
    pub verbose: bool,
    /// Print the command without running it.
    pub dry_run: bool,
}

/// Run `verb` for the project rooted at `project_root`.
#[tracing::instrument]
pub async fn run(
    cfg: &ProjectConfig,
    project_root: &Path,
    verb: &str,
    opts: &RunOptions,
) -> Result<RunOutcome> {
    tracing::info!("project run {verb} {}", cfg.project.name);
    match cfg.target(verb) {
        Target::Skip(_) => Ok(RunOutcome::Skipped),
        Target::Exec(t) => run_exec(verb, project_root, &t, opts).await,
        Target::Workflow(t) => run_workflow(verb, project_root, &t, opts).await,
    }
}

async fn run_exec(
    verb: &str,
    project_root: &Path,
    target: &ExecTarget,
    opts: &RunOptions,
) -> Result<RunOutcome> {
    let wd = resolve_cwd(project_root, target.cwd.as_deref());
    let env = merged_env(&opts.extra_env, &target.env);

    let display = exec_display(&target.exec);
    if opts.verbose || opts.dry_run {
        eprintln!("$ {display}");
    }
    if opts.dry_run {
        return Ok(RunOutcome::Ran);
    }

    let mut cmd = match &target.exec {
        ExecCommand::Shell(s) => {
            if s.trim().is_empty() {
                return Err(SunbeamError::Config(format!(
                    "target {verb:?} has an empty shell command"
                )));
            }
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(s);
            c
        }
        ExecCommand::Argv(v) => {
            let (prog, args) = v.split_first().ok_or_else(|| {
                SunbeamError::Config(format!("target {verb:?} has an empty argv"))
            })?;
            let mut c = tokio::process::Command::new(prog);
            c.args(args);
            c
        }
    };

    cmd.current_dir(&wd);
    cmd.envs(env.iter());
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let status = cmd
        .status()
        .await
        .map_err(|e| SunbeamError::tool(verb.to_string(), format!("spawn failed: {e}")))?;

    if !status.success() {
        let detail = match status.code() {
            Some(code) => format!("exit status {code} from `{display}`"),
            None => format!("terminated by signal from `{display}`"),
        };
        return Err(SunbeamError::tool(verb.to_string(), detail));
    }

    Ok(RunOutcome::Ran)
}

async fn run_workflow(
    verb: &str,
    project_root: &Path,
    target: &WorkflowTarget,
    opts: &RunOptions,
) -> Result<RunOutcome> {
    if target.workflow.is_empty() {
        return Err(SunbeamError::Config(format!(
            "target {verb:?} has an empty workflow path"
        )));
    }

    let workflow_path = project_root.join(&target.workflow);
    let display = format!("wfe run {}", workflow_path.display());
    if opts.verbose || opts.dry_run {
        eprintln!("$ {display}");
    }
    if opts.dry_run {
        return Ok(RunOutcome::Ran);
    }

    let initial_data = build_workflow_inputs(project_root, &target.inputs, &opts.extra_env);

    let ctx_name = {
        let cfg = crate::config::load_config();
        if cfg.current_context.is_empty() {
            "default".to_string()
        } else {
            cfg.current_context.clone()
        }
    };
    let host = crate::workflows::host::create_host(&ctx_name).await?;

    let workflow_id = workflow_path
        .to_str()
        .ok_or_else(|| SunbeamError::Config(format!(
            "workflow path contains non-UTF-8: {}",
            workflow_path.display()
        )))?;

    let instance = wfe::run_workflow_sync(
        &host,
        workflow_id,
        1,
        initial_data,
        std::time::Duration::from_secs(3600),
    )
    .await
    .map_err(|e| SunbeamError::Other(format!("workflow {workflow_id} failed: {e}")));

    crate::workflows::host::shutdown_host(host).await;
    let instance = instance?;

    if instance.status != wfe_core::models::WorkflowStatus::Complete {
        return Err(SunbeamError::Other(format!(
            "workflow {workflow_id} ended with status {:?}",
            instance.status
        )));
    }

    Ok(RunOutcome::Ran)
}

fn resolve_cwd(project_root: &Path, cwd: Option<&str>) -> PathBuf {
    match cwd {
        Some(c) if !c.is_empty() && c != "." => project_root.join(c),
        _ => project_root.to_path_buf(),
    }
}

fn merged_env(
    extra: &BTreeMap<String, String>,
    target: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (k, v) in extra {
        out.insert(k.clone(), v.clone());
    }
    for (k, v) in target {
        out.insert(k.clone(), v.clone());
    }
    out
}

fn exec_display(cmd: &ExecCommand) -> String {
    match cmd {
        ExecCommand::Shell(s) => s.clone(),
        ExecCommand::Argv(v) => v.join(" "),
    }
}

fn build_workflow_inputs(
    project_root: &Path,
    inputs: &BTreeMap<String, serde_yaml::Value>,
    extra_env: &BTreeMap<String, String>,
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "__project_root".into(),
        serde_json::Value::String(project_root.display().to_string()),
    );
    for (k, v) in extra_env {
        obj.insert(format!("env_{k}"), serde_json::Value::String(v.clone()));
    }
    for (k, v) in inputs {
        obj.insert(k.clone(), yaml_to_json(v));
    }
    serde_json::Value::Object(obj)
}

fn yaml_to_json(v: &serde_yaml::Value) -> serde_json::Value {
    use serde_yaml::Value as Y;
    match v {
        Y::Null => serde_json::Value::Null,
        Y::Bool(b) => serde_json::Value::Bool(*b),
        Y::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(i.into())
            } else if let Some(u) = n.as_u64() {
                serde_json::Value::Number(u.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::Null
            }
        }
        Y::String(s) => serde_json::Value::String(s.clone()),
        Y::Sequence(seq) => serde_json::Value::Array(seq.iter().map(yaml_to_json).collect()),
        Y::Mapping(m) => {
            let mut obj = serde_json::Map::new();
            for (k, val) in m {
                let key = match k {
                    Y::String(s) => s.clone(),
                    other => serde_yaml::to_string(other)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                };
                obj.insert(key, yaml_to_json(val));
            }
            serde_json::Value::Object(obj)
        }
        Y::Tagged(t) => yaml_to_json(&t.value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::config::{ExecTarget, ProjectMeta, SkipMarker};
    use std::fs;
    use tempfile::TempDir;

    fn cfg_with_target(target_name: &str, target: Target) -> ProjectConfig {
        let mut targets = BTreeMap::new();
        targets.insert(target_name.to_string(), target);
        ProjectConfig {
            schema: 1,
            project: ProjectMeta {
                name: "t".into(),
                kind: None,
                description: None,
            },
            targets,
            deps: Default::default(),
            tenant: None,
            outputs: Vec::new(),
        }
    }

    fn empty_cfg() -> ProjectConfig {
        ProjectConfig {
            schema: 1,
            project: ProjectMeta {
                name: "t".into(),
                kind: None,
                description: None,
            },
            targets: BTreeMap::new(),
            deps: Default::default(),
            tenant: None,
            outputs: Vec::new(),
        }
    }

    fn shell(s: &str) -> Target {
        Target::Exec(ExecTarget {
            exec: ExecCommand::Shell(s.into()),
            cwd: None,
            env: BTreeMap::new(),
        })
    }

    fn argv(v: &[&str]) -> Target {
        Target::Exec(ExecTarget {
            exec: ExecCommand::Argv(v.iter().map(|s| s.to_string()).collect()),
            cwd: None,
            env: BTreeMap::new(),
        })
    }

    #[tokio::test]
    async fn skip_returns_skipped() {
        let cfg = empty_cfg();
        let tmp = TempDir::new().unwrap();
        let out = run(&cfg, tmp.path(), "build", &RunOptions::default()).await.unwrap();
        assert_eq!(out, RunOutcome::Skipped);
    }

    #[tokio::test]
    async fn explicit_skip_returns_skipped() {
        let cfg = cfg_with_target("test", Target::Skip(SkipMarker::Skip));
        let tmp = TempDir::new().unwrap();
        let out = run(&cfg, tmp.path(), "test", &RunOptions::default()).await.unwrap();
        assert_eq!(out, RunOutcome::Skipped);
    }

    #[tokio::test]
    async fn shell_target_runs_successfully() {
        let cfg = cfg_with_target("build", shell("true"));
        let tmp = TempDir::new().unwrap();
        let out = run(&cfg, tmp.path(), "build", &RunOptions::default()).await.unwrap();
        assert_eq!(out, RunOutcome::Ran);
    }

    #[tokio::test]
    async fn shell_failure_is_reported() {
        let cfg = cfg_with_target("build", shell("false"));
        let tmp = TempDir::new().unwrap();
        let err = run(&cfg, tmp.path(), "build", &RunOptions::default()).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("build"), "msg={msg}");
        assert!(msg.contains("exit status"), "msg={msg}");
    }

    #[tokio::test]
    async fn argv_target_runs_successfully() {
        let cfg = cfg_with_target("lint", argv(&["echo", "hi"]));
        let tmp = TempDir::new().unwrap();
        let out = run(&cfg, tmp.path(), "lint", &RunOptions::default()).await.unwrap();
        assert_eq!(out, RunOutcome::Ran);
    }

    #[tokio::test]
    async fn argv_missing_binary_errors() {
        let cfg = cfg_with_target("lint", argv(&["this-binary-should-not-exist-anywhere"]));
        let tmp = TempDir::new().unwrap();
        let err = run(&cfg, tmp.path(), "lint", &RunOptions::default()).await.unwrap_err();
        assert!(err.to_string().contains("lint"));
    }

    #[tokio::test]
    async fn extra_env_reaches_subprocess() {
        let cfg = cfg_with_target("test", shell("test \"$SB_FOO\" = \"bar\""));
        let tmp = TempDir::new().unwrap();
        let mut opts = RunOptions::default();
        opts.extra_env.insert("SB_FOO".into(), "bar".into());
        let out = run(&cfg, tmp.path(), "test", &opts).await.unwrap();
        assert_eq!(out, RunOutcome::Ran);
    }

    #[tokio::test]
    async fn target_env_overrides_extra_env() {
        let mut env = BTreeMap::new();
        env.insert("SB_FOO".to_string(), "from-target".to_string());
        let target = Target::Exec(ExecTarget {
            exec: ExecCommand::Shell("test \"$SB_FOO\" = \"from-target\"".into()),
            cwd: None,
            env,
        });
        let cfg = cfg_with_target("test", target);
        let tmp = TempDir::new().unwrap();
        let mut opts = RunOptions::default();
        opts.extra_env.insert("SB_FOO".into(), "from-extra".into());
        let out = run(&cfg, tmp.path(), "test", &opts).await.unwrap();
        assert_eq!(out, RunOutcome::Ran);
    }

    #[tokio::test]
    async fn dry_run_does_not_execute() {
        let cfg = cfg_with_target("build", shell("false"));
        let tmp = TempDir::new().unwrap();
        let mut opts = RunOptions::default();
        opts.dry_run = true;
        let out = run(&cfg, tmp.path(), "build", &opts).await.unwrap();
        assert_eq!(out, RunOutcome::Ran);
    }

    #[tokio::test]
    async fn cwd_is_respected() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub").join("marker"), b"x").unwrap();

        let target = Target::Exec(ExecTarget {
            exec: ExecCommand::Shell("test -f marker".into()),
            cwd: Some("sub".into()),
            env: BTreeMap::new(),
        });
        let cfg = cfg_with_target("test", target);
        let out = run(&cfg, tmp.path(), "test", &RunOptions::default()).await.unwrap();
        assert_eq!(out, RunOutcome::Ran);
    }

    #[test]
    fn yaml_to_json_covers_all_variants() {
        let y: serde_yaml::Value = serde_yaml::from_str(
            "- null\n- true\n- 42\n- 3.5\n- hi\n- [1, 2]\n- {a: 1}\n",
        )
        .unwrap();
        let j = yaml_to_json(&y);
        let arr = j.as_array().unwrap();
        assert!(arr[0].is_null());
        assert_eq!(arr[1].as_bool(), Some(true));
        assert_eq!(arr[2].as_i64(), Some(42));
        assert!(arr[3].as_f64().unwrap() > 3.0);
        assert_eq!(arr[4].as_str(), Some("hi"));
        assert!(arr[5].is_array());
        assert_eq!(arr[6].as_object().unwrap()["a"].as_i64(), Some(1));
    }
}
