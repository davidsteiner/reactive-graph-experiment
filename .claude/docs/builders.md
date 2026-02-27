# Builder APIs

The framework provides two ways to define nodes: a **closure API** for ad-hoc nodes and a **trait API** for reusable, parameterised node types. Both produce `NodeRef<T>` and interoperate freely.

## Closure API

Closure builders are associated functions on `Graph` (not `&mut self` methods) to avoid double-borrow issues. The pattern is:

```rust
let builder = Graph::source::<f64>("my.source");        // no borrow
let node_ref = builder.on_activate(|tx| { ... }).build(g);  // borrows &mut Graph here
```

### Source

```rust
let source: NodeRef<f64> = Graph::source("price.feed")
    .on_activate(|tx: Sender<f64>| {
        // Subscribe to external data, return a guard
        let sub = feed.subscribe(move |v| tx.send(v));
        sub  // dropped when source deactivates
    })
    .build(&mut graph);
```

### Compute

Uses typestate to build a typed dep chain:

```rust
let derived: NodeRef<f64> = Graph::compute("mid.rate")
    .dep(bid)              // ComputeBuilder<f64, (f64,)>
    .dep(ask)              // ComputeBuilder<f64, (f64, f64)>
    .compute(|a: &f64, b: &f64| (a + b) / 2.0)  // -> ComputeBuilderReady<f64>
    .build(&mut graph);    // -> NodeRef<f64>
```

Each `.dep()` appends a type to the tuple. The `.compute()` closure must accept references matching the dep types in order. The compiler enforces this.

### Sink

```rust
Graph::sink("logger")
    .dep(mid_rate)
    .on_emit(|val: &f64| println!("mid = {val}"))
    .build(&mut graph);
```

### Async Compute

```rust
let result: NodeRef<Response> = Graph::async_compute("api.call")
    .dep(input_a)
    .dep(input_b)
    .compute(|a: &f64, b: &f64| {
        let a = *a; let b = *b;
        async move { service.call(a, b).await }
    })
    .policy(AsyncPolicy::LatestWins)
    .group(concurrency_handle)
    .build(&mut graph);
```

## Trait API

Trait-based nodes implement `Node` (identity + output type) plus `Source`, `Compute`, `Sink`, or `AsyncCompute`. The builder enforces that dep types match `Compute::Inputs` at compile time.

### Source

```rust
impl Node for SpotRate {
    type Output = Quote;
    type Key = (CurrencyPair, LP);
    fn key(&self) -> Self::Key { (self.pair, self.lp) }
}

impl Source for SpotRate {
    type Guard = Subscription;
    fn activate(&self, tx: Sender<Quote>) -> Subscription { ... }
}

let spot: NodeRef<Quote> = graph.add_source(SpotRate { pair: EURUSD, lp: Citi });
```

### Compute

```rust
impl Compute for MidRate {
    type Inputs = (Quote, Quote);  // declares what types it needs
    fn compute(&self, (bid, ask): (&Quote, &Quote)) -> f64 { ... }
}

let mid: NodeRef<f64> = graph.add_compute(MidRate { pair: EURUSD })
    .dep(bid)    // NodeRef<Quote> — matches first element
    .dep(ask)    // NodeRef<Quote> — matches second element
    .build();    // available only when Remaining = ()
```

Each `.dep()` peels the first element off the `Remaining` type parameter. The builder transitions from `TraitComputeBuilder<C, (Quote, Quote)>` → `TraitComputeBuilder<C, (Quote,)>` → `TraitComputeBuilder<C, ()>`. `.build()` is only available at `()`.

### Sink

```rust
impl Sink for PricePublisher {
    type Inputs = (SpreadQuote,);
    type Key = (CurrencyPair, ClientId);
    fn key(&self) -> Self::Key { (self.pair, self.client) }
    fn emit(&self, (quote,): (&SpreadQuote,)) { ... }
}

graph.add_sink(PricePublisher { pair, client })
    .dep(spread)
    .build();
```

## Typestate Mechanics

Both closure and trait builders use the typestate pattern implemented via macros. For each supported arity (1-8 for closures, 1-4 for trait builders), a macro generates:

1. An `impl` block for the builder with a `Deps`/`Remaining` tuple of that arity
2. A `.dep()` method that consumes `self`, pushes the dep ID, and returns a builder with the tuple shifted by one element

For closure builders, the shift appends to `Deps`:
```
ComputeBuilder<Out, ()>        → .dep(a) → ComputeBuilder<Out, (A,)>
ComputeBuilder<Out, (A,)>      → .dep(b) → ComputeBuilder<Out, (A, B)>
```

For trait builders, the shift removes from `Remaining`:
```
TraitComputeBuilder<C, (A, B)> → .dep(a) → TraitComputeBuilder<C, (B,)>
TraitComputeBuilder<C, (B,)>   → .dep(b) → TraitComputeBuilder<C, ()>
```

## Type Erasure: Dispatch Traits

When `.build()` is called on a trait-based builder, it needs to create a `Box<dyn AnyNodeImpl>` that can invoke the concrete node's `compute`/`emit` method using type-erased values from the store. This is done through dispatch traits:

```rust
trait ComputeDispatch<C: Compute> {
    fn make_node_impl(compute: C) -> Box<dyn AnyNodeImpl>;
}

// Generated for each arity:
impl<C, A, B> ComputeDispatch<C> for (A, B)
where C: Compute<Inputs = (A, B), ...> { ... }
```

The generated impl knows to downcast `deps[0]` as `&A` and `deps[1]` as `&B`, then call `node.compute((&a, &b))`.

## Subgraph Builders

`SubgraphBuilder` mirrors both closure and trait APIs. Closure builders gain a `.build_sub(&mut SubgraphBuilder)` method. Trait builders work through `SubgraphTraitComputeBuilder` and `SubgraphTraitSinkBuilder` with the same typestate dep-peeling pattern.

## Node Deduplication

Both `Graph::register_node` and `SubgraphBuilder::register_node` check for existing entries by `NodeId`. If a node with the same ID already exists, the ref count is incremented and the new node impl is discarded. This means two subgraphs referencing `SpotRate { pair: EURUSD, lp: Citi }` share a single source instance.

## Generic Parameter Naming

Macros use non-standard generic names to avoid collisions: `Cc` for arity-3 position and `Ff` for arity-6. This prevents conflicts with common single-letter generics like `C` (which might shadow a `Compute` bound).
