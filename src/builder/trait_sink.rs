use std::any::Any;
use std::marker::PhantomData;
use std::sync::mpsc;

use crate::graph::{AnyNodeImpl, Graph, NodeKind};
use crate::node::Sink;
use crate::node_ref::{NodeId, NodeRef};
use crate::value_store::ValueStore;

/// Helper trait on tuple types to produce type-erased AnyNodeImpl for sink nodes.
pub trait SinkDispatch<S: Sink<Inputs = Self> + Send + Sync + 'static>: Sized {
    fn make_node_impl(sink: S) -> Box<dyn AnyNodeImpl>;
}

/// Typestate builder for trait-based sink nodes.
pub struct TraitSinkBuilder<'g, S: Sink, Remaining> {
    graph: &'g mut Graph,
    sink: S,
    dep_ids: Vec<NodeId>,
    _remaining: PhantomData<Remaining>,
}

impl<'g, S: Sink + Send + Sync + 'static> TraitSinkBuilder<'g, S, S::Inputs> {
    pub fn new(graph: &'g mut Graph, sink: S) -> Self {
        TraitSinkBuilder {
            graph,
            sink,
            dep_ids: Vec::new(),
            _remaining: PhantomData,
        }
    }
}

impl<'g, S: Sink + Send + Sync + 'static> TraitSinkBuilder<'g, S, ()>
where
    S::Inputs: SinkDispatch<S>,
{
    pub fn build(self) -> NodeRef<()> {
        let id = NodeId::from_key::<_, S>(&self.sink.key());
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };

        let dep_ids = self.dep_ids.clone();
        let node_impl = <S::Inputs as SinkDispatch<S>>::make_node_impl(self.sink);

        self.graph
            .register_node(id, NodeKind::Sink, dep_ids, node_impl);
        node_ref
    }
}

struct TraitSinkNodeImpl<S> {
    sink: S,
    do_emit: fn(&S, &[NodeId], &ValueStore),
}

impl<S: Send + Sync + 'static> AnyNodeImpl for TraitSinkNodeImpl<S> {
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
        (self.do_emit)(&self.sink, deps, store);
    }

    fn kind(&self) -> NodeKind {
        NodeKind::Sink
    }
}

macro_rules! impl_trait_sink_arity {
    ([$A:ident : 0]) => {
        impl<'g, S: Sink + Send + Sync + 'static, $A: Send + 'static> TraitSinkBuilder<'g, S, ($A,)> {
            pub fn dep(mut self, node: NodeRef<$A>) -> TraitSinkBuilder<'g, S, ()> {
                self.dep_ids.push(node.id.clone());
                TraitSinkBuilder {
                    graph: self.graph,
                    sink: self.sink,
                    dep_ids: self.dep_ids,
                    _remaining: PhantomData,
                }
            }
        }

        impl<$A: Send + 'static, S> SinkDispatch<S> for ($A,)
        where
            S: Sink<Inputs = ($A,)> + Send + Sync + 'static,
        {
            fn make_node_impl(sink: S) -> Box<dyn AnyNodeImpl> {
                fn do_emit<$A: Send + 'static, S: Sink<Inputs = ($A,)>>(
                    s: &S, deps: &[NodeId], store: &ValueStore,
                ) {
                    if let Some(a) = store.get_by_id(&deps[0]).and_then(|v| v.downcast_ref::<$A>()) {
                        s.emit((a,));
                    }
                }
                Box::new(TraitSinkNodeImpl {
                    sink,
                    do_emit: do_emit::<$A, S>,
                })
            }
        }
    };

    ([$First:ident : $first_idx:tt $(, $Rest:ident : $rest_idx:tt)+]) => {
        impl<'g, S: Sink + Send + Sync + 'static, $First: Send + 'static $(, $Rest: Send + 'static)+>
            TraitSinkBuilder<'g, S, ($First, $($Rest,)+)>
        {
            pub fn dep(mut self, node: NodeRef<$First>) -> TraitSinkBuilder<'g, S, ($($Rest,)+)> {
                self.dep_ids.push(node.id.clone());
                TraitSinkBuilder {
                    graph: self.graph,
                    sink: self.sink,
                    dep_ids: self.dep_ids,
                    _remaining: PhantomData,
                }
            }
        }

        impl<$First: Send + 'static $(, $Rest: Send + 'static)+, S> SinkDispatch<S> for ($First, $($Rest,)+)
        where
            S: Sink<Inputs = ($First, $($Rest,)+)> + Send + Sync + 'static,
        {
            fn make_node_impl(sink: S) -> Box<dyn AnyNodeImpl> {
                fn do_emit<$First: Send + 'static $(, $Rest: Send + 'static)+, S>(
                    s: &S, deps: &[NodeId], store: &ValueStore,
                )
                where
                    S: Sink<Inputs = ($First, $($Rest,)+)>,
                {
                    let Some($First) = store.get_by_id(&deps[$first_idx]).and_then(|v| v.downcast_ref::<$First>()) else { return };
                    $(
                        let Some($Rest) = store.get_by_id(&deps[$rest_idx]).and_then(|v| v.downcast_ref::<$Rest>()) else { return };
                    )+
                    s.emit(($First, $($Rest),+));
                }
                Box::new(TraitSinkNodeImpl {
                    sink,
                    do_emit: do_emit::<$First, $($Rest,)+ S>,
                })
            }
        }
    };
}

impl_trait_sink_arity!([A: 0]);
impl_trait_sink_arity!([A: 0, B: 1]);
impl_trait_sink_arity!([A: 0, B: 1, Cc: 2]);
impl_trait_sink_arity!([A: 0, B: 1, Cc: 2, D: 3]);
impl_trait_sink_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4]);
impl_trait_sink_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4, Ff: 5]);
impl_trait_sink_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4, Ff: 5, G: 6]);
impl_trait_sink_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4, Ff: 5, G: 6, H: 7]);
