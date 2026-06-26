//! Profile + preset + rule → `Overrides` resolution.

use crate::config::{Preset, Profile, Rule};
use crate::error::{Result, SunbeamError};
use crate::manifest_params::{Override, Overrides};
use std::collections::HashMap;

use super::ManifestResource;
use super::shortcuts::expand_shortcut;

/// Resolve a profile into a collection of overrides.
///
/// 1. For each rule, resolve the resource by name (+ namespace/kind disambiguation).
/// 2. If the rule has a `preset`, expand it (profile-scoped presets shadow global).
/// 3. Merge explicit rule shortcuts on top of preset values (explicit wins).
/// 4. For each merged shortcut, look up the tunable and expand to field paths.
/// 5. Return all overrides.
pub fn resolve(
    profile: &Profile,
    global_presets: &HashMap<String, Preset>,
    resources: &[ManifestResource],
) -> Result<Overrides> {
    let mut items = Vec::new();

    for rule in &profile.rules {
        let resource = resolve_resource(rule, resources)?;
        let addr = format!(
            "{}/{}/{}",
            resource.kind.to_lowercase(),
            resource.namespace,
            resource.name
        );

        // 1. Start with preset values (if any)
        let mut values: HashMap<String, serde_json::Value> = if let Some(preset_name) = &rule.preset
        {
            let preset = profile
                .presets
                .get(preset_name)
                .or_else(|| global_presets.get(preset_name))
                .ok_or_else(|| {
                    SunbeamError::Config(format!(
                        "preset '{}' not found (rule for '{}')",
                        preset_name, rule.resource
                    ))
                })?;
            preset.values.clone()
        } else {
            HashMap::new()
        };

        // 2. Merge explicit shortcuts on top (explicit wins)
        for (k, v) in &rule.shortcuts {
            values.insert(k.clone(), v.clone());
        }

        // 3. Merge nested container shortcuts
        for (container_name, cs) in &rule.containers {
            if let Some(mem) = &cs.memory {
                values.insert(
                    format!("containers.{container_name}.memory"),
                    serde_json::json!(mem),
                );
            }
            if let Some(cpu) = &cs.cpu {
                values.insert(
                    format!("containers.{container_name}.cpu"),
                    serde_json::json!(cpu),
                );
            }
            for (env_name, env_val) in &cs.env {
                values.insert(
                    format!("containers.{container_name}.env.{env_name}"),
                    env_val.clone(),
                );
            }
        }

        // 4. Merge volume shortcuts
        for (vol_name, vol_val) in &rule.volumes {
            values.insert(format!("volumes.{vol_name}"), vol_val.clone());
        }

        // 5. Merge top-level env shortcuts
        for (env_name, env_val) in &rule.env {
            values.insert(format!("env.{env_name}"), env_val.clone());
        }

        // 6. Expand each shortcut into overrides
        for (shortcut, value) in values {
            let tunable = resource.tunables.get(&shortcut).or_else(|| {
                // config.<NAME> shortcuts map to the "config" tunable
                if shortcut.starts_with("config.") {
                    resource.tunables.get("config")
                } else {
                    None
                }
            });
            let tunable_path = tunable.and_then(|t| t.path.as_deref());

            // If no tunable and no explicit path, this shortcut is not allowed
            if tunable.is_none() && tunable_path.is_none() {
                // Some shortcuts have in-tree defaults even without a tunable
                // (they're baked into the shortcut expansion logic). We allow
                // those through — the expand_shortcut function will validate
                // kind support.
            }

            let mut overrides = expand_shortcut(
                &addr,
                &shortcut,
                &value,
                &resource.kind,
                tunable_path,
                &resource.doc,
            )?;
            items.append(&mut overrides);
        }
    }

    Ok(Overrides { items })
}

