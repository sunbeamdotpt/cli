//! Integration tests for the `sdk::profiles` + `sdk::manifest_params` +
//! `sdk::manifests` pipeline, driven by a real `kustomize build` over a
//! temporary fixture tree.
//!
//! No containers required — but a `kustomize` binary must be on PATH (the
//! fixture discovery path shells out to it, exactly like the CLI does).
//! The container-dependent suites skip without Docker; this one skips
//! without kustomize.

mod common;

use std::collections::HashMap;
use std::process::Command;

fn kustomize_available() -> bool {
    Command::new("kustomize")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Write a two-namespace kustomize fixture: gitea (devtools) with a `scale`
/// tunable and postgres (data) with no tunables.
fn write_fixture(base: &std::path::Path) {
    let gitea = base.join("devtools");
    std::fs::create_dir_all(&gitea).unwrap();
    std::fs::write(
        gitea.join("kustomization.yaml"),
        "resources:\n  - deployment.yaml\n",
    )
    .unwrap();
    std::fs::write(
        gitea.join("deployment.yaml"),
        r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: gitea
  namespace: devtools
  annotations:
    sunbeam.pt/tunable: |
      scale: integer
spec:
  replicas: 1
  selector:
    matchLabels:
      app: gitea
  template:
    metadata:
      labels:
        app: gitea
    spec:
      containers:
        - name: gitea
          image: gitea/gitea:1
"#,
    )
    .unwrap();

    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("kustomization.yaml"),
        "resources:\n  - statefulset.yaml\n",
    )
    .unwrap();
    std::fs::write(
        data.join("statefulset.yaml"),
        r#"apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: postgres
  namespace: data
spec:
  serviceName: postgres
  replicas: 1
  selector:
    matchLabels:
      app: postgres
  template:
    metadata:
      labels:
        app: postgres
    spec:
      containers:
        - name: postgres
          image: postgres:17
"#,
    )
    .unwrap();
}

const PROFILE_YAML: &str = r#"
presets:
  small:
    scale: 2
rules:
  - resource: gitea
    preset: small
  - resource: postgres
    namespace: data
    kind: StatefulSet
    scale: 3
"#;

/// Discovery + validation + resolution over the kustomize-built fixture.
#[tokio::test]
async fn profile_pipeline_over_kustomize_fixture() {
    if !kustomize_available() {
        eprintln!("skipping profiles: no kustomize binary on PATH");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    write_fixture(&base);

    // 1. Discovery: shells out to `kustomize build` per namespace directory
    //    and parses tunable annotations.
    let resources = sdk::profiles::discover_manifests(&base)
        .await
        .expect("discover_manifests");
    assert_eq!(resources.len(), 2, "resources: {resources:?}");

    let gitea = resources
        .iter()
        .find(|r| r.name == "gitea")
        .expect("gitea resource");
    assert_eq!(gitea.kind, "Deployment");
    assert_eq!(gitea.namespace, "devtools");
    assert!(
        gitea.tunables.contains_key("scale"),
        "tunables: {:?}",
        gitea.tunables
    );

    // 2. Load + validate a profile against the discovered resources.
    let profile_path = tmp.path().join("ci.yaml");
    std::fs::write(&profile_path, PROFILE_YAML).unwrap();
    let profile = sdk::profiles::load_profile(&profile_path).expect("load_profile");

    sdk::profiles::validate_profile(&profile, &HashMap::new(), &resources)
        .expect("profile should validate");

    // 3. Resolve to Overrides: preset expansion (scale=2) and an explicit
    //    rule shortcut (scale=3) both become Set overrides.
    let overrides = sdk::profiles::resolve_profile_overrides(&profile, &HashMap::new(), &resources)
        .expect("resolve_profile_overrides");
    let sets: Vec<_> = overrides
        .items
        .iter()
        .filter_map(|o| match o {
            sdk::manifest_params::Override::Set {
                resource,
                field_path,
                value,
            } => Some((resource.clone(), field_path.clone(), value.clone())),
            _ => None,
        })
        .collect();
    assert!(
        sets.contains(&(
            "deployment/devtools/gitea".to_string(),
            "spec/replicas".to_string(),
            "2".to_string()
        )),
        "sets: {sets:?}"
    );
    assert!(
        sets.contains(&(
            "statefulset/data/postgres".to_string(),
            "spec/replicas".to_string(),
            "3".to_string()
        )),
        "sets: {sets:?}"
    );
}

/// A profile that references an undeclared shortcut must fail validation.
#[tokio::test]
async fn profile_with_undeclared_shortcut_fails_validation() {
    if !kustomize_available() {
        eprintln!("skipping profiles: no kustomize binary on PATH");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    write_fixture(&base);
    let resources = sdk::profiles::discover_manifests(&base)
        .await
        .expect("discover_manifests");

    let profile_path = tmp.path().join("bad.yaml");
    std::fs::write(
        &profile_path,
        "rules:\n  - resource: gitea\n    bogus_shortcut: 5\n",
    )
    .unwrap();
    let profile = sdk::profiles::load_profile(&profile_path).expect("load_profile");

    let err = sdk::profiles::validate_profile(&profile, &HashMap::new(), &resources)
        .expect_err("undeclared shortcut must fail validation");
    assert!(
        err.to_string().contains("bogus_shortcut"),
        "unexpected error: {err}"
    );
}

/// The CLI override surface: `--set` / `--disable` parsing, `is_disabled`
/// matching, and namespace filtering over built manifest text.
#[test]
fn overrides_and_namespace_filtering() {
    // Two documents as kustomize would emit them.
    let manifests = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: gitea
  namespace: devtools
spec:
  replicas: 1
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: settings
  namespace: devtools
---
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: postgres
  namespace: data
spec:
  replicas: 1
"#;

    // --set / --disable parsing.
    let overrides = sdk::manifest_params::Overrides::from_cli(
        &["deployment/devtools/gitea/spec/replicas=4".to_string()],
        &["configmap/*".to_string()],
        &[],
    )
    .expect("from_cli");
    assert!(overrides.is_disabled("ConfigMap", "devtools", "settings"));
    assert!(!overrides.is_disabled("Deployment", "devtools", "gitea"));

    // Malformed --set values are rejected.
    assert!(
        sdk::manifest_params::Overrides::from_cli(&["no-equals".to_string()], &[], &[]).is_err()
    );
    assert!(
        sdk::manifest_params::Overrides::from_cli(&["too/few=1".to_string()], &[], &[]).is_err()
    );

    // Applying the override changes only the targeted document.
    let patched = sdk::manifest_params::apply_overrides(manifests, &overrides).expect("apply");
    assert!(patched.contains("replicas: 4"), "patched:\n{patched}");
    assert!(
        !patched.contains("name: settings"),
        "disabled configmap should be filtered:\n{patched}"
    );

    // Namespace filtering keeps only the requested namespace's documents.
    let filtered = sdk::manifests::filter_by_namespace(manifests, "data", &[]);
    assert!(filtered.contains("name: postgres"));
    assert!(!filtered.contains("name: gitea"));
}
