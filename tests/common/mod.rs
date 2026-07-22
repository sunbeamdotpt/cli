//! Shared helpers for the integration test suites.
//!
//! Every container-backed suite must pass in environments without a Docker
//! runtime (e.g. CI). `docker_available()` probes the runtime once per test
//! binary; suites check it up front and return early with a notice.
#![allow(dead_code)]

use std::sync::OnceLock;

static DOCKER_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Probe for a Docker-compatible runtime. Result is cached per test binary.
pub fn docker_available() -> bool {
    *DOCKER_AVAILABLE.get_or_init(|| {
        std::process::Command::new("docker")
            .arg("info")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Returns true (and prints a skip notice) when Docker is unavailable.
pub fn skip_without_docker(suite: &str) -> bool {
    if docker_available() {
        false
    } else {
        eprintln!("skipping {suite}: no Docker-compatible runtime available");
        true
    }
}

// ---------------------------------------------------------------------------
// sdk managed-tool cache pre-warming
//
// WORKAROUND (sdk mail #39): `sdk::kube::kustomize_build` lazily downloads
// kustomize/helm into `~/.sunbeam/bin` via `reqwest::blocking` when the cache
// is cold — and that client's internal runtime panics when dropped inside an
// async context (i.e. always, in tests and on fresh machines). Tests must
// therefore populate the cache from a SYNC context before calling anything
// that reaches `kustomize_build`. Remove once the sdk makes the downloader
// async. URLs/versions mirror the sdk's pinned tools.rs values.
// ---------------------------------------------------------------------------

/// Locate an executable on PATH.
fn path_lookup(tool: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(tool);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn platform() -> Option<(&'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some(("darwin", "arm64")),
        ("macos", "x86_64") => Some(("darwin", "amd64")),
        ("linux", "aarch64") => Some(("linux", "arm64")),
        ("linux", "x86_64") => Some(("linux", "amd64")),
        _ => None,
    }
}

fn tool_url(tool: &str, os: &str, arch: &str) -> Option<(String, String)> {
    match tool {
        "kustomize" => Some((
            format!(
                "https://github.com/kubernetes-sigs/kustomize/releases/download/\
                 kustomize%2Fv5.8.1/kustomize_v5.8.1_{os}_{arch}.tar.gz"
            ),
            "kustomize".to_string(),
        )),
        "helm" => Some((
            format!("https://get.helm.sh/helm-v4.1.0-{os}-{arch}.tar.gz"),
            format!("{os}-{arch}/helm"),
        )),
        _ => None,
    }
}

fn prewarm_one(bin: &std::path::Path, tool: &str) -> bool {
    let dest = bin.join(tool);
    if dest.exists() {
        return true;
    }
    // Prefer a copy from PATH (offline-friendly, no network).
    if let Some(src) = path_lookup(tool)
        && std::fs::copy(&src, &dest).is_ok()
    {
        return true;
    }
    // Otherwise fetch the sdk-pinned tarball with plain curl+tar (sync, no
    // runtime involved).
    let Some((os, arch)) = platform() else {
        return false;
    };
    let Some((url, entry)) = tool_url(tool, os, arch) else {
        return false;
    };
    let ok = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "curl -sfL '{url}' | tar -xz -C '{}'",
            bin.display()
        ))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return false;
    }
    let nested = bin.join(&entry);
    if nested == dest {
        true
    } else {
        std::fs::rename(&nested, &dest).is_ok()
    }
}

/// Populate the sdk's managed tool cache under `home/.sunbeam/bin` with
/// kustomize and helm from a synchronous context. Returns false (caller
/// should skip) if a tool can't be provided.
pub fn prewarm_tool_cache(home: &std::path::Path) -> bool {
    let bin = home.join(".sunbeam").join("bin");
    if std::fs::create_dir_all(&bin).is_err() {
        return false;
    }
    prewarm_one(&bin, "kustomize") && prewarm_one(&bin, "helm")
}
