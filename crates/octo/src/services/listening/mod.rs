//! Listening: each person's completed plays, kept on this server for their Listening page
//! (the dashboard's for the admin, the Family page's for members), and the page worked out
//! from them.

pub mod genres;
pub mod listening_log;
pub mod rollups;

use std::sync::Arc;

use chrono::Offset;
use serde_json::{Value, json};

pub use genres::AlbumLookup;
pub use listening_log::{Draft, ListeningLog, Play, Settled, UNKNOWN_APP, UNKNOWN_PLATFORM, platform_of};

use crate::services::last_fm::LastFmService;

/// Writes plays that have ended, each once its genre is worked out: the album and Last.fm are
/// asked here, after the caller has answered, and the file is written off the async workers.
pub fn write_settled(log: Arc<ListeningLog>, last_fm: Arc<LastFmService>, settled: Vec<Settled>) {
    for play in settled {
        let log = Arc::clone(&log);
        let last_fm = Arc::clone(&last_fm);
        tokio::spawn(async move {
            // The track's own artist: an album credit such as "Various Artists" names nobody.
            let artist = play.play.artist.clone();
            let genre = genres::resolve(&log, &last_fm, &play.genres, play.album.clone(), &artist).await;
            let _ = tokio::task::spawn_blocking(move || log.write(play, genre)).await;
        });
    }
}

/// The server's time zone: its name as `TZ` gives it ("America/Los_Angeles"), or None when
/// `TZ` is not set; and its offset from UTC now, in minutes.
pub fn time_zone() -> (Option<String>, i32) {
    let name = std::env::var("TZ")
        .ok()
        .map(|tz| tz.trim().trim_start_matches(':').to_string())
        .filter(|tz| !tz.is_empty());
    let offset = chrono::Local::now().offset().fix().local_minus_utc() / 60;
    (name, offset)
}

/// The time zone as the page says it: the city of its name ("Los Angeles"), "UTC", or the
/// offset ("UTC-07:00").
pub fn time_zone_label(name: Option<&str>, offset_minutes: i32) -> String {
    if let Some(tz) = name {
        if matches!(tz, "UTC" | "Etc/UTC" | "GMT" | "Etc/GMT" | "Universal" | "Zulu") {
            return "UTC".to_string();
        }
        if tz.contains('/') {
            return tz.rsplit('/').next().unwrap_or(tz).replace('_', " ");
        }
    }
    if offset_minutes == 0 {
        "UTC".to_string()
    } else {
        let sign = if offset_minutes < 0 { '-' } else { '+' };
        let minutes = offset_minutes.abs();
        format!("UTC{sign}{:02}:{:02}", minutes / 60, minutes % 60)
    }
}

/// The Listening page's data: one person's (`user`) or everyone's, for `days`, with the people
/// grid when `with_people`. Reads the people's files: blocking.
pub fn page(log: &ListeningLog, days: i64, user: Option<&str>, with_people: bool) -> Value {
    let (name, offset) = time_zone();
    let mut page = page_in(log, days, user, with_people, &chrono::Local);
    page["timeZone"] = json!(time_zone_label(name.as_deref(), offset));
    page["timeZoneName"] = json!(name);
    page["utcOffsetMinutes"] = json!(offset);
    page
}

/// [`page`] in a given time zone, for tests.
pub fn page_in<Tz: chrono::TimeZone>(
    log: &ListeningLog,
    days: i64,
    user: Option<&str>,
    with_people: bool,
    tz: &Tz,
) -> Value {
    let now = log.now();
    let (name, rollup) = match user.map(str::trim).filter(|u| !u.is_empty()) {
        Some(user) => {
            let name = log.name_of(user);
            let plays = log.plays_of(user);
            let rows: Vec<(&str, &Play)> = plays.iter().map(|p| (name.as_str(), p)).collect();
            (Some(name.clone()), rollups::rollup(&rows, now, days, tz))
        }
        None => {
            let names: std::collections::HashMap<String, String> = log.people().into_iter().collect();
            let all = log.all_plays();
            let rows: Vec<(&str, &Play)> = all
                .iter()
                .flat_map(|(key, plays)| {
                    let name = names.get(key).map_or(key.as_str(), String::as_str);
                    plays.iter().map(move |p| (name, p))
                })
                .collect();
            (None, rollups::rollup(&rows, now, days, tz))
        }
    };
    let people: Option<Vec<rollups::Person>> = with_people.then(|| {
        let mut people: Vec<rollups::Person> = log
            .people()
            .into_iter()
            .map(|(key, name)| rollups::person(&name, &log.plays_of(&key), now, days, tz))
            .collect();
        people.sort_by(|a, b| b.hours.total_cmp(&a.hours).then(a.name.cmp(&b.name)));
        people
    });
    json!({
        "range": days,
        "user": name,
        "people": people,
        "recording": log.recording(),
        "rollup": rollup,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_zone_is_named_by_its_city_or_its_offset() {
        assert_eq!(time_zone_label(Some("America/Los_Angeles"), -420), "Los Angeles");
        assert_eq!(time_zone_label(Some("Etc/UTC"), 0), "UTC");
        assert_eq!(time_zone_label(None, 0), "UTC");
        assert_eq!(time_zone_label(None, -420), "UTC-07:00");
        assert_eq!(time_zone_label(Some("EST5EDT"), 330), "UTC+05:30");
    }
}
