//! The `Listening` section: whether Octo keeps each person's plays for their Listening page,
//! and for how long.

use serde::{Deserialize, Serialize};

/// The shortest and longest time plays are kept, in days.
pub const KEEP_DAYS_RANGE: (i32, i32) = (7, 365);

/// What Octo keeps of each person's listening. On by default, 90 days.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ListeningSettings {
    /// Whether completed plays are written down at all.
    pub enabled: bool,

    /// How many days of plays are kept (7 to 365). Older plays are removed once a day.
    pub keep_days: i32,
}

impl Default for ListeningSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_days: 90,
        }
    }
}

impl ListeningSettings {
    /// The days kept, 7 to 365.
    pub fn effective_keep_days(&self) -> i64 {
        i64::from(self.keep_days.clamp(KEEP_DAYS_RANGE.0, KEEP_DAYS_RANGE.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigTree;

    #[test]
    fn on_by_default_for_ninety_days() {
        let settings = ListeningSettings::default();
        assert!(settings.enabled);
        assert_eq!(settings.effective_keep_days(), 90);
    }

    #[test]
    fn binds_from_the_section_and_clamps_the_days() {
        let mut tree = ConfigTree::new();
        tree.set("Listening:Enabled", Some("false".into()));
        tree.set("Listening:KeepDays", Some("2".into()));
        let (all, warnings) = crate::settings::AppSettings::bind(&tree);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!all.listening.enabled);
        assert_eq!(all.listening.effective_keep_days(), 7);
        let long = ListeningSettings {
            keep_days: 9000,
            ..Default::default()
        };
        assert_eq!(long.effective_keep_days(), 365);
    }
}
