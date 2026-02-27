use std::any::Any;
use std::sync::mpsc;

use crate::node_ref::NodeId;
use crate::subgraph::Subgraph;

/// Commands sent from EngineHandle to the engine event loop.
pub enum Command {
    /// Update a source value.
    SourceUpdate {
        id: NodeId,
        value: Box<dyn Any + Send + Sync>,
    },
    /// Merge a subgraph and return the node IDs via the reply channel.
    Merge {
        subgraph: Subgraph,
        reply: mpsc::Sender<Vec<NodeId>>,
    },
    /// Remove nodes (decrement ref counts).
    Remove { node_ids: Vec<NodeId> },
    /// Notification that an async computation completed (wake event loop).
    AsyncComplete,
    /// Notification that a source value arrived (wake event loop).
    SourceReady,
    /// Shut down the event loop.
    Shutdown,
}
