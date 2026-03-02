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