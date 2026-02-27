use std::any::Any;
use std::marker::PhantomData;
use std::sync::mpsc;

use crate::node_ref::NodeId;

/// A typed handle for pushing values into the graph from a source.
/// The type parameter ensures only values of the correct type can be sent.
pub struct Sender<T> {
    pub(crate) id: NodeId,
    pub(crate) tx: mpsc::Sender<(NodeId, Box<dyn Any + Send + Sync>)>,
    pub(crate) _marker: PhantomData<fn(T)>,
}

impl<T: Send + Sync + 'static> Sender<T> {
    pub fn send(&self, value: T) {
        let _ = self.tx.send((self.id.clone(), Box::new(value)));
    }
}

// Sender is Send because it contains NodeId (Send), mpsc::Sender (Send), and PhantomData (Send).
// Note: mpsc::Sender is !Sync, so Sender is also !Sync (auto-derived correctly).

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender {
            id: self.id.clone(),
            tx: self.tx.clone(),
            _marker: PhantomData,
        }
    }
}
