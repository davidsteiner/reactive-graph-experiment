use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use crate::async_compute::{AsyncManager, ConcurrencyGroupHandle};
use crate::error::ErrorStrategy;
use crate::graph::{Graph, NodeKind};
use crate::introspection::{GraphSnapshot, NodeInfo};
use crate::metrics::{CycleMetrics, EngineMetrics, MetricsCollector};
use crate::node_ref::NodeId;
use crate::topology::Topology;

/// Shared queue for deferred node removal. SubgraphHandle pushes to this on drop;
/// Engine drains it at the start of every public mutable method.
pub(crate) type RemovalQueue = Rc<RefCell<Vec<Vec<NodeId>>>>;

/// The engine owns the graph and topology, and runs propagation cycles.
pub struct Engine {
    pub(crate) graph: Graph,
    pub(crate) topology: Topology,
    /// Guards returned by source activation (dropped on deactivation).
    pub(crate) guards: HashMap<NodeId, Box<dyn Any + Send>>,
    /// Set of currently active nodes.
    pub(crate) active: HashSet<NodeId>,
    /// Manages async compute execution.
    pub(crate) async_manager: AsyncManager,
    /// Per-node error strategies.
    pub(crate) error_strategies: HashMap<NodeId, ErrorStrategy>,
    /// Metrics collector.
    pub(crate) metrics: MetricsCollector,
    /// Deferred removals from dropped SubgraphHandles.
    pub(crate) pending_removals: RemovalQueue,
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            graph: Graph::new(),
            topology: Topology::new(),
            guards: HashMap::new(),
            active: HashSet::new(),
            async_manager: AsyncManager::new(),
            error_strategies: HashMap::new(),
            metrics: MetricsCollector::new(),
            pending_removals: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Drain any deferred removals enqueued by dropped SubgraphHandles.
    pub(crate) fn drain_pending_removals(&mut self) {
        let removals: Vec<Vec<NodeId>> = self.pending_removals.borrow_mut().drain(..).collect();
        for node_ids in removals {
            for id in node_ids.iter().rev() {
                self.deactivate_node(id);
            }
        }
    }

    /// Access the graph for building nodes.
    pub fn graph(&mut self) -> &mut Graph {
        self.drain_pending_removals();
        &mut self.graph
    }

    /// Create a concurrency group with a maximum number of concurrent async tasks.
    pub fn concurrency_group(&mut self, max_concurrent: usize) -> ConcurrencyGroupHandle {
        self.async_manager.create_group(max_concurrent)
    }

    /// Set the error strategy for a node.
    pub fn set_error_strategy(&mut self, id: &NodeId, strategy: ErrorStrategy) {
        self.error_strategies.insert(id.clone(), strategy);
    }

    /// Get current engine metrics.
    pub fn metrics(&self) -> EngineMetrics {
        let mut sources = 0;
        let mut computes = 0;
        let mut sinks = 0;
        let mut async_computes = 0;

        for id in &self.active {
            if let Some(entry) = self.graph.entries.get(id) {
                match entry.kind {
                    NodeKind::Source => sources += 1,
                    NodeKind::Compute => computes += 1,
                    NodeKind::Sink => sinks += 1,
                    NodeKind::AsyncCompute => async_computes += 1,
                }
            }
        }

        EngineMetrics {
            total_cycles: self.metrics.total_cycles(),
            active_sources: sources,
            active_computes: computes,
            active_sinks: sinks,
            active_async_computes: async_computes,
            pending_async: 0, // TODO: track in async_manager
            last_cycle: self.metrics.last_cycle().cloned(),
        }
    }

    /// Take a snapshot of the current graph for introspection.
    pub fn snapshot(&self) -> GraphSnapshot {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        for id in &self.active {
            if let Some(entry) = self.graph.entries.get(id) {
                nodes.push(NodeInfo {
                    id: id.clone(),
                    kind: entry.kind,
                    deps: entry.deps.clone(),
                    ref_count: entry.ref_count,
                });

                for dep in &entry.deps {
                    edges.push((dep.clone(), id.clone()));
                }
            }
        }

        GraphSnapshot { nodes, edges }
    }

    /// Activate a sink node, recursively activating its upstream dependencies.
    pub fn activate_sink(&mut self, sink_id: &NodeId) {
        self.drain_pending_removals();
        self.activate_node(sink_id);
    }

