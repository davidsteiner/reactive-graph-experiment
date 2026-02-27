use std::any::Any;
use std::marker::PhantomData;
use std::sync::mpsc;

use crate::graph::{AnyNodeImpl, Graph, NodeKind};
use crate::node_ref::{NodeId, NodeRef};
use crate::value_store::ValueStore;

/// Marker type for closure-based compute nodes.
pub(crate) struct ClosureCompute;

/// Builder for closure-based compute nodes.
/// `Deps` accumulates dependency types as a tuple via typestate.
pub struct ComputeBuilder<Out, Deps> {
    pub(crate) key: String,
    pub(crate) dep_ids: Vec<NodeId>,
    pub(crate) _marker: PhantomData<(Out, Deps)>,
}

/// Builder after .compute() has been called with the closure.
pub struct ComputeBuilderReady<Out> {
    key: String,
    dep_ids: Vec<NodeId>,
    node_impl: Box<dyn AnyNodeImpl>,
    _marker: PhantomData<Out>,
}

impl<Out: Send + Sync + 'static> ComputeBuilder<Out, ()> {
    pub fn new(key: &str) -> Self {
        ComputeBuilder {
            key: key.to_string(),
            dep_ids: Vec::new(),
            _marker: PhantomData,
        }
    }
}

impl<Out: Send + Sync + 'static> ComputeBuilderReady<Out> {
    pub fn into_parts(self) -> (String, Vec<NodeId>, Box<dyn AnyNodeImpl>) {
        (self.key, self.dep_ids, self.node_impl)
    }

    pub fn build(self, graph: &mut Graph) -> NodeRef<Out> {
        let (key, dep_ids, node_impl) = self.into_parts();
        let id = NodeId::from_key::<_, ClosureCompute>(&key);
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };
        graph.register_node(id, NodeKind::Compute, dep_ids, node_impl);
        node_ref
    }
}

/// Type-erased compute implementation that stores a closure.
struct ClosureComputeNode<Func> {
    compute_fn: Func,
    extract_and_call: fn(&Func, &[NodeId], &ValueStore) -> Option<Box<dyn Any + Send + Sync>>,
}

impl<Func: Send + Sync + 'static> AnyNodeImpl for ClosureComputeNode<Func> {
    fn activate(
        &self,
        _id: &NodeId,
        _tx: &mpsc::Sender<(NodeId, Box<dyn Any + Send + Sync>)>,
    ) -> Option<Box<dyn Any + Send>> {
        None
    }

    fn compute(&self, deps: &[NodeId], store: &ValueStore) -> Option<Box<dyn Any + Send + Sync>> {
        (self.extract_and_call)(&self.compute_fn, deps, store)
    }

    fn emit(&self, _deps: &[NodeId], _store: &ValueStore) {}

    fn kind(&self) -> NodeKind {
        NodeKind::Compute
    }
}

macro_rules! impl_compute_arity {
    // Arity 1: () -> (A,)
    (start => ($A:ident), (0)) => {
        impl<Out: Send + Sync + 'static> ComputeBuilder<Out, ()> {
            pub fn dep<$A: Send + 'static>(mut self, node: NodeRef<$A>) -> ComputeBuilder<Out, ($A,)> {
                self.dep_ids.push(node.id.clone());
                ComputeBuilder {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    _marker: PhantomData,
                }
            }
        }

        impl<Out: Send + Sync + 'static, $A: Send + 'static> ComputeBuilder<Out, ($A,)> {
            pub fn compute<Func: Fn(&$A) -> Out + Send + Sync + 'static>(self, f: Func) -> ComputeBuilderReady<Out> {
                fn extract_and_call<Out: Send + Sync + 'static, $A: 'static, Func: Fn(&$A) -> Out>(
                    f: &Func, deps: &[NodeId], store: &ValueStore,
                ) -> Option<Box<dyn Any + Send + Sync>> {
                    let a = store.get_by_id(&deps[0])?.downcast_ref::<$A>()?;
                    Some(Box::new(f(a)))
                }
                ComputeBuilderReady {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    node_impl: Box::new(ClosureComputeNode {
                        compute_fn: f,
                        extract_and_call: extract_and_call::<Out, $A, Func>,
                    }),
                    _marker: PhantomData,
                }
            }
        }
    };

    // Arity N
    (($($Prev:ident),+) => ($($All:ident),+), ($($idx:tt),+)) => {
        impl<Out: Send + Sync + 'static, $($Prev: Send + 'static),+> ComputeBuilder<Out, ($($Prev,)+)> {
            pub fn dep<Last: Send + 'static>(mut self, node: NodeRef<Last>) -> ComputeBuilder<Out, ($($Prev,)+ Last,)> {
                self.dep_ids.push(node.id.clone());
                ComputeBuilder {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    _marker: PhantomData,
                }
            }
        }

        impl<Out: Send + Sync + 'static, $($All: Send + 'static),+> ComputeBuilder<Out, ($($All,)+)> {
            pub fn compute<Func: Fn($(&$All),+) -> Out + Send + Sync + 'static>(self, f: Func) -> ComputeBuilderReady<Out> {
                fn extract_and_call<Out: Send + Sync + 'static, $($All: 'static),+, Func: Fn($(&$All),+) -> Out>(
                    f: &Func, deps: &[NodeId], store: &ValueStore,
                ) -> Option<Box<dyn Any + Send + Sync>> {
                    $(
                        let $All = store.get_by_id(&deps[$idx])?.downcast_ref::<$All>()?;
                    )+
                    Some(Box::new(f($($All),+)))
                }
                ComputeBuilderReady {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    node_impl: Box::new(ClosureComputeNode {
                        compute_fn: f,
                        extract_and_call: extract_and_call::<Out, $($All,)+ Func>,
                    }),
                    _marker: PhantomData,
                }
            }
        }
    };
}

impl_compute_arity!(start => (A), (0));
impl_compute_arity!((A) => (A, B), (0, 1));
impl_compute_arity!((A, B) => (A, B, Cc), (0, 1, 2));
impl_compute_arity!((A, B, Cc) => (A, B, Cc, D), (0, 1, 2, 3));
impl_compute_arity!((A, B, Cc, D) => (A, B, Cc, D, E), (0, 1, 2, 3, 4));
impl_compute_arity!((A, B, Cc, D, E) => (A, B, Cc, D, E, Ff), (0, 1, 2, 3, 4, 5));
impl_compute_arity!((A, B, Cc, D, E, Ff) => (A, B, Cc, D, E, Ff, G), (0, 1, 2, 3, 4, 5, 6));
impl_compute_arity!((A, B, Cc, D, E, Ff, G) => (A, B, Cc, D, E, Ff, G, H), (0, 1, 2, 3, 4, 5, 6, 7));
