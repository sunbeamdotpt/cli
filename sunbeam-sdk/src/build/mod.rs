//! BuildKit CLI wrapper for container image builds.

use crate::error::{Result, SunbeamError};

/// Arguments for a BuildKit build invocation.
#[derive(Debug, Clone)]
pub struct BuildArgs {
    /// Frontend to use (e.g. "dockerfile.v0").
    pub frontend: String,
    /// Path to the local build context.
    pub local_context: String,
    /// Path to the Dockerfile directory.
    pub local_dockerfile: String,
    /// Output specification (e.g. "type=image,name=...,push=true").
    pub output: String,
    /// Additional build arguments as key-value pairs.
    pub build_args: Vec<(String, String)>,
}

/// Output from a successful build.
#[derive(Debug, Clone)]
pub struct BuildOutput {
    /// Image digest (e.g. "sha256:...").
    pub digest: String,
    /// Whether the build succeeded.
    pub success: bool,
}

/// Typed wrapper around the `buildctl` CLI.
pub struct BuildKitClient {
    /// BuildKit daemon address (e.g. "unix:///run/buildkit/buildkitd.sock").
    pub host: String,
}

impl BuildKitClient {
    /// Create a new BuildKitClient pointing at the given host.
    pub fn new(host: &str) -> Self {
        Self {
            host: host.to_string(),
        }
    }

