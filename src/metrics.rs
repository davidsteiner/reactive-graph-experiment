use std::collections::HashMap;
use std::time::Duration;

use crate::node_ref::NodeId;

/// Metrics for a single propagation cycle.
#[derive(Debug, Clone)]
pub struct CycleMetrics {
    pub cycle_id: u64,
    pub duration: Duration,
    pub nodes_computed: usize,
    pub per_node_durations: HashMap<NodeId, Duration>,
}

/// Aggregate engine metrics.
#[derive(Debug, Clone)]
pub struct EngineMetrics {
    pub total_cycles: u64,
    pub active_sources: usize,
    pub active_computes: usize,
    pub active_sinks: usize,
    pub active_async_computes: usize,
    pub pending_async: usize,
    pub last_cycle: Option<CycleMetrics>,
}

/// Collects metrics during engine operation.
pub struct MetricsCollector {
    total_cycles: u64,
    last_cycle: Option<CycleMetrics>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        MetricsCollector {
            total_cycles: 0,
            last_cycle: None,
        }
    }

    /// Record a completed cycle.
    pub fn record_cycle(&mut self, metrics: CycleMetrics) {
        self.total_cycles += 1;
        self.last_cycle = Some(metrics);
    }

    pub fn total_cycles(&self) -> u64 {
        self.total_cycles
    }

    pub fn last_cycle(&self) -> Option<&CycleMetrics> {
        self.last_cycle.as_ref()
    }
}
