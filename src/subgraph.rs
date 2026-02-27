use std::marker::PhantomData;
use std::rc::Rc;

use crate::engine::Engine;
use crate::graph::{AnyNodeImpl, NodeKind};
use crate::node_ref::{NodeId, NodeRef};

/// A descriptor for a node to be merged into the engine.
pub(crate) struct NodeDescriptor {
    pub id: NodeId,
    pub kind: NodeKind,
    pub deps: Vec<NodeId>,
    pub node_impl: Box<dyn AnyNodeImpl>,
}

/// Builder for constructing graph fragments outside the engine.
/// Implements the same builder API as Graph.
pub struct SubgraphBuilder {
    descriptors: Vec<NodeDescriptor>,
    /// Track which NodeIds were contributed by this builder
    node_ids: Vec<NodeId>,
    /// For dedup within the builder, track registered IDs
    registered: std::collections::HashSet<NodeId>,
}

impl SubgraphBuilder {
    pub fn new() -> Self {
        SubgraphBuilder {
            descriptors: Vec::new(),
            node_ids: Vec::new(),
            registered: std::collections::HashSet::new(),
        }
    }

    /// Register a node descriptor. Returns true if newly added, false if dedup'd.
    fn register_node(
        &mut self,
        id: NodeId,
        kind: NodeKind,
        deps: Vec<NodeId>,
        node_impl: Box<dyn AnyNodeImpl>,
    ) -> bool {
        self.node_ids.push(id.clone());
        if !self.registered.insert(id.clone()) {
            return false;
        }
        self.descriptors.push(NodeDescriptor {
            id,
            kind,
            deps,
            node_impl,
        });
        true
    }

    // === Closure-based builder API (mirrors Graph) ===

    pub fn source<T: Send + Sync + 'static>(key: &str) -> crate::builder::source::SourceBuilder<T> {
        crate::builder::source::SourceBuilder::new(key)
    }

    pub fn compute<T: Send + Sync + 'static>(
        key: &str,
    ) -> crate::builder::compute::ComputeBuilder<T, ()> {
        crate::builder::compute::ComputeBuilder::new(key)
    }

    pub fn sink(key: &str) -> crate::builder::sink::SinkBuilder<()> {
        crate::builder::sink::SinkBuilder::new(key)
    }

    // === Trait-based builder API ===

    pub fn add_source<S: crate::node::Source + Send + Sync + 'static>(
        &mut self,
        source: S,
    ) -> NodeRef<S::Output> {
        let id = NodeId::from_key::<_, S>(&source.key());
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };

        // Create the node impl
        let node_impl = crate::builder::trait_source::make_source_impl(source);
        self.register_node(id, NodeKind::Source, vec![], node_impl);
        node_ref
    }

    pub fn add_compute<C: crate::node::Compute + Send + Sync + 'static>(
        &mut self,
        compute: C,
    ) -> SubgraphTraitComputeBuilder<C, C::Inputs> {
        SubgraphTraitComputeBuilder {
            builder: self,
            compute,
            dep_ids: Vec::new(),
            _remaining: PhantomData,
        }
    }

    pub fn add_sink<S: crate::node::Sink + Send + Sync + 'static>(
        &mut self,
        sink: S,
    ) -> SubgraphTraitSinkBuilder<S, S::Inputs> {
        SubgraphTraitSinkBuilder {
            builder: self,
            sink,
            dep_ids: Vec::new(),
            _remaining: PhantomData,
        }
    }

    /// Consume the builder into a Subgraph ready for merging.
    pub fn build(self) -> Subgraph {
        Subgraph {
            descriptors: self.descriptors,
            node_ids: self.node_ids,
        }
    }
}

/// Extension to SourceBuilder and other closure builders to work with SubgraphBuilder.
impl crate::builder::source::SourceBuilder<f64> {}

// We need the closure builders to also accept SubgraphBuilder as a target.
// The simplest approach: SourceBuilder, ComputeBuilderReady, and SinkBuilderReady
// get a `build_into_subgraph` method.

