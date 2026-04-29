//! `sunbeam vcs maintenance …` — per-repo git maintenance via AdminService.
//!
//! Maps to `AdminService.RunMaintenance` gRPC:
//!   run --repo <ulid> --kind <geometric_repack|bitmap_regen|retention_sweep>

use crate::error::Result;
use crate::output::{OutputFormat, render};
use crate::vcs::client::{connect_admin_client, map_status, resolve_token};
use gitserv_proto::pb::{MaintenanceKind, RepoId, RunMaintenanceRequest};
use serde::Serialize;

#[derive(Debug, clap::Args)]
pub struct MaintenanceArgs {
    #[command(subcommand)]
    pub command: MaintenanceCmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum MaintenanceCmd {
    /// Run a maintenance operation on a repository (system-admin only).
    Run {
        /// Repo ULID.
        #[arg(long)]
        repo: String,
        /// Maintenance kind: geometric_repack | bitmap_regen | retention_sweep.
        #[arg(long, value_parser = parse_maintenance_kind)]
        kind: MaintenanceKind,
    },
}

fn parse_maintenance_kind(s: &str) -> std::result::Result<MaintenanceKind, String> {
    match s {
        "geometric_repack" => Ok(MaintenanceKind::GeometricRepack),
        "bitmap_regen" => Ok(MaintenanceKind::BitmapRegen),
        "retention_sweep" => Ok(MaintenanceKind::RetentionSweep),
        other => Err(format!(
            "unknown maintenance kind '{other}'; valid: geometric_repack, bitmap_regen, retention_sweep"
        )),
    }
}

#[derive(Debug, Serialize)]
struct MaintenanceResult {
    status: String,
    loose_objects_before: u64,
    loose_objects_after: u64,
    pack_count_before: u64,
    pack_count_after: u64,
    elapsed_ms: u64,
    detail: String,
}

pub async fn run(args: MaintenanceArgs, endpoint: &str, format: OutputFormat) -> Result<()> {
    let domain = crate::config::domain().to_string();
    let token = resolve_token(&domain).await?;
    let mut client = connect_admin_client(endpoint, &token).await?;

    match args.command {
        MaintenanceCmd::Run { repo, kind } => {
            let resp = client
                .run_maintenance(RunMaintenanceRequest {
                    repo: Some(RepoId { ulid: repo }),
                    kind: kind as i32,
                })
                .await
                .map_err(map_status)?
                .into_inner();

            let result = MaintenanceResult {
                status: resp.status,
                loose_objects_before: resp.loose_objects_before,
                loose_objects_after: resp.loose_objects_after,
                pack_count_before: resp.pack_count_before,
                pack_count_after: resp.pack_count_after,
                elapsed_ms: resp.elapsed_ms,
                detail: resp.detail,
            };

            render(&result, format)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(clap::Parser)]
    struct Cmd {
        #[command(subcommand)]
        sub: MaintenanceCmd,
    }

    fn parse(args: &[&str]) -> MaintenanceCmd {
        Cmd::try_parse_from(args).unwrap().sub
    }

    #[test]
    fn run_geometric_repack() {
        match parse(&[
            "cmd",
            "run",
            "--repo",
            "01K000000000000000000000A",
            "--kind",
            "geometric_repack",
        ]) {
            MaintenanceCmd::Run { repo, kind } => {
                assert_eq!(repo, "01K000000000000000000000A");
                assert_eq!(kind, MaintenanceKind::GeometricRepack);
            }
        }
    }

    #[test]
    fn run_bitmap_regen() {
        match parse(&[
            "cmd",
            "run",
            "--repo",
            "01K000000000000000000000B",
            "--kind",
            "bitmap_regen",
        ]) {
            MaintenanceCmd::Run { kind, .. } => {
                assert_eq!(kind, MaintenanceKind::BitmapRegen);
            }
        }
    }

    #[test]
    fn run_retention_sweep() {
        match parse(&[
            "cmd",
            "run",
            "--repo",
            "01K000000000000000000000C",
            "--kind",
            "retention_sweep",
        ]) {
            MaintenanceCmd::Run { kind, .. } => {
                assert_eq!(kind, MaintenanceKind::RetentionSweep);
            }
        }
    }

    #[test]
    fn run_unknown_kind_rejected() {
        let result = Cmd::try_parse_from([
            "cmd",
            "run",
            "--repo",
            "01K000000000000000000000A",
            "--kind",
            "bogus",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn run_missing_repo_rejected() {
        let result = Cmd::try_parse_from(["cmd", "run", "--kind", "bitmap_regen"]);
        assert!(result.is_err());
    }

    #[test]
    fn maintenance_result_serializes() {
        let r = MaintenanceResult {
            status: "completed".into(),
            loose_objects_before: 100,
            loose_objects_after: 0,
            pack_count_before: 3,
            pack_count_after: 1,
            elapsed_ms: 420,
            detail: "repacked 2 packs".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"status\":\"completed\""), "{json}");
        assert!(json.contains("\"elapsed_ms\":420"), "{json}");
    }
}
