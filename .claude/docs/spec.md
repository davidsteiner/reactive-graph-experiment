# Reactive Graph Engine — Specification

## Overview

A general-purpose framework for building demand-driven, forward-propagating computation graphs in Rust. The graph is composed of three primitives — **sources**, **compute nodes**, and **sinks** — connected by declared dependencies. Data flows forward (push) while activation flows backward (pull): a sink activates the compute nodes it depends on, which in turn activate their sources. When demand is removed, unused nodes deactivate and release their resources.

The framework is domain-agnostic. It provides the graph lifecycle, propagation, and dependency management. Users supply the domain logic by implementing sources, compute functions, and sinks.

## Design Goals

- **Demand-driven activation.** Nodes only activate when something downstream needs them. No resources are consumed for parts of the graph that have no active sinks.
- **Push-based propagation.** When a source emits a new value, it propagates forward through dependent compute nodes to sinks. No polling.
- **Dynamic topology.** Nodes and subgraphs can be added and removed at runtime. The graph grows and shrinks as demand changes.
- **Automatic deduplication.** Nodes are identified by a unique key. When multiple subgraphs reference the same node, they share it. Reference counting manages the lifecycle.
- **Clean programming model.** Users declare dependencies and implement compute functions. They never manage subscriptions, propagation, ordering, or lifecycle manually.
- **Static type safety.** Dependencies carry their output type. The compiler verifies that compute functions and sink callbacks receive the correct types from their declared dependencies. No runtime downcasting.

## Primitives

