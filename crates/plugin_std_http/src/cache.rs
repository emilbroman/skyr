//! HTTP cache freshness logic for `Std/HTTP.Get`.
//!
//! Decides whether a previously fetched response is still fresh based on the
//! response's `cache-control` and `date` headers, so the `check` lifecycle
//! method can skip the network round-trip when nothing has expired.
//!
//! Recognised `cache-control` directives: `max-age=N` and `immutable` set a
//! freshness window; `no-store`, `no-cache`, and `must-revalidate` force a
//! refetch. Any directive we don't recognise is silently skipped.

use std::time::{Duration, SystemTime};

#[derive(Debug, PartialEq, Eq)]
pub enum CacheDecision {
    /// Response is fresh until `now < date + max_age`.
    Fresh { max_age: Duration },
    /// Header explicitly demands a refetch.
    Refetch,
    /// No usable freshness info — caller should refetch.
    Unknown,
}

pub fn parse_cache_control(value: &str) -> CacheDecision {
    let mut max_age: Option<Duration> = None;
    let mut force_refetch = false;

    for raw in value.split(',') {
        let directive = raw.trim();
        if directive.is_empty() {
            continue;
        }
        let (name, arg) = match directive.split_once('=') {
            Some((n, a)) => (n.trim().to_lowercase(), Some(a.trim().trim_matches('"'))),
            None => (directive.to_lowercase(), None),
        };
        match name.as_str() {
            "no-store" | "no-cache" | "must-revalidate" => force_refetch = true,
            "max-age" => {
                if let Some(secs_str) = arg
                    && let Ok(secs) = secs_str.parse::<u64>()
                {
                    max_age = Some(Duration::from_secs(secs));
                }
            }
            "immutable" => {
                // No-op on its own; relies on max-age for the window.
            }
            _ => {
                // Unknown directive — skip per spec.
            }
        }
    }

    if force_refetch {
        CacheDecision::Refetch
    } else if let Some(max_age) = max_age {
        CacheDecision::Fresh { max_age }
    } else {
        CacheDecision::Unknown
    }
}

pub fn is_fresh(date: SystemTime, max_age: Duration, now: SystemTime) -> bool {
    let elapsed = now.duration_since(date).unwrap_or(Duration::ZERO);
    elapsed <= max_age
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duration(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[test]
    fn parses_max_age() {
        assert_eq!(
            parse_cache_control("max-age=600"),
            CacheDecision::Fresh {
                max_age: duration(600)
            }
        );
    }

    #[test]
    fn parses_max_age_with_whitespace_and_other_directives() {
        assert_eq!(
            parse_cache_control(" public ,  max-age=3600 , immutable "),
            CacheDecision::Fresh {
                max_age: duration(3600)
            }
        );
    }

    #[test]
    fn no_store_overrides_max_age() {
        assert_eq!(
            parse_cache_control("max-age=600, no-store"),
            CacheDecision::Refetch
        );
    }

    #[test]
    fn no_cache_overrides_max_age() {
        assert_eq!(
            parse_cache_control("no-cache, max-age=60"),
            CacheDecision::Refetch
        );
    }

    #[test]
    fn must_revalidate_forces_refetch() {
        assert_eq!(
            parse_cache_control("must-revalidate"),
            CacheDecision::Refetch
        );
    }

    #[test]
    fn unknown_directives_are_skipped() {
        assert_eq!(parse_cache_control("foo, bar=baz"), CacheDecision::Unknown);
        assert_eq!(
            parse_cache_control("foo=1, max-age=10, bar"),
            CacheDecision::Fresh {
                max_age: duration(10)
            }
        );
    }

    #[test]
    fn malformed_max_age_is_skipped() {
        assert_eq!(parse_cache_control("max-age=abc"), CacheDecision::Unknown);
        assert_eq!(parse_cache_control("max-age="), CacheDecision::Unknown);
    }

    #[test]
    fn quoted_max_age_is_accepted() {
        assert_eq!(
            parse_cache_control("max-age=\"120\""),
            CacheDecision::Fresh {
                max_age: duration(120)
            }
        );
    }

    #[test]
    fn empty_value_is_unknown() {
        assert_eq!(parse_cache_control(""), CacheDecision::Unknown);
        assert_eq!(parse_cache_control(" , , "), CacheDecision::Unknown);
    }

    #[test]
    fn immutable_alone_is_unknown() {
        assert_eq!(parse_cache_control("immutable"), CacheDecision::Unknown);
    }

    #[test]
    fn case_insensitive_directive_names() {
        assert_eq!(
            parse_cache_control("Max-Age=42"),
            CacheDecision::Fresh {
                max_age: duration(42)
            }
        );
        assert_eq!(parse_cache_control("NO-STORE"), CacheDecision::Refetch);
    }

    #[test]
    fn fresh_when_within_window() {
        let date = SystemTime::UNIX_EPOCH + duration(1_000_000);
        let now = date + duration(300);
        assert!(is_fresh(date, duration(600), now));
    }

    #[test]
    fn stale_when_past_window() {
        let date = SystemTime::UNIX_EPOCH + duration(1_000_000);
        let now = date + duration(601);
        assert!(!is_fresh(date, duration(600), now));
    }

    #[test]
    fn fresh_when_server_clock_ahead() {
        let date = SystemTime::UNIX_EPOCH + duration(2_000_000);
        let now = SystemTime::UNIX_EPOCH + duration(1_000_000);
        assert!(is_fresh(date, duration(60), now));
    }

    #[test]
    fn fresh_at_exact_boundary() {
        let date = SystemTime::UNIX_EPOCH + duration(1_000_000);
        let now = date + duration(600);
        assert!(is_fresh(date, duration(600), now));
    }
}
