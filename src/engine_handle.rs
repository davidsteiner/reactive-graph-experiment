use std::any::Any;
use std::marker::PhantomData;
use std::sync::mpsc;

use crate::command::Command;
use crate::node_ref::{NodeId, NodeRef};
use crate::subgraph::Subgraph;

/// A Send + Sync + Clone handle to an engine running on its own thread.
/// All operations are serialized through a message channel.
#[derive(Clone)]
pub struct EngineHandle {
    tx: mpsc::Sender<Command>,
}

// EngineHandle is Send + Sync because mpsc::Sender is Send
// (and we only store the sender, not the receiver).
// mpsc::Sender is not Sync, but we wrap it in Clone so each thread gets its own.

impl EngineHandle {
    pub(crate) fn new(tx: mpsc::Sender<Command>) -> Self {
        EngineHandle { tx }
    }

    /// Type-safe source update. The value type must match the NodeRef<T>.
    pub fn update_source<T: Send + Sync + 'static>(&self, node_ref: &NodeRef<T>, value: T) {
        let _ = self.tx.send(Command::SourceUpdate {
            id: node_ref.id.clone(),
            value: Box::new(value),
        });
    }

    /// Merge a subgraph into the engine. Returns the node IDs of merged nodes.
    /// Blocks until the engine processes the merge.
    pub fn merge(&self, subgraph: Subgraph) -> RemoteSubgraphHandle {
        let (reply_tx, reply_rx) = mpsc::channel();
        let _ = self.tx.send(Command::Merge {
            subgraph,
            reply: reply_tx,
        });
        let node_ids = reply_rx.recv().unwrap_or_default();
        RemoteSubgraphHandle {
            node_ids,
            tx: self.tx.clone(),
        }
    }

    /// Shut down the engine event loop.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

/// Handle to a merged subgraph via EngineHandle. Dropping sends a Remove command.
pub struct RemoteSubgraphHandle {
    node_ids: Vec<NodeId>,
    tx: mpsc::Sender<Command>,
}

impl Drop for RemoteSubgraphHandle {
    fn drop(&mut self) {
        if !self.node_ids.is_empty() {
            let _ = self.tx.send(Command::Remove {
                node_ids: std::mem::take(&mut self.node_ids),
            });
        }
    }
}
