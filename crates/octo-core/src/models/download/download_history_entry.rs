//! The entries of `downloads-history.json`.

use serde::{Deserialize, Serialize};

use crate::tagging::tag_plan::TagReport;

/// One entry in the running log of songs Octo has fetched (via download-on-star or
/// permanent-mode playback). Surfaced in the admin dashboard's "Fetched songs" view.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DownloadHistoryEntry {
    // A JSON null in these strings reads as the default.
    #[serde(deserialize_with = "crate::models::null_as_default")]
    pub artist: String,
    #[serde(deserialize_with = "crate::models::null_as_default")]
    pub title: String,
    pub album: Option<String>,

    /// Absolute path the file was saved to.
    #[serde(deserialize_with = "crate::models::null_as_default")]
    pub path: String,

    /// File format, upper-cased from the extension (FLAC, MP3, M4A).
    #[serde(deserialize_with = "crate::models::null_as_default")]
    pub format: String,

    /// Where it came from: "Soulseek" (FLAC) or "YouTube" (MP3).
    #[serde(deserialize_with = "crate::models::null_as_default")]
    pub source: String,

    /// Cover art URL (Deezer), for the thumbnail in the log.
    pub cover_art_url: Option<String>,

    pub size_bytes: i64,

    /// For a file that claims to be lossless and whose spectrum says it was made from a lossy
    /// one, what it was likely made from ("about 128 kbps MP3"). Null otherwise.
    pub transcoded_from: Option<String>,

    /// How the file was tagged: the release that won, by how much, and where each field
    /// came from. Null for entries written before this existed.
    pub tagging: Option<TagReport>,

    /// When it was saved (ISO 8601, UTC).
    #[serde(deserialize_with = "crate::models::null_as_default")]
    pub downloaded_at: String,

    /// Who asked for this file, when Octo could tell. A star or a play carries the Subsonic
    /// username; an acquisition Octo started itself carries nobody, and so does every entry
    /// written before this field existed.
    ///
    /// A list rather than one name, because a second user starring a track that is already
    /// being fetched joins that transfer instead of starting another. Recording only whoever
    /// got there first would attribute the file to one of them and silently drop the rest.
    pub requested_by: Option<Vec<String>>,

    /// The download's row in the live downloads list ("provider:id"), the key getAcquisition and
    /// the dashboard read its log by. None for entries written before this existed.
    pub key: Option<String>,

    /// The download's log, saved when it ended so the dashboard can show it after the live list
    /// forgets it (three hours). Trimmed to stay small (`DownloadHistoryService::attach_log`).
    /// None for entries written before this existed, and for older entries whose log was let go
    /// to keep the file small.
    pub log: Option<Vec<super::acquisition_event::AcquisitionEvent>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_history_fixture_round_trips_byte_for_byte() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/rust-migration/fixtures/state/downloads-history.json"
        );
        let text = std::fs::read_to_string(path).expect("the fixture is in the repo");

        let entries: Vec<DownloadHistoryEntry> = serde_json::from_str(&text).expect("the fixture reads");
        assert!(entries.iter().any(|entry| entry.tagging.is_some()));
        assert!(entries.iter().any(|entry| entry.tagging.is_none()));

        assert!(entries.iter().any(|entry| entry.log.is_some()));
        assert_eq!(
            crate::json::to_string_without_nulls(&entries),
            text.trim_end_matches('\n')
        );
    }

    /// A log line as 2026.10.13 reads it: no Heard.
    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    #[allow(dead_code)]
    struct LineBefore {
        #[serde(
            default = "crate::json::datetime::min_value",
            with = "crate::json::datetime::utc"
        )]
        at: chrono::DateTime<chrono::Utc>,
        #[serde(default, deserialize_with = "crate::models::null_as_default")]
        kind: String,
        #[serde(default, deserialize_with = "crate::models::null_as_default")]
        text: String,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        candidates: Option<Vec<super::super::acquisition_event::AcquisitionCandidate>>,
    }

    /// The entry as 2026.10.13 reads it.
    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "PascalCase", default)]
    #[allow(dead_code)]
    #[derive(Default)]
    struct EntryBefore {
        #[serde(deserialize_with = "crate::models::null_as_default")]
        artist: String,
        #[serde(deserialize_with = "crate::models::null_as_default")]
        title: String,
        album: Option<String>,
        #[serde(deserialize_with = "crate::models::null_as_default")]
        path: String,
        #[serde(deserialize_with = "crate::models::null_as_default")]
        format: String,
        #[serde(deserialize_with = "crate::models::null_as_default")]
        source: String,
        cover_art_url: Option<String>,
        size_bytes: i64,
        transcoded_from: Option<String>,
        tagging: Option<TagReport>,
        #[serde(deserialize_with = "crate::models::null_as_default")]
        downloaded_at: String,
        requested_by: Option<Vec<String>>,
        key: Option<String>,
        log: Option<Vec<LineBefore>>,
    }

    /// A history file whose check line keeps what AcoustID heard still loads where Heard is
    /// unknown (a parse error there would start the history fresh), and a line without it is
    /// written as before.
    #[test]
    fn a_history_file_with_heard_recordings_still_loads_in_the_entry_before_them() {
        use super::super::acquisition_event::{AcquisitionEvent, AcquisitionHeard};

        let check = AcquisitionEvent {
            at: chrono::DateTime::from_timestamp(1_760_000_000, 0).expect("a time"),
            kind: "check".into(),
            text: "Passed the checks".into(),
            detail: Some(
                "the right length; AcoustID: no match above 85% (best 61%), so the name and length decide"
                    .into(),
            ),
            candidates: None,
            heard: Some(vec![AcquisitionHeard {
                title: "Teardrop".into(),
                artist: "Massive Attack".into(),
                score: 61,
            }]),
        };
        let plain = AcquisitionEvent {
            heard: None,
            text: "Found 3 copies".into(),
            ..check.clone()
        };
        let entries = vec![DownloadHistoryEntry {
            artist: "Massive Attack".into(),
            title: "Teardrop".into(),
            log: Some(vec![check.clone(), plain]),
            ..Default::default()
        }];
        let text = crate::json::to_string_without_nulls(&entries);
        assert!(
            text.contains(r#""Heard":[{"Title":"Teardrop","Artist":"Massive Attack","Score":61}]"#),
            "{text}"
        );
        assert_eq!(text.matches("\"Heard\"").count(), 1, "{text}");

        let before: Vec<EntryBefore> = serde_json::from_str(&text).expect("the older entry reads it");
        let log = before[0].log.as_ref().expect("the log");
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].text, "Passed the checks");
        assert_eq!(log[0].detail, check.detail);

        let again: Vec<DownloadHistoryEntry> = serde_json::from_str(&text).expect("reads back");
        assert_eq!(again, entries);
    }

    #[test]
    fn a_null_in_a_string_field_still_reads() {
        // A hand-edited null in a string field must not lose the whole log.
        let entries: Vec<DownloadHistoryEntry> = serde_json::from_str(
            r#"[{"Artist":null,"Title":"T","Tagging":{"Confidence":null,"Notes":null}}]"#,
        )
        .expect("reads");
        assert_eq!(entries[0].artist, "");
        assert_eq!(entries[0].title, "T");
        assert!(
            entries[0]
                .tagging
                .as_ref()
                .is_some_and(|tagging| tagging.notes.is_empty())
        );
    }
}
