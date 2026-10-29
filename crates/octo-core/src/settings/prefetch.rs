//! The `Prefetch` section: getting the songs that play next ahead of time, as real FLAC copies
//! kept in a folder of their own outside the music folder, so an outside song plays lossless
//! from its first second. Everything is read live.

use serde::{Deserialize, Serialize};

/// Which upcoming songs are fetched ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize, Serialize)]
pub enum PrefetchScope {
    /// Only songs a radio offered (song radio, the stations, the mixes).
    #[default]
    RadioOnly,

    /// Every outside song coming up in the play queue.
    All,
}

impl PrefetchScope {
    /// Every scope, in the order the dashboard lists them.
    pub const ALL: [PrefetchScope; 2] = [PrefetchScope::RadioOnly, PrefetchScope::All];

    /// The scope's name, as settings.json and the dashboard spell it.
    pub fn name(self) -> &'static str {
        match self {
            PrefetchScope::RadioOnly => "RadioOnly",
            PrefetchScope::All => "All",
        }
    }
}

/// Getting upcoming songs ahead, off until someone turns it on.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PrefetchSettings {
    /// Whether upcoming songs are fetched ahead at all. PREFETCH_ENABLED.
    pub enabled: bool,

    /// Which upcoming songs: radio ones only, or every outside song. PREFETCH_SCOPE.
    pub scope: PrefetchScope,

    /// How many songs after the playing one are fetched (1 to 10). PREFETCH_COUNT.
    pub count: i32,

    /// How many are fetched at once (1 to 3). PREFETCH_AT_ONCE.
    pub at_once: i32,

    /// How much the folder may hold, in GB (1 to 2000). PREFETCH_CACHE_GB.
    pub cache_gb: i32,

    /// How many days a song nobody played is kept (1 to 365). PREFETCH_KEEP_DAYS.
    pub keep_days: i32,

    /// No new song is fetched while the folder's disk has less free than this, in GB (1 to 500).
    /// PREFETCH_MIN_FREE_GB.
    pub min_free_gb: i32,

    /// Nothing new is fetched while nobody has played anything for ten minutes.
    /// PREFETCH_PAUSE_WHEN_STOPPED.
    pub pause_when_stopped: bool,

    /// Where the songs are kept. Blank: `prefetch` beside settings.json. PREFETCH_FOLDER.
    pub folder: String,

    /// Keep the first and a few random outside songs of each listener's mixes ready, so a mix
    /// starts lossless even on shuffle. PREFETCH_KEEP_MIXES_READY.
    pub keep_mixes_ready: bool,

    /// How many songs of each mix are kept ready, from its start and at random each (1 to 20).
    /// PREFETCH_KEEP_READY_COUNT.
    pub keep_ready_count: i32,
}

impl Default for PrefetchSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            scope: PrefetchScope::RadioOnly,
            count: 3,
            at_once: 1,
            cache_gb: 20,
            keep_days: 30,
            min_free_gb: 10,
            pause_when_stopped: true,
            folder: String::new(),
            keep_mixes_ready: false,
            keep_ready_count: 5,
        }
    }
}

const GB: u64 = 1024 * 1024 * 1024;

impl PrefetchSettings {
    /// Songs after the playing one, 1 to 10.
    pub fn effective_count(&self) -> usize {
        self.count.clamp(1, 10) as usize
    }

    /// Songs fetched at once, 1 to 3.
    pub fn effective_at_once(&self) -> usize {
        self.at_once.clamp(1, 3) as usize
    }

    /// The folder's size cap in bytes, 1 to 2000 GB.
    pub fn effective_cache_bytes(&self) -> u64 {
        self.cache_gb.clamp(1, 2000) as u64 * GB
    }

    /// Days an unplayed song is kept, 1 to 365.
    pub fn effective_keep_days(&self) -> i64 {
        i64::from(self.keep_days.clamp(1, 365))
    }

    /// The free space floor in bytes, 1 to 500 GB.
    pub fn effective_min_free_bytes(&self) -> u64 {
        self.min_free_gb.clamp(1, 500) as u64 * GB
    }

    /// Songs kept ready per mix, from its start and at random each, 1 to 20.
    pub fn effective_keep_ready_count(&self) -> usize {
        self.keep_ready_count.clamp(1, 20) as usize
    }

    /// The folder as typed, trimmed and without a trailing slash, or None for the default.
    pub fn effective_folder(&self) -> Option<String> {
        let folder = self.folder.trim();
        let folder = if folder.len() > 1 {
            folder.trim_end_matches('/')
        } else {
            folder
        };
        (!folder.is_empty()).then(|| folder.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigTree;

    #[test]
    fn off_by_default_with_modest_limits() {
        let settings = PrefetchSettings::default();
        assert!(!settings.enabled);
        assert_eq!(settings.scope, PrefetchScope::RadioOnly);
        assert_eq!(settings.effective_count(), 3);
        assert_eq!(settings.effective_at_once(), 1);
        assert_eq!(settings.effective_cache_bytes(), 20 * GB);
        assert_eq!(settings.effective_keep_days(), 30);
        assert_eq!(settings.effective_min_free_bytes(), 10 * GB);
        assert!(settings.pause_when_stopped);
        assert_eq!(settings.effective_folder(), None);
        assert!(!settings.keep_mixes_ready);
        assert_eq!(settings.effective_keep_ready_count(), 5);
    }

    #[test]
    fn binds_from_the_section_and_clamps_at_read() {
        let mut tree = ConfigTree::new();
        tree.set("Prefetch:Enabled", Some("true".into()));
        tree.set("Prefetch:Scope", Some("All".into()));
        tree.set("Prefetch:Count", Some("40".into()));
        tree.set("Prefetch:AtOnce", Some("0".into()));
        tree.set("Prefetch:CacheGb", Some("9000".into()));
        tree.set("Prefetch:KeepDays", Some("0".into()));
        tree.set("Prefetch:MinFreeGb", Some("501".into()));
        tree.set("Prefetch:PauseWhenStopped", Some("false".into()));
        tree.set("Prefetch:Folder", Some(" /cache/ahead/ ".into()));
        tree.set("Prefetch:KeepMixesReady", Some("true".into()));
        tree.set("Prefetch:KeepReadyCount", Some("99".into()));
        let (all, warnings) = crate::settings::AppSettings::bind(&tree);
        assert!(warnings.is_empty(), "{warnings:?}");
        let settings = all.prefetch;
        assert!(settings.enabled);
        assert_eq!(settings.scope, PrefetchScope::All);
        assert_eq!(settings.effective_count(), 10);
        assert_eq!(settings.effective_at_once(), 1);
        assert_eq!(settings.effective_cache_bytes(), 2000 * GB);
        assert_eq!(settings.effective_keep_days(), 1);
        assert_eq!(settings.effective_min_free_bytes(), 500 * GB);
        assert!(!settings.pause_when_stopped);
        assert_eq!(settings.effective_folder().as_deref(), Some("/cache/ahead"));
        assert!(settings.keep_mixes_ready);
        assert_eq!(settings.effective_keep_ready_count(), 20);
    }
}