The framework provides two APIs for defining nodes: a **closure/builder API** for ad-hoc, one-off nodes, and a **trait-based API** for reusable, parameterised node types. Both APIs produce `NodeRef<T>` and interoperate freely — trait-based nodes and closure-based nodes can depend on each other. This section describes the closure/builder API; the trait-based API is described in [Trait-Based Node Definitions](#trait-based-node-definitions).

### Source

A source is a leaf node with no dependencies within the graph. Its value is produced externally — by a network subscription, a file watcher, a timer, a hardware sensor, or any other mechanism.

The framework does not prescribe how sources obtain their data. Instead, when a source activates, the framework invokes a user-provided activation callback. When it deactivates, a corresponding deactivation callback is invoked. What these callbacks do is entirely up to the user.

```rust
let source: NodeRef<f64> = graph.source("my.source.id")
    .on_activate(|tx: Sender<f64>| {
        // Called when the first downstream dependent activates this source.
        // `tx` is a typed handle to push values into the graph.
        // Return a resource that will be dropped on deactivation.
        let subscription = some_external_system.subscribe("topic", move |value| {
            tx.send(value);
        });
        subscription // dropped when source deactivates
    })
    .build();
```

A source is parameterised by its output type. The `Sender<T>` handle ensures only values of the correct type can be pushed. The `build()` method returns a `NodeRef<T>` — a typed, lightweight reference that downstream nodes use to declare dependencies. `NodeRef<T>` carries the output type at compile time but is not the value itself; it is a reference into the graph's internal storage.

### Compute Node

A compute node declares its dependencies (other sources or compute nodes) and implements a function that produces an output from its inputs. It is recomputed whenever any of its dependencies change.

```rust
let derived: NodeRef<f64> = graph.compute("my.derived.id")
    .dep(source_a) // NodeRef<f64>
    .dep(source_b) // NodeRef<f64>
    .compute(|a: &f64, b: &f64| {
        a + b
    })
    .build();
```

Each call to `.dep()` adds a typed dependency to the builder. The compute closure's parameters must match the types of the declared dependencies in order. The compiler enforces this — a type mismatch between a dependency's output type and the corresponding closure parameter is a compile-time error. The return type of the closure determines the output type of the node.

Compute nodes can depend on other compute nodes, forming arbitrary DAGs.

### Sink

A sink is a terminal node that consumes values but produces no output within the graph. Sinks are the source of demand — activating a sink is what causes the upstream graph to come alive.

Like sources, the framework does not prescribe what a sink does with the values it receives. It invokes a user-provided callback whenever the sink's inputs change.

```rust
let sink = graph.sink("my.sink.id")
    .dep(derived) // NodeRef<f64>
    .on_emit(|value: &f64| {
        websocket.send(value);
    })
    .build();
```

A sink can also have an activation/deactivation callback for acquiring and releasing external resources (connections, file handles, etc.).

## Trait-Based Node Definitions

The closure/builder API shown above works well for ad-hoc, one-off nodes. For nodes that are reused, parameterised, or composed into larger subgraphs, the framework provides a trait-based API where node types formally declare their identity, input types, and output types. The compiler enforces that only compatible types can be wired together.

### Design Motivation

In domains like FX pricing, nearly every node is parameterised — scoped by currency pair, liquidity provider, client, or some combination. Dependencies cannot be declared as types alone: `Dep<SpotRate>` can't tell you *which* `SpotRate` instance. So:

- **Traits declare the value-type interface** — "I take `(Quote, Quote)` and produce `f64`"
- **Wiring is explicit** — the builder connects specific node instances
- **The compiler validates type compatibility** at wire-up time

This avoids auto-wiring ambiguity while preserving full type safety.

### Node Base Trait

All nodes share identity and an output type:

```rust
trait Node {
    type Output: Send + Sync + 'static;
    type Key: Hash + Eq + Debug;
    fn key(&self) -> Self::Key;
}
```

`Key` is the deduplication identity. It is scoped by concrete type, so `SpotRate`'s key space can never collide with `MidRate`'s. `Debug` on `Key` provides human-readable representation for observability.

### InputTuple

`InputTuple` maps a tuple of value types to a tuple of references, used by `Compute`, `AsyncCompute`, and `Sink` to express their input signature:

```rust
trait InputTuple {
    type Ref<'a>;
}

impl InputTuple for (A,) {
    type Ref<'a> = (&'a A,);
}

impl InputTuple for (A, B) {
    type Ref<'a> = (&'a A, &'a B);
}

// ... up to reasonable arity
```

### Source Trait

```rust
trait Source: Node {
    type Guard;  // RAII resource dropped on deactivation
    fn activate(&self, tx: Sender<Self::Output>) -> Self::Guard;
}
```

The `Guard` associated type replaces the closure-based deactivation pattern. When the source deactivates, the guard is dropped, releasing whatever resources it holds (subscriptions, connections, timers).

### Compute Trait

```rust
trait Compute: Node {
    type Inputs;  // tuple of value types, e.g. (Quote, Quote)
    fn compute(&self, inputs: <Self::Inputs as InputTuple>::Ref<'_>) -> Self::Output;
}
```

`type Inputs = (Quote, Quote)` says "I need two Quotes." It does *not* say which upstream nodes provide them — that is a wiring concern handled by the builder. The `compute` method receives references to the dependency values and returns the output value.

### Sink Trait

```rust
trait Sink {
    type Inputs;
    type Key: Hash + Eq + Debug;
    fn key(&self) -> Self::Key;
    fn emit(&self, inputs: <Self::Inputs as InputTuple>::Ref<'_>);
}
```

### AsyncCompute Trait

```rust
trait AsyncCompute: Node {
    type Inputs;
    fn compute(&self, inputs: <Self::Inputs as InputTuple>::Ref<'_>) -> impl Future<Output = Self::Output>;
}
```

The same async semantics apply as described in the [Async Compute Nodes](#async-compute-nodes) section: the closure should copy or clone values it needs, and the engine manages dispatch, cancellation, and re-triggering.

### Type-Checked Builder Wiring

`graph.add_source`, `graph.add_compute`, and `graph.add_sink` start typed builders that know the expected inputs from the trait:

```rust
// Sources — no deps, just add
let bid: NodeRef<Quote> = graph.add_source(SpotRate { pair: EURUSD, lp: Citi });
let ask: NodeRef<Quote> = graph.add_source(SpotRate { pair: EURUSD, lp: JPMC });

// Compute — builder enforces dep types match Compute::Inputs
let mid: NodeRef<f64> = graph.add_compute(MidRate { pair: EURUSD })
    .dep(bid)    // NodeRef<Quote> — matches first element of (Quote, Quote) ✓
    .dep(ask)    // NodeRef<Quote> — matches second element of (Quote, Quote) ✓
    .build();    // returns NodeRef<f64>

// This would NOT compile:
// let mid = graph.add_compute(MidRate { pair: EURUSD })
//     .dep(name_ref)  // NodeRef<String> — doesn't match Quote ✗
//     .dep(ask)
//     .build();

// Sink
graph.add_sink(PriceLogger { pair: EURUSD })
    .dep(mid)    // NodeRef<f64> — matches (f64,) ✓
    .build();
```

Each `.dep()` call advances the builder's typestate, consuming one element of the `Inputs` tuple. When all inputs are satisfied, `.build()` becomes available. If too few or too many deps are provided, or if a dep's output type doesn't match the corresponding element, the code does not compile.

### Coexistence with Closure API

Both APIs produce `NodeRef<T>` and interoperate freely. Trait-based nodes and closure-based nodes can depend on each other:

```rust
// Trait-based source
let bid: NodeRef<Quote> = graph.add_source(SpotRate { pair: EURUSD, lp: Citi });

// Ad-hoc closure compute depending on it
let doubled: NodeRef<f64> = graph.compute("doubled.bid")
    .dep(bid)
    .compute(|q: &Quote| q.price * 2.0)
    .build();
```

### Testability

Because compute logic is a plain method on the node struct, it can be tested in isolation without constructing a graph:

```rust
#[test]
fn test_mid_rate() {
    let node = MidRate { pair: EURUSD };
    let bid = Quote { price: 1.1000 };
    let ask = Quote { price: 1.1002 };
    assert_eq!(node.compute((&bid, &ask)), 1.1001);
}
```

## Example: FX Pricing Graph

The following example demonstrates the trait-based API in a realistic FX pricing scenario. It shows parameterised nodes, key-based deduplication, and subgraph composition.

### Node Definitions

```rust
// --- Sources ---

struct SpotRate {
    pair: CurrencyPair,
    lp: LiquidityProvider,
}

impl Node for SpotRate {
    type Output = Quote;
    type Key = (CurrencyPair, LiquidityProvider);
    fn key(&self) -> Self::Key { (self.pair, self.lp) }
}

impl Source for SpotRate {
    type Guard = Subscription;
    fn activate(&self, tx: Sender<Quote>) -> Subscription {
        exchange.subscribe(self.pair, self.lp, move |q| tx.send(q))
    }
}

struct ClientConfig {
    pair: CurrencyPair,
    client: ClientId,
}

impl Node for ClientConfig {
    type Output = SpreadConfig;
    type Key = (CurrencyPair, ClientId);
    fn key(&self) -> Self::Key { (self.pair, self.client) }
}

impl Source for ClientConfig {
    type Guard = ConfigWatch;
    fn activate(&self, tx: Sender<SpreadConfig>) -> ConfigWatch {
        config_service.watch(self.pair, self.client, move |c| tx.send(c))
    }
}

// --- Compute ---

struct MidRate {
    pair: CurrencyPair,
}

impl Node for MidRate {
    type Output = f64;
    type Key = CurrencyPair;
    fn key(&self) -> Self::Key { self.pair }
}

impl Compute for MidRate {
    type Inputs = (Quote, Quote);  // bid, ask
    fn compute(&self, (bid, ask): (&Quote, &Quote)) -> f64 {
        (bid.price + ask.price) / 2.0
    }
}

struct ClientSpread {
    pair: CurrencyPair,
    client: ClientId,
}

impl Node for ClientSpread {
    type Output = SpreadQuote;
    type Key = (CurrencyPair, ClientId);
    fn key(&self) -> Self::Key { (self.pair, self.client) }
}

impl Compute for ClientSpread {
    type Inputs = (f64, SpreadConfig);  // mid rate, client config
    fn compute(&self, (mid, config): (&f64, &SpreadConfig)) -> SpreadQuote {
        SpreadQuote {
            bid: mid - config.half_spread,
            ask: mid + config.half_spread,
        }
    }
}

// --- Sink ---

struct PricePublisher {
    pair: CurrencyPair,
    client: ClientId,
}

impl Sink for PricePublisher {
    type Inputs = (SpreadQuote,);
    type Key = (CurrencyPair, ClientId);
    fn key(&self) -> Self::Key { (self.pair, self.client) }
    fn emit(&self, (quote,): (&SpreadQuote,)) {
        connection.send(self.pair, quote);
    }
}
```

### Subgraph Construction and Deduplication

```rust
fn pricing_subgraph(
    graph: &mut SubgraphBuilder,
    pair: CurrencyPair,
    client: ClientId,
) {
    let bid = graph.add_source(SpotRate { pair, lp: Citi });
    let ask = graph.add_source(SpotRate { pair, lp: JPMC });

    let mid = graph.add_compute(MidRate { pair })
        .dep(bid).dep(ask).build();

    let config = graph.add_source(ClientConfig { pair, client });

    let spread = graph.add_compute(ClientSpread { pair, client })
        .dep(mid).dep(config).build();

    graph.add_sink(PricePublisher { pair, client })
        .dep(spread).build();
}
```

Two clients subscribing to EURUSD both call `pricing_subgraph(graph, EURUSD, client)`. The `SpotRate { pair: EURUSD, lp: Citi }` nodes produce the same key `(EURUSD, Citi)` both times, so they are deduplicated — a single subscription feeds both pricing paths. The `MidRate { pair: EURUSD }` node is similarly shared since its key is just `EURUSD`. Each client gets their own `ClientSpread` and `PricePublisher` because those keys include the client ID.

## Subgraphs and Composition

A subgraph is a self-contained fragment of the graph — a collection of sources, compute nodes, and/or sinks with internal wiring. Subgraphs are the primary unit of dynamic topology change.

```rust
fn build_subgraph(graph: &mut SubgraphBuilder) -> NodeRef<f64> {
    let a: NodeRef<f64> = graph.source("source.a")
        .on_activate(|tx: Sender<f64>| { /* ... */ }).build();
    let b: NodeRef<f64> = graph.source("source.b")
        .on_activate(|tx: Sender<f64>| { /* ... */ }).build();

    let combined: NodeRef<f64> = graph.compute("compute.combined")
        .dep(a).dep(b)
        .compute(|a: &f64, b: &f64| { /* ... */ })
        .build();

    combined
}
```

Subgraph builders are composable — a builder function can call other builder functions to construct deeper dependency trees. Each function only knows about its own local dependencies. No component needs a global view of the graph.

### Merging

Subgraphs are merged into the live engine. During merging, nodes are matched by their unique ID:

- If a node with the same ID already exists, the existing instance is reused and its reference count is incremented.
- If it does not exist, it is created and activated.

This allows independently constructed subgraphs to share infrastructure transparently.

### Handles and RAII Lifecycle

Merging a subgraph returns a `SubgraphHandle`. When the handle is dropped, it enqueues its node IDs for deactivation via a shared removal queue (`Rc<RefCell<Vec<Vec<NodeId>>>>`). The engine drains this queue at the start of every public `&mut self` method (`graph()`, `activate_sink()`, `run_cycle()`, `run_cycle_parallel()`, `merge()`), at which point the nodes are deactivated and their reference counts decremented. Nodes whose reference count reaches zero are removed from the graph, cascading through their dependencies.

```rust
// Activate a subgraph
let handle = engine.merge(subgraph);

// Later — drop the handle to enqueue teardown
drop(handle);

// Deactivation happens on the next engine method call (e.g., run_cycle)
engine.run_cycle();
```

This deferred approach is observationally equivalent to synchronous deactivation because the borrow checker prevents holding both a `SubgraphHandle` and an `&mut Engine` simultaneously — by the time any engine method runs, the handle must have been released. This eliminates the need for `unsafe` raw pointers while preserving RAII semantics.

`SubgraphHandle` is `!Send` (due to `Rc`), which is correct — it is designed for the single-threaded direct Engine API. For cross-thread use, `RemoteSubgraphHandle` communicates via channels through the `EngineHandle`.

## Lifecycle

### Activation (reference count 0 → 1)

1. Recursively activate all dependencies that are not already active.
2. Register in the reverse dependency (dependents) map.
3. For source nodes, invoke the activation callback.
4. If all dependencies already have values, perform an initial computation.

### Deactivation (reference count → 0)

1. For source nodes, invoke the deactivation callback (or drop the activation resource).
2. For sinks, invoke the deactivation callback if one exists.
3. Remove the node's cached value.
4. Unregister from the dependents map.
5. Recursively deactivate dependencies whose reference count has reached zero.
6. Remove the node from the engine.

### Value Update (source pushes a new value)

1. The source sends its new value to the engine via its `Sender<T>`.
2. The coordination layer drains pending updates into the value store.
3. The dirty set is computed: all transitive dependents of updated sources.
4. The dirty set is topologically sorted and scheduled onto the execution layer, respecting dependency order.
5. Each compute node receives references to its dependency values from the store, computes, and writes its result back.
6. When propagation reaches a sink, the sink's emit callback is dispatched.

## Execution Model

The engine is structured as two layers: a **coordination layer** that manages graph state and scheduling, and an **execution layer** that runs compute functions and sink emissions in parallel.

### Value Store

The engine maintains a centralised value store that holds the latest value for every active node. Compute functions and sink callbacks receive `&T` references into this store — values are not sent through channels between nodes. The store is the single source of truth for all node values within a propagation cycle.

### Coordination Layer

A single-threaded event loop manages all graph state:

```
Event loop:
    drain all pending source updates into value store
    compute dirty set (transitive dependents of updated sources)
    topologically sort dirty set
    schedule execution onto thread pool (see Execution Layer)
    await completion of all scheduled work
    drain all pending topology changes (merge/remove)
```

The coordination layer never executes compute functions or sink callbacks itself — it only schedules them. Source updates and topology changes are processed in alternating phases to avoid mutating the graph during propagation.

### Execution Layer

The dependency DAG within a single propagation cycle reveals which nodes can run in parallel. Independent branches — nodes with no dependency relationship — are scheduled concurrently onto a thread pool.

The scheduler maintains a **ready count** for each dirty node: the number of dirty dependencies that have not yet completed. When a node's ready count reaches zero, it is dispatched to the execution layer. When it completes, the ready counts of its dependents are decremented.

```
Source updates arrive
        │
   Coordination layer drains queue, computes dirty set
        │
   Topological sort, initialise ready counts
        │
   Schedule nodes with ready count = 0
        │
   ┌─────────┬─────────┐
   │ node A  │ node B  │  (independent — run in parallel)
   └────┬────┴────┬────┘
        │         │
        ▼         ▼
      node C (ready count hits 0 — dispatched)
        │
        ▼
      sink (emit dispatched)
```

Nodes execute in one of three modes:

**Sync compute.** `Compute::compute` runs on the thread pool. The engine provides `&T` references to dependency values in the store. The node returns a value, which is written back to the store. This is the fast path for pure, CPU-bound transformations.

**Async compute.** `AsyncCompute::compute` is spawned onto an async runtime. Since the future may outlive the propagation cycle, the node should clone or copy the dependency values it needs. When the future completes, the result is fed back into the engine as a synthetic source update, triggering a new propagation cycle from that node. The current cycle does not wait for async compute to complete — downstream nodes may temporarily hold stale values while the computation is in flight.

If a new input arrives while an async computation is pending, the behaviour is configurable per node:

- **Latest-wins:** cancel the in-flight computation and start a new one with the latest inputs.
- **Queue:** let the in-flight computation complete, then re-trigger with the latest inputs.
- **Debounce:** wait for a quiet period before dispatching.

**Concurrency groups.** The per-node policies above control what happens when a *single* node is re-triggered. But when many async compute nodes become dirty simultaneously — for example, 500 trade pricers all re-triggered by a curve rebuild — the engine needs to limit how many are dispatched at once to avoid overwhelming an external service.

A **concurrency group** is a shared resource that caps the number of in-flight async computations across a set of nodes:

```rust
let quant_api = graph.concurrency_group(10); // max 10 in-flight

let price = graph.add_async_compute(TradePricer { trade_id })
    .dep(spot).dep(curve).dep(trade_def)
    .group(quant_api)
    .build();
```

The scheduler extends the ready-count mechanism: when a node in a group becomes ready, it is only dispatched if the group has a free slot. Otherwise it enters the group's queue. When an in-flight computation completes, the next queued node is dispatched. Within a group, dispatch order is FIFO by default.

Concurrency groups compose with per-node policies:

- **Debounce + group:** each node waits for its inputs to settle before entering the group's queue. This reduces the burst — only nodes with genuinely new inputs after the debounce window are queued.
- **Latest-wins + group:** if a node is queued (not yet dispatched) and new inputs arrive, its pending inputs are updated in place. There is nothing to cancel since the computation has not started yet.

**Sink emission.** `Sink::emit` is dispatched to the thread pool. Since sinks typically perform I/O (sending data over a network, writing to a file), they run outside the coordination layer. The engine awaits completion of all sink emissions before starting the next cycle, providing natural backpressure — if sinks are slow, source updates queue up and conflate.

### Batching and Consistency

This architecture preserves two key guarantees:

- **No redundant computation.** A compute node is evaluated at most once per propagation cycle, even if multiple inputs changed.
- **Snapshot consistency.** All sync compute nodes and sinks within a single propagation cycle observe a consistent set of input values. The value store is not mutated by new source updates until the current cycle completes.

When multiple source updates arrive between processing cycles, they are drained and applied together before propagation begins. This naturally conflates rapid updates — by the time the engine drains the queue, it takes the latest value per source.

### Async Compute Nodes

Some compute nodes need to perform work that is not instantaneous — calling an external service, performing IO, or running a long computation. The framework supports async compute nodes that do not block the propagation cycle.

Using the closure API:

```rust
let async_node: NodeRef<Response> = graph.compute_async("my.async.id")
    .dep(source_a) // NodeRef<f64>
    .dep(source_b) // NodeRef<f64>
    .compute(|a: &f64, b: &f64| {
        let a = *a;
        let b = *b;
        async move {
            let result = external_service.call(a, b).await;
            result
        }
    })
    .build();
```

Using the trait API with a concurrency group:

```rust
struct TradePricer {
    trade_id: TradeId,
    quant_client: QuantClient,
}

impl Node for TradePricer {
    type Output = PricingResult;
    type Key = TradeId;
    fn key(&self) -> Self::Key { self.trade_id }
}

impl AsyncCompute for TradePricer {
    type Inputs = (f64, DiscountCurve, TradeDefinition);
    async fn compute(&self, (spot, curve, trade): (&f64, &DiscountCurve, &TradeDefinition)) -> PricingResult {
        self.quant_client.price(self.trade_id, *spot, curve, trade).await
    }
}

// Wire up with concurrency limit and debounce
let quant_api = graph.concurrency_group(10);

let price = graph.add_async_compute(TradePricer { trade_id, quant_client })
    .dep(spot).dep(curve).dep(trade_def)
    .group(quant_api)
    .debounce(Duration::from_millis(50))
    .build();
```

Async compute closures and trait methods receive typed references to their dependencies. Since the returned future typically outlives the borrow, the implementation should clone or copy the values it needs. The output type of the future determines the node's output type.

When the async computation completes, the result is fed back into the engine as a synthetic source update, triggering forward propagation from that node. The current propagation cycle does not wait — downstream nodes may temporarily hold stale values until the result arrives.

## Error Handling

Compute functions and async compute functions may fail. The framework provides a configurable error strategy per node:

- **Propagate stale.** Retain the last good value and do not propagate an update. Optionally emit a diagnostic to an error sink.
- **Propagate error.** Forward the error as the node's value. Downstream nodes receive `Result<T, E>` and decide how to handle it.
- **Retry.** Re-invoke the compute function (with backoff), up to a configurable limit.

Source node failures (e.g., a dropped connection) are surfaced through the activation callback's return type or via a separate error channel.

## Observability

The engine exposes the following metrics:

- **Active node count** by type (source, compute, sink).
- **Propagation cycle duration** (time from first source update to last sink emission in a cycle).
- **Per-node compute duration** for profiling hot paths.
- **Pending async computations** count.
- **Activation/deactivation events** for tracking graph topology changes over time.

The engine also supports a **graph introspection API** that returns the current topology: active nodes, edges, reference counts, and last-updated timestamps. This can be used to build live visualisations of the running graph.

## Thread Safety and Ownership

The coordination layer is single-threaded. All graph state mutations (merge, remove, dirty-set computation, scheduling) occur on its event loop. Compute functions and sink callbacks execute on the thread pool but only access the value store through references provided by the scheduler — they never mutate graph topology.

The value store holds `Box<dyn Any + Send + Sync>`, which makes `&ValueStore` naturally `Sync`. This allows the parallel scheduler to share the value store across threads without `unsafe` — `&ValueStore` can be sent to scoped threads directly. The `+ Sync` bound on `Node::Output` ensures all node output types satisfy this requirement. Source activation guards (stored separately in `Engine::guards`) remain `Box<dyn Any + Send>` since they are never shared across threads.

The codebase contains zero `unsafe` blocks. Thread safety is enforced entirely through Rust's type system:
- `&ValueStore` is `Sync` because its contents are `Send + Sync`.
- `NodeRef<T>` is `Send + Sync` because it contains only `NodeId` (Send+Sync) and `PhantomData<fn() -> T>` (Send+Sync for all T).
- `SubgraphHandle` uses `Rc<RefCell<...>>` for deferred deactivation, making it `!Send` — correct for the single-threaded direct Engine API.
- `Sender<T>` is `Send` but not `Sync` (wraps `mpsc::Sender`), correct for use from a single external thread.

External code interacts with the engine through a channel-based `EngineHandle`:

```rust
let handle = engine.handle();

// From any thread or async task:
handle.merge(subgraph);
handle.update_source(&source_ref, value); // type-checked: value must match NodeRef<T>
handle.remove(subgraph_handle);
```

The `EngineHandle` is `Send + Sync + Clone`. It serialises all operations through a message channel to the coordination layer's event loop, ensuring there are no data races. The `update_source` method takes a `&NodeRef<T>` and a value of type `T` (where `T: Send + Sync + 'static`), so the compiler prevents pushing a value of the wrong type to a source.

Node authors do not need to reason about concurrency. `Compute::compute` receives `&T` references and returns a value — the engine handles all synchronisation. `Sink::emit` similarly receives references and performs its side effects. The only concurrency-aware code is in `Source::activate`, which runs on an external thread and communicates with the engine exclusively through the provided `Sender<T>`.

## Non-Goals

The framework does not provide:

- **Distribution.** It is a single-process, in-memory graph engine. Distribution (partitioning work across nodes, networked pub/sub, orchestration) is handled by the application layer.
- **Persistence.** Graph state is ephemeral. If the process restarts, the graph must be rebuilt. The application is responsible for persisting any state it needs to reconstruct the graph.
- **Serialisation format.** The framework is agnostic to how values are serialised. Source callbacks and sink callbacks handle their own serialisation.
- **Domain logic.** The framework knows nothing about the data flowing through it. All domain semantics live in user-provided compute functions, source callbacks, and sink callbacks.

## Open Questions

- **Change detection.** Should compute nodes be re-evaluated when any dependency updates, or only when a dependency's value actually changes? The latter avoids unnecessary recomputation but requires equality checking on all value types.
- **Priority and scheduling.** Should some propagation paths be prioritised over others? For example, a user-facing sink might need lower latency than a logging sink.
- **Backpressure.** If a sink cannot keep up with the rate of updates, should the engine drop intermediate values (conflation), buffer them, or signal backpressure upstream?
- **Conditional dependencies.** Should a compute node be able to change its dependencies at runtime based on its inputs? This enables data-dependent graph shapes but significantly complicates lifecycle management.
- **Nested subgraphs.** Should subgraph handles compose hierarchically, so dropping a parent handle tears down all children?
