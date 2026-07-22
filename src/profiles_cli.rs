//! Profile CLI command dispatch.
//!
//! Handles all `sunbeam config profile <action>` subcommands.

#![allow(dead_code)]

use sdk::config::{self, Preset, Profile, Rule};
use sdk::error::{Result, SunbeamError};
use sdk::info;
use serde_json::Value;
use std::collections::HashMap;

/// Dispatch a profile subcommand.
pub async fn dispatch(logger: &sdk::logger::Logger, action: ProfileAction) -> Result<()> {
    let mut config = config::load_config();

    match action {
        ProfileAction::AddRule {
            profile,
            resource,
            namespace,
            kind,
            preset,
            file,
            shortcuts,
        } => {
            let rule = if let Some(path) = file {
                let content = std::fs::read_to_string(&path).map_err(|e| SunbeamError::Io {
                    context: format!("read rule file: {}", path.display()),
                    source: e,
                })?;
                serde_json::from_str(&content).map_err(SunbeamError::Json)?
            } else {
                build_rule_from_cli(&resource, namespace, kind, preset, &shortcuts)?
            };
            config.add_rule(&profile, rule);
            config::save_config(&config)?;
            info!(logger, "Added rule to profile", profile = profile);
        }

        ProfileAction::RmRule { profile, resource } => {
            if config.remove_rule(&profile, &resource) {
                config::save_config(&config)?;
                info!(
                    logger,
                    "Removed rule from profile",
                    resource = resource,
                    profile = profile
                );
            } else {
                info!(
                    logger,
                    "No rule for resource in profile",
                    resource = resource,
                    profile = profile
                );
            }
        }

        ProfileAction::AddPreset {
            profile,
            preset,
            file,
            values,
        } => {
            let p = if let Some(path) = file {
                let content = std::fs::read_to_string(&path).map_err(|e| SunbeamError::Io {
                    context: format!("read preset file: {}", path.display()),
                    source: e,
                })?;
                serde_json::from_str(&content).map_err(SunbeamError::Json)?
            } else {
                build_preset_from_cli(&values)?
            };
            config.add_preset(&profile, &preset, p);
            config::save_config(&config)?;
            info!(
                logger,
                "Added preset to profile",
                preset = preset,
                profile = profile
            );
        }

        ProfileAction::SetPreset {
            profile,
            preset,
            values,
        } => {
            let parsed = parse_cli_values(&values)?;
            config.set_preset(&profile, &preset, parsed);
            config::save_config(&config)?;
            info!(
                logger,
                "Updated preset in profile",
                preset = preset,
                profile = profile
            );
        }

        ProfileAction::RmPreset { profile, preset } => {
            if config.remove_preset(&profile, &preset) {
                config::save_config(&config)?;
                info!(
                    logger,
                    "Removed preset from profile",
                    preset = preset,
                    profile = profile
                );
            } else {
                info!(
                    logger,
                    "No preset in profile",
                    preset = preset,
                    profile = profile
                );
            }
        }

        ProfileAction::Cp { src, dst } => {
            if config.copy_profile(&src, &dst) {
                config::save_config(&config)?;
                info!(logger, "Copied profile", src = src, dst = dst);
            } else {
                return Err(SunbeamError::Config(format!("Profile '{src}' not found")));
            }
        }

        ProfileAction::Diff { a, b } => {
            let profile_a = config
                .profiles
                .get(&a)
                .ok_or_else(|| SunbeamError::Config(format!("Profile '{a}' not found")))?;
            let profile_b = config
                .profiles
                .get(&b)
                .ok_or_else(|| SunbeamError::Config(format!("Profile '{b}' not found")))?;
            print_profile_diff(&a, profile_a, &b, profile_b);
        }

        ProfileAction::Validate { profile } => {
            let profile_obj = config
                .profiles
                .get(&profile)
                .ok_or_else(|| SunbeamError::Config(format!("Profile '{profile}' not found")))?;
            let resources =
                sdk::profiles::discover_manifests(&config::get_infra_dir().join("base")).await?;
            sdk::profiles::validate_profile(profile_obj, &config.presets, &resources)?;
            println!("Profile '{profile}' is valid.");
        }

        ProfileAction::Discover { output } => {
            let resources =
                sdk::profiles::discover_manifests(&config::get_infra_dir().join("base")).await?;
            match output {
                OutputFormat::Yaml => print_discover_yaml(&resources),
                OutputFormat::Json => print_discover_json(&resources),
            }
        }
    }

    Ok(())
}

