# Architecture

## Module Structure

```
src/
  lib.rs              Crate root, re-exports, tests
  node.rs             Node, Source, Compute, Sink, AsyncCompute traits
  input_tuple.rs      InputTuple trait + macro impls (arities 1-12)
  node_ref.rs         NodeRef<T>, NodeId
  value_store.rs      Typed centralised storage (HashMap<NodeId, Box<dyn Any + Send + Sync>>)
  graph.rs            Graph struct, AnyNodeImpl trait, AsyncPolicy enum
  topology.rs         Dep/dependent maps, Kahn's topological sort
  sender.rs           Sender<T> for source updates
  engine.rs           Engine: owns Graph + Topology, runs propagation cycles
  engine_handle.rs    EngineHandle: Send + Sync + Clone, channel-based API
  event_loop.rs       Single-threaded coordination event loop
  command.rs          Command enum for engine messages
  scheduler.rs        Wave-based parallel dispatch with ready counts
  async_compute.rs    AsyncManager, ConcurrencyGroupHandle, tokio runtime
  subgraph.rs         SubgraphBuilder, Subgraph, SubgraphHandle
  error.rs            ErrorStrategy enum
  metrics.rs          CycleMetrics, EngineMetrics, MetricsCollector
  introspection.rs    GraphSnapshot, NodeInfo
  builder/
    mod.rs
    source.rs         SourceBuilder (closure API)
    compute.rs        ComputeBuilder with typestate dep chain (closure API)
    sink.rs           SinkBuilder with typestate dep chain (closure API)
    trait_source.rs   Trait-based source registration
    trait_compute.rs  Trait-based compute builder with typestate + ComputeDispatch
    trait_sink.rs     Trait-based sink builder with typestate + SinkDispatch
    async_compute.rs  AsyncComputeBuilder (closure + trait), AsyncPolicyWrapper
```

## Type System

### NodeId

`NodeId` is the identity of a node. It combines a `TypeId` (scoping by concrete node type) with a hash of the user-provided key. This means `SpotRate { pair: EURUSD }` and `MidRate { pair: EURUSD }` can never collide even if their keys hash to the same value, because they are different Rust types. Closure-based nodes use marker types (`ClosureSource`, `ClosureCompute`, etc.) as their scope.

```rust
pub struct NodeId {
    type_id: TypeId,
    key_hash: u64,
    key_debug: String,  // for observability
}
```

### NodeRef\<T\>

A lightweight handle to a node in the graph. Carries the output type `T` at compile time but stores no value — it's just a `NodeId` plus `PhantomData<fn() -> T>`. `Clone + Send + Sync` (auto-derived, no `unsafe`), but not `Copy` (the `NodeId` contains a `String`).

### InputTuple

Maps a tuple of value types to a tuple of references. Implemented via macro for arities 1-12:

```rust
trait InputTuple {
    type Ref<'a>;
}

// Generated: (A,) -> (&'a A,), (A, B) -> (&'a A, &'a B), etc.
```

This is used by `Compute`, `Sink`, and `AsyncCompute` traits to express their input signature.

## Type Erasure

The graph stores nodes as `Box<dyn AnyNodeImpl>` — a trait object that erases the concrete types. `AnyNodeImpl` is `Send + Sync` and has methods for `activate`, `compute`, `emit`, `compute_async`, `async_policy`, and `concurrency_group_id`.

For trait-based nodes, dispatch traits (`ComputeDispatch<C>`, `SinkDispatch<S>`, `AsyncComputeDispatch<AC>`) are implemented on the input tuple types. These know how to downcast from the value store and call the concrete node's methods:

```
(A, B) implements ComputeDispatch<MidRate>
  → knows to downcast dep[0] as &A, dep[1] as &B
  → calls MidRate::compute((&a, &b))
  → boxes the result
```

## Graph and Value Store

