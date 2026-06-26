use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap_or_default();

    // Embed lima-sunbeam.yaml for VM provisioning
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let lima_yaml_src = manifest_dir.join("../lima-sunbeam.yaml");
    let lima_yaml_dst = out_dir.join("lima-sunbeam.yaml");
    fs::copy(&lima_yaml_src, &lima_yaml_dst)
        .unwrap_or_else(|_| panic!("lima-sunbeam.yaml not found at {}", lima_yaml_src.display()));
    println!("cargo:rerun-if-changed={}", lima_yaml_src.display());

    // Generate Kanban gRPC client stubs from vendored proto definitions.
    let kanban_proto_dir = manifest_dir.join("proto");
    let kanban_protos = &[
        "sunbeam/kanban/v1/auth.proto",
        "sunbeam/kanban/v1/attachments.proto",
        "sunbeam/kanban/v1/boards.proto",
        "sunbeam/kanban/v1/cards.proto",
        "sunbeam/kanban/v1/events.proto",
        "sunbeam/kanban/v1/aggregated_boards.proto",
        "sunbeam/kanban/v1/github.proto",
        "sunbeam/kanban/v1/projects.proto",
        "sunbeam/kanban/v1/public_boards.proto",
        "sunbeam/kanban/v1/search.proto",
        "sunbeam/kanban/v1/templates.proto",
        "ory/keto/relation_tuples/v1alpha2/read_service.proto",
    ];
    let kanban_include_dirs = std::slice::from_ref(&kanban_proto_dir);
    let kanban_proto_paths: Vec<_> = kanban_protos
        .iter()
        .map(|p| kanban_proto_dir.join(p))
        .collect();

    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&kanban_proto_paths, kanban_include_dirs)
        .unwrap_or_else(|e| panic!("failed to compile kanban protos: {e}"));

    for p in kanban_protos {
        println!(
            "cargo:rerun-if-changed={}",
            kanban_proto_dir.join(p).display()
        );
    }

    // Set version info from git
    let commit = git_commit_sha();
    println!("cargo:rustc-env=SUNBEAM_COMMIT={commit}");

    // Build target triple and build date
    println!("cargo:rustc-env=SUNBEAM_TARGET={target}");
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    println!("cargo:rustc-env=SUNBEAM_BUILD_DATE={date}");

    // Rebuild if git HEAD changes (workspace root is two levels up)
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}

fn git_commit_sha() -> String {
    Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}