/// Profile subcommand actions.
#[derive(Debug, Clone)]
pub enum ProfileAction {
    /// Add a rule to a profile.
    AddRule {
        profile: String,
        resource: String,
        namespace: Option<String>,
        kind: Option<String>,
        preset: Option<String>,
        file: Option<std::path::PathBuf>,
        shortcuts: Vec<String>,
    },
    /// Remove a rule from a profile.
    RmRule { profile: String, resource: String },
    /// Add a preset to a profile.
    AddPreset {
        profile: String,
        preset: String,
        file: Option<std::path::PathBuf>,
        values: Vec<String>,
    },
    /// Update an existing preset in a profile.
    SetPreset {
        profile: String,
        preset: String,
        values: Vec<String>,
    },
    /// Remove a preset from a profile.
    RmPreset { profile: String, preset: String },
    /// Copy a profile.
    Cp { src: String, dst: String },
    /// Diff two profiles.
    Diff { a: String, b: String },
    /// Validate a profile against manifest tunables.
    Validate { profile: String },
    /// Discover all tunables across base manifests.
    Discover { output: OutputFormat },
}

/// Output format for `discover`.
#[derive(Debug, Clone, Copy, Default)]
pub enum OutputFormat {
    /// YAML output.
    #[default]
    Yaml,
    /// JSON output.
    Json,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_rule_from_cli(
    resource: &str,
    namespace: Option<String>,
    kind: Option<String>,
    preset: Option<String>,
    shortcuts: &[String],
) -> Result<Rule> {
    let mut rule = Rule {
        resource: resource.to_string(),
        namespace,
        kind,
        preset,
        shortcuts: HashMap::new(),
        containers: HashMap::new(),
        volumes: HashMap::new(),
        env: HashMap::new(),
    };

    for s in shortcuts {
        // Parse "key=value" or "key={json}"
        let (key, val_str) = s
            .split_once('=')
            .ok_or_else(|| SunbeamError::Config(format!("shortcut must be key=value: {s}")))?;
        let value = parse_value(val_str);
        rule.shortcuts.insert(key.to_string(), value);
    }

    Ok(rule)
}

fn build_preset_from_cli(values: &[String]) -> Result<Preset> {
    let parsed = parse_cli_values(values)?;
    Ok(Preset { values: parsed })
}

fn parse_cli_values(values: &[String]) -> Result<HashMap<String, Value>> {
    let mut result = HashMap::new();
    for s in values {
        let (key, val_str) = s
            .split_once('=')
            .ok_or_else(|| SunbeamError::Config(format!("value must be key=value: {s}")))?;
        result.insert(key.to_string(), parse_value(val_str));
    }
    Ok(result)
}

fn parse_value(s: &str) -> Value {
    if s.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if s.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if s.eq_ignore_ascii_case("null") {
        return Value::Null;
    }
    if let Ok(n) = s.parse::<i64>() {
        return Value::Number(n.into());
    }
    if let Ok(f) = s.parse::<f64>()
        && let Some(n) = serde_json::Number::from_f64(f)
    {
        return Value::Number(n);
    }
    if ((s.starts_with('{') && s.ends_with('}')) || (s.starts_with('[') && s.ends_with(']')))
        && let Ok(v) = serde_json::from_str(s)
    {
        return v;
    }
    Value::String(s.to_string())
}

fn print_profile_diff(name_a: &str, a: &Profile, name_b: &str, b: &Profile) {
    println!("--- {name_a}");
    println!("+++ {name_b}");

    let a_presets: HashMap<_, _> = a.presets.iter().collect();
    let b_presets: HashMap<_, _> = b.presets.iter().collect();

    for key in a_presets
        .keys()
        .chain(b_presets.keys())
        .collect::<std::collections::HashSet<_>>()
    {
        match (a_presets.get(key), b_presets.get(key)) {
            (Some(av), Some(bv)) => {
                if av.values != bv.values {
                    println!("! preset[{key}]");
                }
            }
            (Some(_), None) => println!("- preset[{key}]"),
            (None, Some(_)) => println!("+ preset[{key}]"),
            _ => {}
        }
    }

    let a_rules: HashMap<_, _> = a.rules.iter().map(|r| (&r.resource, r)).collect();
    let b_rules: HashMap<_, _> = b.rules.iter().map(|r| (&r.resource, r)).collect();

    for key in a_rules
        .keys()
        .chain(b_rules.keys())
        .collect::<std::collections::HashSet<_>>()
    {
        match (a_rules.get(key), b_rules.get(key)) {
            (Some(ar), Some(br)) => {
                if ar.shortcuts != br.shortcuts || ar.preset != br.preset {
                    println!("! rule[{key}]");
                }
            }
            (Some(_), None) => println!("- rule[{key}]"),
            (None, Some(_)) => println!("+ rule[{key}]"),
            _ => {}
        }
    }
}

fn print_discover_yaml(resources: &[sdk::profiles::ManifestResource]) {
    for r in resources {
        if r.tunables.is_empty() {
            continue;
        }
        println!("---");
        println!("# {}/{} in {}", r.kind, r.name, r.namespace);
        println!("resource: {}", r.name);
        if !r.namespace.is_empty() {
            println!("namespace: {}", r.namespace);
        }
        println!("kind: {}", r.kind);
        println!("tunables:");
        for (name, tunable) in &r.tunables {
            if let Some(path) = &tunable.path {
                println!("  {name}: {} @ {path}", tunable.type_hint);
            } else {
                println!("  {name}: {}", tunable.type_hint);
            }
        }
    }
}

fn print_discover_json(resources: &[sdk::profiles::ManifestResource]) {
    let mut out = Vec::new();
    for r in resources {
        if r.tunables.is_empty() {
            continue;
        }
        let mut tunables = HashMap::new();
        for (name, tunable) in &r.tunables {
            let mut entry = HashMap::new();
            entry.insert("type".to_string(), Value::String(tunable.type_hint.clone()));
            if let Some(path) = &tunable.path {
                entry.insert("path".to_string(), Value::String(path.clone()));
            }
            tunables.insert(name.clone(), Value::Object(entry.into_iter().collect()));
        }
        let mut obj = HashMap::new();
        obj.insert("resource".to_string(), Value::String(r.name.clone()));
        obj.insert("namespace".to_string(), Value::String(r.namespace.clone()));
        obj.insert("kind".to_string(), Value::String(r.kind.clone()));
        obj.insert(
            "tunables".to_string(),
            Value::Object(tunables.into_iter().collect()),
        );
        out.push(Value::Object(obj.into_iter().collect()));
    }
    let body = match serde_json::to_string_pretty(&Value::Array(out)) {
        Ok(s) => s,
        // Serializing a serde_json::Value is infallible.
        Err(_) => unreachable!(),
    };
    println!("{body}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_value() {
        assert_eq!(parse_value("true"), Value::Bool(true));
        assert_eq!(parse_value("42"), Value::Number(42i64.into()));
        assert_eq!(parse_value("hello"), Value::String("hello".into()));
        assert_eq!(parse_value("{\"a\":1}"), serde_json::json!({"a": 1}));
    }

    #[test]
    fn test_build_rule_from_cli() {
        let rule = build_rule_from_cli(
            "pingora",
            Some("ingress".to_string()),
            None,
            None,
            &["scale=0".to_string()],
        )
        .unwrap();
        assert_eq!(rule.resource, "pingora");
        assert_eq!(rule.namespace, Some("ingress".to_string()));
        assert_eq!(rule.shortcuts["scale"], Value::Number(0.into()));
    }

    #[test]
    fn test_build_preset_from_cli() {
        let preset =
            build_preset_from_cli(&["instances=1".to_string(), "memory=512Mi".to_string()])
                .unwrap();
        assert_eq!(preset.values["instances"], Value::Number(1.into()));
        assert_eq!(preset.values["memory"], Value::String("512Mi".into()));
    }

    #[test]
    fn test_parse_cli_values_invalid() {
        let err = parse_cli_values(&["noseparator".to_string()]).unwrap_err();
        assert!(err.to_string().contains("key=value"));
    }

    #[test]
    fn test_parse_value_variants() {
        assert_eq!(parse_value("FALSE"), Value::Bool(false));
        assert_eq!(parse_value("null"), Value::Null);
        assert_eq!(parse_value("1.5"), serde_json::json!(1.5));
        assert_eq!(parse_value("[1,2]"), serde_json::json!([1, 2]));
        // Looks like JSON but malformed → falls back to string.
        assert_eq!(parse_value("{broken"), Value::String("{broken".to_string()));
        assert_eq!(parse_value(""), Value::String(String::new()));
    }

    #[test]
    fn test_build_rule_from_cli_rejects_bad_shortcut() {
        let err =
            build_rule_from_cli("gitea", None, None, None, &["nokey".to_string()]).unwrap_err();
        assert!(err.to_string().contains("key=value"));
    }

    #[test]
    fn test_print_profile_diff_all_branches() {
        use sdk::config::{Preset, Profile, Rule};

        let preset = |v: i64| Preset {
            values: HashMap::from([("scale".to_string(), Value::Number(v.into()))]),
        };
        let rule = |resource: &str, scale: i64| Rule {
            resource: resource.to_string(),
            namespace: None,
            kind: None,
            preset: None,
            shortcuts: HashMap::from([("scale".to_string(), Value::Number(scale.into()))]),
            containers: HashMap::new(),
            volumes: HashMap::new(),
            env: HashMap::new(),
        };

        let a = Profile {
            presets: HashMap::from([
                ("changed".to_string(), preset(1)),
                ("removed".to_string(), preset(1)),
            ]),
            rules: vec![rule("changed", 1), rule("removed", 1)],
            ..Default::default()
        };
        let b = Profile {
            presets: HashMap::from([
                ("changed".to_string(), preset(2)),
                ("added".to_string(), preset(1)),
            ]),
            rules: vec![rule("changed", 2), rule("added", 1)],
            ..Default::default()
        };

        // Changed/removed/added for presets and rules — output goes to stdout.
        print_profile_diff("a", &a, "b", &b);
        // Identical profiles hit the no-difference branches.
        print_profile_diff("a", &a, "a2", &a);
    }

    // ---------------------------------------------------------------------
    // Dispatch tests with HOME redirected to a tempdir
    // ---------------------------------------------------------------------

    /// Redirect $HOME so sdk::config reads/writes a throwaway config.
    fn temp_home() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        dir
    }

