use std::any::Any;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::mpsc;

use crate::async_compute::ConcurrencyGroupHandle;
use crate::graph::{AnyNodeImpl, AsyncPolicy, Graph, NodeKind};
use crate::node::AsyncCompute;
use crate::node_ref::{NodeId, NodeRef};
use crate::value_store::ValueStore;

/// Marker type for closure-based async compute nodes.
pub(crate) struct ClosureAsyncCompute;

/// Builder for closure-based async compute nodes.
pub struct AsyncComputeBuilder<Out, Deps> {
    pub(crate) key: String,
    pub(crate) dep_ids: Vec<NodeId>,
    pub(crate) _marker: PhantomData<(Out, Deps)>,
}

impl<Out: Send + Sync + 'static> AsyncComputeBuilder<Out, ()> {
    pub fn new(key: &str) -> Self {
        AsyncComputeBuilder {
            key: key.to_string(),
            dep_ids: Vec::new(),
            _marker: PhantomData,
        }
    }
}

/// Builder after .compute_async() has been called.
pub struct AsyncComputeBuilderReady<Out> {
    key: String,
    dep_ids: Vec<NodeId>,
    node_impl: Box<dyn AnyNodeImpl>,
    policy: AsyncPolicy,
    group: Option<ConcurrencyGroupHandle>,
    _marker: PhantomData<Out>,
}

impl<Out: Send + Sync + 'static> AsyncComputeBuilderReady<Out> {
    pub fn policy(mut self, policy: AsyncPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn group(mut self, group: ConcurrencyGroupHandle) -> Self {
        self.group = Some(group);
        self
    }

    pub fn build(self, graph: &mut Graph) -> NodeRef<Out> {
        let id = NodeId::from_key::<_, ClosureAsyncCompute>(&self.key);
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };
        // Wrap the node_impl to apply the builder-level policy and group
        let node_impl: Box<dyn AnyNodeImpl> = Box::new(AsyncPolicyWrapper {
            inner: self.node_impl,
            policy: self.policy,
            group_id: self.group.map(|g| g.0),
        });
        graph.register_node(id, NodeKind::AsyncCompute, self.dep_ids, node_impl);
        node_ref
    }

    pub fn into_parts(
        self,
    ) -> (
        String,
        Vec<NodeId>,
        Box<dyn AnyNodeImpl>,
        AsyncPolicy,
        Option<ConcurrencyGroupHandle>,
    ) {
        (
            self.key,
            self.dep_ids,
            self.node_impl,
            self.policy,
            self.group,
        )
    }
}

/// Wrapper that overrides async_policy and concurrency_group_id on any AnyNodeImpl.
struct AsyncPolicyWrapper {
    inner: Box<dyn AnyNodeImpl>,
    policy: AsyncPolicy,
    group_id: Option<usize>,
}

impl AnyNodeImpl for AsyncPolicyWrapper {
    fn activate(
        &self,
        id: &NodeId,
        tx: &mpsc::Sender<(NodeId, Box<dyn Any + Send + Sync>)>,
    ) -> Option<Box<dyn Any + Send>> {
        self.inner.activate(id, tx)
    }

    fn compute(&self, deps: &[NodeId], store: &ValueStore) -> Option<Box<dyn Any + Send + Sync>> {
        self.inner.compute(deps, store)
    }

    fn compute_async(
        &self,
        deps: &[NodeId],
        store: &ValueStore,
    ) -> Option<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>> {
        self.inner.compute_async(deps, store)
    }

    fn emit(&self, deps: &[NodeId], store: &ValueStore) {
        self.inner.emit(deps, store)
    }

    fn kind(&self) -> NodeKind {
        self.inner.kind()
    }

    fn async_policy(&self) -> AsyncPolicy {
        self.policy.clone()
    }

    fn concurrency_group_id(&self) -> Option<usize> {
        self.group_id
    }
}

/// Type-erased async compute implementation.
struct ClosureAsyncComputeNode<Func> {
    func: Func,
    extract_and_call:
        fn(
            &Func,
            &[NodeId],
            &ValueStore,
        ) -> Option<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>>,
    policy: AsyncPolicy,
    group_id: Option<usize>,
}

