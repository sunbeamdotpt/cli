//! Parse `sunbeam.pt/tunable` annotations from manifest YAML.
//!
//! The annotation is a YAML map of `shortcut_name: type @ path` entries.
//!
//! ```yaml
//! sunbeam.pt/tunable: |
//!   instances: integer @ spec.instances
//!   storage: quantity @ spec.storage.size
//!   config: object @ spec.postgresql.parameters
//! ```

use crate::error::{Result, SunbeamError};
use std::collections::HashMap;

/// A parsed tunable entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tunable {
    /// The declared type hint.
    pub type_hint: String,
    /// Explicit path (without `spec` prefix). `None` means "use in-tree default".
    pub path: Option<String>,
}

/// Parse a `sunbeam.pt/tunable` annotation string into a map of shortcut names
/// to tunable declarations.
///
/// Grammar per line:
///   `name: type [@ path]`
///
/// The `spec` prefix is implied — paths in the annotation never include it.
pub fn parse_tunable_annotation(text: &str) -> Result<HashMap<String, Tunable>> {
    let mut result = HashMap::new();

    let value: serde_yaml::Value = serde_yaml::from_str(text)
        .map_err(|e| SunbeamError::Config(format!("invalid tunable annotation YAML: {e}")))?;

    let mapping = value
        .as_mapping()
        .ok_or_else(|| SunbeamError::Config("tunable annotation must be a YAML mapping".into()))?;

    for (key, val) in mapping {
        let key_str = key.as_str().ok_or_else(|| {
            SunbeamError::Config("tunable annotation keys must be strings".into())
        })?;
        let (type_hint, path) = parse_tunable_value(val)?;
        result.insert(
            key_str.to_string(),
            Tunable {
                type_hint: type_hint.to_string(),
                path: path.map(String::from),
            },
        );
    }

    Ok(result)
}

/// Parse a single tunable value which may be:
/// - `"integer"` → type only, no explicit path
/// - `"integer @ spec.instances"` → type + explicit path
fn parse_tunable_value(val: &serde_yaml::Value) -> Result<(&str, Option<&str>)> {
    let s = val
        .as_str()
        .ok_or_else(|| SunbeamError::Config("tunable value must be a string".into()))?;

    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(SunbeamError::Config(
            "tunable value must not be empty".into(),
        ));
    }

    // Split on " @ " — the space-around-at is required to avoid splitting
    // inside a path segment.
    if let Some((type_part, path_part)) = trimmed.split_once(" @ ") {
        let type_hint = type_part.trim();
        let path = path_part.trim();
        if type_hint.is_empty() {
            return Err(SunbeamError::Config(
                "tunable type must not be empty".into(),
            ));
        }
        if path.is_empty() {
            return Err(SunbeamError::Config(
                "tunable path must not be empty".into(),
            ));
        }
        Ok((type_hint, Some(path)))
    } else {
        Ok((trimmed, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_type_only() {
        let text = r#"
scale: integer
memory: quantity
"#;
        let map = parse_tunable_annotation(text).unwrap();
        assert_eq!(
            map["scale"],
            Tunable {
                type_hint: "integer".into(),
                path: None,
            }
        );
        assert_eq!(
            map["memory"],
            Tunable {
                type_hint: "quantity".into(),
                path: None,
            }
        );
    }

    #[test]
    fn test_parse_with_explicit_path() {
        let text = r#"
instances: integer @ spec.instances
storage: quantity @ spec.storage.size
config: object @ spec.postgresql.parameters
"#;
        let map = parse_tunable_annotation(text).unwrap();
        assert_eq!(
            map["instances"],
            Tunable {
                type_hint: "integer".into(),
                path: Some("spec.instances".into()),
            }
        );
        assert_eq!(
            map["storage"],
            Tunable {
                type_hint: "quantity".into(),
                path: Some("spec.storage.size".into()),
            }
        );
        assert_eq!(
            map["config"],
            Tunable {
                type_hint: "object".into(),
                path: Some("spec.postgresql.parameters".into()),
            }
        );
    }

    #[test]
    fn test_parse_mixed_type_and_path() {
        let text = r#"
scale: integer
memory: quantity
env.DISABLE_SECURITY_PLUGIN: bool
env.OPENSEARCH_JAVA_OPTS: string
containers.exporter.memory: quantity
containers.exporter.cpu: quantity
volumes.data.source: string
"#;
        let map = parse_tunable_annotation(text).unwrap();
        assert_eq!(map["scale"].path, None);
        assert_eq!(map["memory"].path, None);
        assert_eq!(map["env.DISABLE_SECURITY_PLUGIN"].type_hint, "bool");
        assert_eq!(map["containers.exporter.memory"].type_hint, "quantity");
    }

    #[test]
    fn test_parse_crd_examples() {
        let text = r#"
instances: integer @ spec.instances
storage: quantity @ spec.storage.size
config: object @ spec.postgresql.parameters
refresh_after: duration @ spec.refreshAfter
template_system: string @ spec.destination.templates.secretsSystem.text
"#;
        let map = parse_tunable_annotation(text).unwrap();
        assert_eq!(map["refresh_after"].path, Some("spec.refreshAfter".into()));
        assert_eq!(
            map["template_system"].path,
            Some("spec.destination.templates.secretsSystem.text".into())
        );
    }

    #[test]
    fn test_parse_invalid_yaml() {
        let text = "not a mapping: [1, 2";
        let err = parse_tunable_annotation(text).unwrap_err();
        assert!(err.to_string().contains("tunable"));
    }

    #[test]
    fn test_parse_non_mapping() {
        let text = "just a string";
        let err = parse_tunable_annotation(text).unwrap_err();
        assert!(err.to_string().contains("mapping"));
    }

    #[test]
    fn test_parse_empty_value() {
        let text = r#"
scale: ""
"#;
        let err = parse_tunable_annotation(text).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn test_parse_empty_annotation() {
        let text = "{}";
        let map = parse_tunable_annotation(text).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn test_parse_no_at_in_path() {
        // "@" without spaces should NOT be treated as path separator
        let text = r#"
foo: string@spec.foo
"#;
        let map = parse_tunable_annotation(text).unwrap();
        assert_eq!(map["foo"].type_hint, "string@spec.foo");
        assert_eq!(map["foo"].path, None);
    }

    #[test]
    fn test_all_type_hints() {
        let text = r#"
a: integer
b: quantity
c: string
d: bool
e: object
f: array
g: duration
"#;
        let map = parse_tunable_annotation(text).unwrap();
        assert_eq!(map["a"].type_hint, "integer");
        assert_eq!(map["b"].type_hint, "quantity");
        assert_eq!(map["c"].type_hint, "string");
        assert_eq!(map["d"].type_hint, "bool");
        assert_eq!(map["e"].type_hint, "object");
        assert_eq!(map["f"].type_hint, "array");
        assert_eq!(map["g"].type_hint, "duration");
    }
}
