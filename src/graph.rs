use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc;
use std::time::Duration;

use crate::node_ref::{NodeId, NodeRef};
use crate::value_store::ValueStore;

/// The kind of a node in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Source,
    Compute,
    Sink,
    AsyncCompute,
}

/// Policy for handling concurrent triggers of async compute nodes.
#[derive(Debug, Clone)]
pub enum AsyncPolicy {
    /// Cancel the in-flight computation and start a new one.
    LatestWins,
    /// Queue pending computations and execute them sequentially.
    Queue,
    /// Wait for a quiet period before starting computation.
    Debounce(Duration),
}

impl Default for AsyncPolicy {
    fn default() -> Self {
        AsyncPolicy::LatestWins
    }
}

/// Registry entry for a node.
pub struct NodeEntry {
    pub id: NodeId,
    pub kind: NodeKind,
    pub deps: Vec<NodeId>,
    pub ref_count: usize,
    pub node_impl: Box<dyn AnyNodeImpl>,
}

/// Type-erased node implementation so the graph can invoke nodes without knowing concrete types.
// Safety: AnyNodeImpl is Send + Sync because:
// - Compute and emit methods take &self (shared ref) and only read from the value store
// - Activate is only called from the single-threaded coordination layer
pub trait AnyNodeImpl: Send + Sync {
    /// For sources: activate and return a guard. `tx` is the channel sender.
    fn activate(
        &self,
        id: &NodeId,
        tx: &mpsc::Sender<(NodeId, Box<dyn Any + Send + Sync>)>,
    ) -> Option<Box<dyn Any + Send>>;

    /// For compute nodes: read deps from the value store, compute, return boxed result.
    fn compute(&self, deps: &[NodeId], store: &ValueStore) -> Option<Box<dyn Any + Send + Sync>>;

    /// For sinks: read deps from the value store and emit.
    fn emit(&self, deps: &[NodeId], store: &ValueStore);

    /// Return the NodeKind.
    fn kind(&self) -> NodeKind;

    /// For async compute nodes: read deps and return a future to execute.
    fn compute_async(
        &self,
        _deps: &[NodeId],
        _store: &ValueStore,
    ) -> Option<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>> {
        None
    }

    /// Return the async policy for this node (only meaningful for AsyncCompute).
    fn async_policy(&self) -> AsyncPolicy {
        AsyncPolicy::default()
    }

    /// Return the concurrency group ID for this node, if any.
    fn concurrency_group_id(&self) -> Option<usize> {
        None
    }
}

/// The core graph structure. Owns node registry and value store.
pub struct Graph {
    pub(crate) entries: HashMap<NodeId, NodeEntry>,
    pub(crate) value_store: ValueStore,
    pub(crate) source_tx: mpsc::Sender<(NodeId, Box<dyn Any + Send + Sync>)>,
    pub(crate) source_rx: mpsc::Receiver<(NodeId, Box<dyn Any + Send + Sync>)>,
}

impl Graph {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Graph {
            entries: HashMap::new(),
            value_store: ValueStore::new(),
            source_tx: tx,
            source_rx: rx,
        }
    }

    /// Register a node. Returns true if the node was newly inserted, false if it already existed
    /// (in which case ref_count is incremented).
    pub(crate) fn register_node(
        &mut self,
        id: NodeId,
        kind: NodeKind,
        deps: Vec<NodeId>,
        node_impl: Box<dyn AnyNodeImpl>,
    ) -> bool {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.ref_count += 1;
            return false;
        }
        self.entries.insert(
            id.clone(),
            NodeEntry {
                id,
                kind,
                deps,
                ref_count: 1,
                node_impl,
            },
        );
        true
    }

    /// Get a node entry by ID.
    pub fn get_entry(&self, id: &NodeId) -> Option<&NodeEntry> {
        self.entries.get(id)
    }

    /// Get a mutable node entry by ID.
    pub fn get_entry_mut(&mut self, id: &NodeId) -> Option<&mut NodeEntry> {
        self.entries.get_mut(id)
    }

    // === Closure-based builder API ===
    // These return builders that don't borrow Graph. Call .build(&mut graph) on the result.

    /// Start building a closure-based source node.
    pub fn source<T: Send + Sync + 'static>(key: &str) -> crate::builder::source::SourceBuilder<T> {
        crate::builder::source::SourceBuilder::new(key)
    }

    /// Start building a closure-based compute node.
    pub fn compute<T: Send + Sync + 'static>(
        key: &str,
    ) -> crate::builder::compute::ComputeBuilder<T, ()> {
        crate::builder::compute::ComputeBuilder::new(key)
    }

    /// Start building a closure-based sink.
    pub fn sink(key: &str) -> crate::builder::sink::SinkBuilder<()> {
        crate::builder::sink::SinkBuilder::new(key)
    }

    /// Start building a closure-based async compute node.
    pub fn async_compute<T: Send + Sync + 'static>(
        key: &str,
    ) -> crate::builder::async_compute::AsyncComputeBuilder<T, ()> {
        crate::builder::async_compute::AsyncComputeBuilder::new(key)
    }

    // === Trait-based builder API ===

    /// Add a trait-based source node.
    pub fn add_source<S: crate::node::Source + Send + Sync + 'static>(
        &mut self,
        source: S,
    ) -> NodeRef<S::Output> {
        crate::builder::trait_source::register_source(self, source)
    }

    /// Start building a trait-based compute node.
    pub fn add_compute<C: crate::node::Compute + Send + Sync + 'static>(
        &mut self,
        compute: C,
    ) -> crate::builder::trait_compute::TraitComputeBuilder<C, C::Inputs> {
        crate::builder::trait_compute::TraitComputeBuilder::new(self, compute)
    }

    /// Start building a trait-based async compute node.
    pub fn add_async_compute<AC: crate::node::AsyncCompute + Send + Sync + 'static>(
        &mut self,
        ac: AC,
    ) -> crate::builder::async_compute::TraitAsyncComputeBuilder<'_, AC, AC::Inputs> {
        crate::builder::async_compute::TraitAsyncComputeBuilder::new(self, ac)
    }

    /// Start building a trait-based sink node.
    pub fn add_sink<S: crate::node::Sink + Send + Sync + 'static>(
        &mut self,
        sink: S,
    ) -> crate::builder::trait_sink::TraitSinkBuilder<S, S::Inputs> {
        crate::builder::trait_sink::TraitSinkBuilder::new(self, sink)
    }
}
