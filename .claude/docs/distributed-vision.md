# Distributed Framework - Long Term Vision

## Serialization Strategy

### Two-path model
- **Same engine**: direct memory access, zero cost — no serialization within a single engine instance
- **Remote partition**: serialize only at network partition boundaries

### NodeValue trait over blanket serde bounds
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

### Type dispatch over the wire
Tagged envelope with a type registry:

```rust
struct WireMessage {
    epoch: u64,
    node_id: NodeId,
    type_tag: u32,
    payload: Bytes,
}
```

Type registry maps tags to deserialization functions, registered at startup. This is the distributed equivalent of the current `TypeId`-based `downcast_ref` dispatch.

## Scaling Model

### Target use case
The primary use case is FX/derivatives pricing: market data sources feed shared intermediate calculations (interpolated curves, derived rates), which fan out to potentially thousands of independent trade-specific pricing nodes. The graph is shallow (3-4 levels) but wide at the leaf levels.

### Architecture: shared core + worker pool
- **Shared core**: market data sources and shared intermediate nodes (e.g., vol surfaces, discount curves). Runs on a single engine or small HA cluster. Small, fast, everything depends on it.
- **Worker pool**: each worker takes a slice of the trade population. Workers subscribe to boundary values from the core. When market data updates propagate through the core, boundary values are pushed to workers, and each worker runs its local levels independently.
- Workers **do not coordinate with each other** — they are independent consumers of the core's outputs. Adding a worker just means assigning trades to it.

### Scaling characteristics
- **Horizontal scaling within a level**: the main scaling axis. Thousands of independent pricing nodes spread across workers, scales linearly with machines.
- **Pipeline parallelism**: the core can work on epoch N+1 while workers finish epoch N. Improves throughput.
- **Cross-partition barrier cost is negligible** with only 3-4 levels — a few network round-trips per cycle.

### Trade assignment
How trades are distributed across workers is a deployment/business decision: round-robin, by currency pair (to share intermediate calcs locally), by desk, etc. Configured by the platform team, not by quants.

## Developer Experience

### Design principle
Distribution should be invisible to the end users of the framework (quants). They think in terms of computation — "this trade depends on spot, vol surface, and discount curve" — and the framework handles placement, serialization, and coordination.

### Quant-facing API
Looks identical to the single-engine API:

```rust
// Quant writes this — no awareness of distribution
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

// Registration — also no distribution awareness
graph.compute("trade-12345")
    .dep(&spot_eurusd)
    .dep(&vol_surface_eurusd)
    .dep(&discount_curve_usd)
    .node(VanillaOptionPricer { strike: 1.05, expiry: date });
```

The quant's only concession to distribution is `Serialize + Deserialize` on their types — which they'd want anyway for persistence and reporting.

### Infrastructure-facing API
Separate layer configured by the platform team:

```rust
let cluster = ClusterConfig::new()
    .core(CoreConfig::new().sources(&market_data_feeds))
    .worker_pool(WorkerPoolConfig::new()
        .min_workers(4)
        .assignment(AssignmentStrategy::ByCurrencyPair))
    .build();
```

### Automatic placement from dependency declarations
The dependency declarations already contain all the information the framework needs:
- `.dep(&spot_eurusd)` tells the framework this trade needs that value, which lives in the core
- Leaf-level compute nodes with no dependants → assigned to workers
- Nodes with many dependants across trades → stay in the core
- The serialization boundary emerges naturally from the graph topology

`NodeRef<T>` works transparently regardless of whether the dependency is local or remote.

### Backward activation
When a new trade is added to a worker, the worker tells the core "I need these intermediate nodes active." The core does its local backward activation to sources. The worker doesn't need to know anything about the core's internal topology.

## Autoscaling

### Principle: framework handles lifecycle, infrastructure handles scaling decisions
The framework does **not** own autoscaling. Infrastructure orchestrators (Kubernetes HPA, ECS autoscaling) already solve this well. The framework's responsibility is handling the **consequences** of scaling — workers appearing and disappearing gracefully.

### Responsibility boundary

| Concern | Owner |
|---|---|
| How many workers should be running | Infrastructure (K8s/ECS) |
| When to add/remove instances | Infrastructure (based on metrics the framework exposes) |
| What happens when a worker joins | Framework (registration, trade assignment, activation) |
| What happens when a worker leaves | Framework (trade redistribution, cleanup) |
| Health/readiness signals | Framework exposes, infrastructure consumes |

### Worker lifecycle

- **Registration**: new worker connects to the core, announces itself, gets assigned a slice of trades. The core pushes boundary values and the worker builds its local graph.
- **Deregistration**: worker signals the core it is shutting down (scale-down or rolling deploy). Its trades are redistributed to remaining workers before it exits.
- **Failure**: worker disappears without warning. The core detects this via heartbeat timeout and reassigns its trades to surviving workers.

### Readiness
Trade reassignment has a warm-up cost — boundary values must be pushed from the core, the local graph built, and a full propagation cycle completed. The framework exposes a **readiness probe** ("has this worker completed its first propagation cycle") so infrastructure doesn't route work to it prematurely.

### Metrics the framework exposes
Custom metrics for infrastructure to scale on (richer than raw CPU):
- Trades per worker
- Propagation cycle latency
- Worker queue depth

## State Recovery

### Principle: sources own their recovery
The framework does not checkpoint or replay values. Each source is responsible for re-establishing its own state on restart. For the primary use case, this means Redis key-value cache for cold start with pub/sub for live updates thereafter.

### Three source states
- **Has a value** — normal, propagate downstream
- **No value yet** — source is activated but hasn't received data. Downstream nodes wait. Not an error.
- **Error** — source failed (e.g., Redis connection lost, bad data). Error strategy applies.

The "no value yet" state is intentional and useful beyond recovery — e.g., a new currency pair being onboarded where the feed hasn't produced its first tick yet. The subgraph stays dormant until the first value arrives, then lights up naturally.

### Recovery is a normal update
When a source re-establishes its value after a restart, it is treated as a regular value update — the source sets its value, dependants are marked dirty, and the next propagation cycle runs. No special recovery mode exists. The framework doesn't need to know it was a restart.

### Observability
The framework must provide visibility into which sources are in the "no value yet" state and which subgraphs are blocked as a result. Without this, a missing Redis key becomes a silent mystery.