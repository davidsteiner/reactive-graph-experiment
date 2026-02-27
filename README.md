# reactive-graph-experiment

A proof-of-concept reactive computation graph engine in Rust.

You define sources, compute nodes, and sinks. When a source value changes, the
engine figures out what depends on it and re-runs only the affected nodes in
topological order.

## Features

- Trait-based and closure-based APIs for defining nodes
- Subgraphs with automatic dedup of shared nodes
- Sequential and parallel (wave-based) execution
- Async compute nodes with configurable policies (latest-wins, queue, debounce)
- Dynamic merge/removal of subgraphs at runtime
- Zero unsafe code

## Example

See [`examples/fx_pricing/`](examples/fx_pricing/) for a working demo that
streams FX rates through a reactive graph. It runs self-contained with Docker
Compose.

## Status

This is a PoC. The API will change.
