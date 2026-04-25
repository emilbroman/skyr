use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Consequence-oriented severity category that producers self-classify failure
/// reports into. The category is fixed at incident-open time and never
/// escalates or de-escalates within a single incident.
///
/// Variants are ordered from least-severe to most-severe; the derived `Ord`
/// reflects this so `worst_open_category` lookups are simply `.max()` over the
/// open set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Category {
    /// The system is working correctly and is refusing to roll out
    /// configuration it has determined to be invalid.
    BadConfiguration,
    /// The entity itself is stable, but a derived/dependent configuration
    /// could not be applied.
    CannotProgress,
    /// The configuration DAG has drifted from reality and reconciliation
    /// failed due to an irreconcilable inconsistency.
    InconsistentState,
    /// A failure in Skyr's own infrastructure (broker, DB, plugin host, etc.).
    /// The user's configuration is not at fault.
    SystemError,
    /// The entity is not behaving as intended, resulting in user-visible
    /// downtime.
    Crash,
}

impl Category {
    /// All five categories in defined (severity) order, least-severe first.
    pub const ALL: [Category; 5] = [
        Category::BadConfiguration,
        Category::CannotProgress,
        Category::InconsistentState,
        Category::SystemError,
        Category::Crash,
    ];

    /// Returns the canonical SCREAMING_SNAKE_CASE name used in the database
    /// and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Category::BadConfiguration => "BAD_CONFIGURATION",
            Category::CannotProgress => "CANNOT_PROGRESS",
            Category::InconsistentState => "INCONSISTENT_STATE",
            Category::SystemError => "SYSTEM_ERROR",
            Category::Crash => "CRASH",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Error, Debug, Clone)]
#[error("invalid incident category: {0:?}")]
pub struct InvalidCategory(pub String);

impl FromStr for Category {
    type Err = InvalidCategory;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BAD_CONFIGURATION" => Ok(Category::BadConfiguration),
            "CANNOT_PROGRESS" => Ok(Category::CannotProgress),
            "INCONSISTENT_STATE" => Ok(Category::InconsistentState),
            "SYSTEM_ERROR" => Ok(Category::SystemError),
            "CRASH" => Ok(Category::Crash),
            other => Err(InvalidCategory(other.to_string())),
        }
    }
}

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
        // Crash is the most severe; BadConfiguration is the least.
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