impl<T: Send + Sync + 'static> crate::builder::source::SourceBuilder<T> {
    pub fn build_sub(self, builder: &mut SubgraphBuilder) -> NodeRef<T> {
        let (key, node_impl) = self.into_parts();
        let id = NodeId::from_key::<_, crate::builder::source::ClosureSource>(&key);
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };
        builder.register_node(id, NodeKind::Source, vec![], node_impl);
        node_ref
    }
}

impl<T: Send + Sync + 'static> crate::builder::compute::ComputeBuilderReady<T> {
    pub fn build_sub(self, builder: &mut SubgraphBuilder) -> NodeRef<T> {
        let (key, dep_ids, node_impl) = self.into_parts();
        let id = NodeId::from_key::<_, crate::builder::compute::ClosureCompute>(&key);
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };
        builder.register_node(id, NodeKind::Compute, dep_ids, node_impl);
        node_ref
    }
}

impl crate::builder::sink::SinkBuilderReady {
    pub fn build_sub(self, builder: &mut SubgraphBuilder) -> NodeRef<()> {
        let (key, dep_ids, node_impl) = self.into_parts();
        let id = NodeId::from_key::<_, crate::builder::sink::ClosureSink>(&key);
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };
        builder.register_node(id, NodeKind::Sink, dep_ids, node_impl);
        node_ref
    }
}

/// A built subgraph ready to be merged into an engine.
pub struct Subgraph {
    pub(crate) descriptors: Vec<NodeDescriptor>,
    pub(crate) node_ids: Vec<NodeId>,
}

/// Handle returned when merging a subgraph. Dropping it enqueues deactivation
/// of the contributed nodes, which the engine processes at the start of its
/// next public method call (RAII lifecycle, no unsafe).
pub struct SubgraphHandle {
    node_ids: Vec<NodeId>,
    pending_removals: crate::engine::RemovalQueue,
}

impl Drop for SubgraphHandle {
    fn drop(&mut self) {
        let ids = std::mem::take(&mut self.node_ids);
        if !ids.is_empty() {
            self.pending_removals.borrow_mut().push(ids);
        }
    }
}

impl Engine {
    /// Merge a subgraph into the live engine. Returns a handle for RAII lifecycle.
    pub fn merge(&mut self, subgraph: Subgraph) -> SubgraphHandle {
        self.drain_pending_removals();
        let node_ids = subgraph.node_ids.clone();

        // Register all descriptors into the graph
        for desc in subgraph.descriptors {
            self.graph
                .register_node(desc.id, desc.kind, desc.deps, desc.node_impl);
        }

        // Activate sink nodes (which cascades to their deps)
        for id in &node_ids {
            if let Some(entry) = self.graph.entries.get(id) {
                if entry.kind == NodeKind::Sink {
                    self.activate_sink(id);
                }
            }
        }

        SubgraphHandle {
            node_ids,
            pending_removals: Rc::clone(&self.pending_removals),
        }
    }
}

// === Trait builders for SubgraphBuilder ===

pub struct SubgraphTraitComputeBuilder<'g, C: crate::node::Compute, Remaining> {
    builder: &'g mut SubgraphBuilder,
    compute: C,
    dep_ids: Vec<NodeId>,
    _remaining: PhantomData<Remaining>,
}

pub struct SubgraphTraitSinkBuilder<'g, S: crate::node::Sink, Remaining> {
    builder: &'g mut SubgraphBuilder,
    sink: S,
    dep_ids: Vec<NodeId>,
    _remaining: PhantomData<Remaining>,
}

// Build methods when Remaining = ()
impl<'g, C> SubgraphTraitComputeBuilder<'g, C, ()>
where
    C: crate::node::Compute + Send + Sync + 'static,
    C::Inputs: crate::builder::trait_compute::ComputeDispatch<C>,
{
    pub fn build(self) -> NodeRef<C::Output> {
        let id = NodeId::from_key::<_, C>(&self.compute.key());
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };
        let dep_ids = self.dep_ids.clone();
        let node_impl =
            <C::Inputs as crate::builder::trait_compute::ComputeDispatch<C>>::make_node_impl(
                self.compute,
            );
        self.builder
            .register_node(id, NodeKind::Compute, dep_ids, node_impl);
        node_ref
    }
}