impl<Func: Send + Sync + 'static> AnyNodeImpl for ClosureAsyncComputeNode<Func> {
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

    fn compute_async(
        &self,
        deps: &[NodeId],
        store: &ValueStore,
    ) -> Option<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>> {
        (self.extract_and_call)(&self.func, deps, store)
    }

    fn emit(&self, _deps: &[NodeId], _store: &ValueStore) {}

    fn kind(&self) -> NodeKind {
        NodeKind::AsyncCompute
    }

    fn async_policy(&self) -> AsyncPolicy {
        self.policy.clone()
    }

    fn concurrency_group_id(&self) -> Option<usize> {
        self.group_id
    }
}

// === Closure builder dep/compute_async macros ===

macro_rules! impl_async_compute_arity {
    // Arity 1: () -> (A,)
    (start => ($A:ident), (0)) => {
        impl<Out: Send + Sync + 'static> AsyncComputeBuilder<Out, ()> {
            pub fn dep<$A: Send + 'static>(
                mut self,
                node: NodeRef<$A>,
            ) -> AsyncComputeBuilder<Out, ($A,)> {
                self.dep_ids.push(node.id.clone());
                AsyncComputeBuilder {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    _marker: PhantomData,
                }
            }
        }

        impl<Out: Send + Sync + 'static, $A: Send + 'static> AsyncComputeBuilder<Out, ($A,)> {
            pub fn compute_async<Func>(self, f: Func) -> AsyncComputeBuilderReady<Out>
            where
                Func: Fn(&$A) -> Pin<Box<dyn Future<Output = Out> + Send>> + Send + Sync + 'static,
            {
                fn extract_and_call<
                    Out: Send + Sync + 'static,
                    $A: 'static,
                    Func: Fn(&$A) -> Pin<Box<dyn Future<Output = Out> + Send>>,
                >(
                    f: &Func,
                    deps: &[NodeId],
                    store: &ValueStore,
                ) -> Option<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>>
                {
                    let a = store.get_by_id(&deps[0])?.downcast_ref::<$A>()?;
                    let future = f(a);
                    Some(Box::pin(async move {
                        let result = future.await;
                        Box::new(result) as Box<dyn Any + Send + Sync>
                    }))
                }
                AsyncComputeBuilderReady {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    node_impl: Box::new(ClosureAsyncComputeNode {
                        func: f,
                        extract_and_call: extract_and_call::<Out, $A, Func>,
                        policy: AsyncPolicy::default(),
                        group_id: None,
                    }),
                    policy: AsyncPolicy::default(),
                    group: None,
                    _marker: PhantomData,
                }
            }
        }
    };

    // Arity N
    (($($Prev:ident),+) => ($($All:ident),+), ($($idx:tt),+)) => {
        impl<Out: Send + Sync + 'static, $($Prev: Send + 'static),+> AsyncComputeBuilder<Out, ($($Prev,)+)> {
            pub fn dep<Last: Send + 'static>(
                mut self,
                node: NodeRef<Last>,
            ) -> AsyncComputeBuilder<Out, ($($Prev,)+ Last,)> {
                self.dep_ids.push(node.id.clone());
                AsyncComputeBuilder {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    _marker: PhantomData,
                }
            }
        }

        impl<Out: Send + Sync + 'static, $($All: Send + 'static),+> AsyncComputeBuilder<Out, ($($All,)+)> {
            pub fn compute_async<Func>(self, f: Func) -> AsyncComputeBuilderReady<Out>
            where
                Func: Fn($(&$All),+) -> Pin<Box<dyn Future<Output = Out> + Send>>
                    + Send
                    + Sync
                    + 'static,
            {
                fn extract_and_call<
                    Out: Send + Sync + 'static,
                    $($All: 'static,)+
                    Func: Fn($(&$All),+) -> Pin<Box<dyn Future<Output = Out> + Send>>,
                >(
                    f: &Func,
                    deps: &[NodeId],
                    store: &ValueStore,
                ) -> Option<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>>
                {
                    $(
                        let $All = store.get_by_id(&deps[$idx])?.downcast_ref::<$All>()?;
                    )+
                    let future = f($($All),+);
                    Some(Box::pin(async move {
                        let result = future.await;
                        Box::new(result) as Box<dyn Any + Send + Sync>
                    }))
                }
                AsyncComputeBuilderReady {
                    key: self.key,
                    dep_ids: self.dep_ids,
                    node_impl: Box::new(ClosureAsyncComputeNode {
                        func: f,
                        extract_and_call: extract_and_call::<Out, $($All,)+ Func>,
                        policy: AsyncPolicy::default(),
                        group_id: None,
                    }),
                    policy: AsyncPolicy::default(),
                    group: None,
                    _marker: PhantomData,
                }
            }
        }
    };
}

