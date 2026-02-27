# Async Compute

Async compute nodes perform work that can't complete within a single propagation cycle — external API calls, I/O, long-running computations. The result feeds back into the graph as a synthetic source update, triggering a new propagation cycle.

## How It Works

1. During a propagation cycle, when an async compute node is dirty, `compute_async` is called. It returns `Option<Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + Send>>>`.
2. The engine collects all async futures from the cycle into `Vec<AsyncSpawn>`.
3. After the sync cycle completes, futures are handed to `AsyncManager::spawn()`.
4. `AsyncManager` spawns them on a tokio multi-thread runtime (2 worker threads).
5. When a future completes, the result is sent through an mpsc channel (`result_tx`), and a `Command::AsyncComplete` notification wakes the event loop.
6. On the next event loop iteration, `drain_results()` pulls completed results into the value store, which triggers forward propagation from those nodes.

## Policies

Each async node has an `AsyncPolicy` that controls behavior when new inputs arrive while a computation is in-flight:

### LatestWins (default)
The in-flight `JoinHandle` is aborted and a new computation starts with the latest inputs. Only the most recent result matters.

### Queue
The in-flight computation runs to completion. The new future is stored in a per-node `VecDeque`. When the current computation finishes, the next queued future is spawned. This guarantees sequential execution per node.

### Debounce(Duration)
The in-flight computation is aborted. A new tokio task is spawned that first `sleep`s for the debounce duration, then executes the future. If another trigger arrives during the sleep, the task is aborted again and the timer resets. Only inputs that have "settled" produce a computation.

## Concurrency Groups

A concurrency group limits how many async computations can be in-flight simultaneously across a set of nodes.

```rust
let quant_api = engine.concurrency_group(10); // max 10 concurrent
```

When `AsyncManager::try_spawn()` is called for a node in a group:
- If `group.in_flight < group.max_concurrent`: spawn immediately, increment `in_flight`.
- Otherwise: push into the group's `VecDeque<QueuedWork>` (FIFO).

When a task completes:
- Decrement `group.in_flight`.
- Pop the next item from the group queue and spawn it.

### Composition with Policies

- **Debounce + group**: The node debounces locally, so only settled inputs enter the group's queue. This reduces burst pressure.
- **LatestWins + group**: If the node is queued (not yet dispatched), its future could be replaced in place (not yet implemented — currently it cancels and re-queues).
- **Queue + group**: Per-node queue and group queue interact independently. The node won't spawn its next queued task until the current one finishes AND the group has capacity.

## AsyncManager Internals

```rust
pub struct AsyncManager {
    runtime: tokio::runtime::Runtime,      // multi-thread, 2 workers
    result_tx / result_rx: mpsc::channel,  // completed results
    node_states: HashMap<NodeId, AsyncNodeState>,
    groups: HashMap<usize, ConcurrencyGroupState>,
    notifier: Option<mpsc::Sender<Command>>,  // wakes event loop
}
```

`AsyncNodeState` tracks per-node: current policy, in-flight `JoinHandle`, group ID, and pending queue (for Queue policy).

`ConcurrencyGroupState` tracks: max concurrent, current in-flight count, and a FIFO queue of `QueuedWork { node_id, future }`.

## AsyncPolicyWrapper

When using the closure builder API, the user calls `.policy()` and `.group()` on the builder. These values need to be associated with the `AnyNodeImpl` trait object. `AsyncPolicyWrapper` wraps the inner `Box<dyn AnyNodeImpl>` and overrides `async_policy()` and `concurrency_group_id()`:

```rust
struct AsyncPolicyWrapper {
    inner: Box<dyn AnyNodeImpl>,
    policy: AsyncPolicy,
    group_id: Option<usize>,
}
```

All other `AnyNodeImpl` methods delegate to `inner`.

## Event Loop Integration

The `AsyncManager` holds an optional notifier (`mpsc::Sender<Command>`). When a future completes, it sends `Command::AsyncComplete` to the event loop's command channel. This wakes the event loop from its blocking `recv()`, causing it to run a new cycle that drains the async results and propagates them forward.

Without the notifier, async results would only be picked up when the next source update or command arrives.