    /// Recursively activate a node and its dependencies.
    fn activate_node(&mut self, id: &NodeId) {
        if self.active.contains(id) {
            return;
        }

        let entry = match self.graph.entries.get(id) {
            Some(e) => e,
            None => return,
        };

        // Register in topology
        let deps = entry.deps.clone();
        let kind = entry.kind;
        self.topology.add_node(id.clone(), deps.clone());
        self.active.insert(id.clone());

        // Recursively activate dependencies first
        for dep in &deps {
            self.activate_node(dep);
        }

        // For sources, invoke activation callback
        if kind == NodeKind::Source {
            let guard = self
                .graph
                .entries
                .get(id)
                .and_then(|e| e.node_impl.activate(id, &self.graph.source_tx));
            if let Some(guard) = guard {
                self.guards.insert(id.clone(), guard);
            }
        }

        // If all deps already have values, do initial computation
        if kind == NodeKind::Compute {
            let entry = self.graph.entries.get(id).unwrap();
            let dep_ids = entry.deps.clone();
            let all_have_values = dep_ids.iter().all(|d| self.graph.value_store.contains(d));
            if all_have_values {
                if let Some(result) = self
                    .graph
                    .entries
                    .get(id)
                    .unwrap()
                    .node_impl
                    .compute(&dep_ids, &self.graph.value_store)
                {
                    self.graph.value_store.set_by_id(id, result);
                }
            }
        }
    }

    /// Deactivate a node. If ref_count reaches 0, removes and cascades.
    pub(crate) fn deactivate_node(&mut self, id: &NodeId) {
        let entry = match self.graph.entries.get_mut(id) {
            Some(e) => e,
            None => return,
        };

        entry.ref_count = entry.ref_count.saturating_sub(1);
        if entry.ref_count > 0 {
            return;
        }

        let deps = entry.deps.clone();
        let kind = entry.kind;

        // Drop guard for sources
        if kind == NodeKind::Source {
            self.guards.remove(id);
        }

        // Remove value
        self.graph.value_store.remove(id);

        // Remove from topology
        self.topology.remove_node(id);
        self.active.remove(id);

        // Remove the node entry
        self.graph.entries.remove(id);

        // Clean up error strategy
        self.error_strategies.remove(id);

        // Cascade to deps whose ref_count may now be 0
        for dep in &deps {
            self.deactivate_node(dep);
        }
    }

    /// Run a single propagation cycle: drain pending source updates, compute dirty set,
    /// topologically sort, and execute compute/sink nodes in order.
    pub fn run_cycle(&mut self) {
        self.drain_pending_removals();
        self.run_cycle_impl(false);
    }

    /// Run a propagation cycle with parallel execution on the thread pool.
    pub fn run_cycle_parallel(&mut self) {
        self.drain_pending_removals();
        self.run_cycle_impl(true);
    }

    fn run_cycle_impl(&mut self, parallel: bool) {
        let cycle_start = Instant::now();

        // 1. Drain all pending source updates into value store
        let mut updated_sources = HashSet::new();
        while let Ok((id, value)) = self.graph.source_rx.try_recv() {
            self.graph.value_store.set_by_id(&id, value);
            updated_sources.insert(id);
        }

        // 1b. Drain async compute results into value store
        let async_results = self.async_manager.drain_results();
        for (id, value) in async_results {
            self.graph.value_store.set_by_id(&id, value);
            updated_sources.insert(id);
        }

        if updated_sources.is_empty() {
            return;
        }

        // 2. Compute dirty set (transitive dependents of updated sources)
        let dirty = self.topology.dirty_set(&updated_sources);

        // 3. Topological sort of dirty set
        let sorted = self.topology.topo_sort(&dirty);

        let mut per_node_durations = HashMap::new();

        if parallel {
            let async_spawns = self.execute_parallel_with_async(&sorted, &dirty);
            self.spawn_async_work(async_spawns);
        } else {
            let async_spawns = self.execute_sequential_with_async(&sorted, &mut per_node_durations);
            self.spawn_async_work(async_spawns);
        }

        // Record metrics
        let cycle_id = self.metrics.total_cycles() + 1;
        self.metrics.record_cycle(CycleMetrics {
            cycle_id,
            duration: cycle_start.elapsed(),
            nodes_computed: sorted.len(),
            per_node_durations,
        });
    }

