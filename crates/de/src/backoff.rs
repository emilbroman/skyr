use std::time::Duration;

/// Initial backoff delay after the first failure.
pub(crate) const BACKOFF_INITIAL: Duration = Duration::from_secs(5);
/// Maximum backoff delay between reconciliation attempts.
pub(crate) const BACKOFF_MAX: Duration = Duration::from_secs(24 * 60 * 60);
/// Multiplicative growth factor per failed attempt.
pub(crate) const BACKOFF_FACTOR: f64 = 1.1;

/// Compute the backoff duration for the given number of consecutive failures.
pub(crate) fn backoff_duration(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let factor = BACKOFF_FACTOR.powi(failures.saturating_sub(1) as i32);
    let delay_secs = (BACKOFF_INITIAL.as_secs_f64() * factor).min(BACKOFF_MAX.as_secs_f64());
    Duration::from_secs_f64(delay_secs)
}
