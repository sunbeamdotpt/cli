//! Version control commands — repo tool dispatch.

use crate::cli::VcsAction;
use crate::error::{Result, SunbeamError};
use camino::Utf8PathBuf;
use repo_rs_cmd::Command as RepoCommand;
use std::sync::Arc;

/// Top-level dispatch for repo subcommands.
pub async fn dispatch(_logger: &crate::logger::Logger, action: VcsAction) -> Result<()> {
    let result = match &action {
        VcsAction::Init(args) => {
            let ctx = make_minimal_context();
            args.execute(&ctx).await
        }
        VcsAction::Version(args) => {
            let ctx = make_minimal_context();
            args.execute(&ctx).await
        }
        VcsAction::Help(args) => {
            let ctx = make_minimal_context();
            args.execute(&ctx).await
        }
        _ => {
            let cwd = std::env::current_dir().map_err(|e| SunbeamError::Io {
                context: "getting current directory".into(),
                source: e,
            })?;
            let start = Utf8PathBuf::from_path_buf(cwd)
                .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().to_string()));
            let ctx = repo_rs_engine::Context::load(&start, Arc::new(repo_rs_git::DefaultBackend))
                .map_err(|e| SunbeamError::Other(format!("repo context load error: {e}").into()))?;

            match &action {
                VcsAction::Sync(args) => args.execute(&ctx).await,
                VcsAction::Upload(args) => args.execute(&ctx).await,
                VcsAction::Download(args) => args.execute(&ctx).await,
                VcsAction::Start(args) => args.execute(&ctx).await,
                VcsAction::Status(args) => args.execute(&ctx).await,
                VcsAction::Diff(args) => args.execute(&ctx).await,
                VcsAction::Stage(args) => args.execute(&ctx).await,
                VcsAction::Rebase(args) => args.execute(&ctx).await,
                VcsAction::CherryPick(args) => args.execute(&ctx).await,
                VcsAction::Abandon(args) => args.execute(&ctx).await,
                VcsAction::Checkout(args) => args.execute(&ctx).await,
                VcsAction::Branches(args) => args.execute(&ctx).await,
                VcsAction::Forall(args) => args.execute(&ctx).await,
                VcsAction::Grep(args) => args.execute(&ctx).await,
                VcsAction::Manifest(args) => args.execute(&ctx).await,
                VcsAction::Info(args) => args.execute(&ctx).await,
                VcsAction::List(args) => args.execute(&ctx).await,
                VcsAction::Prune(args) => args.execute(&ctx).await,
                VcsAction::Gc(args) => args.execute(&ctx).await,
                VcsAction::Diffmanifests(args) => args.execute(&ctx).await,
                VcsAction::Wipe(args) => args.execute(&ctx).await,
                VcsAction::Selfupdate(args) => args.execute(&ctx).await,
                VcsAction::Smartsync(args) => args.execute(&ctx).await,
                VcsAction::Overview(args) => args.execute(&ctx).await,
                // These three are handled above; never reached.
                VcsAction::Init(_) | VcsAction::Version(_) | VcsAction::Help(_) => unreachable!(),
            }
        }
    };

    match result {
        Ok(code) => {
            if code == std::process::ExitCode::SUCCESS {
                Ok(())
            } else {
                Err(SunbeamError::Other(
                    format!("repo command exited with non-zero status").into(),
                ))
            }
        }
        Err(e) => Err(SunbeamError::Other(
            format!("repo command error: {e}").into(),
        )),
    }
}

fn make_minimal_context() -> repo_rs_engine::Context {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let repo_root = Utf8PathBuf::from_path_buf(cwd)
        .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().to_string()));
    repo_rs_engine::Context {
        repo_root,
        client: repo_rs_model::RepoClient {
            repo_dir: Utf8PathBuf::from(""),
            manifest_project: repo_rs_model::client::MetaProject {
                name: String::new(),
                path: Utf8PathBuf::from(""),
                gitdir: Utf8PathBuf::from(""),
            },
            repo_project: repo_rs_model::client::MetaProject {
                name: String::new(),
                path: Utf8PathBuf::from(""),
                gitdir: Utf8PathBuf::from(""),
            },
            projects: indexmap::IndexMap::new(),
            submanifests: indexmap::IndexMap::new(),
        },
        outer_client: None,
        progress: Box::new(()),
        color_choice: anstream::ColorChoice::Auto,
        git: Arc::new(repo_rs_git::DefaultBackend),
    }
}
