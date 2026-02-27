FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src/ src/
COPY examples/ examples/
RUN cargo build --example fx_pricing --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/examples/fx_pricing /usr/local/bin/fx_pricing
CMD ["fx_pricing"]
