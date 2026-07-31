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