/// Resolve a rule's `resource` field to a concrete `ManifestResource`.
///
/// Resolution order:
/// 1. Exact match on name + namespace + kind (if all provided)
/// 2. Match on name + namespace
/// 3. Match on name + kind
/// 4. Match on name alone (error if ambiguous)
fn resolve_resource<'a>(
    rule: &Rule,
    resources: &'a [ManifestResource],
) -> Result<&'a ManifestResource> {
    let name = &rule.resource;
    let ns = rule.namespace.as_deref();
    let kind = rule.kind.as_deref();

    // 1. Exact match
    if let Some(n) = ns {
        if let Some(k) = kind {
            if let Some(r) = resources
                .iter()
                .find(|r| r.name == *name && r.namespace == *n && r.kind == *k)
            {
                return Ok(r);
            }
        }
    }

    // 2. Name + namespace + kind filter
    // If the rule specifies a kind, only consider resources of that kind
    // even when matching by namespace. This prevents a ServiceAccount from
    // shadowing a Deployment when the Deployment lacks a namespace field.
    if let Some(n) = ns {
        let matches: Vec<_> = resources
            .iter()
            .filter(|r| r.name == *name && r.namespace == *n && kind.map_or(true, |k| r.kind == k))
            .collect();
        if matches.len() == 1 {
            return Ok(matches[0]);
        }
    }

    // 3. Name + kind (namespace may be empty due to Helm charts)
    if let Some(k) = kind {
        let matches: Vec<_> = resources
            .iter()
            .filter(|r| r.name == *name && r.kind == *k)
            .collect();
        if matches.len() == 1 {
            return Ok(matches[0]);
        }
    }

    // 4. Name alone
    let matches: Vec<_> = resources.iter().filter(|r| r.name == *name).collect();
    match matches.len() {
        0 => Err(SunbeamError::Config(format!(
            "resource '{}' not found in manifests",
            name
        ))),
        1 => Ok(matches[0]),
        _ => Err(SunbeamError::Config(format!(
            "resource '{}' is ambiguous ({} matches) — disambiguate with --namespace or --kind",
            name,
            matches.len()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ContainerShortcuts, Preset, Profile, Rule};
    use crate::profiles::Tunable;
    use serde_json::Value;
    use std::collections::HashMap;

    fn make_resource_with_doc(
        name: &str,
        namespace: &str,
        kind: &str,
        tunables: HashMap<String, Tunable>,
        doc: serde_json::Value,
    ) -> ManifestResource {
        ManifestResource {
            kind: kind.to_string(),
            name: name.to_string(),
            namespace: namespace.to_string(),
            tunables,
            doc,
        }
    }

    fn make_resource(
        name: &str,
        namespace: &str,
        kind: &str,
        tunables: HashMap<String, Tunable>,
    ) -> ManifestResource {
        make_resource_with_doc(
            name,
            namespace,
            kind,
            tunables,
            serde_json::json!({
                "kind": kind,
                "metadata": { "name": name, "namespace": namespace },
                "spec": {
                    "replicas": 1,
                    "template": {
                        "spec": {
                            "containers": [
                                {"name": "main", "image": "busybox"}
                            ]
                        }
                    }
                }
            }),
        )
    }

    #[test]
    fn test_resolve_simple_rule() {
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

        let overrides = resolve(&profile, &HashMap::new(), &resources).unwrap();
        assert_eq!(overrides.items.len(), 1);
        assert_eq!(
            overrides.items[0],
            Override::Set {
                resource: "deployment/devtools/gitea".into(),
                field_path: "spec/replicas".into(),
                value: "3".into(),
            }
        );
    }

    #[test]
    fn test_resolve_with_preset() {
        let mut tunables = HashMap::new();
        tunables.insert(
            "scale".to_string(),
            Tunable {
                type_hint: "integer".to_string(),
                path: None,
            },
        );
        tunables.insert(
            "memory".to_string(),
            Tunable {
                type_hint: "quantity".to_string(),
                path: None,
            },
        );

        let resources = vec![make_resource("gitea", "devtools", "Deployment", tunables)];

        let mut preset = Preset::default();
        preset
            .values
            .insert("scale".to_string(), Value::Number(1.into()));
        preset
            .values
            .insert("memory".to_string(), Value::String("512Mi".into()));

        let mut profile = Profile::default();
        profile.presets.insert("tiny".to_string(), preset);
        profile.rules.push(Rule {
            resource: "gitea".to_string(),
            namespace: None,
            kind: None,
            preset: Some("tiny".to_string()),
            shortcuts: HashMap::new(),
            containers: HashMap::new(),
            volumes: HashMap::new(),
            env: HashMap::new(),
        });

        let overrides = resolve(&profile, &HashMap::new(), &resources).unwrap();
        assert_eq!(overrides.items.len(), 3); // scale + memory req + memory lim
    }

    #[test]
    fn test_explicit_overrides_preset() {
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
        preset
            .values
            .insert("scale".to_string(), Value::Number(1.into()));

        let mut profile = Profile::default();
        profile.presets.insert("tiny".to_string(), preset);
        profile.rules.push(Rule {
            resource: "gitea".to_string(),
            namespace: None,
            kind: None,
            preset: Some("tiny".to_string()),
            shortcuts: {
                let mut m = HashMap::new();
                m.insert("scale".to_string(), Value::Number(5.into()));
                m
            },
            containers: HashMap::new(),
            volumes: HashMap::new(),
            env: HashMap::new(),
        });

        let overrides = resolve(&profile, &HashMap::new(), &resources).unwrap();
        assert_eq!(overrides.items.len(), 1);
        assert!(matches!(&overrides.items[0], Override::Set { value, .. } if value == "5"));
    }

    #[test]
    fn test_global_preset_shadowed_by_profile_preset() {
        let mut tunables = HashMap::new();
        tunables.insert(
            "scale".to_string(),
            Tunable {
                type_hint: "integer".to_string(),
                path: None,
            },
        );

        let resources = vec![make_resource("gitea", "devtools", "Deployment", tunables)];

        let mut global_preset = Preset::default();
        global_preset
            .values
            .insert("scale".to_string(), Value::Number(1.into()));

        let mut profile_preset = Preset::default();
        profile_preset
            .values
            .insert("scale".to_string(), Value::Number(2.into()));

        let mut profile = Profile::default();
        profile.presets.insert("tiny".to_string(), profile_preset);
        profile.rules.push(Rule {
            resource: "gitea".to_string(),
            namespace: None,
            kind: None,
            preset: Some("tiny".to_string()),
            shortcuts: HashMap::new(),
            containers: HashMap::new(),
            volumes: HashMap::new(),
            env: HashMap::new(),
        });

        let mut global_presets = HashMap::new();
        global_presets.insert("tiny".to_string(), global_preset);

        let overrides = resolve(&profile, &global_presets, &resources).unwrap();
        assert!(matches!(&overrides.items[0], Override::Set { value, .. } if value == "2")); // profile preset wins
    }

    #[test]
    fn test_missing_preset_errors() {
        let resources = vec![make_resource(
            "gitea",
            "devtools",
            "Deployment",
            HashMap::new(),
        )];

        let profile = Profile {
            rules: vec![Rule {
                resource: "gitea".to_string(),
                namespace: None,
                kind: None,
                preset: Some("nonexistent".to_string()),
                shortcuts: HashMap::new(),
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            }],
            ..Default::default()
        };

        let err = resolve(&profile, &HashMap::new(), &resources).unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn test_resource_not_found() {
        let resources = vec![];
        let profile = Profile {
            rules: vec![Rule {
                resource: "missing".to_string(),
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

        let err = resolve(&profile, &HashMap::new(), &resources).unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn test_ambiguous_resource() {
        let resources = vec![
            make_resource("gitea", "devtools", "Deployment", HashMap::new()),
            make_resource("gitea", "matrix", "Deployment", HashMap::new()),
        ];
        let profile = Profile {
            rules: vec![Rule {
                resource: "gitea".to_string(),
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

        let err = resolve(&profile, &HashMap::new(), &resources).unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
    }

    #[test]
    fn test_disambiguate_by_namespace() {
        let mut tunables = HashMap::new();
        tunables.insert(
            "scale".to_string(),
            Tunable {
                type_hint: "integer".to_string(),
                path: None,
            },
        );

        let resources = vec![
            make_resource("gitea", "devtools", "Deployment", tunables.clone()),
            make_resource("gitea", "matrix", "Deployment", tunables),
        ];
        let profile = Profile {
            rules: vec![Rule {
                resource: "gitea".to_string(),
                namespace: Some("devtools".to_string()),
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

        let overrides = resolve(&profile, &HashMap::new(), &resources).unwrap();
        assert!(
            matches!(&overrides.items[0], Override::Set { resource, .. } if resource == "deployment/devtools/gitea")
        );
    }

    #[test]
    fn test_container_shortcuts_in_rule() {
        let mut tunables = HashMap::new();
        tunables.insert(
            "containers.exporter.memory".to_string(),
            Tunable {
                type_hint: "quantity".to_string(),
                path: None,
            },
        );

        let doc = serde_json::json!({
            "kind": "Deployment",
            "metadata": { "name": "gitea", "namespace": "devtools" },
            "spec": {
                "replicas": 1,
                "template": {
                    "spec": {
                        "containers": [
                            {"name": "main", "image": "busybox"},
                            {"name": "exporter", "image": "prom/node-exporter"}
                        ]
                    }
                }
            }
        });
        let resources = vec![make_resource_with_doc(
            "gitea",
            "devtools",
            "Deployment",
            tunables,
            doc,
        )];

        let mut profile = Profile::default();
        let mut cs = ContainerShortcuts::default();
        cs.memory = Some("128Mi".to_string());
        let mut containers = HashMap::new();
        containers.insert("exporter".to_string(), cs);
        profile.rules.push(Rule {
            resource: "gitea".to_string(),
            namespace: None,
            kind: None,
            preset: None,
            shortcuts: HashMap::new(),
            containers,
            volumes: HashMap::new(),
            env: HashMap::new(),
        });

        let overrides = resolve(&profile, &HashMap::new(), &resources).unwrap();
        assert!(!overrides.items.is_empty());
    }
}
