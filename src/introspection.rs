use crate::graph::NodeKind;
use crate::node_ref::NodeId;

/// Information about a single node in the graph.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub id: NodeId,
    pub kind: NodeKind,
    pub deps: Vec<NodeId>,
    pub ref_count: usize,
}

/// A snapshot of the graph at a point in time.
#[derive(Debug, Clone)]
pub struct GraphSnapshot {
    pub nodes: Vec<NodeInfo>,
    pub edges: Vec<(NodeId, NodeId)>,
}
