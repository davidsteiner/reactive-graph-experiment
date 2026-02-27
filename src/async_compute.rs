use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc;

use crate::command::Command;
use crate::graph::AsyncPolicy;
use crate::node_ref::NodeId;

/// Handle for a concurrency group. Created by Engine::concurrency_group().
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConcurrencyGroupHandle(pub(crate) usize);

struct AsyncNodeState {
    policy: AsyncPolicy,
    in_flight: Option<tokio::task::JoinHandle<()>>,
    group_id: Option<usize>,
    pending: VecDeque<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>>,
}

struct ConcurrencyGroupState {
    max_concurrent: usize,
    in_flight: usize,
    queue: VecDeque<QueuedWork>,
}

struct QueuedWork {
    node_id: NodeId,
    future: Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>,
}

/// Manages async compute execution, policies, and concurrency groups.
pub struct AsyncManager {
    runtime: tokio::runtime::Runtime,
    result_tx: mpsc::Sender<(NodeId, Box<dyn Any + Send + Sync>)>,
    pub(crate) result_rx: mpsc::Receiver<(NodeId, Box<dyn Any + Send + Sync>)>,
    node_states: HashMap<NodeId, AsyncNodeState>,
    groups: HashMap<usize, ConcurrencyGroupState>,
    next_group_id: usize,
    notifier: Option<mpsc::Sender<Command>>,
}

