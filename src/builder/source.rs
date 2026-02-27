use std::any::Any;
use std::marker::PhantomData;
use std::sync::{Mutex, mpsc};

use crate::graph::{AnyNodeImpl, Graph, NodeKind};
use crate::node_ref::{NodeId, NodeRef};
use crate::sender::Sender;

/// Marker type for closure-based source nodes (used as TypeId scope).
pub(crate) struct ClosureSource;

/// Builder for closure-based source nodes.
pub struct SourceBuilder<T> {
    key: String,
    activate_fn: Option<Box<dyn FnOnce(Sender<T>) -> Box<dyn Any + Send> + Send>>,
    _marker: PhantomData<T>,
}

impl<T: Send + Sync + 'static> SourceBuilder<T> {
    pub fn new(key: &str) -> Self {
        SourceBuilder {
            key: key.to_string(),
            activate_fn: None,
            _marker: PhantomData,
        }
    }

    /// Set the activation callback.
    pub fn on_activate<G: Send + 'static>(
        mut self,
        f: impl FnOnce(Sender<T>) -> G + Send + 'static,
    ) -> Self {
        self.activate_fn = Some(Box::new(move |tx| Box::new(f(tx))));
        self
    }

    /// Consume into key and node impl.
    pub fn into_parts(self) -> (String, Box<dyn AnyNodeImpl>) {
        (
            self.key,
            Box::new(ClosureSourceActivatable {
                activate_fn: Mutex::new(self.activate_fn),
            }),
        )
    }

    /// Finalize and register the source in the graph.
    pub fn build(self, graph: &mut Graph) -> NodeRef<T> {
        let (key, node_impl) = self.into_parts();
        let id = NodeId::from_key::<_, ClosureSource>(&key);
        let node_ref = NodeRef {
            id: id.clone(),
            _marker: PhantomData,
        };
        graph.register_node(id, NodeKind::Source, vec![], node_impl);
        node_ref
    }
}

/// Closure source that can be activated once (FnOnce).
pub(crate) struct ClosureSourceActivatable<T> {
    pub(crate) activate_fn: Mutex<Option<Box<dyn FnOnce(Sender<T>) -> Box<dyn Any + Send> + Send>>>,
}

impl<T: Send + Sync + 'static> AnyNodeImpl for ClosureSourceActivatable<T> {
    fn activate(
        &self,
        id: &NodeId,
        tx: &mpsc::Sender<(NodeId, Box<dyn Any + Send + Sync>)>,
    ) -> Option<Box<dyn Any + Send>> {
        let f = self.activate_fn.lock().ok()?.take()?;
        let sender = Sender {
            id: id.clone(),
            tx: tx.clone(),
            _marker: PhantomData,
        };
        Some(f(sender))
    }

    fn compute(
        &self,
        _deps: &[NodeId],
        _store: &crate::value_store::ValueStore,
    ) -> Option<Box<dyn Any + Send + Sync>> {
        None
    }

    fn emit(&self, _deps: &[NodeId], _store: &crate::value_store::ValueStore) {}

    fn kind(&self) -> NodeKind {
        NodeKind::Source
    }
}
