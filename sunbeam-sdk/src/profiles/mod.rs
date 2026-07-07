//! Sunbeam Profiles — manifest-anchored override system.
//!
//! Profiles replace kustomize overlays and hardcoded environment-specific blocks
//! with a declarative override system. Environment-specific tweaks live in
//! `~/.sunbeam/config.json` or `infra/sbbb/profiles/`, not in `infra/sbbb/overlays/`.
//!
//! ## One-sentence mental model
//!
//! > A profile is a **preset** for `--set` and `--disable`, validated against
//! > tunables declared in the base manifests.
//!
//! ## Module map
//!
//! | Module | Purpose |
//! |--------|---------|
//! | `tunables` | Parse `sunbeam.pt/tunable` annotations from manifest YAML |
//! | `shortcuts` | Expand shortcut names (`memory`, `scale`, `env.FOO`) to field paths |
//! | `resolver` | Resolve a profile + presets into `manifest_params::Overrides` |
//! | `validate` | Validate profile rules against declared tunables |

pub mod resolver;
pub mod shortcuts;
pub mod tunables;
pub mod validate;

pub use tunables::Tunable;

use crate::config::{Preset, Profile};
use crate::error::Result;
use crate::manifest_params::Overrides;
use std::collections::HashMap;

/// A discovered resource with its parsed tunables.
#[derive(Debug, Clone)]
pub struct ManifestResource {
    /// Kubernetes kind, e.g. "Deployment".
    pub kind: String,
    /// Resource name from `metadata.name`.
    pub name: String,
    /// Resource namespace from `metadata.namespace`.
    pub namespace: String,
    /// Parsed tunables from `sunbeam.pt/tunable` annotation.
    pub tunables: HashMap<String, tunables::Tunable>,
    /// The raw manifest document as JSON (for path resolution).
    pub doc: serde_json::Value,
}

/// Load a profile from a YAML file on disk.
pub fn load_profile(path: &std::path::Path) -> Result<Profile> {
    let content = std::fs::read_to_string(path).map_err(|e| crate::error::SunbeamError::Io {
        context: format!("read profile {}", path.display()),
        source: e,
    })?;
    serde_yaml::from_str(&content).map_err(|e| {
        crate::error::SunbeamError::Config(format!("parse profile {}: {e}", path.display()))
    })
}

/// Discover all manifest resources under the given base directory.
///
/// Scans `<base_dir>/*/` for `kustomization.yaml` files, runs
/// `kustomize build --enable-helm` for each, and parses tunable annotations.
pub async fn discover_manifests(base_dir: &std::path::Path) -> Result<Vec<ManifestResource>> {
    let mut resources = Vec::new();

    let entries = std::fs::read_dir(base_dir).map_err(|e| crate::error::SunbeamError::Io {
        context: format!("read base dir: {}", base_dir.display()),
        source: e,
    })?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let has_kustomization =
            path.join("kustomization.yaml").is_file() || path.join("kustomization.yml").is_file();
        if !has_kustomization {
            continue;
        }

        // Build this namespace's manifests
        let manifests = match crate::kube::kustomize_build(&path, "", "").await {
            Ok(m) => m,
            Err(e) => {
                tracing::info!("Skipping {}: kustomize build failed: {e}", path.display());
                continue;
            }
        };

        for doc in manifests.split("\n---") {
            let doc = doc.trim();
            if doc.is_empty() {
                continue;
            }
            let Ok(value): std::result::Result<serde_json::Value, _> = serde_yaml::from_str(doc)
            else {
                continue;
            };

            let Some(kind) = value.get("kind").and_then(|v| v.as_str()) else {
                continue;
            };
            let metadata = value.get("metadata").and_then(|v| v.as_object());
            let name = metadata
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let namespace = metadata
                .and_then(|m| m.get("namespace"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }

            let tunables = metadata
                .and_then(|m| m.get("annotations"))
                .and_then(|v| v.as_object())
                .and_then(|anns| anns.get("sunbeam.pt/tunable"))
                .and_then(|v| v.as_str())
                .map(tunables::parse_tunable_annotation)
                .transpose()?
                .unwrap_or_default();

            resources.push(ManifestResource {
                kind: kind.to_string(),
                name,
                namespace,
                tunables,
                doc: value,
            });
        }
    }

    Ok(resources)
}

/// Resolve a profile into overrides, given discovered resources and global presets.
pub fn resolve_profile_overrides(
    profile: &Profile,
    global_presets: &HashMap<String, Preset>,
    resources: &[ManifestResource],
) -> Result<Overrides> {
    resolver::resolve(profile, global_presets, resources)
}

/// Validate a profile against discovered resources.
pub fn validate_profile(
    profile: &Profile,
    global_presets: &HashMap<String, Preset>,
    resources: &[ManifestResource],
) -> Result<()> {
    validate::validate(profile, global_presets, resources)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_resource_default() {
        let r = ManifestResource {
            kind: "Deployment".to_string(),
            name: "test".to_string(),
            namespace: "default".to_string(),
            tunables: HashMap::new(),
            doc: serde_json::json!({"kind": "Deployment"}),
        };
        assert_eq!(r.kind, "Deployment");
        assert!(r.tunables.is_empty());
    }
}
