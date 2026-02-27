# FX Pricing Example

A real-time FX pricing engine built on the reactive-graph library. Spot rates
flow in from Valkey Pub/Sub, propagate through a reactive computation graph, and
produce continuously updating client-specific price quotes.

## Architecture

```
┌────────────┐      Valkey Pub/Sub        ┌──────────────────────────────────────┐
│ price-feed │ ──── fx:{pair}:{side} ───▶ │  fx-pricing server                   │
│ (random    │                            │                                      │
│  walk)     │                            │  SpotRate ──┐                        │
└────────────┘                            │  (bid)      ├──▶ MidRate ──▶ Client  │
                                          │  SpotRate ──┘    (avg)      Spread   │
                                          │  (ask)                   ──▶ Sink    │
                                          │                             (stdout) │
                                          └──────────────────────────────────────┘
```

Each trade registers a **subgraph** in the engine:

| Node | Type | Key (dedup) | Description |
|------|------|-------------|-------------|
| SpotRate (bid) | Source | `(pair, Bid)` | Subscribes to `fx:{pair}:bid` via Valkey Pub/Sub |
| SpotRate (ask) | Source | `(pair, Ask)` | Subscribes to `fx:{pair}:ask` via Valkey Pub/Sub |
| MidRate | Compute | `pair` | `(bid + ask) / 2` |
| ClientSpread | Compute | `trade_id` | Applies client-specific spread in basis points |
| PricePublisher | Sink | `trade_id` | Prints the final quote to stdout |

Sources and mid-rate nodes are **shared** across trades for the same currency
pair. Only the spread computation and publisher are per-trade. When a trade is
removed, its per-trade nodes are deactivated while shared nodes stay alive as
long as at least one trade still uses them.

## Running with Docker Compose

From this directory:

```sh
docker compose up --build
```

This starts four services:

| Service | Role |
|---------|------|
| `valkey` | Valkey 8 for Pub/Sub transport |
| `fx-pricing` | The reactive-graph pricing engine + HTTP API on port 3000 |
| `price-feed` | Publishes random-walk spot rates to Redis every 500ms |
| `seed-trades` | Creates 4 sample trades on startup, then exits |

After ~10 seconds you will see continuous price output from the `fx-pricing`
container:

```
[abc123] Hedge Fund Alpha EURUSD — bid: 1.07990 ask: 1.08010 mid: 1.08000 spread: 2bps
[def456] Bank Gamma USDJPY       — bid: 149.993 ask: 150.008 mid: 150.000 spread: 1.5bps
...
```

Prices update every 500ms with a random walk that mean-reverts toward realistic
base rates (EURUSD ~1.08, GBPUSD ~1.27, USDJPY ~150, AUDUSD ~0.65).

Press **Ctrl-C** to stop everything.

## HTTP API

The pricing server exposes a REST API on port 3000.

**Create a trade** — subscribes to spot rates and starts streaming prices:

```sh
curl -X POST http://localhost:3000/trades \
  -H "Content-Type: application/json" \
  -d '{"pair":"EURUSD","spread_bps":5,"client":"My Desk"}'
```

**List active trades:**

```sh
curl http://localhost:3000/trades
```

**Remove a trade** — tears down its subgraph:

```sh
curl -X DELETE http://localhost:3000/trades/{trade_id}
```

Supported currency pairs: `EURUSD`, `GBPUSD`, `USDJPY`, `AUDUSD`.

## Running without Docker

You need a running Valkey (or Redis-compatible) instance. Then:

```sh
# Terminal 1: start the pricing server
REDIS_URL=redis://127.0.0.1:6379 cargo run --example fx_pricing

# Terminal 2: publish random rates (requires valkey-cli or redis-cli)
sh price-feed.sh

# Terminal 3: create trades
sh seed-trades.sh
```
