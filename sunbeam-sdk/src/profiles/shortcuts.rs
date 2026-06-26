//! Shortcut → field path expansion.
//!
//! Shortcuts are resource-type-aware aliases for common Kubernetes field paths.
//! They are declared in `sunbeam.pt/tunable` annotations and expanded by the
//! CLI into full `Override::Set` objects.
//!
//! ## In-tree defaults (no `@` required)
//!
//! | Shortcut | Supported kinds | Default path |
//! |----------|-----------------|-------------|
//! | `scale` | Deployment, StatefulSet, ReplicaSet, DaemonSet | `spec/replicas` |
//! | `instances` | StatefulSet | `spec/instances` |
//! | `storage` | PersistentVolumeClaim | `spec/resources/requests/storage` |
//! | `memory` | Deployment, StatefulSet, DaemonSet, Job, CronJob | all containers: `requests.memory` + `limits.memory` |
//! | `cpu` | same | all containers: `requests.cpu` + `limits.cpu` |
//! | `env.<NAME>` | same | walk all containers' `env`, match by `name` |
//! | `ports` | Deployment, StatefulSet, DaemonSet, Job | first container with `ports` |
//! | `volumes.<NAME>` | Deployment, StatefulSet, DaemonSet, Job, Pod | `spec/volumes`, match by `name` |
//! | `containers.<NAME>.memory` | same | specific container: `resources/requests/memory` + `limits/memory` |
//! | `containers.<NAME>.cpu` | same | specific container: `resources/requests/cpu` + `limits/cpu` |
//! | `containers.<NAME>.env.<NAME>` | same | specific container's `env`, match by `name` |
//! | `image` | same | `spec/template/spec/containers/0/image` |
//! | `schedule` | CronJob | `spec/schedule` |
//! | `type` | Service | `spec/type` |
//!
//! ## CRD behavior
//!
//! Single-word shortcuts do NOT work on CRDs. CRDs must declare custom
//! shortcuts with explicit `@ spec.path.here` paths. `config.<NAME>` on a CRD
//! finds the key `<NAME>` in the object at the declared path.

use crate::error::{Result, SunbeamError};
use crate::manifest_params::Override;
use serde_json::Value;
use std::collections::HashMap;