    /// Run a build via `buildctl build`.
    pub async fn build(&self, args: &BuildArgs) -> Result<BuildOutput> {
        let mut cmd = tokio::process::Command::new("buildctl");
        cmd.arg("--addr").arg(&self.host);
        cmd.arg("build");
        cmd.arg("--frontend").arg(&args.frontend);
        cmd.arg("--local")
            .arg(format!("context={}", args.local_context));
        cmd.arg("--local")
            .arg(format!("dockerfile={}", args.local_dockerfile));
        cmd.arg("--output").arg(&args.output);

        for (key, value) in &args.build_args {
            cmd.arg("--opt")
                .arg(format!("build-arg:{}={}", key, value));
        }

        let output = cmd.output().await.map_err(|e| {
            SunbeamError::ExternalTool {
                tool: "buildctl".into(),
                detail: format!("failed to spawn: {e}"),
            }
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            return Err(SunbeamError::ExternalTool {
                tool: "buildctl".into(),
                detail: format!("build failed: {stderr}"),
            });
        }

        // Try to extract digest from output
        let digest = extract_digest(&stdout)
            .or_else(|| extract_digest(&stderr))
            .unwrap_or_default();

        Ok(BuildOutput {
            digest,
            success: true,
        })
    }

    /// Prune build cache via `buildctl prune`.
    pub async fn prune(&self) -> Result<()> {
        let output = tokio::process::Command::new("buildctl")
            .arg("--addr")
            .arg(&self.host)
            .arg("prune")
            .output()
            .await
            .map_err(|e| SunbeamError::ExternalTool {
                tool: "buildctl".into(),
                detail: format!("failed to spawn: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SunbeamError::ExternalTool {
                tool: "buildctl".into(),
                detail: format!("prune failed: {stderr}"),
            });
        }
        Ok(())
    }

    /// Get BuildKit daemon status via `buildctl debug workers`.
    pub async fn status(&self) -> Result<String> {
        let output = tokio::process::Command::new("buildctl")
            .arg("--addr")
            .arg(&self.host)
            .arg("debug")
            .arg("workers")
            .output()
            .await
            .map_err(|e| SunbeamError::ExternalTool {
                tool: "buildctl".into(),
                detail: format!("failed to spawn: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SunbeamError::ExternalTool {
                tool: "buildctl".into(),
                detail: format!("status failed: {stderr}"),
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Extract a sha256 digest from buildctl output.
fn extract_digest(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(pos) = line.find("sha256:") {
            // Grab the sha256:hex portion
            let rest = &line[pos..];
            let digest: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == ':')
                .collect();
            if digest.len() > 7 {
                return Some(digest);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let c = BuildKitClient::new("unix:///run/buildkit/buildkitd.sock");
        assert_eq!(c.host, "unix:///run/buildkit/buildkitd.sock");
    }

    #[test]
    fn test_build_args_construction() {
        let args = BuildArgs {
            frontend: "dockerfile.v0".into(),
            local_context: "/src".into(),
            local_dockerfile: "/src".into(),
            output: "type=image,name=registry.example.com/app:latest,push=true".into(),
            build_args: vec![
                ("VERSION".into(), "1.0".into()),
                ("DEBUG".into(), "false".into()),
            ],
        };
        assert_eq!(args.frontend, "dockerfile.v0");
        assert_eq!(args.build_args.len(), 2);
        assert_eq!(args.build_args[0], ("VERSION".into(), "1.0".into()));
    }

    #[test]
    fn test_build_output() {
        let out = BuildOutput {
            digest: "sha256:abc123".into(),
            success: true,
        };
        assert!(out.success);
        assert_eq!(out.digest, "sha256:abc123");
    }

    #[test]
    fn test_extract_digest() {
        assert_eq!(
            extract_digest("exporting manifest sha256:abc123def456 done"),
            Some("sha256:abc123def456".into())
        );
        assert_eq!(extract_digest("no digest here"), None);
        assert_eq!(extract_digest(""), None);
        assert_eq!(
            extract_digest("sha256:deadbeef"),
            Some("sha256:deadbeef".into())
        );
    }

    #[test]
    fn test_extract_digest_multiline() {
        let output = "step 1/5: done\nresolving sha256:aabbccdd0011223344\nfinished";
        assert_eq!(
            extract_digest(output),
            Some("sha256:aabbccdd0011223344".into())
        );
    }

    #[tokio::test]
    async fn test_build_missing_buildctl() {
        let c = BuildKitClient::new("unix:///nonexistent.sock");
        let args = BuildArgs {
            frontend: "dockerfile.v0".into(),
            local_context: "/tmp".into(),
            local_dockerfile: "/tmp".into(),
            output: "type=image,name=test,push=false".into(),
            build_args: vec![],
        };
        // This will either fail because buildctl is not found or because
        // the socket doesn't exist — either way it should be an error.
        let result = c.build(&args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_prune_missing_buildctl() {
        let c = BuildKitClient::new("unix:///nonexistent.sock");
        let result = c.prune().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_status_missing_buildctl() {
        let c = BuildKitClient::new("unix:///nonexistent.sock");
        let result = c.status().await;
        assert!(result.is_err());
    }

    // ── extract_digest edge cases ──────────────────────────────────

    #[test]
    fn test_extract_digest_bare_prefix_too_short() {
        // "sha256:" alone is 7 chars — must be > 7 to count.
        assert_eq!(extract_digest("sha256:"), None);
    }

    #[test]
    fn test_extract_digest_min_valid_length() {
        // 8 chars total ("sha256:a") should be accepted.
        assert_eq!(extract_digest("sha256:a"), Some("sha256:a".into()));
    }

    #[test]
    fn test_extract_digest_stops_at_non_alnum() {
        // Trailing space / punctuation must be excluded.
        assert_eq!(
            extract_digest("digest=sha256:aabb1122 done"),
            Some("sha256:aabb1122".into())
        );
        assert_eq!(
            extract_digest("sha256:aabb1122,size=42"),
            Some("sha256:aabb1122".into())
        );
    }

    #[test]
    fn test_extract_digest_first_match_wins() {
        let output = "sha256:first111\nsha256:second22";
        assert_eq!(extract_digest(output), Some("sha256:first111".into()));
    }

    #[test]
    fn test_extract_digest_mid_line() {
        assert_eq!(
            extract_digest("prefix sha256:cafe0123 suffix"),
            Some("sha256:cafe0123".into())
        );
    }

    #[test]
    fn test_extract_digest_sha256_only_colon_no_hex() {
        // Contains "sha256:" but nothing alphanumeric follows — still 7 chars, rejected.
        assert_eq!(extract_digest("sha256: "), None);
    }

    // ── BuildArgs field combinations ───────────────────────────────

    #[test]
    fn test_build_args_empty_extras() {
        let args = BuildArgs {
            frontend: "gateway.v0".into(),
            local_context: "/app".into(),
            local_dockerfile: "/app/docker".into(),
            output: "type=oci,dest=/tmp/out.tar".into(),
            build_args: vec![],
        };
        assert!(args.build_args.is_empty());
        assert_eq!(args.frontend, "gateway.v0");
        assert_eq!(args.local_context, "/app");
        assert_eq!(args.local_dockerfile, "/app/docker");
        assert_eq!(args.output, "type=oci,dest=/tmp/out.tar");
    }

    #[test]
    fn test_build_args_clone() {
        let args = BuildArgs {
            frontend: "dockerfile.v0".into(),
            local_context: "/src".into(),
            local_dockerfile: "/src".into(),
            output: "type=image".into(),
            build_args: vec![("K".into(), "V".into())],
        };
        let cloned = args.clone();
        assert_eq!(cloned.frontend, args.frontend);
        assert_eq!(cloned.build_args, args.build_args);
    }

    #[test]
    fn test_build_args_debug() {
        let args = BuildArgs {
            frontend: "dockerfile.v0".into(),
            local_context: ".".into(),
            local_dockerfile: ".".into(),
            output: "type=image".into(),
            build_args: vec![],
        };
        let dbg = format!("{:?}", args);
        assert!(dbg.contains("dockerfile.v0"));
    }

    // ── BuildOutput states ─────────────────────────────────────────

    #[test]
    fn test_build_output_failure_state() {
        let out = BuildOutput {
            digest: String::new(),
            success: false,
        };
        assert!(!out.success);
        assert!(out.digest.is_empty());
    }

    #[test]
    fn test_build_output_clone() {
        let out = BuildOutput {
            digest: "sha256:abc".into(),
            success: true,
        };
        let c = out.clone();
        assert_eq!(c.digest, out.digest);
        assert_eq!(c.success, out.success);
    }

    #[test]
    fn test_build_output_debug() {
        let out = BuildOutput {
            digest: "sha256:abc".into(),
            success: true,
        };
        let dbg = format!("{:?}", out);
        assert!(dbg.contains("sha256:abc"));
        assert!(dbg.contains("true"));
    }

    // ── Error message content ──────────────────────────────────────

    #[tokio::test]
    async fn test_build_error_contains_tool_name() {
        let c = BuildKitClient::new("unix:///nonexistent.sock");
        let args = BuildArgs {
            frontend: "dockerfile.v0".into(),
            local_context: "/tmp".into(),
            local_dockerfile: "/tmp".into(),
            output: "type=image,name=test,push=false".into(),
            build_args: vec![("A".into(), "B".into())],
        };
        let err = c.build(&args).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("buildctl"), "error should mention buildctl: {msg}");
    }

    #[tokio::test]
    async fn test_prune_error_contains_tool_name() {
        let c = BuildKitClient::new("unix:///nonexistent.sock");
        let err = c.prune().await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("buildctl"), "error should mention buildctl: {msg}");
    }

    #[tokio::test]
    async fn test_status_error_contains_tool_name() {
        let c = BuildKitClient::new("unix:///nonexistent.sock");
        let err = c.status().await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("buildctl"), "error should mention buildctl: {msg}");
    }

    // ── BuildKitClient misc ────────────────────────────────────────

    #[test]
    fn test_new_preserves_host_verbatim() {
        let c = BuildKitClient::new("tcp://10.0.0.1:1234");
        assert_eq!(c.host, "tcp://10.0.0.1:1234");
    }

    #[test]
    fn test_new_empty_host() {
        let c = BuildKitClient::new("");
        assert_eq!(c.host, "");
    }

    // ── build() with build_args exercises the --opt loop ───────────

    #[tokio::test]
    async fn test_build_with_multiple_build_args_errors() {
        // Exercises the for-loop that appends --opt build-arg:K=V flags.
        // buildctl is absent so we just confirm it errors properly.
        let c = BuildKitClient::new("unix:///nonexistent.sock");
        let args = BuildArgs {
            frontend: "dockerfile.v0".into(),
            local_context: "/tmp".into(),
            local_dockerfile: "/tmp".into(),
            output: "type=image,name=test,push=false".into(),
            build_args: vec![
                ("RUST_LOG".into(), "debug".into()),
                ("CI".into(), "true".into()),
                ("VERSION".into(), "2.0.0".into()),
            ],
        };
        let err = c.build(&args).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("buildctl"));
        assert!(msg.contains("failed to spawn") || msg.contains("build failed"));
    }
}
