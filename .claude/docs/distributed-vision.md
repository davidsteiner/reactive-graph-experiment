# Distributed Framework - Long Term Vision

## Architecture: Central Graph with Remote Workers

### Overview
The graph runs on a single machine — topology, scheduling, value store, deduplication, backward activation — all unchanged from the single-engine model. The only thing that changes is **where computation happens**. Expensive compute nodes can be offloaded to a pool of stateless workers over the network.

Workers are dumb executors. They receive serialized inputs, run a compute function, and return the serialized result. They have no concept of the graph, dependencies, epochs, or other nodes.

### How it works
1. The engine runs its wave-based scheduler as normal
2. Within a wave, nodes marked for remote execution have their inputs serialized and sent to an available worker
3. The worker looks up the compute function in a type registry, runs it, returns the result
4. The engine deserializes the result and stores it in the value store like any other node
5. The engine proceeds to the next wave

This is the existing parallel scheduler model, with network calls instead of thread spawns.

### What stays unchanged
Everything that makes the framework valuable works exactly as it does today:
- Dependency tracking and dirty propagation
- Automatic deduplication of shared nodes via ref-counting
- Backward activation on subgraph merge
- Subgraph lifecycle (SubgraphHandle RAII, ref-count-based cleanup)
- Source collapsing (multiple ticks between cycles naturally merge)
- Error strategies

### What this avoids
Compared to a fully distributed graph:
- No coordinator process
- No mirror source nodes
- No distributed deduplication or cluster-wide ref-counting
- No epoch alignment protocol
- No distributed scheduling or cross-partition barriers

### Limitations
- **Central machine is a bottleneck** — all values flow through it. In practice this is acceptable because the central machine orchestrates and stores values but doesn't do heavy computation.
- **Network round-trip per wave** — the engine sends inputs to workers and waits for results before proceeding. With shallow graphs (3-4 levels) this is 3-4 round-trips per cycle, negligible on a local network.
- **Memory** — the value store holds everything centrally. Acceptable for typical use cases (pricing results are small), could become a constraint at extreme scale.

### Evolution path
This architecture doesn't close the door on full distribution. The serialization layer (NodeValue trait, type registry) carries over directly. If the central machine becomes a bottleneck, specific parts of the graph can be promoted to independent engines — but only when there's evidence that it's needed.

## Serialization

### NodeValue trait
A `NodeValue` trait lets each type choose its optimal encoding rather than forcing `Serialize + Deserialize` on everything:

```rust
trait NodeValue: Send + Sync + Clone + 'static {
    fn serialize_into(&self, buf: &mut Vec<u8>);
    fn deserialize_from(buf: &[u8]) -> Self;
}
```

A blanket impl covers serde-compatible types with bincode as the default. Types with better options (e.g., DataFrames) implement the trait directly.

### Format choices
- **bincode** for small typed values (prices, structs) — fastest, minimal overhead, Rust-to-Rust
- **Arrow IPC** for columnar data (Polars DataFrames) — near-zero-cost serialization since Polars is Arrow-backed internally; the receiver can use buffers directly without a deserialization pass

### Two-path optimisation
- **Local nodes**: direct memory access, zero cost — no serialization
- **Remote nodes**: serialize inputs, send to worker, deserialize result

The engine knows which nodes are local and which are remote, so serialization only happens when necessary.

### Type registry on workers
Workers need to instantiate compute functions from a type tag:

```rust
type ComputeFn = fn(&[u8]) -> Vec<u8>; // deserialize inputs, compute, serialize output

struct WorkerRegistry {
    functions: HashMap<u32, ComputeFn>,
}
```

Registered at worker startup. The engine and workers must agree on type tags — these are part of the deployment configuration.

## Workers

### Stateless design
Workers hold no graph state. They are a pool of compute resources:
- Receive: `(type_tag, serialized_inputs)`
- Execute: look up compute function, deserialize inputs, run, serialize output
- Return: `serialized_output`

Workers can be restarted, replaced, or scaled without any impact on the graph.

### Autoscaling