impl_async_compute_arity!(start => (A), (0));
impl_async_compute_arity!((A) => (A, B), (0, 1));
impl_async_compute_arity!((A, B) => (A, B, Cc), (0, 1, 2));
impl_async_compute_arity!((A, B, Cc) => (A, B, Cc, D), (0, 1, 2, 3));
impl_async_compute_arity!((A, B, Cc, D) => (A, B, Cc, D, E), (0, 1, 2, 3, 4));
impl_async_compute_arity!((A, B, Cc, D, E) => (A, B, Cc, D, E, Ff), (0, 1, 2, 3, 4, 5));
impl_async_compute_arity!((A, B, Cc, D, E, Ff) => (A, B, Cc, D, E, Ff, G), (0, 1, 2, 3, 4, 5, 6));
impl_async_compute_arity!((A, B, Cc, D, E, Ff, G) => (A, B, Cc, D, E, Ff, G, H), (0, 1, 2, 3, 4, 5, 6, 7));

// === Trait-based async compute builder ===

/// Helper trait on tuple types to produce type-erased AnyNodeImpl for async compute nodes.
pub trait AsyncComputeDispatch<AC: AsyncCompute<Inputs = Self> + Send + 'static>: Sized {
    fn make_node_impl(ac: AC, policy: AsyncPolicy, group_id: Option<usize>)
    -> Box<dyn AnyNodeImpl>;
}

/// Typestate builder for trait-based async compute nodes.
pub struct TraitAsyncComputeBuilder<'g, AC: AsyncCompute, Remaining> {
    graph: &'g mut Graph,
    ac: AC,
    dep_ids: Vec<NodeId>,
    policy: AsyncPolicy,
    group: Option<ConcurrencyGroupHandle>,
    _remaining: PhantomData<Remaining>,
}

impl<'g, AC: AsyncCompute + Send + Sync + 'static> TraitAsyncComputeBuilder<'g, AC, AC::Inputs> {
    pub fn new(graph: &'g mut Graph, ac: AC) -> Self {
        TraitAsyncComputeBuilder {
            graph,
            ac,
            dep_ids: Vec::new(),
            policy: AsyncPolicy::default(),
            group: None,
            _remaining: PhantomData,
        }
    }
}

impl<'g, AC: AsyncCompute + Send + Sync + 'static> TraitAsyncComputeBuilder<'g, AC, ()>
where
    AC::Inputs: AsyncComputeDispatch<AC>,
{
    pub fn policy(mut self, policy: AsyncPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn group(mut self, group: ConcurrencyGroupHandle) -> Self {
        self.group = Some(group);
        self
    }

    pub fn build(self) -> NodeRef<AC::Output> {
        let id = NodeId::from_key::<_, AC>(&self.ac.key());
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };

        let dep_ids = self.dep_ids.clone();
        let group_id = self.group.map(|g| g.0);
        let node_impl = <AC::Inputs as AsyncComputeDispatch<AC>>::make_node_impl(
            self.ac,
            self.policy,
            group_id,
        );

        self.graph
            .register_node(id, NodeKind::AsyncCompute, dep_ids, node_impl);
        node_ref
    }
}

struct TraitAsyncComputeNodeImpl<AC> {
    ac: AC,
    do_compute: fn(
        &AC,
        &[NodeId],
        &ValueStore,
    ) -> Option<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>>,
    policy: AsyncPolicy,
    group_id: Option<usize>,
}

impl<AC: Send + Sync + 'static> AnyNodeImpl for TraitAsyncComputeNodeImpl<AC> {
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

    fn compute_async(
        &self,
        deps: &[NodeId],
        store: &ValueStore,
    ) -> Option<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>> {
        (self.do_compute)(&self.ac, deps, store)
    }

    fn emit(&self, _deps: &[NodeId], _store: &ValueStore) {}

    fn kind(&self) -> NodeKind {
        NodeKind::AsyncCompute
    }

    fn async_policy(&self) -> AsyncPolicy {
        self.policy.clone()
    }

    fn concurrency_group_id(&self) -> Option<usize> {
        self.group_id
    }
}

