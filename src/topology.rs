use std::collections::{HashMap, HashSet, VecDeque};

use crate::node_ref::NodeId;

/// Dependency and dependent maps, with topological sort on dirty subgraphs.
pub struct Topology {
    /// node -> its dependencies (what it reads from)
    pub(crate) deps: HashMap<NodeId, Vec<NodeId>>,
    /// node -> its dependents (what reads from it)
    pub(crate) dependents: HashMap<NodeId, Vec<NodeId>>,
}

impl Topology {
    pub fn new() -> Self {
        Topology {
            deps: HashMap::new(),
            dependents: HashMap::new(),
        }
    }

    /// Register a node and its dependencies.
    pub fn add_node(&mut self, id: NodeId, deps: Vec<NodeId>) {
        // Add reverse edges
        for dep in &deps {
            self.dependents
                .entry(dep.clone())
                .or_default()
                .push(id.clone());
        }
        self.deps.insert(id.clone(), deps);
        self.dependents.entry(id).or_default();
    }

    /// Remove a node from the topology.
    pub fn remove_node(&mut self, id: &NodeId) {
        // Remove from dependents of our deps
        if let Some(deps) = self.deps.remove(id) {
            for dep in &deps {
                if let Some(dep_dependents) = self.dependents.get_mut(dep) {
                    dep_dependents.retain(|d| d != id);
                }
            }
        }
        self.dependents.remove(id);
    }

    /// Compute the dirty set: all transitive dependents of the given source nodes.
    pub fn dirty_set(&self, sources: &HashSet<NodeId>) -> HashSet<NodeId> {
        let mut dirty = HashSet::new();
        let mut queue: VecDeque<NodeId> = sources.iter().cloned().collect();

        while let Some(node) = queue.pop_front() {
            if let Some(deps) = self.dependents.get(&node) {
                for dep in deps {
                    if dirty.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }
        dirty
    }

    /// Topological sort of the dirty set using Kahn's algorithm.
    /// Returns nodes in dependency order (deps before dependents).
    pub fn topo_sort(&self, dirty: &HashSet<NodeId>) -> Vec<NodeId> {
        if dirty.is_empty() {
            return Vec::new();
        }

        // Compute in-degree within the dirty subgraph
        let mut in_degree: HashMap<NodeId, usize> = HashMap::new();
        for node in dirty {
            in_degree.entry(node.clone()).or_insert(0);
            if let Some(deps) = self.deps.get(node) {
                let dirty_dep_count = deps.iter().filter(|d| dirty.contains(*d)).count();
                in_degree.insert(node.clone(), dirty_dep_count);
            }
        }

        // Start with nodes that have 0 in-degree within the dirty set
        let mut queue: VecDeque<NodeId> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(id, _)| id.clone())
            .collect();

        let mut sorted = Vec::new();

        while let Some(node) = queue.pop_front() {
            sorted.push(node.clone());

            if let Some(dependents) = self.dependents.get(&node) {
                for dep in dependents {
                    if let Some(deg) = in_degree.get_mut(dep) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            queue.push_back(dep.clone());
                        }
                    }
                }
            }
        }

        sorted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_ref::NodeId;

    fn nid(name: &str) -> NodeId {
        NodeId::from_key::<_, ()>(name)
    }

    #[test]
    fn test_topo_sort_correctness() {
        let mut topo = Topology::new();

        // S -> A -> C
        // S -> B -> C
        let s = nid("S");
        let a = nid("A");
        let b = nid("B");
        let c = nid("C");

        topo.add_node(s.clone(), vec![]);
        topo.add_node(a.clone(), vec![s.clone()]);
        topo.add_node(b.clone(), vec![s.clone()]);
        topo.add_node(c.clone(), vec![a.clone(), b.clone()]);

        let dirty_sources: HashSet<NodeId> = [s.clone()].into_iter().collect();
        let dirty = topo.dirty_set(&dirty_sources);

        assert!(dirty.contains(&a));
        assert!(dirty.contains(&b));
        assert!(dirty.contains(&c));
        assert!(!dirty.contains(&s)); // sources themselves are not in dirty set

        let sorted = topo.topo_sort(&dirty);
        assert_eq!(sorted.len(), 3);

        // C must come after both A and B
        let pos_a = sorted.iter().position(|n| n == &a).unwrap();
        let pos_b = sorted.iter().position(|n| n == &b).unwrap();
        let pos_c = sorted.iter().position(|n| n == &c).unwrap();
        assert!(pos_c > pos_a);
        assert!(pos_c > pos_b);
    }
}
