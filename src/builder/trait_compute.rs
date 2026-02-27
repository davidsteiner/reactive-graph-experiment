use std::any::Any;
use std::marker::PhantomData;
use std::sync::mpsc;

use crate::graph::{AnyNodeImpl, Graph, NodeKind};
use crate::node::Compute;
use crate::node_ref::{NodeId, NodeRef};
use crate::value_store::ValueStore;

/// Helper trait on tuple types to produce type-erased AnyNodeImpl for compute nodes.
pub trait ComputeDispatch<C: Compute<Inputs = Self> + Send + 'static>: Sized {
    fn make_node_impl(compute: C) -> Box<dyn AnyNodeImpl>;
}

/// Typestate builder for trait-based compute nodes.
/// `Remaining` starts as `C::Inputs` and shrinks as deps are wired via `.dep()`.
pub struct TraitComputeBuilder<'g, C: Compute, Remaining> {
    graph: &'g mut Graph,
    compute: C,
    dep_ids: Vec<NodeId>,
    _remaining: PhantomData<Remaining>,
}

impl<'g, C: Compute + Send + Sync + 'static> TraitComputeBuilder<'g, C, C::Inputs> {
    pub fn new(graph: &'g mut Graph, compute: C) -> Self {
        TraitComputeBuilder {
            graph,
            compute,
            dep_ids: Vec::new(),
            _remaining: PhantomData,
        }
    }
}

impl<'g, C: Compute + Send + Sync + 'static> TraitComputeBuilder<'g, C, ()>
where
    C::Inputs: ComputeDispatch<C>,
{
    pub fn build(self) -> NodeRef<C::Output> {
        let id = NodeId::from_key::<_, C>(&self.compute.key());
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };

        let dep_ids = self.dep_ids.clone();
        let node_impl = <C::Inputs as ComputeDispatch<C>>::make_node_impl(self.compute);

        self.graph
            .register_node(id, NodeKind::Compute, dep_ids, node_impl);
        node_ref
    }
}

struct TraitComputeNodeImpl<C> {
    compute: C,
    do_compute: fn(&C, &[NodeId], &ValueStore) -> Option<Box<dyn Any + Send + Sync>>,
}

impl<C: Send + Sync + 'static> AnyNodeImpl for TraitComputeNodeImpl<C> {
    fn activate(
        &self,
        _id: &NodeId,
        _tx: &mpsc::Sender<(NodeId, Box<dyn Any + Send + Sync>)>,
    ) -> Option<Box<dyn Any + Send>> {
        None
    }

    fn compute(&self, deps: &[NodeId], store: &ValueStore) -> Option<Box<dyn Any + Send + Sync>> {
        (self.do_compute)(&self.compute, deps, store)
    }

    fn emit(&self, _deps: &[NodeId], _store: &ValueStore) {}

    fn kind(&self) -> NodeKind {
        NodeKind::Compute
    }
}

macro_rules! impl_trait_compute_arity {
    ([$A:ident : 0]) => {
        impl<'g, C: Compute + Send + Sync + 'static, $A: Send + 'static> TraitComputeBuilder<'g, C, ($A,)> {
            pub fn dep(mut self, node: NodeRef<$A>) -> TraitComputeBuilder<'g, C, ()> {
                self.dep_ids.push(node.id.clone());
                TraitComputeBuilder {
                    graph: self.graph,
                    compute: self.compute,
                    dep_ids: self.dep_ids,
                    _remaining: PhantomData,
                }
            }
        }

        impl<$A: Send + 'static, C> ComputeDispatch<C> for ($A,)
        where
            C: Compute<Inputs = ($A,)> + Send + Sync + 'static,
            C::Output: Send + Sync + 'static,
        {
            fn make_node_impl(compute: C) -> Box<dyn AnyNodeImpl> {
                fn do_compute<$A: Send + 'static, C>(
                    c: &C, deps: &[NodeId], store: &ValueStore,
                ) -> Option<Box<dyn Any + Send + Sync>>
                where
                    C: Compute<Inputs = ($A,)>,
                    C::Output: Send + Sync + 'static,
                {
                    let a = store.get_by_id(&deps[0])?.downcast_ref::<$A>()?;
                    Some(Box::new(c.compute((a,))))
                }
                Box::new(TraitComputeNodeImpl {
                    compute,
                    do_compute: do_compute::<$A, C>,
                })
            }
        }
    };

    ([$First:ident : $first_idx:tt $(, $Rest:ident : $rest_idx:tt)+]) => {
        impl<'g, C: Compute + Send + Sync + 'static, $First: Send + 'static $(, $Rest: Send + 'static)+>
            TraitComputeBuilder<'g, C, ($First, $($Rest,)+)>
        {
            pub fn dep(mut self, node: NodeRef<$First>) -> TraitComputeBuilder<'g, C, ($($Rest,)+)> {
                self.dep_ids.push(node.id.clone());
                TraitComputeBuilder {
                    graph: self.graph,
                    compute: self.compute,
                    dep_ids: self.dep_ids,
                    _remaining: PhantomData,
                }
            }
        }

        impl<$First: Send + 'static $(, $Rest: Send + 'static)+, C> ComputeDispatch<C> for ($First, $($Rest,)+)
        where
            C: Compute<Inputs = ($First, $($Rest,)+)> + Send + Sync + 'static,
            C::Output: Send + Sync + 'static,
        {
            fn make_node_impl(compute: C) -> Box<dyn AnyNodeImpl> {
                fn do_compute<$First: Send + 'static $(, $Rest: Send + 'static)+, C>(
                    c: &C, deps: &[NodeId], store: &ValueStore,
                ) -> Option<Box<dyn Any + Send + Sync>>
                where
                    C: Compute<Inputs = ($First, $($Rest,)+)>,
                    C::Output: Send + Sync + 'static,
                {
                    let $First = store.get_by_id(&deps[$first_idx])?.downcast_ref::<$First>()?;
                    $(
                        let $Rest = store.get_by_id(&deps[$rest_idx])?.downcast_ref::<$Rest>()?;
                    )+
                    Some(Box::new(c.compute(($First, $($Rest),+))))
                }
                Box::new(TraitComputeNodeImpl {
                    compute,
                    do_compute: do_compute::<$First, $($Rest,)+ C>,
                })
            }
        }
    };
}

impl_trait_compute_arity!([A: 0]);
impl_trait_compute_arity!([A: 0, B: 1]);
impl_trait_compute_arity!([A: 0, B: 1, Cc: 2]);
impl_trait_compute_arity!([A: 0, B: 1, Cc: 2, D: 3]);
impl_trait_compute_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4]);
impl_trait_compute_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4, Ff: 5]);
impl_trait_compute_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4, Ff: 5, G: 6]);
impl_trait_compute_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4, Ff: 5, G: 6, H: 7]);