macro_rules! impl_trait_async_compute_arity {
    ([$A:ident : 0]) => {
        impl<'g, AC: AsyncCompute + Send + Sync + 'static, $A: Send + 'static>
            TraitAsyncComputeBuilder<'g, AC, ($A,)>
        {
            pub fn dep(mut self, node: NodeRef<$A>) -> TraitAsyncComputeBuilder<'g, AC, ()> {
                self.dep_ids.push(node.id.clone());
                TraitAsyncComputeBuilder {
                    graph: self.graph,
                    ac: self.ac,
                    dep_ids: self.dep_ids,
                    policy: self.policy,
                    group: self.group,
                    _remaining: PhantomData,
                }
            }
        }

        impl<$A: Send + 'static, AC> AsyncComputeDispatch<AC> for ($A,)
        where
            AC: AsyncCompute<Inputs = ($A,)> + Send + Sync + 'static,
            AC::Output: Send + Sync + 'static,
        {
            fn make_node_impl(
                ac: AC,
                policy: AsyncPolicy,
                group_id: Option<usize>,
            ) -> Box<dyn AnyNodeImpl> {
                fn do_compute<$A: Send + 'static, AC>(
                    c: &AC,
                    deps: &[NodeId],
                    store: &ValueStore,
                ) -> Option<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>>
                where
                    AC: AsyncCompute<Inputs = ($A,)>,
                    AC::Output: Send + Sync + 'static,
                {
                    let a = store.get_by_id(&deps[0])?.downcast_ref::<$A>()?;
                    let future = c.compute((a,));
                    Some(Box::pin(async move {
                        let result = future.await;
                        Box::new(result) as Box<dyn Any + Send + Sync>
                    }))
                }
                Box::new(TraitAsyncComputeNodeImpl {
                    ac,
                    do_compute: do_compute::<$A, AC>,
                    policy,
                    group_id,
                })
            }
        }
    };

    ([$First:ident : $first_idx:tt $(, $Rest:ident : $rest_idx:tt)+]) => {
        impl<'g, AC: AsyncCompute + Send + Sync + 'static, $First: Send + 'static $(, $Rest: Send + 'static)+>
            TraitAsyncComputeBuilder<'g, AC, ($First, $($Rest,)+)>
        {
            pub fn dep(mut self, node: NodeRef<$First>) -> TraitAsyncComputeBuilder<'g, AC, ($($Rest,)+)> {
                self.dep_ids.push(node.id.clone());
                TraitAsyncComputeBuilder {
                    graph: self.graph,
                    ac: self.ac,
                    dep_ids: self.dep_ids,
                    policy: self.policy,
                    group: self.group,
                    _remaining: PhantomData,
                }
            }
        }

        impl<$First: Send + 'static $(, $Rest: Send + 'static)+, AC> AsyncComputeDispatch<AC>
            for ($First, $($Rest,)+)
        where
            AC: AsyncCompute<Inputs = ($First, $($Rest,)+)> + Send + Sync + 'static,
            AC::Output: Send + Sync + 'static,
        {
            fn make_node_impl(
                ac: AC,
                policy: AsyncPolicy,
                group_id: Option<usize>,
            ) -> Box<dyn AnyNodeImpl> {
                fn do_compute<$First: Send + 'static $(, $Rest: Send + 'static)+, AC>(
                    c: &AC,
                    deps: &[NodeId],
                    store: &ValueStore,
                ) -> Option<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>>
                where
                    AC: AsyncCompute<Inputs = ($First, $($Rest,)+)>,
                    AC::Output: Send + Sync + 'static,
                {
                    let $First = store.get_by_id(&deps[$first_idx])?.downcast_ref::<$First>()?;
                    $(
                        let $Rest = store.get_by_id(&deps[$rest_idx])?.downcast_ref::<$Rest>()?;
                    )+
                    let future = c.compute(($First, $($Rest),+));
                    Some(Box::pin(async move {
                        let result = future.await;
                        Box::new(result) as Box<dyn Any + Send + Sync>
                    }))
                }
                Box::new(TraitAsyncComputeNodeImpl {
                    ac,
                    do_compute: do_compute::<$First, $($Rest,)+ AC>,
                    policy,
                    group_id,
                })
            }
        }
    };
}

impl_trait_async_compute_arity!([A: 0]);
impl_trait_async_compute_arity!([A: 0, B: 1]);
impl_trait_async_compute_arity!([A: 0, B: 1, Cc: 2]);
impl_trait_async_compute_arity!([A: 0, B: 1, Cc: 2, D: 3]);
impl_trait_async_compute_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4]);
impl_trait_async_compute_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4, Ff: 5]);
impl_trait_async_compute_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4, Ff: 5, G: 6]);
impl_trait_async_compute_arity!([A: 0, B: 1, Cc: 2, D: 3, E: 4, Ff: 5, G: 6, H: 7]);