`Graph` owns:
- `entries: HashMap<NodeId, NodeEntry>` — the node registry
- `value_store: ValueStore` — holds `HashMap<NodeId, Box<dyn Any + Send + Sync>>`
- `source_tx` / `source_rx` — mpsc channel for source updates

`NodeEntry` stores the node's ID, kind, dependency list, reference count, and boxed `AnyNodeImpl`.

The `ValueStore` provides typed access via `NodeRef<T>` (downcasting internally) and type-erased access via `NodeId`.

## Execution Pipeline

### Single-Threaded Path (`Engine::run_cycle`)

1. **Drain sources**: Pull all `(NodeId, Box<dyn Any + Send + Sync>)` from `source_rx` into the value store. Also drain async results from `AsyncManager`.
2. **Dirty set**: BFS from updated sources through the `dependents` map.
3. **Topo sort**: Kahn's algorithm on the dirty subgraph.
4. **Execute**: Walk the sorted list sequentially. Compute nodes produce values written to the store. Sinks emit. Async compute nodes are collected as `AsyncSpawn` items.
5. **Spawn async**: Hand collected futures to `AsyncManager`.
6. **Record metrics**: Cycle duration, node count, per-node durations.

### Parallel Path (`Engine::run_cycle_parallel`)

Same as above, but step 4 uses the `Scheduler`:

1. Compute ready counts: for each dirty node, count how many of its deps are also dirty.
2. **Wave loop**: collect all nodes with ready count 0 → dispatch them in parallel via `std::thread::scope` → write results back → decrement dependents' counts → repeat.
3. Async compute nodes are filtered out of the parallel path and spawned separately.

The wave structure guarantees no data races: each wave reads only from values produced in prior waves, and writes only to its own slots.

### Event Loop Path (`start_engine`)

The engine runs on a dedicated thread. External code communicates via `EngineHandle` (wraps `mpsc::Sender<Command>`):

```
                        ┌──────────────────────┐
  EngineHandle ──cmd──> │     Event Loop        │
  (any thread)          │  recv → batch cmds    │
                        │  source updates       │
                        │  topology changes      │
                        │  run_cycle()          │
                        │  async completions    │
                        └──────────────────────┘
```

Commands: `SourceUpdate`, `Merge`, `Remove`, `AsyncComplete`, `Shutdown`.

The event loop blocks on `cmd_rx.recv()`, then drains all pending commands (batching), processes source updates and topology changes, and runs a propagation cycle.

## Lifecycle

### Activation

Activating a sink recursively activates its upstream dependencies. Sources get their `activate` callback invoked, which returns a guard (any `Send` type, stored as `Box<dyn Any + Send>`) that is stored and dropped on deactivation. If a compute node's deps all have values when activated, it runs an initial computation.

### Deactivation

When a node's ref count reaches 0:
1. Source guards are dropped
2. Values are removed from the store
3. The node is removed from topology and the entry map
4. Cascades to dependencies whose ref count may now be 0

### Reference Counting

Each call to `register_node` with an existing ID increments `ref_count`. Deactivation decrements it. This enables deduplication: two subgraphs referencing the same source share a single instance.

## Subgraphs

`SubgraphBuilder` mirrors the `Graph` builder API but records `NodeDescriptor`s instead of registering directly. Calling `.build()` produces a `Subgraph`. Merging into the engine via `Engine::merge()` or `EngineHandle::merge()` registers all descriptors and activates sinks.

`SubgraphHandle` (local) uses a shared `Rc<RefCell<Vec<Vec<NodeId>>>>` removal queue — dropping the handle enqueues its node IDs, and the engine drains the queue at the start of every public `&mut self` method. This provides RAII semantics without `unsafe`. `SubgraphHandle` is `!Send` (correct for the single-threaded direct API). `RemoteSubgraphHandle` (via EngineHandle) sends `Command::Remove` through the channel.

## Dependencies

- `rayon = "1"` (present in Cargo.toml but not used — parallel execution uses `std::thread::scope`)
- `tokio = { version = "1", features = ["rt-multi-thread", "sync", "time"] }` for async compute runtime
