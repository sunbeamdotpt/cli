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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

/// Print a table of all discoverable parameters.
pub fn print_catalog(resources: &[ResourceEntry]) {
    println!("Available manifest parameters:");
    println!();
    for r in resources {
        println!("  {}  ({} fields)", r.address, r.fields.len());
        for f in &r.fields {
            let current_str = match &f.current {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let display = if current_str.len() > 40 {
                format!("{}...", &current_str[..37])
            } else {
                current_str
            };
            println!("    {:50} {:10}  = {}", f.path, f.type_hint, display);
        }
        println!();
    }
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
            let addr = format!("{}/{}/{}", kind.to_lowercase(), ns, name);
            if addr != *resource {
                continue;
            }

            // Convert value string to appropriate JSON value
            let parsed_value = parse_value(value);

            // Apply the field path
            set_field(doc, field_path, parsed_value)?;
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
fn parse_value(s: &str) -> Value {
    // Try bool
    if s.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if s.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
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
                        "Cannot set field on non-object/non-array"
                    )));
                }
            }
            return Ok(());
        }

        // Navigate deeper
        current = match current {
            Value::Object(map) => map.get_mut(*part).ok_or_else(|| {
                crate::error::SunbeamError::Other(format!("Missing field: {part}"))
            })?,
            Value::Array(arr) => {
                let idx = part.parse::<usize>().map_err(|_| {
                    crate::error::SunbeamError::Other(format!("Expected array index: {part}"))
                })?;
                arr.get_mut(idx).ok_or_else(|| {
                    crate::error::SunbeamError::Other(format!("Index out of bounds: {idx}"))
                })?
            }
            _ => {
                return Err(crate::error::SunbeamError::Other(format!(
                    "Cannot navigate into non-object/non-array at: {part}"
                )));
            }
        };
    }

    Ok(())
}

/// Discover manifests and build catalog from a kustomize overlay.
pub async fn discover_from_overlay(
    overlay: &std::path::Path,
    domain: &str,
    email: &str,
) -> Result<Vec<ResourceEntry>> {
    let manifests = crate::kube::kustomize_build(overlay, domain, email).await?;
    Ok(build_catalog(&manifests))
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
        assert_eq!(parse_value("true"), Value::Bool(true));
        assert_eq!(parse_value("42"), Value::Number(42i64.into()));
        assert_eq!(parse_value("hello"), Value::String("hello".into()));
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
}
