use std::any::Any;
use std::marker::PhantomData;
use std::sync::mpsc;

use crate::graph::{AnyNodeImpl, Graph, NodeKind};
use crate::node_ref::{NodeId, NodeRef};
use crate::value_store::ValueStore;

/// Marker type for closure-based sink nodes.
pub(crate) struct ClosureSink;

/// Builder for closure-based sink nodes.
pub struct SinkBuilder<Deps> {
    pub(crate) key: String,
    pub(crate) dep_ids: Vec<NodeId>,
    pub(crate) _marker: PhantomData<Deps>,
}

/// Builder after .on_emit() has been called.
pub struct SinkBuilderReady {
    key: String,
    dep_ids: Vec<NodeId>,
    node_impl: Box<dyn AnyNodeImpl>,
}

impl SinkBuilder<()> {
    pub fn new(key: &str) -> Self {
        SinkBuilder {
            key: key.to_string(),
            dep_ids: Vec::new(),
            _marker: PhantomData,
        }
    }
}

impl SinkBuilderReady {
    pub fn into_parts(self) -> (String, Vec<NodeId>, Box<dyn AnyNodeImpl>) {
        (self.key, self.dep_ids, self.node_impl)
    }

    pub fn build(self, graph: &mut Graph) -> NodeRef<()> {
        let (key, dep_ids, node_impl) = self.into_parts();
        let id = NodeId::from_key::<_, ClosureSink>(&key);
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };
        graph.register_node(id, NodeKind::Sink, dep_ids, node_impl);
        node_ref
    }
}

struct ClosureSinkNode<Func> {
    emit_fn: Func,
    extract_and_emit: fn(&Func, &[NodeId], &ValueStore),
}

impl<Func: Send + Sync + 'static> AnyNodeImpl for ClosureSinkNode<Func> {
    fn activate(
        &self,
        _id: &NodeId,
        _tx: &mpsc::Sender<(NodeId, Box<dyn Any + Send + Sync>)>,
    ) -> Option<Box<dyn Any + Send>> {
        None
    }

    fn compute(&self, _deps: &[NodeId], _store: &ValueStore) -> Option<Box<dyn Any + Send + Sync>> {
        None
    }

    fn emit(&self, deps: &[NodeId], store: &ValueStore) {
        (self.extract_and_emit)(&self.emit_fn, deps, store);
    }

    fn kind(&self) -> NodeKind {
        NodeKind::Sink
    }
}

macro_rules! impl_sink_arity {
    (start => ($A:ident), (0)) => {
        impl SinkBuilder<()> {
            pub fn dep<$A: Send + 'static>(mut self, node: NodeRef<$A>) -> SinkBuilder<($A,)> {
                self.dep_ids.push(node.id.clone());
                SinkBuilder {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    _marker: PhantomData,
                }
            }
        }

        impl<$A: Send + 'static> SinkBuilder<($A,)> {
            pub fn on_emit<Func: Fn(&$A) + Send + Sync + 'static>(self, f: Func) -> SinkBuilderReady {
                fn extract_and_emit<$A: 'static, Func: Fn(&$A)>(
                    f: &Func, deps: &[NodeId], store: &ValueStore,
                ) {
                    if let Some(a) = store.get_by_id(&deps[0]).and_then(|v| v.downcast_ref::<$A>()) {
                        f(a);
                    }
                }
                SinkBuilderReady {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    node_impl: Box::new(ClosureSinkNode {
                        emit_fn: f,
                        extract_and_emit: extract_and_emit::<$A, Func>,
                    }),
                }
            }
        }
    };

    (($($Prev:ident),+) => ($($All:ident),+), ($($idx:tt),+)) => {
        impl<$($Prev: Send + 'static),+> SinkBuilder<($($Prev,)+)> {
            pub fn dep<Last: Send + 'static>(mut self, node: NodeRef<Last>) -> SinkBuilder<($($Prev,)+ Last,)> {
                self.dep_ids.push(node.id.clone());
                SinkBuilder {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    _marker: PhantomData,
                }
            }
        }

        impl<$($All: Send + 'static),+> SinkBuilder<($($All,)+)> {
            pub fn on_emit<Func: Fn($(&$All),+) + Send + Sync + 'static>(self, f: Func) -> SinkBuilderReady {
                fn extract_and_emit<$($All: 'static),+, Func: Fn($(&$All),+)>(
                    f: &Func, deps: &[NodeId], store: &ValueStore,
                ) {
                    $(
                        let Some($All) = store.get_by_id(&deps[$idx]).and_then(|v| v.downcast_ref::<$All>()) else {
                            return;
                        };
                    )+
                    f($($All),+);
                }
                SinkBuilderReady {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    node_impl: Box::new(ClosureSinkNode {
                        emit_fn: f,
                        extract_and_emit: extract_and_emit::<$($All,)+ Func>,
                    }),
                }
            }
        }
    };
}

impl_sink_arity!(start => (A), (0));
impl_sink_arity!((A) => (A, B), (0, 1));
impl_sink_arity!((A, B) => (A, B, Cc), (0, 1, 2));
impl_sink_arity!((A, B, Cc) => (A, B, Cc, D), (0, 1, 2, 3));
impl_sink_arity!((A, B, Cc, D) => (A, B, Cc, D, E), (0, 1, 2, 3, 4));
impl_sink_arity!((A, B, Cc, D, E) => (A, B, Cc, D, E, Ff), (0, 1, 2, 3, 4, 5));
impl_sink_arity!((A, B, Cc, D, E, Ff) => (A, B, Cc, D, E, Ff, G), (0, 1, 2, 3, 4, 5, 6));
impl_sink_arity!((A, B, Cc, D, E, Ff, G) => (A, B, Cc, D, E, Ff, G, H), (0, 1, 2, 3, 4, 5, 6, 7));
