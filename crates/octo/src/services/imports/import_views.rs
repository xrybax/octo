//! What the dashboard and the Octo apps read about imports. The names are a contract with both:
//! the apps decode these fields by name, so a rename here is a change there. Written in
//! camelCase, nulls kept.

use chrono::{DateTime, Utc};
use octo_core::json::datetime;
use serde::Serialize;

/// Everything the import page shows for one person. LibraryProblem says why Octo cannot tell
/// which songs the library has, when it cannot; until then nothing is fetched.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOverview {
    pub lists: Vec<ListSummary>,
    pub trickle: TrickleView,
    pub library_problem: Option<String>,
    /// Only for a family member whose missing songs wait for approval; absent for everyone else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalView>,
}

/// A family member's lists fetch their missing songs only once `by` approves them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalView {
    pub needed: bool,
    pub by: String,
}

/// One list. Partial says, in words, why it is not the whole source: a link that shows only its
/// first songs, or a list saved from a Spotify account that is never read again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSummary {
    pub id: String,
    pub name: String,
    pub source: String,
    pub by: Option<String>,
    pub image_url: Option<String>,
    pub total: i32,
    pub have: i32,
    pub missing: i32,
    pub queued: i32,
    pub downloading: i32,
    pub done: i32,
    pub not_found: i32,
    pub skipped: i32,
    pub keep_playlist: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream_without_downloading: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming_playlist_id: Option<String>,
    pub playlist_id: Option<String>,
    pub playlist_note: Option<String>,
    pub get_missing: bool,
    pub partial: Option<String>,
    pub gone: bool,
    pub can_refresh: bool,
    #[serde(with = "datetime::utc")]
    pub read_utc: DateTime<Utc>,
    #[serde(with = "datetime::utc_option")]
    pub matched_utc: Option<DateTime<Utc>>,
}

/// One song of a list, or of the trickle. Progress is 0 to 1 while it downloads.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackView {
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub seconds: Option<i32>,
    pub state: String,
    pub detail: Option<String>,
    pub library_id: Option<String>,
    pub progress: Option<f64>,
    #[serde(with = "datetime::utc_option")]
    pub updated_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDetail {
    pub list: ListSummary,
    pub tracks: Vec<TrackView>,
}

/// The trickle as one person sees it: its state (see `TrickleStates`), when the next song
/// starts, the pace, their counts, the song running now, and the songs that finished last.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrickleView {
    pub state: String,
    #[serde(with = "datetime::utc_option")]
    pub next_utc: Option<DateTime<Utc>>,
    pub per_hour: i32,
    pub queued: i32,
    pub downloading: i32,
    pub done: i32,
    pub not_found: i32,
    pub skipped: i32,
    pub current: Option<TrackView>,
    pub recent: Vec<TrackView>,
    pub next: Vec<TrackView>,
}

/// The answer to an action: whether it did what was asked, what to tell the person, and anything
/// it made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportActionResult {
    pub ok: bool,
    pub message: String,
    pub url: Option<String>,
    pub list_id: Option<String>,
    pub count: i32,
}

impl ImportActionResult {
    pub fn ok(message: impl Into<String>) -> Self {
        ImportActionResult {
            ok: true,
            message: message.into(),
            url: None,
            list_id: None,
            count: 0,
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        ImportActionResult {
            ok: false,
            ..Self::ok(message)
        }
    }

    pub fn with_count(mut self, count: i32) -> Self {
        self.count = count;
        self
    }

    pub fn with_list(mut self, list_id: Option<String>) -> Self {
        self.list_id = list_id;
        self
    }

    pub fn with_url(mut self, url: String) -> Self {
        self.url = Some(url);
        self
    }
}
