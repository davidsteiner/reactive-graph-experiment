use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use reactive_graph::{Compute, Node, Sender, Sink, Source, Subgraph, SubgraphBuilder};

use crate::types::{CurrencyPair, Side, SpreadQuote};

// === SpotRate Source ===

pub struct SpotRate {
    pub pair: CurrencyPair,
    pub side: Side,
    pub redis_url: String,
}

pub struct SpotRateGuard {
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for SpotRateGuard {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Node for SpotRate {
    type Output = f64;
    type Key = (CurrencyPair, Side);

    fn key(&self) -> Self::Key {
        (self.pair, self.side)
    }
}

impl Source for SpotRate {
    type Guard = SpotRateGuard;

    fn activate(&self, tx: Sender<f64>) -> Self::Guard {
        let channel = format!("fx:{}:{}", self.pair, self.side);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let redis_url = self.redis_url.clone();

        let handle = std::thread::spawn(move || {
            let client = match redis::Client::open(redis_url.as_str()) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Redis client error: {e}");
                    return;
                }
            };
            let mut conn = match client.get_connection() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Redis connection error: {e}");
                    return;
                }
            };

            // Set read timeout so get_message() returns periodically for shutdown checks
            let _ = conn.set_read_timeout(Some(Duration::from_millis(500)));

            let mut pubsub = conn.as_pubsub();
            if let Err(e) = pubsub.subscribe(&channel) {
                eprintln!("Redis subscribe error: {e}");
                return;
            }

            println!("Subscribed to {channel}");

            while !shutdown_clone.load(Ordering::Relaxed) {
                match pubsub.get_message() {
                    Ok(msg) => {
                        if let Ok(payload) = msg.get_payload::<String>() {
                            if let Ok(rate) = payload.parse::<f64>() {
                                tx.send(rate);
                            }
                        }
                    }
                    Err(_) => continue, // timeout — loop to check shutdown flag
                }
            }

            println!("Unsubscribed from {channel}");
        });

        SpotRateGuard {
            shutdown,
            handle: Some(handle),
        }
    }
}

// === MidRate Compute ===

pub struct MidRate {
    pub pair: CurrencyPair,
}

impl Node for MidRate {
    type Output = f64;
    type Key = CurrencyPair;

    fn key(&self) -> Self::Key {
        self.pair
    }
}

impl Compute for MidRate {
    type Inputs = (f64, f64);

    fn compute(&self, (bid, ask): (&f64, &f64)) -> f64 {
        (bid + ask) / 2.0
    }
}

// === ClientSpread Compute ===

pub struct ClientSpread {
    pub trade_id: String,
    pub pair: CurrencyPair,
    pub client: String,
    pub spread_bps: f64,
}

impl Node for ClientSpread {
    type Output = SpreadQuote;
    type Key = String;

    fn key(&self) -> Self::Key {
        self.trade_id.clone()
    }
}

impl Compute for ClientSpread {
    type Inputs = (f64,);

    fn compute(&self, (mid,): (&f64,)) -> SpreadQuote {
        let half_spread = mid * self.spread_bps / 10_000.0 / 2.0;
        SpreadQuote {
            pair: self.pair,
            client: self.client.clone(),
            trade_id: self.trade_id.clone(),
            bid: mid - half_spread,
            ask: mid + half_spread,
            mid: *mid,
            spread_bps: self.spread_bps,
        }
    }
}

// === PricePublisher Sink ===

pub struct PricePublisher {
    pub trade_id: String,
}

impl Sink for PricePublisher {
    type Inputs = (SpreadQuote,);
    type Key = String;

    fn key(&self) -> Self::Key {
        self.trade_id.clone()
    }

    fn emit(&self, (quote,): (&SpreadQuote,)) {
        println!(
            "[{}] {} {} — bid: {:.5} ask: {:.5} mid: {:.5} spread: {}bps",
            quote.trade_id,
            quote.client,
            quote.pair,
            quote.bid,
            quote.ask,
            quote.mid,
            quote.spread_bps
        );
    }
}

// === Build trade subgraph ===

pub fn build_trade_subgraph(
    trade_id: String,
    pair: CurrencyPair,
    client: String,
    spread_bps: f64,
    redis_url: String,
) -> Subgraph {
    let mut sb = SubgraphBuilder::new();

    // Shared sources — dedup'd by (pair, side) across trades
    let bid = sb.add_source(SpotRate {
        pair,
        side: Side::Bid,
        redis_url: redis_url.clone(),
    });
    let ask = sb.add_source(SpotRate {
        pair,
        side: Side::Ask,
        redis_url,
    });

    // Shared compute — dedup'd by pair across trades
    let mid = sb.add_compute(MidRate { pair }).dep(bid).dep(ask).build();

    // Per-trade compute
    let spread = sb
        .add_compute(ClientSpread {
            trade_id: trade_id.clone(),
            pair,
            client,
            spread_bps,
        })
        .dep(mid)
        .build();

    // Per-trade sink
    sb.add_sink(PricePublisher { trade_id }).dep(spread).build();

    sb.build()
}
