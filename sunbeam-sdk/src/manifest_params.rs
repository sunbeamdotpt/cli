//! Runtime manifest parameter discovery and override application.
//!
//! Makes every field of every Kubernetes manifest addressable via
//! `--set kind/namespace/name/field/path=value` syntax.

use crate::error::Result;
use serde_json::Value;

/// A discovered resource with its addressable fields.
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    /// e.g. "Deployment"
    pub kind: String,
    /// e.g. "gitea"
    pub name: String,
    /// e.g. "devtools"
    pub namespace: String,
    /// e.g. "deployment/devtools/gitea"
    pub address: String,
    /// Addressable fields
    pub fields: Vec<FieldEntry>,
}

/// An addressable field within a resource.
#[derive(Debug, Clone)]
pub struct FieldEntry {
    /// Slash-separated path, e.g. "spec/replicas"
    pub path: String,
    /// Current value
    pub current: Value,
    /// Human-readable type hint
    pub type_hint: &'static str,
}

/// Parsed user override.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Override {
    /// Set a field to a new value.
    Set {
        /// Resource address: kind/namespace/name
        resource: String,
        /// Field path within the resource
        field_path: String,
        /// New value as string (coerced at apply time)
        value: String,
    },
    /// Exclude a resource from deployment.
    Disable {
        /// Resource address or glob pattern
        pattern: String,
    },
    /// Re-enable a previously disabled resource.
    Enable {
        /// Resource address or glob pattern
        pattern: String,
    },
}

/// Collection of user overrides.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Overrides {
    pub items: Vec<Override>,
}

impl Overrides {
    /// Parse CLI arguments into overrides.
    pub fn from_cli(
        set_args: &[String],
        disable_args: &[String],
        enable_args: &[String],
    ) -> Result<Self> {
        let mut items = Vec::new();

        for s in set_args {
            let (addr, value) = s.split_once('=').ok_or_else(|| {
                crate::error::SunbeamError::Config(format!("--set value must contain '=': {s}"))
            })?;
            let parts: Vec<&str> = addr.split('/').collect();
            if parts.len() < 4 {
                return Err(crate::error::SunbeamError::Config(format!(
                    "--set address must have at least 4 slash-separated parts (kind/namespace/name/field): {addr}"
                )));
            }
            let resource = parts[..3].join("/");
            let field_path = parts[3..].join("/");
            items.push(Override::Set {
                resource,
                field_path,
                value: value.to_string(),
            });
        }

        for d in disable_args {
            items.push(Override::Disable {
                pattern: d.to_string(),
            });
        }

        for e in enable_args {
            items.push(Override::Enable {
                pattern: e.to_string(),
            });
        }

        Ok(Overrides { items })
    }

    /// Returns true if any resource matching `address` should be disabled.
    pub fn is_disabled(&self, kind: &str, namespace: &str, name: &str) -> bool {
        let addr = format!("{}/{}/{}", kind.to_lowercase(), namespace, name);
        let mut disabled = false;
        for item in &self.items {
            match item {
                Override::Disable { pattern } => {
                    if glob_match(pattern, &addr) {
                        disabled = true;
                    }
                }
                Override::Enable { pattern } => {
                    if glob_match(pattern, &addr) {
                        disabled = false;
                    }
                }
                _ => {}
            }
        }
        disabled
    }
}

/// Simple glob matching: `*` matches any sequence of non-slash characters.
fn glob_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern.eq_ignore_ascii_case(text);
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.is_empty() {
        return true;
    }
    let mut cursor = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let found = text[cursor..].to_lowercase().find(&part.to_lowercase());
        match found {
            Some(pos) => {
                if i == 0 && pos != 0 {
                    // First part must match at start
                    return false;
                }
                cursor += pos + part.len();
            }
            None => return false,
        }
    }
    // If pattern doesn't end with *, ensure we consumed to end
    if !pattern.ends_with('*') && cursor != text.len() {
        return false;
    }
    true
}