/// Expand a shortcut key + value into one or more `Override::Set` objects.
///
/// `kind` is the Kubernetes kind (e.g. "Deployment").
/// `tunable_path` is the explicit path from the `@` syntax, if any.
/// `doc` is the manifest document (used for named searches).
pub fn expand_shortcut(
    resource_addr: &str,
    shortcut: &str,
    value: &Value,
    kind: &str,
    tunable_path: Option<&str>,
    doc: &Value,
) -> Result<Vec<Override>> {
    // If an explicit path was declared via `@`, use it directly.
    if let Some(path) = tunable_path {
        return expand_explicit_path(resource_addr, shortcut, value, kind, path, doc);
    }

    // Otherwise, use in-tree defaults.
    match shortcut {
        "scale" => expand_scale(resource_addr, value, kind),
        "instances" => expand_instances(resource_addr, value, kind),
        "storage" => expand_storage(resource_addr, value, kind),
        "memory" => expand_memory(resource_addr, value, kind, doc, None),
        "cpu" => expand_cpu(resource_addr, value, kind, doc, None),
        "ports" => expand_ports(resource_addr, value, kind, doc),
        "image" => expand_image(resource_addr, value, kind, doc),
        "schedule" => expand_schedule(resource_addr, value, kind),
        "type" => expand_type(resource_addr, value, kind),
        _ => {
            // Named shortcuts: env.<NAME>, volumes.<NAME>, containers.<NAME>.{memory,cpu,env.<NAME>}
            if let Some(rest) = shortcut.strip_prefix("env.") {
                expand_env_named(resource_addr, rest, value, kind, doc)
            } else if let Some(rest) = shortcut.strip_prefix("volumes.") {
                expand_volume_named(resource_addr, rest, value, kind, doc)
            } else if let Some(rest) = shortcut.strip_prefix("containers.") {
                expand_container_named(resource_addr, rest, value, kind, doc)
            } else {
                Err(SunbeamError::Config(format!(
                    "shortcut '{shortcut}' has no default for kind '{kind}' — add an explicit path with @"
                )))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// In-tree expansions
// ---------------------------------------------------------------------------

fn expand_scale(addr: &str, value: &Value, kind: &str) -> Result<Vec<Override>> {
    let supported = ["Deployment", "StatefulSet", "ReplicaSet", "DaemonSet"];
    if !supported.contains(&kind) {
        return Err(SunbeamError::Config(format!(
            "'scale' shortcut not supported for kind '{kind}'"
        )));
    }
    Ok(vec![Override::Set {
        resource: addr.into(),
        field_path: "spec/replicas".into(),
        value: json_to_string(value),
    }])
}

fn expand_instances(addr: &str, value: &Value, kind: &str) -> Result<Vec<Override>> {
    if kind != "StatefulSet" {
        return Err(SunbeamError::Config(format!(
            "'instances' shortcut only supported for StatefulSet, got '{kind}'"
        )));
    }
    Ok(vec![Override::Set {
        resource: addr.into(),
        field_path: "spec/instances".into(),
        value: json_to_string(value),
    }])
}

fn expand_storage(addr: &str, value: &Value, kind: &str) -> Result<Vec<Override>> {
    if kind != "PersistentVolumeClaim" {
        return Err(SunbeamError::Config(format!(
            "'storage' shortcut only supported for PersistentVolumeClaim, got '{kind}'"
        )));
    }
    Ok(vec![Override::Set {
        resource: addr.into(),
        field_path: "spec/resources/requests/storage".into(),
        value: json_to_string(value),
    }])
}

fn expand_schedule(addr: &str, value: &Value, kind: &str) -> Result<Vec<Override>> {
    if kind != "CronJob" {
        return Err(SunbeamError::Config(format!(
            "'schedule' shortcut only supported for CronJob, got '{kind}'"
        )));
    }
    Ok(vec![Override::Set {
        resource: addr.into(),
        field_path: "spec/schedule".into(),
        value: json_to_string(value),
    }])
}

fn expand_type(addr: &str, value: &Value, kind: &str) -> Result<Vec<Override>> {
    if kind != "Service" {
        return Err(SunbeamError::Config(format!(
            "'type' shortcut only supported for Service, got '{kind}'"
        )));
    }
    Ok(vec![Override::Set {
        resource: addr.into(),
        field_path: "spec/type".into(),
        value: json_to_string(value),
    }])
}

fn expand_image(addr: &str, value: &Value, kind: &str, _doc: &Value) -> Result<Vec<Override>> {
    let supported = ["Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob"];
    if !supported.contains(&kind) {
        return Err(SunbeamError::Config(format!(
            "'image' shortcut not supported for kind '{kind}'"
        )));
    }
    Ok(vec![Override::Set {
        resource: addr.into(),
        field_path: "spec/template/spec/containers/0/image".into(),
        value: json_to_string(value),
    }])
}

fn expand_ports(addr: &str, value: &Value, kind: &str, doc: &Value) -> Result<Vec<Override>> {
    let supported = ["Deployment", "StatefulSet", "DaemonSet", "Job"];
    if !supported.contains(&kind) {
        return Err(SunbeamError::Config(format!(
            "'ports' shortcut not supported for kind '{kind}'"
        )));
    }
    // Find first container with ports
    let containers = doc
        .get("spec")
        .and_then(|v| v.get("template"))
        .and_then(|v| v.get("spec"))
        .and_then(|v| v.get("containers"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| SunbeamError::Config("no containers found for ports shortcut".into()))?;

    for (idx, c) in containers.iter().enumerate() {
        if c.get("ports").is_some() {
            return Ok(vec![Override::Set {
                resource: addr.into(),
                field_path: format!("spec/template/spec/containers/{idx}/ports"),
                value: json_to_string(value),
            }]);
        }
    }

    // Fallback: set on first container
    Ok(vec![Override::Set {
        resource: addr.into(),
        field_path: "spec/template/spec/containers/0/ports".into(),
        value: json_to_string(value),
    }])
}

fn expand_memory(
    addr: &str,
    value: &Value,
    kind: &str,
    doc: &Value,
    container_name: Option<&str>,
) -> Result<Vec<Override>> {
    let supported = ["Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob"];
    if !supported.contains(&kind) {
        return Err(SunbeamError::Config(format!(
            "'memory' shortcut not supported for kind '{kind}'"
        )));
    }

    let val_str = json_to_string(value);
    let mut overrides = Vec::new();

    if let Some(name) = container_name {
        let idx = find_container_index(doc, name)?;
        overrides.push(Override::Set {
            resource: addr.into(),
            field_path: format!("spec/template/spec/containers/{idx}/resources/requests/memory"),
            value: val_str.clone(),
        });
        overrides.push(Override::Set {
            resource: addr.into(),
            field_path: format!("spec/template/spec/containers/{idx}/resources/limits/memory"),
            value: val_str,
        });
    } else {
        // Apply to ALL containers
        let containers = doc
            .get("spec")
            .and_then(|v| v.get("template"))
            .and_then(|v| v.get("spec"))
            .and_then(|v| v.get("containers"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                SunbeamError::Config("no containers found for memory shortcut".into())
            })?;

        for idx in 0..containers.len() {
            overrides.push(Override::Set {
                resource: addr.into(),
                field_path: format!(
                    "spec/template/spec/containers/{idx}/resources/requests/memory"
                ),
                value: val_str.clone(),
            });
            overrides.push(Override::Set {
                resource: addr.into(),
                field_path: format!("spec/template/spec/containers/{idx}/resources/limits/memory"),
                value: val_str.clone(),
            });
        }
    }

    Ok(overrides)
}

fn expand_cpu(
    addr: &str,
    value: &Value,
    kind: &str,
    doc: &Value,
    container_name: Option<&str>,
) -> Result<Vec<Override>> {
    let supported = ["Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob"];
    if !supported.contains(&kind) {
        return Err(SunbeamError::Config(format!(
            "'cpu' shortcut not supported for kind '{kind}'"
        )));
    }

    let val_str = json_to_string(value);
    let mut overrides = Vec::new();

    if let Some(name) = container_name {
        let idx = find_container_index(doc, name)?;
        overrides.push(Override::Set {
            resource: addr.into(),
            field_path: format!("spec/template/spec/containers/{idx}/resources/requests/cpu"),
            value: val_str.clone(),
        });
        overrides.push(Override::Set {
            resource: addr.into(),
            field_path: format!("spec/template/spec/containers/{idx}/resources/limits/cpu"),
            value: val_str,
        });
    } else {
        let containers = doc
            .get("spec")
            .and_then(|v| v.get("template"))
            .and_then(|v| v.get("spec"))
            .and_then(|v| v.get("containers"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| SunbeamError::Config("no containers found for cpu shortcut".into()))?;

        for idx in 0..containers.len() {
            overrides.push(Override::Set {
                resource: addr.into(),
                field_path: format!("spec/template/spec/containers/{idx}/resources/requests/cpu"),
                value: val_str.clone(),
            });
            overrides.push(Override::Set {
                resource: addr.into(),
                field_path: format!("spec/template/spec/containers/{idx}/resources/limits/cpu"),
                value: val_str.clone(),
            });
        }
    }

    Ok(overrides)
}

fn expand_env_named(
    addr: &str,
    var_name: &str,
    value: &Value,
    kind: &str,
    doc: &Value,
) -> Result<Vec<Override>> {
    let supported = ["Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob"];
    if !supported.contains(&kind) {
        return Err(SunbeamError::Config(format!(
            "'env' shortcut not supported for kind '{kind}'"
        )));
    }

    let containers = doc
        .get("spec")
        .and_then(|v| v.get("template"))
        .and_then(|v| v.get("spec"))
        .and_then(|v| v.get("containers"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| SunbeamError::Config("no containers found for env shortcut".into()))?;

    for (idx, c) in containers.iter().enumerate() {
        if let Some(env) = c.get("env").and_then(|v| v.as_array()) {
            for (env_idx, e) in env.iter().enumerate() {
                if e.get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|n| n == var_name)
                {
                    return Ok(vec![Override::Set {
                        resource: addr.into(),
                        field_path: format!(
                            "spec/template/spec/containers/{idx}/env/{env_idx}/value"
                        ),
                        value: json_to_string(value),
                    }]);
                }
            }
        }
    }

    // If not found, add to first container's env array
    let entry = serde_json::json!({
        "name": var_name,
        "value": json_to_string(value)
    });
    Ok(vec![Override::Set {
        resource: addr.into(),
        field_path: "spec/template/spec/containers/0/env".into(),
        value: serde_json::to_string(&vec![entry]).unwrap_or_default(),
    }])
}

fn expand_volume_named(
    addr: &str,
    vol_name: &str,
    value: &Value,
    kind: &str,
    doc: &Value,
) -> Result<Vec<Override>> {
    let supported = ["Deployment", "StatefulSet", "DaemonSet", "Job", "Pod"];
    if !supported.contains(&kind) {
        return Err(SunbeamError::Config(format!(
            "'volumes' shortcut not supported for kind '{kind}'"
        )));
    }

    let volumes = doc
        .get("spec")
        .and_then(|v| v.get("template"))
        .and_then(|v| v.get("spec"))
        .and_then(|v| v.get("volumes"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| SunbeamError::Config("no volumes found".into()))?;

    for (idx, vol) in volumes.iter().enumerate() {
        if vol
            .get("name")
            .and_then(|v| v.as_str())
            .is_some_and(|n| n == vol_name)
        {
            return Ok(vec![Override::Set {
                resource: addr.into(),
                field_path: format!("spec/volumes/{idx}"),
                value: json_to_string(value),
            }]);
        }
    }

    Err(SunbeamError::Config(format!(
        "volume '{vol_name}' not found"
    )))
}

fn expand_container_named(
    addr: &str,
    rest: &str,
    value: &Value,
    kind: &str,
    doc: &Value,
) -> Result<Vec<Override>> {
    // rest is like "exporter.memory" or "exporter.env.FOO"
    let parts: Vec<&str> = rest.split('.').collect();
    if parts.len() < 2 {
        return Err(SunbeamError::Config(format!(
            "invalid container shortcut: 'containers.{rest}'"
        )));
    }

    let container_name = parts[0];
    let sub_shortcut = parts[1..].join(".");

    match sub_shortcut.as_str() {
        "memory" => expand_memory(addr, value, kind, doc, Some(container_name)),
        "cpu" => expand_cpu(addr, value, kind, doc, Some(container_name)),
        _ => {
            if let Some(env_name) = sub_shortcut.strip_prefix("env.") {
                expand_container_env_named(addr, container_name, env_name, value, kind, doc)
            } else {
                Err(SunbeamError::Config(format!(
                    "unknown container shortcut: 'containers.{rest}'"
                )))
            }
        }
    }
}

fn expand_container_env_named(
    addr: &str,
    container_name: &str,
    var_name: &str,
    value: &Value,
    kind: &str,
    doc: &Value,
) -> Result<Vec<Override>> {
    let supported = ["Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob"];
    if !supported.contains(&kind) {
        return Err(SunbeamError::Config(format!(
            "'env' shortcut not supported for kind '{kind}'"
        )));
    }

    let idx = find_container_index(doc, container_name)?;

    let container = doc
        .get("spec")
        .and_then(|v| v.get("template"))
        .and_then(|v| v.get("spec"))
        .and_then(|v| v.get("containers"))
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.get(idx))
        .ok_or_else(|| SunbeamError::Config("container not found".into()))?;

    if let Some(env) = container.get("env").and_then(|v| v.as_array()) {
        for (env_idx, e) in env.iter().enumerate() {
            if e.get("name")
                .and_then(|v| v.as_str())
                .is_some_and(|n| n == var_name)
            {
                return Ok(vec![Override::Set {
                    resource: addr.into(),
                    field_path: format!("spec/template/spec/containers/{idx}/env/{env_idx}/value"),
                    value: json_to_string(value),
                }]);
            }
        }
    }

    // Add new env var
    let entry = serde_json::json!({
        "name": var_name,
        "value": json_to_string(value)
    });
    Ok(vec![Override::Set {
        resource: addr.into(),
        field_path: format!("spec/template/spec/containers/{idx}/env"),
        value: serde_json::to_string(&vec![entry]).unwrap_or_default(),
    }])
}

// ---------------------------------------------------------------------------
// Explicit path expansion (CRDs and `@` syntax)
// ---------------------------------------------------------------------------

fn expand_explicit_path(
    addr: &str,
    shortcut: &str,
    value: &Value,
    _kind: &str,
    path: &str,
    doc: &Value,
) -> Result<Vec<Override>> {
    // Convert dot-separated path to slash-separated for Override::Set
    let field_path = path.replace('.', "/");

    // Special case: config.<NAME> on a CRD — find the key in the object at path
    if let Some(config_key) = shortcut.strip_prefix("config.") {
        return expand_config_key(addr, config_key, value, &field_path, doc);
    }

    Ok(vec![Override::Set {
        resource: addr.into(),
        field_path,
        value: json_to_string(value),
    }])
}

fn expand_config_key(
    addr: &str,
    key: &str,
    value: &Value,
    field_path: &str,
    doc: &Value,
) -> Result<Vec<Override>> {
    // Navigate to the object at field_path
    let parts: Vec<&str> = field_path.split('/').collect();
    let mut current = doc;
    for part in &parts {
        current = match current {
            Value::Object(map) => map.get(*part).unwrap_or(&Value::Null),
            Value::Array(arr) => part
                .parse::<usize>()
                .ok()
                .and_then(|i| arr.get(i))
                .unwrap_or(&Value::Null),
            _ => &Value::Null,
        };
    }

    if let Some(obj) = current.as_object() {
        if obj.contains_key(key) {
            // Key exists — set it directly
            let key_path = format!("{field_path}/{key}");
            return Ok(vec![Override::Set {
                resource: addr.into(),
                field_path: key_path,
                value: json_to_string(value),
            }]);
        }
    }

    // Key doesn't exist — set the whole object with this key merged in
    let mut merged = if let Some(obj) = current.as_object() {
        obj.clone()
    } else {
        serde_json::Map::new()
    };
    merged.insert(key.to_string(), value.clone());
    Ok(vec![Override::Set {
        resource: addr.into(),
        field_path: field_path.to_string(),
        value: serde_json::to_string(&Value::Object(merged)).unwrap_or_default(),
    }])
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_container_index(doc: &Value, name: &str) -> Result<usize> {
    let containers = doc
        .get("spec")
        .and_then(|v| v.get("template"))
        .and_then(|v| v.get("spec"))
        .and_then(|v| v.get("containers"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| SunbeamError::Config("no containers found".into()))?;

    containers
        .iter()
        .enumerate()
        .find_map(|(idx, c)| {
            c.get("name")
                .and_then(|v| v.as_str())
                .filter(|n| *n == name)
                .map(|_| idx)
        })
        .ok_or_else(|| SunbeamError::Config(format!("container '{name}' not found")))
}

fn json_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "null".to_string(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deployment_doc() -> Value {
        serde_json::json!({
            "kind": "Deployment",
            "metadata": { "name": "gitea", "namespace": "devtools" },
            "spec": {
                "replicas": 1,
                "template": {
                    "spec": {
                        "containers": [
                            {
                                "name": "gitea",
                                "image": "gitea/gitea:latest",
                                "ports": [{"containerPort": 3000}],
                                "resources": {
                                    "requests": { "memory": "128Mi", "cpu": "100m" },
                                    "limits": { "memory": "256Mi", "cpu": "500m" }
                                },
                                "env": [
                                    {"name": "FOO", "value": "bar"}
                                ]
                            },
                            {
                                "name": "exporter",
                                "resources": {
                                    "requests": { "memory": "64Mi", "cpu": "50m" },
                                    "limits": { "memory": "128Mi", "cpu": "100m" }
                                }
                            }
                        ],
                        "volumes": [
                            {"name": "data", "emptyDir": {}}
                        ]
                    }
                }
            }
        })
    }

    fn statefulset_doc() -> Value {
        serde_json::json!({
            "kind": "StatefulSet",
            "metadata": { "name": "postgres", "namespace": "data" },
            "spec": {
                "replicas": 3,
                "template": {
                    "spec": {
                        "containers": [
                            {"name": "postgres", "image": "postgres:15"}
                        ]
                    }
                }
            }
        })
    }

    fn pvc_doc() -> Value {
        serde_json::json!({
            "kind": "PersistentVolumeClaim",
            "metadata": { "name": "opensearch-data", "namespace": "data" },
            "spec": {
                "resources": {
                    "requests": { "storage": "50Gi" }
                }
            }
        })
    }

    fn cronjob_doc() -> Value {
        serde_json::json!({
            "kind": "CronJob",
            "metadata": { "name": "backup", "namespace": "data" },
            "spec": {
                "schedule": "0 2 * * *",
                "jobTemplate": {
                    "spec": {
                        "template": {
                            "spec": {
                                "containers": [
                                    {"name": "backup", "image": "busybox"}
                                ]
                            }
                        }
                    }
                }
            }
        })
    }

    fn crd_doc() -> Value {
        serde_json::json!({
            "kind": "Cluster",
            "apiVersion": "postgresql.cnpg.io/v1",
            "metadata": { "name": "postgres", "namespace": "data" },
            "spec": {
                "instances": 3,
                "storage": { "size": "500Gi" },
                "postgresql": {
                    "parameters": {
                        "max_connections": "200",
                        "shared_buffers": "2GB"
                    }
                }
            }
        })
    }

    // -- scale --

    #[test]
    fn test_expand_scale_deployment() {
        let doc = deployment_doc();
        let overrides = expand_shortcut(
            "deployment/devtools/gitea",
            "scale",
            &serde_json::json!(3),
            "Deployment",
            None,
            &doc,
        )
        .unwrap();
        assert_eq!(overrides.len(), 1);
        assert_eq!(
            overrides[0],
            Override::Set {
                resource: "deployment/devtools/gitea".into(),
                field_path: "spec/replicas".into(),
                value: "3".into(),
            }
        );
    }

    #[test]
    fn test_expand_scale_unsupported_kind() {
        let doc = pvc_doc();
        let err = expand_shortcut(
            "persistentvolumeclaim/data/opensearch-data",
            "scale",
            &serde_json::json!(3),
            "PersistentVolumeClaim",
            None,
            &doc,
        )
        .unwrap_err();
        assert!(err.to_string().contains("scale"));
    }

    // -- instances --

    #[test]
    fn test_expand_instances_statefulset() {
        let doc = statefulset_doc();
        let overrides = expand_shortcut(
            "statefulset/data/postgres",
            "instances",
            &serde_json::json!(1),
            "StatefulSet",
            None,
            &doc,
        )
        .unwrap();
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/instances")
        );
    }

    #[test]
    fn test_expand_instances_wrong_kind() {
        let doc = deployment_doc();
        let err = expand_shortcut(
            "deployment/devtools/gitea",
            "instances",
            &serde_json::json!(1),
            "Deployment",
            None,
            &doc,
        )
        .unwrap_err();
        assert!(err.to_string().contains("instances"));
    }

    // -- storage --

    #[test]
    fn test_expand_storage_pvc() {
        let doc = pvc_doc();
        let overrides = expand_shortcut(
            "persistentvolumeclaim/data/opensearch-data",
            "storage",
            &serde_json::json!("10Gi"),
            "PersistentVolumeClaim",
            None,
            &doc,
        )
        .unwrap();
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/resources/requests/storage")
        );
        assert!(matches!(&overrides[0], Override::Set { value, .. } if value == "10Gi"));
    }

    // -- memory --

    #[test]
    fn test_expand_memory_all_containers() {
        let doc = deployment_doc();
        let overrides = expand_shortcut(
            "deployment/devtools/gitea",
            "memory",
            &serde_json::json!("512Mi"),
            "Deployment",
            None,
            &doc,
        )
        .unwrap();
        // 2 containers × 2 scopes (requests + limits) = 4 overrides
        assert_eq!(overrides.len(), 4);
        assert!(overrides.iter().any(|o| matches!(o, Override::Set { field_path, .. } if field_path == "spec/template/spec/containers/0/resources/requests/memory")));
        assert!(overrides.iter().any(|o| matches!(o, Override::Set { field_path, .. } if field_path == "spec/template/spec/containers/0/resources/limits/memory")));
        assert!(overrides.iter().any(|o| matches!(o, Override::Set { field_path, .. } if field_path == "spec/template/spec/containers/1/resources/requests/memory")));
        assert!(overrides.iter().any(|o| matches!(o, Override::Set { field_path, .. } if field_path == "spec/template/spec/containers/1/resources/limits/memory")));
    }

    #[test]
    fn test_expand_memory_named_container() {
        let doc = deployment_doc();
        let overrides = expand_shortcut(
            "deployment/devtools/gitea",
            "containers.exporter.memory",
            &serde_json::json!("128Mi"),
            "Deployment",
            None,
            &doc,
        )
        .unwrap();
        assert_eq!(overrides.len(), 2);
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/template/spec/containers/1/resources/requests/memory")
        );
        assert!(
            matches!(&overrides[1], Override::Set { field_path, .. } if field_path == "spec/template/spec/containers/1/resources/limits/memory")
        );
    }

    // -- cpu --

    #[test]
    fn test_expand_cpu_all_containers() {
        let doc = deployment_doc();
        let overrides = expand_shortcut(
            "deployment/devtools/gitea",
            "cpu",
            &serde_json::json!("250m"),
            "Deployment",
            None,
            &doc,
        )
        .unwrap();
        assert_eq!(overrides.len(), 4);
    }

    // -- env --

    #[test]
    fn test_expand_env_existing() {
        let doc = deployment_doc();
        let overrides = expand_shortcut(
            "deployment/devtools/gitea",
            "env.FOO",
            &serde_json::json!("baz"),
            "Deployment",
            None,
            &doc,
        )
        .unwrap();
        assert_eq!(overrides.len(), 1);
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/template/spec/containers/0/env/0/value")
        );
        assert!(matches!(&overrides[0], Override::Set { value, .. } if value == "baz"));
    }

    #[test]
    fn test_expand_env_new() {
        let doc = deployment_doc();
        let overrides = expand_shortcut(
            "deployment/devtools/gitea",
            "env.NEW_VAR",
            &serde_json::json!("hello"),
            "Deployment",
            None,
            &doc,
        )
        .unwrap();
        assert_eq!(overrides.len(), 1);
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path.contains("env"))
        );
    }

    #[test]
    fn test_expand_container_env_named() {
        let doc = deployment_doc();
        let overrides = expand_shortcut(
            "deployment/devtools/gitea",
            "containers.exporter.env.METRICS_PORT",
            &serde_json::json!("9090"),
            "Deployment",
            None,
            &doc,
        )
        .unwrap();
        assert_eq!(overrides.len(), 1);
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path.contains("containers/1/env"))
        );
    }

    // -- volumes --

    #[test]
    fn test_expand_volume_named() {
        let doc = deployment_doc();
        let overrides = expand_shortcut(
            "deployment/devtools/gitea",
            "volumes.data",
            &serde_json::json!({"name": "data", "persistentVolumeClaim": {"claimName": "new-claim"}}),
            "Deployment",
            None,
            &doc,
        )
        .unwrap();
        assert_eq!(overrides.len(), 1);
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/volumes/0")
        );
    }

    #[test]
    fn test_expand_volume_not_found() {
        let doc = deployment_doc();
        let err = expand_shortcut(
            "deployment/devtools/gitea",
            "volumes.missing",
            &serde_json::json!({}),
            "Deployment",
            None,
            &doc,
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    // -- image --

    #[test]
    fn test_expand_image() {
        let doc = deployment_doc();
        let overrides = expand_shortcut(
            "deployment/devtools/gitea",
            "image",
            &serde_json::json!("gitea/gitea:1.21"),
            "Deployment",
            None,
            &doc,
        )
        .unwrap();
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/template/spec/containers/0/image")
        );
    }

    // -- ports --

    #[test]
    fn test_expand_ports() {
        let doc = deployment_doc();
        let overrides = expand_shortcut(
            "deployment/devtools/gitea",
            "ports",
            &serde_json::json!([{"containerPort": 80, "hostPort": 80}]),
            "Deployment",
            None,
            &doc,
        )
        .unwrap();
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/template/spec/containers/0/ports")
        );
    }

    // -- schedule --

    #[test]
    fn test_expand_schedule_cronjob() {
        let doc = cronjob_doc();
        let overrides = expand_shortcut(
            "cronjob/data/backup",
            "schedule",
            &serde_json::json!("0 3 * * *"),
            "CronJob",
            None,
            &doc,
        )
        .unwrap();
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/schedule")
        );
    }

    // -- type --

    #[test]
    fn test_expand_type_service() {
        let doc = serde_json::json!({
            "kind": "Service",
            "metadata": { "name": "livekit-server-turn", "namespace": "media" },
            "spec": { "type": "LoadBalancer", "ports": [{"port": 443}] }
        });
        let overrides = expand_shortcut(
            "service/media/livekit-server-turn",
            "type",
            &serde_json::json!("ClusterIP"),
            "Service",
            None,
            &doc,
        )
        .unwrap();
        assert_eq!(overrides.len(), 1);
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/type")
        );
        assert!(matches!(&overrides[0], Override::Set { value, .. } if value == "ClusterIP"));
    }

    #[test]
    fn test_expand_type_wrong_kind() {
        let doc = deployment_doc();
        let err = expand_shortcut(
            "deployment/devtools/gitea",
            "type",
            &serde_json::json!("ClusterIP"),
            "Deployment",
            None,
            &doc,
        )
        .unwrap_err();
        assert!(err.to_string().contains("type"));
    }

    // -- explicit path (@ syntax) --

    #[test]
    fn test_expand_explicit_path() {
        let doc = crd_doc();
        let overrides = expand_shortcut(
            "cluster/data/postgres",
            "instances",
            &serde_json::json!(1),
            "Cluster",
            Some("spec.instances"),
            &doc,
        )
        .unwrap();
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/instances")
        );
    }

    #[test]
    fn test_expand_config_key_existing() {
        let doc = crd_doc();
        let overrides = expand_shortcut(
            "cluster/data/postgres",
            "config.max_connections",
            &serde_json::json!("100"),
            "Cluster",
            Some("spec.postgresql.parameters"),
            &doc,
        )
        .unwrap();
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/postgresql/parameters/max_connections")
        );
    }

    #[test]
    fn test_expand_config_key_new() {
        let doc = crd_doc();
        let overrides = expand_shortcut(
            "cluster/data/postgres",
            "config.work_mem",
            &serde_json::json!("4MB"),
            "Cluster",
            Some("spec.postgresql.parameters"),
            &doc,
        )
        .unwrap();
        assert!(
            matches!(&overrides[0], Override::Set { field_path, .. } if field_path == "spec/postgresql/parameters")
        );
        // Value should be a JSON object with the new key merged in
        assert!(matches!(&overrides[0], Override::Set { value, .. } if value.contains("work_mem")));
    }

    // -- error cases --

    #[test]
    fn test_expand_unknown_shortcut() {
        let doc = deployment_doc();
        let err = expand_shortcut(
            "deployment/devtools/gitea",
            "unknown",
            &serde_json::json!("x"),
            "Deployment",
            None,
            &doc,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown"));
    }

    #[test]
    fn test_expand_memory_no_containers() {
        let doc = pvc_doc();
        let err = expand_shortcut(
            "persistentvolumeclaim/data/opensearch-data",
            "memory",
            &serde_json::json!("512Mi"),
            "PersistentVolumeClaim",
            None,
            &doc,
        )
        .unwrap_err();
        assert!(err.to_string().contains("memory"));
    }

    #[test]
    fn test_json_to_string() {
        assert_eq!(json_to_string(&serde_json::json!("hello")), "hello");
        assert_eq!(json_to_string(&serde_json::json!(42)), "42");
        assert_eq!(json_to_string(&serde_json::json!(true)), "true");
        assert_eq!(json_to_string(&serde_json::json!(null)), "null");
        assert_eq!(json_to_string(&serde_json::json!({"a": 1})), "{\"a\":1}");
    }
}
