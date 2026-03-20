use anyhow::Result;

pub const MANAGED_NS: &[&str] = &[
    "data",
    "devtools",
    "ingress",
    "lasuite",
    "matrix",
    "media",
    "monitoring",
    "ory",
    "storage",
    "vault-secrets-operator",
];

/// Return only the YAML documents that belong to the given namespace.
pub fn filter_by_namespace(manifests: &str, namespace: &str) -> String {
    let mut kept = Vec::new();
    for doc in manifests.split("\n---") {
        let doc = doc.trim();
        if doc.is_empty() {
            continue;
        }
        let has_ns = doc.contains(&format!("namespace: {namespace}"));
        let is_ns_resource =
            doc.contains("kind: Namespace") && doc.contains(&format!("name: {namespace}"));
        if has_ns || is_ns_resource {
            kept.push(doc);
        }
    }
    if kept.is_empty() {
        return String::new();
    }
    format!("---\n{}\n", kept.join("\n---\n"))
}

pub async fn cmd_apply(_env: &str, _domain: &str, _email: &str, _namespace: &str) -> Result<()> {
    todo!("cmd_apply: kustomize build + kube-rs apply pipeline")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MULTI_DOC: &str = "\
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: meet-config
  namespace: lasuite
data:
  FOO: bar
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: meet-backend
  namespace: lasuite
spec:
  replicas: 1
---
apiVersion: v1
kind: Namespace
metadata:
  name: lasuite
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: pingora-config
  namespace: ingress
data:
  config.toml: |
    hello
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: pingora
  namespace: ingress
spec:
  replicas: 1
";

    #[test]
    fn test_keeps_matching_namespace() {
        let result = filter_by_namespace(MULTI_DOC, "lasuite");
        assert!(result.contains("name: meet-config"));
        assert!(result.contains("name: meet-backend"));
    }

    #[test]
    fn test_excludes_other_namespaces() {
        let result = filter_by_namespace(MULTI_DOC, "lasuite");
        assert!(!result.contains("namespace: ingress"));
        assert!(!result.contains("name: pingora-config"));
        assert!(!result.contains("name: pingora\n"));
    }

    #[test]
    fn test_includes_namespace_resource_itself() {
        let result = filter_by_namespace(MULTI_DOC, "lasuite");
        assert!(result.contains("kind: Namespace"));
    }

    #[test]
    fn test_ingress_filter() {
        let result = filter_by_namespace(MULTI_DOC, "ingress");
        assert!(result.contains("name: pingora-config"));
        assert!(result.contains("name: pingora"));
        assert!(!result.contains("namespace: lasuite"));
    }

    #[test]
    fn test_unknown_namespace_returns_empty() {
        let result = filter_by_namespace(MULTI_DOC, "nonexistent");
        assert!(result.trim().is_empty());
    }

    #[test]
    fn test_empty_input_returns_empty() {
        let result = filter_by_namespace("", "lasuite");
        assert!(result.trim().is_empty());
    }

    #[test]
    fn test_result_starts_with_separator() {
        let result = filter_by_namespace(MULTI_DOC, "lasuite");
        assert!(result.starts_with("---"));
    }

    #[test]
    fn test_does_not_include_namespace_resource_for_wrong_ns() {
        let result = filter_by_namespace(MULTI_DOC, "ingress");
        assert!(!result.contains("kind: Namespace"));
    }

    #[test]
    fn test_single_doc_matching() {
        let doc = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n  namespace: ory\n";
        let result = filter_by_namespace(doc, "ory");
        assert!(result.contains("name: x"));
    }

    #[test]
    fn test_single_doc_not_matching() {
        let doc = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n  namespace: ory\n";
        let result = filter_by_namespace(doc, "lasuite");
        assert!(result.trim().is_empty());
    }
}