    fn execute_sequential_with_async(
        &mut self,
        sorted: &[NodeId],
        per_node_durations: &mut HashMap<NodeId, std::time::Duration>,
    ) -> Vec<AsyncSpawn> {
        let mut async_spawns = Vec::new();

        for node_id in sorted {
            let node_start = Instant::now();
            let entry = match self.graph.entries.get(node_id) {
                Some(e) => e,
                None => continue,
            };

            match entry.kind {
                NodeKind::Compute => {
                    let dep_ids = entry.deps.clone();
                    let strategy = self
                        .error_strategies
                        .get(node_id)
                        .cloned()
                        .unwrap_or_default();
                    self.execute_compute_with_error_handling(node_id, &dep_ids, &strategy);
                }
                NodeKind::Sink => {
                    let dep_ids = entry.deps.clone();
                    entry.node_impl.emit(&dep_ids, &self.graph.value_store);
                }
                NodeKind::AsyncCompute => {
                    let dep_ids = entry.deps.clone();
                    let policy = entry.node_impl.async_policy();
                    let group_id = entry.node_impl.concurrency_group_id();
                    if let Some(future) = entry
                        .node_impl
                        .compute_async(&dep_ids, &self.graph.value_store)
                    {
                        async_spawns.push(AsyncSpawn {
                            node_id: node_id.clone(),
                            future,
                            policy,
                            group_id,
                        });
                    }
                }
                _ => {}
            }

            per_node_durations.insert(node_id.clone(), node_start.elapsed());
        }

        async_spawns
    }

    fn execute_compute_with_error_handling(
        &mut self,
        node_id: &NodeId,
        dep_ids: &[NodeId],
        strategy: &ErrorStrategy,
    ) {
        match strategy {
            ErrorStrategy::PropagateStale => {
                // If compute returns None (failed), keep the old value
                if let Some(entry) = self.graph.entries.get(node_id) {
                    if let Some(result) = entry.node_impl.compute(dep_ids, &self.graph.value_store)
                    {
                        self.graph.value_store.set_by_id(node_id, result);
                    }
                    // else: keep stale value
                }
            }
            ErrorStrategy::PropagateError => {
                // Compute and store result (or None propagates as missing)
                if let Some(entry) = self.graph.entries.get(node_id) {
                    if let Some(result) = entry.node_impl.compute(dep_ids, &self.graph.value_store)
                    {
                        self.graph.value_store.set_by_id(node_id, result);
                    } else {
                        // Remove the value so downstream sees missing deps
                        self.graph.value_store.remove(node_id);
                    }
                }
            }
            ErrorStrategy::Retry {
                max_attempts,
                backoff_ms,
            } => {
                let max = *max_attempts;
                let backoff = *backoff_ms;
                let mut attempts = 0;
                loop {
                    if let Some(entry) = self.graph.entries.get(node_id) {
                        if let Some(result) =
                            entry.node_impl.compute(dep_ids, &self.graph.value_store)
                        {
                            self.graph.value_store.set_by_id(node_id, result);
                            break;
                        }
                    }
                    attempts += 1;
                    if attempts >= max {
                        // Keep stale value after exhausting retries
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(backoff));
                }
            }
        }
    }

    fn execute_parallel_with_async(
        &mut self,
        sorted: &[NodeId],
        dirty: &HashSet<NodeId>,
    ) -> Vec<AsyncSpawn> {
        // For parallel execution, run sync nodes through the scheduler
        // and collect async nodes separately
        let sync_sorted: Vec<NodeId> = sorted
            .iter()
            .filter(|id| {
                self.graph
                    .entries
                    .get(*id)
                    .map(|e| e.kind != NodeKind::AsyncCompute)
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        if !sync_sorted.is_empty() {
            use crate::scheduler::Scheduler;
            Scheduler::execute_parallel(
                &self.graph.entries,
                &mut self.graph.value_store,
                &self.topology,
                &sync_sorted,
                dirty,
            );
        }

        // Collect async spawns
        let mut async_spawns = Vec::new();
        for node_id in sorted {
            let entry = match self.graph.entries.get(node_id) {
                Some(e) => e,
                None => continue,
            };
            if entry.kind == NodeKind::AsyncCompute {
                let dep_ids = entry.deps.clone();
                let policy = entry.node_impl.async_policy();
                let group_id = entry.node_impl.concurrency_group_id();
                if let Some(future) = entry
                    .node_impl
                    .compute_async(&dep_ids, &self.graph.value_store)
                {
                    async_spawns.push(AsyncSpawn {
                        node_id: node_id.clone(),
                        future,
                        policy,
                        group_id,
                    });
                }
            }
        }

        async_spawns
    }

    fn spawn_async_work(&mut self, spawns: Vec<AsyncSpawn>) {
        for spawn in spawns {
            self.async_manager
                .spawn(spawn.node_id, spawn.future, spawn.policy, spawn.group_id);
        }
    }
}

struct AsyncSpawn {
    node_id: NodeId,
    future: std::pin::Pin<
        Box<dyn std::future::Future<Output = Box<dyn std::any::Any + Send + Sync>> + Send>,
    >,
    policy: crate::graph::AsyncPolicy,
    group_id: Option<usize>,
}