#### Principle: framework handles lifecycle, infrastructure handles scaling decisions
The framework does **not** own autoscaling. Infrastructure orchestrators (Kubernetes HPA, ECS autoscaling) already solve this well.

#### Responsibility boundary

| Concern | Owner |
|---|---|
| How many workers should be running | Infrastructure (K8s/ECS) |
| When to add/remove instances | Infrastructure (based on metrics the framework exposes) |
| What happens when a worker joins | Framework (adds to pool, starts sending work) |
| What happens when a worker leaves | Framework (removes from pool, redistributes in-flight work) |
| Health/readiness signals | Framework exposes, infrastructure consumes |

#### Metrics the framework exposes
Custom metrics for infrastructure to scale on (richer than raw CPU):
- Worker utilisation (busy vs idle)
- Propagation cycle latency
- Remote compute queue depth
- Serialization overhead

### Worker failure
If a worker dies mid-computation, the engine detects the failure (connection drop / timeout) and resubmits the work to another available worker. The graph doesn't need to know — it's just waiting for a result. Retrying on a different worker is transparent.

## Consistency and Backpressure

### No distributed consistency problem
Because the graph runs on a single engine, consistency is free — exactly as it is today. A propagation cycle is synchronous within the engine. Every node in a wave sees values from the same cycle. Remote execution doesn't change this: the engine sends inputs and waits for results within the same cycle.

### Backpressure via source collapsing
Backpressure is handled by the existing source collapsing mechanism. If multiple market data ticks arrive while the engine is mid-cycle, sources store the latest value. The next cycle picks up whatever is current. Intermediate ticks are naturally collapsed. No epochs, no skipping protocol, no coordination needed.

## State Recovery

### Principle: sources own their recovery
The framework does not checkpoint or replay values. Each source is responsible for re-establishing its own state on restart.

### Three source states
- **Has a value** — normal, propagate downstream
- **No value yet** — source is activated but hasn't received data. Downstream nodes wait. Not an error.
- **Error** — source failed (e.g., connection lost, bad data). Error strategy applies.

The "no value yet" state is intentional and useful beyond recovery — e.g., a new data feed that hasn't produced its first value yet. The subgraph stays dormant until the first value arrives, then lights up naturally.

### Recovery is a normal update
When a source re-establishes its value after a restart, it is treated as a regular value update — the source sets its value, dependants are marked dirty, and the next propagation cycle runs. No special recovery mode.

### Observability
The framework must provide visibility into which sources are in the "no value yet" state and which subgraphs are blocked as a result.

## Developer Experience

### Design principle
Distribution should be invisible to end users of the framework. They think in terms of computation and dependencies. The framework handles where computation runs.

### User-facing API
Identical to the single-engine API. The only concession is `Serialize + Deserialize` on types that may be computed remotely:

```rust
#[derive(Serialize, Deserialize)]
struct VanillaOptionPricer {
    strike: f64,
    expiry: Date,
}

impl Compute for VanillaOptionPricer {
    type Input = (f64, VolSurface, DiscountCurve);
    type Output = PricingResult;

    fn compute(&self, (spot, vols, curve): Self::Input) -> PricingResult {
        // pricing logic
    }
}

// Registration — no awareness of distribution
graph.compute("trade-12345")
    .dep(&spot_eurusd)
    .dep(&vol_surface_eurusd)
    .dep(&discount_curve_usd)
    .node(VanillaOptionPricer { strike: 1.05, expiry: date });
```

### Infrastructure-facing API
Separate layer for configuring remote execution:

```rust
let engine = Engine::builder()
    .worker_pool(WorkerPoolConfig::new()
        .min_workers(4)
        .connect("worker-pool.internal:9000"))
    .remote_execution(|node| {
        // Decide which nodes run remotely
        // e.g., by node kind, by estimated cost, by annotation
        node.kind() == NodeKind::Compute && node.deps().len() > 2
    })
    .build();
```

### What users don't need to think about
- Which machine runs their computation
- Serialization (derives are the only requirement)
- Worker pool sizing (infrastructure handles this)
- Failure handling (transparent retry)
- Consistency (single engine guarantees it)