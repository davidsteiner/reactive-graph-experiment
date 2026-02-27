use std::any::Any;
use std::collections::{HashMap, HashSet};

use crate::graph::{NodeEntry, NodeKind};
use crate::node_ref::NodeId;
use crate::topology::Topology;
use crate::value_store::ValueStore;

/// Ready-count scheduler that dispatches nodes to a thread pool when their
/// dirty dependencies have all completed.
pub struct Scheduler;

/// Shared reference to graph data needed during parallel execution.
/// This bundle is Send + Sync since it only contains references to Sync data.
struct GraphView<'a> {
    entries: &'a HashMap<NodeId, NodeEntry>,
    value_store: &'a ValueStore,
}

// GraphView is Sync because all contained references are to Sync data:
// - HashMap<NodeId, NodeEntry> contains Box<dyn AnyNodeImpl> which is Send+Sync
// - ValueStore contains HashMap<NodeId, Box<dyn Any + Send + Sync>> which is Sync

impl Scheduler {
    /// Execute a propagation cycle using wave-based parallel dispatch.
    pub fn execute_parallel(
        entries: &HashMap<NodeId, NodeEntry>,
        value_store: &mut ValueStore,
        topology: &Topology,
        sorted: &[NodeId],
        dirty: &HashSet<NodeId>,
    ) {
        if sorted.is_empty() {
            return;
        }

        // Compute ready counts
        let mut ready_count: HashMap<NodeId, usize> = HashMap::new();
        for node_id in sorted {
            if let Some(deps) = topology.deps.get(node_id) {
                let dirty_dep_count = deps.iter().filter(|d| dirty.contains(*d)).count();
                ready_count.insert(node_id.clone(), dirty_dep_count);
            } else {
                ready_count.insert(node_id.clone(), 0);
            }
        }

        let mut remaining: HashSet<NodeId> = sorted.iter().cloned().collect();

        while !remaining.is_empty() {
            let ready: Vec<NodeId> = remaining
                .iter()
                .filter(|id| ready_count.get(*id).copied().unwrap_or(0) == 0)
                .cloned()
                .collect();

            if ready.is_empty() {
                break;
            }

            // Execute wave
            let view = GraphView {
                entries,
                value_store,
            };

            let wave_results: Vec<(NodeId, Option<Box<dyn Any + Send + Sync>>)> =
                if ready.len() == 1 {
                    let node_id = &ready[0];
                    let result = Self::execute_single(&view, node_id);
                    vec![(node_id.clone(), result)]
                } else {
                    std::thread::scope(|s| {
                        let view_ref = &view;
                        let handles: Vec<_> = ready
                            .iter()
                            .map(|node_id| {
                                let id = node_id.clone();
                                s.spawn(move || {
                                    let result = Self::execute_single(view_ref, &id);
                                    (id, result)
                                })
                            })
                            .collect();

                        handles.into_iter().map(|h| h.join().unwrap()).collect()
                    })
                };

            // Write results between waves
            for (node_id, result) in wave_results {
                if let Some(value) = result {
                    value_store.set_by_id(&node_id, value);
                }

                if let Some(dependents) = topology.dependents.get(&node_id) {
                    for dep in dependents {
                        if let Some(count) = ready_count.get_mut(dep) {
                            *count = count.saturating_sub(1);
                        }
                    }
                }

                remaining.remove(&node_id);
            }
        }
    }

    fn execute_single(view: &GraphView, node_id: &NodeId) -> Option<Box<dyn Any + Send + Sync>> {
        let entry = view.entries.get(node_id)?;
        match entry.kind {
            NodeKind::Compute => entry.node_impl.compute(&entry.deps, view.value_store),
            NodeKind::Sink => {
                entry.node_impl.emit(&entry.deps, view.value_store);
                None
            }
            _ => None,
        }
    }
}
