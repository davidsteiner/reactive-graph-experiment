/// Error strategies for node computation failures.
#[derive(Debug, Clone)]
pub enum ErrorStrategy {
    /// Retain the last good value and do not propagate.
    PropagateStale,
    /// Forward the error as the node's value.
    PropagateError,
    /// Re-invoke with backoff, up to max_attempts.
    Retry {
        max_attempts: usize,
        backoff_ms: u64,
    },
}

impl Default for ErrorStrategy {
    fn default() -> Self {
        ErrorStrategy::PropagateStale
    }
}
