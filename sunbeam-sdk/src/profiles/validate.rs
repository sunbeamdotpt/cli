//! Profile validation against declared tunables.

use crate::config::{Preset, Profile};
use crate::error::{Result, SunbeamError};
use std::collections::HashMap;

use super::resolver::resolve;
use super::ManifestResource;

/// Validate that every shortcut in every rule maps to a declared tunable
/// (or has an in-tree default), and that preset references resolve.
pub fn validate(
    profile: &Profile,
    global_presets: &HashMap<String, Preset>,
    resources: &[ManifestResource],
) -> Result<()> {
    // Attempt full resolution — any error is a validation failure.
    // The resolver already produces descriptive errors.
    let _overrides = resolve(profile, global_presets, resources)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Preset, Profile, Rule};
    use crate::profiles::tunables::Tunable;
    use serde_json::Value;
    use std::collections::HashMap;

    fn make_resource(
        name: &str,
        namespace: &str,
        kind: &str,
        tunables: HashMap<String, Tunable>,
    ) -> ManifestResource {
        ManifestResource {
            kind: kind.to_string(),
            name: name.to_string(),
            namespace: namespace.to_string(),
            tunables,
            doc: serde_json::json!({
                "kind": kind,
                "metadata": { "name": name, "namespace": namespace },
                "spec": {
                    "replicas": 1,
                    "template": {
                        "spec": {
                            "containers": [{"name": "main", "image": "busybox"}]
                        }
                    }
                }
            }),
        }
    }

    #[test]
    fn test_valid_profile_passes() {
        let mut tunables = HashMap::new();
        tunables.insert(
            "scale".to_string(),
            Tunable {
                type_hint: "integer".to_string(),
                path: None,
            },
        );

        let resources = vec![make_resource("gitea", "devtools", "Deployment", tunables)];
        let profile = Profile {
            rules: vec![Rule {
                resource: "gitea".to_string(),
                namespace: None,
                kind: None,
                preset: None,
                shortcuts: {
                    let mut m = HashMap::new();
                    m.insert("scale".to_string(), Value::Number(3.into()));
                    m
                },
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            }],
            ..Default::default()
        };

        assert!(validate(&profile, &HashMap::new(), &resources).is_ok());
    }

    #[test]
    fn test_missing_preset_fails() {
        let resources = vec![make_resource("gitea", "devtools", "Deployment", HashMap::new())];
        let profile = Profile {
            rules: vec![Rule {
                resource: "gitea".to_string(),
                namespace: None,
                kind: None,
                preset: Some("missing".to_string()),
                shortcuts: HashMap::new(),
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            }],
            ..Default::default()
        };

        let err = validate(&profile, &HashMap::new(), &resources).unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn test_resource_not_found_fails() {
        let resources = vec![];
        let profile = Profile {
            rules: vec![Rule {
                resource: "ghost".to_string(),
                namespace: None,
                kind: None,
                preset: None,
                shortcuts: HashMap::new(),
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            }],
            ..Default::default()
        };

        let err = validate(&profile, &HashMap::new(), &resources).unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn test_preset_with_valid_rule() {
        let mut tunables = HashMap::new();
        tunables.insert(
            "scale".to_string(),
            Tunable {
                type_hint: "integer".to_string(),
                path: None,
            },
        );

        let resources = vec![make_resource("gitea", "devtools", "Deployment", tunables)];

        let mut preset = Preset::default();
        preset.values.insert("scale".to_string(), Value::Number(0.into()));

        let mut profile = Profile::default();
        profile.presets.insert("off".to_string(), preset);
        profile.rules.push(Rule {
            resource: "gitea".to_string(),
            namespace: None,
            kind: None,
            preset: Some("off".to_string()),
            shortcuts: HashMap::new(),
            containers: HashMap::new(),
            volumes: HashMap::new(),
            env: HashMap::new(),
        });

        assert!(validate(&profile, &HashMap::new(), &resources).is_ok());
    }
}
