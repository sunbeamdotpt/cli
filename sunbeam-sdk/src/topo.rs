//! Topological sort for project dependency graphs.
//!
//! Kahn's algorithm. Given a DAG where edges point from a project to its
//! dependencies (A → B means "A depends on B"), produces groups such that
//! every project in group N has all its deps in groups < N. Projects within
//! a group are independent and safe to build in parallel.
//!
//! Cycles are detected and returned verbatim so the caller can print them.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Input edge: `project -> [deps...]`.
pub type Graph = BTreeMap<String, Vec<String>>;

/// Topological sort result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortedGroups(pub Vec<Vec<String>>);

impl SortedGroups {
    /// Flatten into a single sequential build order (loses parallelism info).
    pub fn flatten(&self) -> Vec<String> {
        self.0.iter().flatten().cloned().collect()
    }

    /// Total number of projects across all groups.
    pub fn len(&self) -> usize {
        self.0.iter().map(|g| g.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Error returned when the input graph has a cycle.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("dependency cycle detected: {}", path.join(" → "))]
pub struct CycleError {
    /// Nodes participating in the cycle, in traversal order.
    /// The last element is the node that closed the cycle (repeats the first).
    pub path: Vec<String>,
}

/// Topologically sort a dependency graph into parallelizable groups.
///
/// Each returned inner vec contains nodes that have no remaining deps on
/// each other and can be processed concurrently. Groups are ordered so that
/// group N's nodes only depend on nodes in groups 0..N.
///
/// Within a group, nodes are sorted alphabetically for determinism.
///
/// Unknown deps (referenced in values but not present as keys) are treated
/// as leaves: they're added to the graph with no deps of their own. This
/// matches the spec (deps.projects can name anything in the workspace).
pub fn sort(graph: &Graph) -> Result<SortedGroups, CycleError> {
    let mut nodes: BTreeSet<String> = graph.keys().cloned().collect();
    for deps in graph.values() {
        for d in deps {
            nodes.insert(d.clone());
        }
    }

    let mut indegree: BTreeMap<String, usize> = nodes.iter().map(|n| (n.clone(), 0)).collect();
    let mut reverse: BTreeMap<String, Vec<String>> =
        nodes.iter().map(|n| (n.clone(), Vec::new())).collect();

    for (node, deps) in graph {
        for dep in deps {
            *indegree.entry(node.clone()).or_insert(0) += 1;
            reverse.entry(dep.clone()).or_default().push(node.clone());
        }
    }

    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut remaining = nodes.len();

    let mut current: Vec<String> = indegree
        .iter()
        .filter(|&(_, &d)| d == 0)
        .map(|(n, _)| n.clone())
        .collect();

    while !current.is_empty() {
        current.sort();
        let mut next: Vec<String> = Vec::new();

        for node in &current {
            remaining -= 1;
            if let Some(children) = reverse.get(node) {
                for child in children {
                    let entry = match indegree.get_mut(child) {
                        Some(entry) => entry,
                        // Every child was seeded into the indegree map during setup.
                        None => unreachable!(),
                    };
                    *entry -= 1;
                    if *entry == 0 {
                        next.push(child.clone());
                    }
                }
            }
        }

        groups.push(current);
        current = next;
    }

    if remaining > 0 {
        return Err(extract_cycle(graph, &indegree));
    }

    Ok(SortedGroups(groups))
}

/// Walk the residual (non-zero indegree) graph to surface one concrete cycle.
fn extract_cycle(graph: &Graph, indegree: &BTreeMap<String, usize>) -> CycleError {
    let start = match indegree
        .iter()
        .find(|&(_, &d)| d > 0)
        .map(|(n, _)| n.clone())
    {
        Some(start) => start,
        // A positive remaining count guarantees at least one unresolved node.
        None => unreachable!(),
    };

    let mut stack: Vec<String> = Vec::new();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(start);

    while let Some(node) = queue.pop_front() {
        if stack.contains(&node) {
            let cycle_start = match stack.iter().position(|n| n == &node) {
                Some(idx) => idx,
                // We just verified the node is in the stack, so it must be found.
                None => unreachable!(),
            };
            let mut path: Vec<String> = stack[cycle_start..].to_vec();
            path.push(node);
            return CycleError { path };
        }
        if !visited.insert(node.clone()) {
            continue;
        }
        stack.push(node.clone());
        if let Some(deps) = graph.get(&node) {
            for dep in deps {
                if indegree.get(dep).copied().unwrap_or(0) > 0 {
                    queue.push_front(dep.clone());
                }
            }
        }
    }

    CycleError { path: stack }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[(&str, &[&str])]) -> Graph {
        edges
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    #[test]
    fn single_node_no_deps() {
        let g = graph(&[("sol", &[])]);
        let sorted = sort(&g).unwrap();
        assert_eq!(sorted.0, vec![vec!["sol".to_string()]]);
    }

    #[test]
    fn linear_chain() {
        let g = graph(&[("sol", &["wfe"]), ("wfe", &[])]);
        let sorted = sort(&g).unwrap();
        assert_eq!(
            sorted.0,
            vec![vec!["wfe".to_string()], vec!["sol".to_string()]]
        );
    }

    #[test]
    fn diamond_deps() {
        let g = graph(&[("d", &["b", "c"]), ("b", &["a"]), ("c", &["a"]), ("a", &[])]);
        let sorted = sort(&g).unwrap();
        assert_eq!(sorted.0.len(), 3);
        assert_eq!(sorted.0[0], vec!["a".to_string()]);
        assert_eq!(sorted.0[1], vec!["b".to_string(), "c".to_string()]);
        assert_eq!(sorted.0[2], vec!["d".to_string()]);
    }

    #[test]
    fn disconnected_components() {
        let g = graph(&[("a", &[]), ("b", &[]), ("c", &["b"])]);
        let sorted = sort(&g).unwrap();
        assert_eq!(sorted.0[0], vec!["a".to_string(), "b".to_string()]);
        assert_eq!(sorted.0[1], vec!["c".to_string()]);
    }

    #[test]
    fn simple_cycle_detected() {
        let g = graph(&[("a", &["b"]), ("b", &["a"])]);
        let err = sort(&g).unwrap_err();
        assert!(err.path.contains(&"a".to_string()));
        assert!(err.path.contains(&"b".to_string()));
        assert_eq!(err.path.first(), err.path.last());
    }

    #[test]
    fn self_cycle_detected() {
        let g = graph(&[("a", &["a"])]);
        let err = sort(&g).unwrap_err();
        assert_eq!(err.path, vec!["a".to_string(), "a".to_string()]);
    }

    #[test]
    fn unknown_deps_treated_as_leaves() {
        let g = graph(&[("sol", &["ext"])]);
        let sorted = sort(&g).unwrap();
        assert_eq!(sorted.0[0], vec!["ext".to_string()]);
        assert_eq!(sorted.0[1], vec!["sol".to_string()]);
    }

    #[test]
    fn flatten_preserves_order() {
        let g = graph(&[("d", &["b", "c"]), ("b", &["a"]), ("c", &["a"]), ("a", &[])]);
        let sorted = sort(&g).unwrap();
        let flat = sorted.flatten();
        assert_eq!(
            flat,
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string()
            ]
        );
    }
}
