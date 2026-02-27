use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CurrencyPair {
    EURUSD,
    GBPUSD,
    USDJPY,
    AUDUSD,
}

impl fmt::Display for CurrencyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CurrencyPair::EURUSD => write!(f, "EURUSD"),
            CurrencyPair::GBPUSD => write!(f, "GBPUSD"),
            CurrencyPair::USDJPY => write!(f, "USDJPY"),
            CurrencyPair::AUDUSD => write!(f, "AUDUSD"),
        }
    }
}

impl std::str::FromStr for CurrencyPair {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "EURUSD" => Ok(CurrencyPair::EURUSD),
            "GBPUSD" => Ok(CurrencyPair::GBPUSD),
            "USDJPY" => Ok(CurrencyPair::USDJPY),
            "AUDUSD" => Ok(CurrencyPair::AUDUSD),
            _ => Err(format!("Unknown currency pair: {s}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Bid,
    Ask,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Bid => write!(f, "bid"),
            Side::Ask => write!(f, "ask"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpreadQuote {
    pub pair: CurrencyPair,
    pub client: String,
    pub trade_id: String,
    pub bid: f64,
    pub ask: f64,
    pub mid: f64,
    pub spread_bps: f64,
}

#[derive(Debug, Deserialize)]
pub struct TradeRequest {
    pub pair: String,
    pub spread_bps: f64,
    pub client: String,
}

#[derive(Debug, Serialize)]
pub struct TradeResponse {
    pub trade_id: String,
    pub pair: String,
    pub client: String,
    pub spread_bps: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TradeInfo {
    pub trade_id: String,
    pub pair: String,
    pub client: String,
    pub spread_bps: f64,
}
