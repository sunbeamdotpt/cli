use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap_or_default();

    // Embed lima-sunbeam.yaml for VM provisioning
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let lima_yaml_src = manifest_dir.join("../../../lima-sunbeam.yaml");
    let lima_yaml_dst = out_dir.join("lima-sunbeam.yaml");
    fs::copy(&lima_yaml_src, &lima_yaml_dst)
        .expect(&format!("lima-sunbeam.yaml not found at {}", lima_yaml_src.display()));
    println!("cargo:rerun-if-changed={}", lima_yaml_src.display());

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
