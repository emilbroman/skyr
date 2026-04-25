//! Re-exports of the canonical [`rq::IncidentCategory`] under the local name
//! `Category`, kept for ergonomic in-crate use. The `rq` crate is the single
//! source of truth for the category set across the status-reporting subsystem.

pub use rq::{IncidentCategory as Category, InvalidCategory};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for c in Category::ALL {
            let s = c.to_string();
            let parsed: Category = s.parse().unwrap();
            assert_eq!(parsed, c);
        }
    }

    #[test]
    fn invalid_returns_error() {
        assert!("nonsense".parse::<Category>().is_err());
        // SCREAMING_SNAKE_CASE is required.
        assert!("crash".parse::<Category>().is_err());
    }

    #[test]
    fn ord_reflects_severity() {
        assert!(Category::Crash > Category::SystemError);
        assert!(Category::SystemError > Category::InconsistentState);
        assert!(Category::InconsistentState > Category::CannotProgress);
        assert!(Category::CannotProgress > Category::BadConfiguration);
    }

    #[test]
    fn worst_open_via_max() {
        let open = [
            Category::BadConfiguration,
            Category::SystemError,
            Category::CannotProgress,
        ];
        assert_eq!(open.iter().max().copied(), Some(Category::SystemError));
    }
}