/// Build a catalog of addressable resources from rendered manifest YAML.
pub fn build_catalog(manifests: &str) -> Vec<ResourceEntry> {
    let mut resources = Vec::new();

    for doc in manifests.split("\n---") {
        let doc = doc.trim();
        if doc.is_empty() {
            continue;
        }
        let Ok(value): std::result::Result<Value, _> = serde_yaml::from_str(doc) else {
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

        let address = format!("{}/{}/{}", kind.to_lowercase(), namespace, name);
        let mut fields = Vec::new();

        // metadata/annotations/*
        if let Some(anns) = metadata
            .and_then(|m| m.get("annotations"))
            .and_then(|v| v.as_object())
        {
            for (k, v) in anns {
                fields.push(FieldEntry {
                    path: format!("metadata/annotations/{k}"),
                    current: v.clone(),
                    type_hint: "string",
                });
            }
        }

        // metadata/labels/*
        if let Some(labels) = metadata
            .and_then(|m| m.get("labels"))
            .and_then(|v| v.as_object())
        {
            for (k, v) in labels {
                fields.push(FieldEntry {
                    path: format!("metadata/labels/{k}"),
                    current: v.clone(),
                    type_hint: "string",
                });
            }
        }

        // Kind-specific fields
        match kind {
            "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" => {
                if let Some(spec) = value.get("spec").and_then(|v| v.as_object()) {
                    // spec/replicas
                    if let Some(repl) = spec.get("replicas") {
                        fields.push(FieldEntry {
                            path: "spec/replicas".into(),
                            current: repl.clone(),
                            type_hint: "integer",
                        });
                    }
                    // Container-level fields
                    let containers = spec
                        .get("template")
                        .and_then(|v| v.get("spec"))
                        .and_then(|v| v.get("containers"))
                        .and_then(|v| v.as_array());
                    if let Some(ctrs) = containers {
                        for (idx, c) in ctrs.iter().enumerate() {
                            let cname = c
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or(&idx.to_string())
                                .to_string();
                            // image
                            if let Some(img) = c.get("image").and_then(|v| v.as_str()) {
                                fields.push(FieldEntry {
                                    path: format!("spec/template/spec/containers/{cname}/image"),
                                    current: Value::String(img.to_string()),
                                    type_hint: "string",
                                });
                            }
                            // resources
                            if let Some(res) = c.get("resources").and_then(|v| v.as_object()) {
                                for scope in ["limits", "requests"] {
                                    if let Some(map) = res.get(scope).and_then(|v| v.as_object()) {
                                        for (k, v) in map {
                                            fields.push(FieldEntry {
                                                path: format!(
                                                    "spec/template/spec/containers/{cname}/resources/{scope}/{k}"
                                                ),
                                                current: v.clone(),
                                                type_hint: "quantity",
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "PersistentVolumeClaim" => {
                if let Some(spec) = value.get("spec").and_then(|v| v.as_object()) {
                    if let Some(res) = spec.get("resources").and_then(|v| v.as_object()) {
                        if let Some(req) = res.get("requests").and_then(|v| v.as_object()) {
                            for (k, v) in req {
                                fields.push(FieldEntry {
                                    path: format!("spec/resources/requests/{k}"),
                                    current: v.clone(),
                                    type_hint: "quantity",
                                });
                            }
                        }
                    }
                }
            }
            "Service" => {
                if let Some(spec) = value.get("spec").and_then(|v| v.as_object()) {
                    if let Some(st) = spec.get("type").and_then(|v| v.as_str()) {
                        fields.push(FieldEntry {
                            path: "spec/type".into(),
                            current: Value::String(st.to_string()),
                            type_hint: "string",
                        });
                    }
                    if let Some(ports) = spec.get("ports").and_then(|v| v.as_array()) {
                        for (idx, p) in ports.iter().enumerate() {
                            if let Some(port) = p.get("port") {
                                fields.push(FieldEntry {
                                    path: format!("spec/ports/{idx}/port"),
                                    current: port.clone(),
                                    type_hint: "integer",
                                });
                            }
                            if let Some(tp) = p.get("targetPort") {
                                fields.push(FieldEntry {
                                    path: format!("spec/ports/{idx}/targetPort"),
                                    current: tp.clone(),
                                    type_hint: "integer",
                                });
                            }
                        }
                    }
                }
            }
            "Ingress" => {
                if let Some(spec) = value.get("spec").and_then(|v| v.as_object()) {
                    if let Some(rules) = spec.get("rules").and_then(|v| v.as_array()) {
                        for (idx, r) in rules.iter().enumerate() {
                            if let Some(host) = r.get("host").and_then(|v| v.as_str()) {
                                fields.push(FieldEntry {
                                    path: format!("spec/rules/{idx}/host"),
                                    current: Value::String(host.to_string()),
                                    type_hint: "string",
                                });
                            }
                        }
                    }
                }
            }
            "ConfigMap" => {
                if let Some(data) = value.get("data").and_then(|v| v.as_object()) {
                    for (k, v) in data {
                        fields.push(FieldEntry {
                            path: format!("data/{k}"),
                            current: v.clone(),
                            type_hint: "string",
                        });
                    }
                }
            }
            "Secret" => {
                if let Some(data) = value.get("stringData").and_then(|v| v.as_object()) {
                    for (k, v) in data {
                        fields.push(FieldEntry {
                            path: format!("stringData/{k}"),
                            current: v.clone(),
                            type_hint: "string",
                        });
                    }
                }
            }
            "CronJob" => {
                if let Some(spec) = value.get("spec").and_then(|v| v.as_object()) {
                    if let Some(schedule) = spec.get("schedule").and_then(|v| v.as_str()) {
                        fields.push(FieldEntry {
                            path: "spec/schedule".into(),
                            current: Value::String(schedule.to_string()),
                            type_hint: "string",
                        });
                    }
                    let containers = spec
                        .get("jobTemplate")
                        .and_then(|v| v.get("spec"))
                        .and_then(|v| v.get("template"))
                        .and_then(|v| v.get("spec"))
                        .and_then(|v| v.get("containers"))
                        .and_then(|v| v.as_array());
                    if let Some(ctrs) = containers {
                        for (idx, c) in ctrs.iter().enumerate() {
                            let cname = c
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or(&idx.to_string())
                                .to_string();
                            if let Some(res) = c.get("resources").and_then(|v| v.as_object()) {
                                for scope in ["limits", "requests"] {
                                    if let Some(map) = res.get(scope).and_then(|v| v.as_object()) {
                                        for (k, v) in map {
                                            fields.push(FieldEntry {
                                                path: format!(
                                                    "spec/jobTemplate/spec/template/spec/containers/{cname}/resources/{scope}/{k}"
                                                ),
                                                current: v.clone(),
                                                type_hint: "quantity",
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        resources.push(ResourceEntry {
            kind: kind.to_string(),
            name,
            namespace,
            address,
            fields,
        });
    }

    resources
}

/// Apply overrides to rendered manifest YAML and return the modified YAML.
pub fn apply_overrides(manifests: &str, overrides: &Overrides) -> Result<String> {
    let mut docs: Vec<Value> = Vec::new();
    for doc in manifests.split("\n---") {
        let doc = doc.trim();
        if doc.is_empty() {
            continue;
        }
        if let Ok(v) = serde_yaml::from_str::<Value>(doc) {
            docs.push(v);
        }
    }

    // Track which resources are disabled
    let mut disabled: Vec<bool> = vec![false; docs.len()];
    for (i, doc) in docs.iter().enumerate() {
        let kind = doc.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let ns = doc
            .get("metadata")
            .and_then(|v| v.get("namespace"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name = doc
            .get("metadata")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if overrides.is_disabled(kind, ns, name) {
            disabled[i] = true;
        }
    }

    // Apply field overrides
    for item in &overrides.items {
        let Override::Set {
            resource,
            field_path,
            value,
        } = item
        else {
            continue;
        };
        for (i, doc) in docs.iter_mut().enumerate() {
            if disabled[i] {
                continue;
            }
            let kind = doc.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let ns = doc
                .get("metadata")
                .and_then(|v| v.get("namespace"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = doc
                .get("metadata")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let addr = format!("{}/{}/{}", kind.to_lowercase(), ns, name);
            if addr != *resource {
                continue;
            }

            // Convert value string to appropriate JSON value
            let mut parsed_value = parse_value(value);

            // Kubernetes env var values MUST be strings. Detect env value paths
            // (e.g. spec/template/spec/containers/0/env/3/value) and force string.
            let parts: Vec<&str> = field_path.split('/').collect();
            if parts.len() >= 3
                && parts[parts.len() - 1] == "value"
                && parts[parts.len() - 3] == "env"
            {
                parsed_value = Value::String(value.clone());
            }

            // CRD parameter values (e.g. spec/postgresql/parameters/max_connections)
            // are typically strings in Kubernetes CRDs even when they look like numbers.
            // Force string if the path contains "parameters" AND we're setting a specific
            // key (path ends with the key name, not "parameters" itself).
            // When replacing the entire parameters object (path ends with "parameters"),
            // let parse_value handle it normally so the object structure is preserved.
            if parts.iter().any(|p| *p == "parameters") && parts.last() != Some(&"parameters") {
                // Always force string for CRD parameters - CRD schemas usually
                // define these as strings even for numeric-looking values.
                parsed_value = Value::String(value.clone());
            }

            // Apply the field path
            if let Err(e) = set_field(doc, field_path, parsed_value) {
                return Err(crate::error::SunbeamError::Other(format!(
                    "Failed to set field on {addr} at {field_path}: {e}"
                )));
            }
        }
    }

    // Re-serialize, skipping disabled docs
    let mut output = String::new();
    for (i, doc) in docs.iter().enumerate() {
        if disabled[i] {
            continue;
        }
        if !output.is_empty() {
            output.push_str("\n---\n");
        }
        output.push_str(&serde_yaml::to_string(doc).map_err(|e| {
            crate::error::SunbeamError::Other(format!("YAML serialization failed: {e}"))
        })?);
    }

    Ok(output)
}

/// Parse a user-provided value string into a JSON Value.
///
/// Booleans are NOT auto-parsed — `"true"` stays a string. This is required
/// because Kubernetes env vars must be strings, and profile shortcuts like
/// `env.FOO: "true"` must not become YAML booleans.
///
/// Numbers ARE parsed so that fields like `replicas` get proper JSON numbers.
/// Env var values are forced back to strings in `apply_overrides` by path
/// detection.
fn parse_value(s: &str) -> Value {
    // Try null
    if s.eq_ignore_ascii_case("null") {
        return Value::Null;
    }
    // Try integer
    if let Ok(n) = s.parse::<i64>() {
        return Value::Number(n.into());
    }
    // Try float
    if let Ok(f) = s.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return Value::Number(n);
        }
    }
    // Try JSON object/array
    if (s.starts_with('{') && s.ends_with('}')) || (s.starts_with('[') && s.ends_with(']')) {
        if let Ok(v) = serde_json::from_str(s) {
            return v;
        }
    }
    // Default to string
    Value::String(s.to_string())
}

/// Set a field in a JSON document by slash-separated path.
///
/// Creates missing intermediate objects automatically. Arrays are only
/// created when the next part is a numeric index; otherwise objects are
/// used. This allows shortcuts like `memory` to work on manifests that
/// don't yet have `spec.template.spec.containers[0].resources`.
fn set_field(doc: &mut Value, path: &str, value: Value) -> Result<()> {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.is_empty() {
        return Ok(());
    }

    let mut current = doc;
    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;

        if is_last {
            // Set the value
            match current {
                Value::Object(map) => {
                    map.insert(part.to_string(), value);
                }
                Value::Array(arr) => {
                    if let Ok(idx) = part.parse::<usize>() {
                        if idx < arr.len() {
                            arr[idx] = value;
                        } else {
                            return Err(crate::error::SunbeamError::Other(format!(
                                "Index {idx} out of bounds (len={})",
                                arr.len()
                            )));
                        }
                    } else {
                        return Err(crate::error::SunbeamError::Other(format!(
                            "Expected array index, got: {part}"
                        )));
                    }
                }
                _ => {
                    return Err(crate::error::SunbeamError::Other(format!(
                        "Cannot set field '{part}' on non-object/non-array (path: {path})"
                    )));
                }
            }
            return Ok(());
        }

        // Navigate deeper — create missing intermediates
        current = match current {
            Value::Object(map) => {
                let next_is_index = parts.get(i + 1).map_or(false, |p| p.parse::<usize>().is_ok());
                map.entry(part.to_string())
                    .or_insert_with(|| if next_is_index { Value::Array(vec![]) } else { Value::Object(serde_json::Map::new()) })
            }
            Value::Array(arr) => {
                let idx = part.parse::<usize>().map_err(|_| {
                    crate::error::SunbeamError::Other(format!("Expected array index: {part}"))
                })?;
                // Extend array if needed
                while arr.len() <= idx {
                    let next_is_index = parts.get(i + 1).map_or(false, |p| p.parse::<usize>().is_ok());
                    arr.push(if next_is_index { Value::Array(vec![]) } else { Value::Object(serde_json::Map::new()) });
                }
                &mut arr[idx]
            }
            _ => {
                return Err(crate::error::SunbeamError::Other(format!(
                    "Cannot navigate into non-object/non-array at '{part}' (path: {path})"
                )));
            }
        };
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MANIFEST: &str = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: gitea
  namespace: devtools
  annotations:
    app.kubernetes.io/part-of: devtools
spec:
  replicas: 1
  template:
    spec:
      containers:
        - name: gitea
          image: gitea/gitea:latest
          resources:
            limits:
              memory: 256Mi
              cpu: 500m
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: gitea-config
  namespace: devtools
data:
  app.ini: "[database]"
---
apiVersion: v1
kind: Service
metadata:
  name: gitea
  namespace: devtools
spec:
  type: ClusterIP
  ports:
    - port: 3000
      targetPort: 3000
"#;

    #[test]
    fn test_build_catalog() {
        let catalog = build_catalog(TEST_MANIFEST);
        assert_eq!(catalog.len(), 3);

        let dep = &catalog[0];
        assert_eq!(dep.address, "deployment/devtools/gitea");
        assert!(dep.fields.iter().any(|f| f.path == "spec/replicas"));
        assert!(
            dep.fields
                .iter()
                .any(|f| f.path == "spec/template/spec/containers/gitea/resources/limits/memory")
        );
    }

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match(
            "deployment/devtools/gitea",
            "deployment/devtools/gitea"
        ));
        assert!(!glob_match(
            "deployment/devtools/gitea",
            "deployment/devtools/penpot"
        ));
    }

    #[test]
    fn test_glob_match_wildcard() {
        assert!(glob_match(
            "deployment/devtools/*",
            "deployment/devtools/gitea"
        ));
        assert!(glob_match(
            "deployment/devtools/*",
            "deployment/devtools/penpot"
        ));
        assert!(!glob_match(
            "deployment/devtools/*",
            "deployment/matrix/tuwunel"
        ));
        assert!(glob_match("*", "deployment/devtools/gitea"));
        assert!(glob_match("deployment/*/*", "deployment/devtools/gitea"));
    }

    #[test]
    fn test_apply_override_replicas() {
        let overrides = Overrides {
            items: vec![Override::Set {
                resource: "deployment/devtools/gitea".into(),
                field_path: "spec/replicas".into(),
                value: "3".into(),
            }],
        };
        let result = apply_overrides(TEST_MANIFEST, &overrides).unwrap();
        assert!(result.contains("replicas: 3"));
    }

    #[test]
    fn test_apply_override_disable() {
        let overrides = Overrides {
            items: vec![Override::Disable {
                pattern: "deployment/devtools/*".into(),
            }],
        };
        let result = apply_overrides(TEST_MANIFEST, &overrides).unwrap();
        assert!(!result.contains("kind: Deployment"));
        assert!(result.contains("kind: ConfigMap"));
    }

    #[test]
    fn test_apply_override_enable_re_enables() {
        let overrides = Overrides {
            items: vec![
                Override::Disable {
                    pattern: "deployment/*/*".into(),
                },
                Override::Enable {
                    pattern: "deployment/devtools/gitea".into(),
                },
            ],
        };
        let result = apply_overrides(TEST_MANIFEST, &overrides).unwrap();
        assert!(result.contains("kind: Deployment"));
    }

    #[test]
    fn test_parse_value() {
        // Booleans are kept as strings (required for Kubernetes env vars)
        assert_eq!(parse_value("true"), Value::String("true".into()));
        assert_eq!(parse_value("false"), Value::String("false".into()));
        // Numbers are parsed for fields like replicas
        assert_eq!(parse_value("42"), Value::Number(42i64.into()));
        // Plain strings stay strings
        assert_eq!(parse_value("hello"), Value::String("hello".into()));
        // JSON objects/arrays are parsed
        assert_eq!(parse_value("{\"a\":1}"), serde_json::json!({"a": 1}));
    }

    #[test]
    fn test_set_field_nested() {
        let mut doc = serde_json::json!({"spec": {"replicas": 1}});
        set_field(&mut doc, "spec/replicas", Value::Number(5i64.into())).unwrap();
        assert_eq!(doc["spec"]["replicas"], 5);
    }

    #[test]
    fn test_set_field_array() {
        let mut doc = serde_json::json!({"spec": {"ports": [{"port": 80}]}});
        set_field(&mut doc, "spec/ports/0/port", Value::Number(8080i64.into())).unwrap();
        assert_eq!(doc["spec"]["ports"][0]["port"], 8080);
    }

    #[test]
    fn test_apply_env_overrides_preserve_names() {
        let manifest = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: opensearch
  namespace: data
spec:
  template:
    spec:
      containers:
        - name: opensearch
          env:
            - name: discovery.type
              value: single-node
            - name: OPENSEARCH_JAVA_OPTS
              value: "-Xms1g -Xmx1536m"
            - name: DISABLE_SECURITY_PLUGIN
              value: "true"
"#;
        let overrides = Overrides {
            items: vec![
                Override::Set {
                    resource: "deployment/data/opensearch".into(),
                    field_path: "spec/template/spec/containers/0/env/0/value".into(),
                    value: "single-node".into(),
                },
                Override::Set {
                    resource: "deployment/data/opensearch".into(),
                    field_path: "spec/template/spec/containers/0/env/1/value".into(),
                    value: "-Xms256m -Xmx512m".into(),
                },
                Override::Set {
                    resource: "deployment/data/opensearch".into(),
                    field_path: "spec/template/spec/containers/0/env/2/value".into(),
                    value: "true".into(),
                },
            ],
        };
        let result = apply_overrides(manifest, &overrides).unwrap();
        // Verify env var names are preserved and values are correct
        assert!(result.contains("name: discovery.type"), "missing discovery.type name");
        assert!(result.contains("value: single-node"), "missing single-node value");
        assert!(result.contains("name: OPENSEARCH_JAVA_OPTS"), "missing OPENSEARCH_JAVA_OPTS name");
        assert!(result.contains("value: -Xms256m -Xmx512m"), "missing OPENSEARCH_JAVA_OPTS value");
        assert!(result.contains("name: DISABLE_SECURITY_PLUGIN"), "missing DISABLE_SECURITY_PLUGIN name");
        assert!(result.contains("value: 'true'"), "missing DISABLE_SECURITY_PLUGIN value");
    }

    #[test]
    fn test_env_value_forced_to_string() {
        let manifest = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test
  namespace: default
spec:
  template:
    spec:
      containers:
        - name: main
          env:
            - name: COUNT
              value: "0"
"#;
        let overrides = Overrides {
            items: vec![Override::Set {
                resource: "deployment/default/test".into(),
                field_path: "spec/template/spec/containers/0/env/0/value".into(),
                value: "90".into(),
            }],
        };
        let result = apply_overrides(manifest, &overrides).unwrap();
        // 90 must be quoted (string), not a bare number
        assert!(result.contains("value: '90'"), "env value should be string, got: {result}");
    }

    #[test]
    fn test_crd_parameters_forced_to_string() {
        let manifest = r#"
apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: postgres
  namespace: data
spec:
  instances: 1
  postgresql:
    parameters:
      max_connections: "100"
      shared_buffers: "128MB"
  storage:
    size: 10Gi
"#;
        let overrides = Overrides {
            items: vec![
                Override::Set {
                    resource: "cluster/data/postgres".into(),
                    field_path: "spec/postgresql/parameters/max_connections".into(),
                    value: "50".into(),
                },
                Override::Set {
                    resource: "cluster/data/postgres".into(),
                    field_path: "spec/postgresql/parameters/shared_buffers".into(),
                    value: "64MB".into(),
                },
            ],
        };
        let result = apply_overrides(manifest, &overrides).unwrap();
        // max_connections must be quoted because "50" looks like a number
        assert!(result.contains("max_connections: '50'"), "max_connections should be quoted string, got: {result}");
        // shared_buffers is already unambiguously a string (contains letters)
        assert!(result.contains("shared_buffers: 64MB"), "shared_buffers should be present, got: {result}");
    }

    #[test]
    fn test_crd_parameters_unquoted_yaml() {
        // This matches the actual kustomize output where 128MB is NOT quoted
        let manifest = r#"
apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: postgres
  namespace: data
spec:
  instances: 1
  postgresql:
    parameters:
      max_connections: "100"
      shared_buffers: 128MB
      work_mem: 4MB
  storage:
    size: 10Gi
"#;
        let overrides = Overrides {
            items: vec![
                Override::Set {
                    resource: "cluster/data/postgres".into(),
                    field_path: "spec/postgresql/parameters/max_connections".into(),
                    value: "50".into(),
                },
            ],
        };
        let result = apply_overrides(manifest, &overrides).unwrap();
        assert!(result.contains("max_connections: '50'"), "max_connections should be quoted string, got: {result}");
    }
}
