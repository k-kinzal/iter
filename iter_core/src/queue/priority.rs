//! [`Priority`] used by the [`Queue`](crate::queue::Queue) abstraction.

use serde::{Deserialize, Serialize};

/// Priority for queue ordering. Higher numeric value means higher priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Priority(u8);

impl Priority {
    /// Background work that may be deferred indefinitely.
    pub const LOW: Self = Self(25);
    /// Default priority used when no value is supplied.
    pub const NORMAL: Self = Self(50);
    /// Foreground work that should be processed promptly.
    pub const HIGH: Self = Self(75);
    /// Critical work that should preempt anything else.
    pub const CRITICAL: Self = Self(100);

    /// Create a `Priority` from an arbitrary `u8` value.
    #[must_use]
    pub const fn new(n: u8) -> Self {
        Self(n)
    }

    /// Return the numeric value backing this priority.
    #[must_use]
    pub const fn value(self) -> u8 {
        self.0
    }

    /// Map a priority keyword to a `Priority`.
    ///
    /// This is the single canonical keyword→priority mapping for the whole
    /// workspace; the shell backend and any Signal source that accepts a
    /// keyword string consume it here rather than re-spelling the four levels.
    /// Returns `None` for an unknown keyword. Matching is ASCII
    /// case-insensitive.
    #[must_use]
    pub fn from_keyword(keyword: &str) -> Option<Self> {
        match keyword.to_ascii_lowercase().as_str() {
            "low" => Some(Self::LOW),
            "normal" => Some(Self::NORMAL),
            "high" => Some(Self::HIGH),
            "critical" => Some(Self::CRITICAL),
            _ => None,
        }
    }

    /// Bucket this priority back to the nearest keyword at or below its value.
    ///
    /// The inverse of [`Priority::from_keyword`] for the four canonical
    /// constants; arbitrary intermediate values bucket down to the
    /// next-lower keyword.
    #[must_use]
    pub fn keyword(self) -> &'static str {
        if self >= Self::CRITICAL {
            "critical"
        } else if self >= Self::HIGH {
            "high"
        } else if self >= Self::NORMAL {
            "normal"
        } else {
            "low"
        }
    }
}

impl Default for Priority {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl From<u8> for Priority {
    fn from(value: u8) -> Self {
        Self::new(value)
    }
}

impl From<Priority> for u8 {
    fn from(value: Priority) -> Self {
        value.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_ordered() {
        assert!(Priority::CRITICAL > Priority::HIGH);
        assert!(Priority::HIGH > Priority::NORMAL);
        assert!(Priority::NORMAL > Priority::LOW);
    }

    #[test]
    fn default_is_normal() {
        assert_eq!(Priority::default(), Priority::NORMAL);
    }

    #[test]
    fn constant_values() {
        assert_eq!(Priority::LOW.value(), 25);
        assert_eq!(Priority::NORMAL.value(), 50);
        assert_eq!(Priority::HIGH.value(), 75);
        assert_eq!(Priority::CRITICAL.value(), 100);
    }

    #[test]
    fn from_keyword_maps_the_four_levels() {
        assert_eq!(Priority::from_keyword("low"), Some(Priority::LOW));
        assert_eq!(Priority::from_keyword("normal"), Some(Priority::NORMAL));
        assert_eq!(Priority::from_keyword("HIGH"), Some(Priority::HIGH));
        assert_eq!(Priority::from_keyword("Critical"), Some(Priority::CRITICAL));
        assert_eq!(Priority::from_keyword("nope"), None);
    }

    #[test]
    fn keyword_buckets_back() {
        assert_eq!(Priority::LOW.keyword(), "low");
        assert_eq!(Priority::NORMAL.keyword(), "normal");
        assert_eq!(Priority::HIGH.keyword(), "high");
        assert_eq!(Priority::CRITICAL.keyword(), "critical");
        assert_eq!(Priority::new(60).keyword(), "normal");
        assert_eq!(Priority::new(80).keyword(), "high");
        assert_eq!(Priority::new(101).keyword(), "critical");
    }

    #[test]
    fn serializes_as_integer() {
        let p = Priority::HIGH;
        let json = serde_json::to_string(&p).expect("serialize");
        assert_eq!(json, "75");
        let back: Priority = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(p, back);
    }

    #[test]
    fn ord_sorts_low_to_high() {
        let mut v = vec![
            Priority::CRITICAL,
            Priority::LOW,
            Priority::HIGH,
            Priority::NORMAL,
        ];
        v.sort();
        assert_eq!(
            v,
            vec![
                Priority::LOW,
                Priority::NORMAL,
                Priority::HIGH,
                Priority::CRITICAL
            ]
        );
    }
}
