use std::any::Any;
use std::marker::PhantomData;
use std::sync::{Mutex, mpsc};

use crate::graph::{AnyNodeImpl, Graph, NodeKind};
use crate::node::Source;
use crate::node_ref::{NodeId, NodeRef};
use crate::sender::Sender;
use crate::value_store::ValueStore;

/// Create a type-erased AnyNodeImpl for a trait-based source.
pub fn make_source_impl<S: Source + Send + Sync + 'static>(source: S) -> Box<dyn AnyNodeImpl> {
    Box::new(TraitSourceImpl {
        source: Mutex::new(Some(source)),
    })
}

/// Register a trait-based source in the graph.
pub fn register_source<S: Source + Send + Sync + 'static>(
    graph: &mut Graph,
    source: S,
) -> NodeRef<S::Output> {
    let id = NodeId::from_key::<_, S>(&source.key());
    let node_ref = NodeRef {
        id: id.clone(),
        _marker: PhantomData,
    };

    let node_impl = make_source_impl(source);
    graph.register_node(id, NodeKind::Source, vec![], node_impl);
    node_ref
}

struct TraitSourceImpl<S> {
    source: Mutex<Option<S>>,
}

impl<S: Source + Send + Sync + 'static> AnyNodeImpl for TraitSourceImpl<S> {
    fn activate(
        &self,
        id: &NodeId,
        tx: &mpsc::Sender<(NodeId, Box<dyn Any + Send + Sync>)>,
    ) -> Option<Box<dyn Any + Send>> {
        let source = self.source.lock().ok()?;
        let source = source.as_ref()?;
        let sender = Sender {
            id: id.clone(),
            tx: tx.clone(),
            _marker: PhantomData,
        };
        let guard = source.activate(sender);
        Some(Box::new(guard))
    }

    fn compute(&self, _deps: &[NodeId], _store: &ValueStore) -> Option<Box<dyn Any + Send + Sync>> {
        None
    }

    fn emit(&self, _deps: &[NodeId], _store: &ValueStore) {}

    fn kind(&self) -> NodeKind {
        NodeKind::Source
    }
}