impl AsyncManager {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");
        let (result_tx, result_rx) = mpsc::channel();
        AsyncManager {
            runtime,
            result_tx,
            result_rx,
            node_states: HashMap::new(),
            groups: HashMap::new(),
            next_group_id: 0,
            notifier: None,
        }
    }

    pub fn set_notifier(&mut self, notifier: mpsc::Sender<Command>) {
        self.notifier = Some(notifier);
    }

    pub fn create_group(&mut self, max_concurrent: usize) -> ConcurrencyGroupHandle {
        let id = self.next_group_id;
        self.next_group_id += 1;
        self.groups.insert(
            id,
            ConcurrencyGroupState {
                max_concurrent,
                in_flight: 0,
                queue: VecDeque::new(),
            },
        );
        ConcurrencyGroupHandle(id)
    }

    /// Spawn an async computation for a node, respecting its policy and concurrency group.
    pub fn spawn(
        &mut self,
        node_id: NodeId,
        future: Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>,
        policy: AsyncPolicy,
        group_id: Option<usize>,
    ) {
        let state = self
            .node_states
            .entry(node_id.clone())
            .or_insert_with(|| AsyncNodeState {
                policy: policy.clone(),
                in_flight: None,
                group_id,
                pending: VecDeque::new(),
            });

        match &state.policy {
            AsyncPolicy::LatestWins => {
                // Cancel existing
                if let Some(handle) = state.in_flight.take() {
                    handle.abort();
                    if let Some(gid) = state.group_id {
                        if let Some(group) = self.groups.get_mut(&gid) {
                            group.in_flight = group.in_flight.saturating_sub(1);
                        }
                    }
                }
                self.try_spawn(node_id, future);
            }
            AsyncPolicy::Queue => {
                if state.in_flight.is_some() {
                    state.pending.push_back(future);
                } else {
                    self.try_spawn(node_id, future);
                }
            }
            AsyncPolicy::Debounce(duration) => {
                let duration = *duration;
                // Cancel existing (both debounce timer and computation)
                if let Some(handle) = state.in_flight.take() {
                    handle.abort();
                    if let Some(gid) = state.group_id {
                        if let Some(group) = self.groups.get_mut(&gid) {
                            group.in_flight = group.in_flight.saturating_sub(1);
                        }
                    }
                }
                self.spawn_debounced(node_id, future, duration);
            }
        }
    }

    fn try_spawn(
        &mut self,
        node_id: NodeId,
        future: Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>,
    ) {
        let group_id = self.node_states.get(&node_id).and_then(|s| s.group_id);

        // Check group capacity
        if let Some(gid) = group_id {
            if let Some(group) = self.groups.get_mut(&gid) {
                if group.in_flight >= group.max_concurrent {
                    group.queue.push_back(QueuedWork { node_id, future });
                    return;
                }
                group.in_flight += 1;
            }
        }

        self.spawn_raw(node_id, future);
    }

    fn spawn_debounced(
        &mut self,
        node_id: NodeId,
        future: Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>,
        duration: std::time::Duration,
    ) {
        let group_id = self.node_states.get(&node_id).and_then(|s| s.group_id);

        if let Some(gid) = group_id {
            if let Some(group) = self.groups.get_mut(&gid) {
                if group.in_flight >= group.max_concurrent {
                    group.queue.push_back(QueuedWork { node_id, future });
                    return;
                }
                group.in_flight += 1;
            }
        }

        let tx = self.result_tx.clone();
        let id = node_id.clone();
        let notifier = self.notifier.clone();
        let handle = self.runtime.spawn(async move {
            tokio::time::sleep(duration).await;
            let result = future.await;
            let _ = tx.send((id, result));
            if let Some(n) = notifier {
                let _ = n.send(Command::AsyncComplete);
            }
        });

        if let Some(state) = self.node_states.get_mut(&node_id) {
            state.in_flight = Some(handle);
        }
    }

    fn spawn_raw(
        &mut self,
        node_id: NodeId,
        future: Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>,
    ) {
        let tx = self.result_tx.clone();
        let id = node_id.clone();
        let notifier = self.notifier.clone();
        let handle = self.runtime.spawn(async move {
            let result = future.await;
            let _ = tx.send((id, result));
            if let Some(n) = notifier {
                let _ = n.send(Command::AsyncComplete);
            }
        });

        if let Some(state) = self.node_states.get_mut(&node_id) {
            state.in_flight = Some(handle);
        }
    }

    /// Drain completed async results and manage queues.
    pub fn drain_results(&mut self) -> Vec<(NodeId, Box<dyn Any + Send + Sync>)> {
        let mut results = Vec::new();
        while let Ok(result) = self.result_rx.try_recv() {
            results.push(result);
        }

        let completed_ids: Vec<NodeId> = results.iter().map(|(id, _)| id.clone()).collect();
        for id in completed_ids {
            self.process_completion(&id);
        }

        results
    }

    fn process_completion(&mut self, completed_id: &NodeId) {
        let (group_id, pending_future) = {
            let state = match self.node_states.get_mut(completed_id) {
                Some(s) => s,
                None => return,
            };
            state.in_flight = None;
            let pending = if matches!(state.policy, AsyncPolicy::Queue) {
                state.pending.pop_front()
            } else {
                None
            };
            (state.group_id, pending)
        };

        // Handle concurrency group
        if let Some(gid) = group_id {
            if let Some(group) = self.groups.get_mut(&gid) {
                group.in_flight = group.in_flight.saturating_sub(1);
            }
            self.try_spawn_from_group_queue(gid);
        }

        // Handle queue policy: spawn next queued task
        if let Some(future) = pending_future {
            self.try_spawn(completed_id.clone(), future);
        }
    }

    fn try_spawn_from_group_queue(&mut self, group_id: usize) {
        loop {
            let can_spawn = self
                .groups
                .get(&group_id)
                .map(|g| g.in_flight < g.max_concurrent && !g.queue.is_empty())
                .unwrap_or(false);

            if !can_spawn {
                break;
            }

            let work = self
                .groups
                .get_mut(&group_id)
                .unwrap()
                .queue
                .pop_front()
                .unwrap();
            self.groups.get_mut(&group_id).unwrap().in_flight += 1;
            self.spawn_raw(work.node_id, work.future);
        }
    }

    /// Get the number of in-flight tasks for a concurrency group.
    pub fn group_in_flight(&self, handle: ConcurrencyGroupHandle) -> usize {
        self.groups.get(&handle.0).map(|g| g.in_flight).unwrap_or(0)
    }
}