    fn test_logger() -> sdk::logger::Logger {
        sdk::logger::Logger::new(sdk::logger::TracingSink)
    }

    #[tokio::test]
    async fn dispatch_add_rule_then_rm_rule() {
        let _home = temp_home();
        let logger = test_logger();

        dispatch(
            &logger,
            ProfileAction::AddRule {
                profile: "ci".into(),
                resource: "gitea".into(),
                namespace: Some("devtools".into()),
                kind: None,
                preset: None,
                file: None,
                shortcuts: vec!["scale=2".into()],
            },
        )
        .await
        .unwrap();

        let cfg = config::load_config();
        let profile = cfg.profiles.get("ci").expect("profile persisted");
        assert_eq!(profile.rules.len(), 1);
        assert_eq!(profile.rules[0].resource, "gitea");
        assert_eq!(profile.rules[0].shortcuts["scale"], Value::Number(2.into()));

        dispatch(
            &logger,
            ProfileAction::RmRule {
                profile: "ci".into(),
                resource: "gitea".into(),
            },
        )
        .await
        .unwrap();
        let cfg = config::load_config();
        assert!(cfg.profiles.get("ci").unwrap().rules.is_empty());

        // Removing again hits the not-found branch (still Ok).
        dispatch(
            &logger,
            ProfileAction::RmRule {
                profile: "ci".into(),
                resource: "gitea".into(),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dispatch_add_rule_from_file() {
        let _home = temp_home();
        let logger = test_logger();

        let rule_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            rule_file.path(),
            serde_json::to_string(&serde_json::json!({
                "resource": "postgres",
                "namespace": "data",
                "scale": 1
            }))
            .unwrap(),
        )
        .unwrap();

        dispatch(
            &logger,
            ProfileAction::AddRule {
                profile: "ci".into(),
                resource: "ignored".into(),
                namespace: None,
                kind: None,
                preset: None,
                file: Some(rule_file.path().to_path_buf()),
                shortcuts: vec![],
            },
        )
        .await
        .unwrap();

        let cfg = config::load_config();
        let profile = cfg.profiles.get("ci").unwrap();
        assert_eq!(profile.rules[0].resource, "postgres");
    }

    #[tokio::test]
    async fn dispatch_preset_lifecycle() {
        let _home = temp_home();
        let logger = test_logger();

        dispatch(
            &logger,
            ProfileAction::AddPreset {
                profile: "ci".into(),
                preset: "small".into(),
                file: None,
                values: vec!["scale=1".into()],
            },
        )
        .await
        .unwrap();
        dispatch(
            &logger,
            ProfileAction::SetPreset {
                profile: "ci".into(),
                preset: "small".into(),
                values: vec!["scale=2".into(), "memory=256Mi".into()],
            },
        )
        .await
        .unwrap();

        let cfg = config::load_config();
        let preset = &cfg.profiles.get("ci").unwrap().presets["small"];
        assert_eq!(preset.values["scale"], Value::Number(2.into()));
        assert_eq!(preset.values["memory"], Value::String("256Mi".into()));

        dispatch(
            &logger,
            ProfileAction::RmPreset {
                profile: "ci".into(),
                preset: "small".into(),
            },
        )
        .await
        .unwrap();
        let cfg = config::load_config();
        assert!(cfg.profiles.get("ci").unwrap().presets.is_empty());

        // Removing again hits the not-found branch (still Ok).
        dispatch(
            &logger,
            ProfileAction::RmPreset {
                profile: "ci".into(),
                preset: "small".into(),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dispatch_cp_and_diff() {
        let _home = temp_home();
        let logger = test_logger();

        dispatch(
            &logger,
            ProfileAction::AddRule {
                profile: "src-prof".into(),
                resource: "gitea".into(),
                namespace: None,
                kind: None,
                preset: None,
                file: None,
                shortcuts: vec!["scale=2".into()],
            },
        )
        .await
        .unwrap();
        dispatch(
            &logger,
            ProfileAction::Cp {
                src: "src-prof".into(),
                dst: "dst-prof".into(),
            },
        )
        .await
        .unwrap();

        let cfg = config::load_config();
        assert!(cfg.profiles.contains_key("dst-prof"));

        // Diff identical profiles (copies) plus a missing-profile error.
        dispatch(
            &logger,
            ProfileAction::Diff {
                a: "src-prof".into(),
                b: "dst-prof".into(),
            },
        )
        .await
        .unwrap();
        let err = dispatch(
            &logger,
            ProfileAction::Diff {
                a: "src-prof".into(),
                b: "nope".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("nope"), "err: {err}");

        let err = dispatch(
            &logger,
            ProfileAction::Cp {
                src: "nope".into(),
                dst: "x".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not found"), "err: {err}");
    }

    #[tokio::test]
    async fn dispatch_validate_and_discover() {
        let _home = temp_home();
        let logger = test_logger();

        // Infra fixture with one kustomization dir (resources discovered only
        // when kustomize is installed; discovery tolerates its absence).
        let infra = tempfile::tempdir().unwrap();
        let base = infra.path().join("base").join("devtools");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("kustomization.yaml"),
            "resources: []
",
        )
        .unwrap();
        sdk::config::set_active_context(sdk::config::Context {
            infra_dir: infra.path().to_string_lossy().to_string(),
            ..Default::default()
        });

        // Profile with no rules validates against any resource set.
        dispatch(
            &logger,
            ProfileAction::AddPreset {
                profile: "ci".into(),
                preset: "small".into(),
                file: None,
                values: vec!["scale=1".into()],
            },
        )
        .await
        .unwrap();

        // Missing profile errors.
        let err = dispatch(
            &logger,
            ProfileAction::Validate {
                profile: "nope".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("nope"), "err: {err}");

        // The Validate/Discover happy paths shell out to kustomize; the SDK
        // downloads it into ~/.sunbeam/bin with a blocking HTTP client, which
        // panics inside a tokio runtime. Pre-seed the tool cache from the
        // system kustomize when one is available; skip otherwise.
        let system_kustomize = std::process::Command::new("which")
            .arg("kustomize")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        if let Some(kustomize) = system_kustomize {
            let bin_dir = _home.path().join(".sunbeam/bin");
            std::fs::create_dir_all(&bin_dir).unwrap();
            std::fs::copy(&kustomize, bin_dir.join("kustomize")).unwrap();
            std::fs::copy(&kustomize, bin_dir.join("helm")).unwrap();

            dispatch(
                &logger,
                ProfileAction::Validate {
                    profile: "ci".into(),
                },
            )
            .await
            .unwrap();
            dispatch(
                &logger,
                ProfileAction::Discover {
                    output: OutputFormat::Yaml,
                },
            )
            .await
            .unwrap();
            dispatch(
                &logger,
                ProfileAction::Discover {
                    output: OutputFormat::Json,
                },
            )
            .await
            .unwrap();
        } else {
            eprintln!("skipping Validate/Discover happy path: no kustomize on PATH");
        }
    }

    #[test]
    fn test_print_discover_yaml_and_json() {
        use sdk::profiles::{ManifestResource, Tunable};

        let mut tunables = HashMap::new();
        tunables.insert(
            "scale".to_string(),
            Tunable {
                type_hint: "integer".to_string(),
                path: None,
            },
        );
        tunables.insert(
            "custom".to_string(),
            Tunable {
                type_hint: "string".to_string(),
                path: Some("spec.template.spec.hostAliases".to_string()),
            },
        );

        let with_tunables = ManifestResource {
            kind: "Deployment".to_string(),
            name: "gitea".to_string(),
            namespace: "devtools".to_string(),
            tunables,
            doc: serde_json::json!({}),
        };
        let without_tunables = ManifestResource {
            kind: "ConfigMap".to_string(),
            name: "settings".to_string(),
            namespace: "devtools".to_string(),
            tunables: HashMap::new(),
            doc: serde_json::json!({}),
        };

        let resources = vec![with_tunables, without_tunables];
        print_discover_yaml(&resources);
        print_discover_json(&resources);
    }
}