impl<'g, S> SubgraphTraitSinkBuilder<'g, S, ()>
where
    S: crate::node::Sink + Send + Sync + 'static,
    S::Inputs: crate::builder::trait_sink::SinkDispatch<S>,
{
    pub fn build(self) -> NodeRef<()> {
        let id = NodeId::from_key::<_, S>(&self.sink.key());
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };
        let dep_ids = self.dep_ids.clone();
        let node_impl =
            <S::Inputs as crate::builder::trait_sink::SinkDispatch<S>>::make_node_impl(self.sink);
        self.builder
            .register_node(id, NodeKind::Sink, dep_ids, node_impl);
        node_ref
    }
}

// dep() peeling macros for subgraph builders
macro_rules! impl_subgraph_compute_dep {
    ([$A:ident : 0]) => {
        impl<'g, C: crate::node::Compute + Send + Sync + 'static, $A: Send + 'static>
            SubgraphTraitComputeBuilder<'g, C, ($A,)>
        {
            pub fn dep(mut self, node: NodeRef<$A>) -> SubgraphTraitComputeBuilder<'g, C, ()> {
                self.dep_ids.push(node.id.clone());
                SubgraphTraitComputeBuilder {
                    builder: self.builder,
                    compute: self.compute,
                    dep_ids: self.dep_ids,
                    _remaining: PhantomData,
                }
            }
        }
    };
    ([$First:ident : $first_idx:tt $(, $Rest:ident : $rest_idx:tt)+]) => {
        impl<'g, C: crate::node::Compute + Send + Sync + 'static, $First: Send + 'static $(, $Rest: Send + 'static)+>
            SubgraphTraitComputeBuilder<'g, C, ($First, $($Rest,)+)>
        {
            pub fn dep(mut self, node: NodeRef<$First>) -> SubgraphTraitComputeBuilder<'g, C, ($($Rest,)+)> {
                self.dep_ids.push(node.id.clone());
                SubgraphTraitComputeBuilder {
                    builder: self.builder,
                    compute: self.compute,
                    dep_ids: self.dep_ids,
                    _remaining: PhantomData,
                }
            }
        }
    };
}

macro_rules! impl_subgraph_sink_dep {
    ([$A:ident : 0]) => {
        impl<'g, S: crate::node::Sink + Send + Sync + 'static, $A: Send + 'static>
            SubgraphTraitSinkBuilder<'g, S, ($A,)>
        {
            pub fn dep(mut self, node: NodeRef<$A>) -> SubgraphTraitSinkBuilder<'g, S, ()> {
                self.dep_ids.push(node.id.clone());
                SubgraphTraitSinkBuilder {
                    builder: self.builder,
                    sink: self.sink,
                    dep_ids: self.dep_ids,
                    _remaining: PhantomData,
                }
            }
        }
    };
    ([$First:ident : $first_idx:tt $(, $Rest:ident : $rest_idx:tt)+]) => {
        impl<'g, S: crate::node::Sink + Send + Sync + 'static, $First: Send + 'static $(, $Rest: Send + 'static)+>
            SubgraphTraitSinkBuilder<'g, S, ($First, $($Rest,)+)>
        {
            pub fn dep(mut self, node: NodeRef<$First>) -> SubgraphTraitSinkBuilder<'g, S, ($($Rest,)+)> {
                self.dep_ids.push(node.id.clone());
                SubgraphTraitSinkBuilder {
                    builder: self.builder,
                    sink: self.sink,
                    dep_ids: self.dep_ids,
                    _remaining: PhantomData,
                }
            }
        }
    };
}

impl_subgraph_compute_dep!([A: 0]);
impl_subgraph_compute_dep!([A: 0, B: 1]);
impl_subgraph_compute_dep!([A: 0, B: 1, Cc: 2]);
impl_subgraph_compute_dep!([A: 0, B: 1, Cc: 2, D: 3]);

impl_subgraph_sink_dep!([A: 0]);
impl_subgraph_sink_dep!([A: 0, B: 1]);
impl_subgraph_sink_dep!([A: 0, B: 1, Cc: 2]);
impl_subgraph_sink_dep!([A: 0, B: 1, Cc: 2, D: 3]);
