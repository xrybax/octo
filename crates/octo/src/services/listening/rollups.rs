//! What the Listening page shows, worked out from plays: a range of the last 7, 30 or 90 days,
//! or a year when plays are kept that long (today and the days before it, in the server's time
//! zone), the range before it for the change, and the plays' days, hours, songs, artists,
//! genres, sessions and apps.
//!
//! Pure functions over the plays and a clock, so every number is tested on fixed rows.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, TimeZone, Timelike, Utc};
use serde::Serialize;

use super::genres::genre_key;
use super::listening_log::{Play, UNKNOWN_APP, UNKNOWN_PLATFORM, platform_of};

/// The ranges the page always offers, in days.
pub const RANGES: [i64; 3] = [7, 30, 90];
/// The year range, offered only when plays are kept that long.
pub const YEAR: i64 = 365;
/// The range shown when none is asked for.
pub const DEFAULT_RANGE: i64 = 30;
/// Plays closer than this (from one's end to the next one's start) are one session.
pub const SESSION_GAP: TimeDelta = TimeDelta::minutes(20);
/// How many artists, songs and genres are listed.
pub const TOP: usize = 6;
pub const TOP_GENRES: usize = 7;
pub const HEARTED_SHOWN: usize = 3;
/// The share of the range before that must be on record for a change to be shown.
pub const ON_RECORD_FOR_CHANGE: f64 = 0.75;

/// The ranges offered with plays kept for `keep_days`: the year only when a year is kept.
pub fn ranges(keep_days: i64) -> Vec<i64> {
    let mut ranges = RANGES.to_vec();
    if keep_days >= YEAR {
        ranges.push(YEAR);
    }
    ranges
}

/// The range asked for: one of [`ranges`] ("30" and "30d" alike); anything else is 30.
pub fn range_of(text: Option<&str>, keep_days: i64) -> i64 {
    let text = text.unwrap_or("").trim().trim_end_matches(['d', 'D']);
    text.parse::<i64>()
        .ok()
        .filter(|days| ranges(keep_days).contains(days))
        .unwrap_or(DEFAULT_RANGE)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rollup {
    pub days: i64,
    pub hours: f64,
    pub plays: usize,
    pub unique_songs: usize,
    pub artists: usize,
    /// The share of plays that were library songs, 0 to 1.
    pub library_share: f64,
    pub hours_before: f64,
    pub change: Change,
    pub first_play: Option<DateTime<Utc>>,
    pub per_day: Vec<DayHours>,
    /// Minutes played in each hour of the week, Monday first, midnight first.
    pub grid: Vec<Vec<f64>>,
    pub top_artists: Vec<TopArtist>,
    pub top_songs: Vec<TopSong>,
    pub genres: Vec<GenreHours>,
    pub untagged_hours: f64,
    pub longest_session: Option<Session>,
    pub best_streak: usize,
    pub current_streak: usize,
    pub hearted: Vec<Hearted>,
    pub apps: Vec<AppShare>,
}

/// The change against the range before, or why there is none.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// "change" with a percent, or "new" with the day listening went on record.
    pub kind: &'static str,
    pub percent: Option<i64>,
    /// The first day on record, `YYYY-MM-DD`.
    pub since: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayHours {
    pub day: String,
    pub hours: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopArtist {
    pub name: String,
    pub plays: usize,
    pub minutes: f64,
    /// Their most played song, whose cover stands for them.
    pub song_id: String,
    pub cover_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopSong {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub plays: usize,
    pub minutes: f64,
    pub local: bool,
    pub song_id: String,
    pub cover_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenreHours {
    pub name: String,
    pub hours: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub minutes: i64,
    pub start: DateTime<Utc>,
    pub plays: usize,
    /// Whose session it was, on a page of more than one person.
    pub who: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hearted {
    pub title: String,
    pub artist: String,
    pub local: bool,
    pub song_id: String,
    pub cover_id: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppShare {
    pub name: String,
    pub platform: String,
    pub plays: usize,
    pub share: f64,
}

/// One card of the people grid.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub name: String,
    pub hours: f64,
    pub plays: usize,
    pub top_genre: Option<String>,
    /// Hours on each day of the range, oldest first.
    pub per_day: Vec<f64>,
}

/// The days of a range ending today: its first day, its start and the start of the range
/// before it.
struct Window {
    first_day: NaiveDate,
    today: NaiveDate,
    start: DateTime<Utc>,
    before: DateTime<Utc>,
    now: DateTime<Utc>,
}

fn window<Tz: TimeZone>(now: DateTime<Utc>, days: i64, tz: &Tz) -> Window {
    let today = now.with_timezone(tz).date_naive();
    let first_day = today - TimeDelta::days(days - 1);
    Window {
        first_day,
        today,
        start: midnight(tz, first_day),
        before: midnight(tz, first_day - TimeDelta::days(days)),
        now,
    }
}

/// The start of a local day, as an instant.
fn midnight<Tz: TimeZone>(tz: &Tz, day: NaiveDate) -> DateTime<Utc> {
    let naive = day.and_hms_opt(0, 0, 0).expect("midnight exists");
    tz.from_local_datetime(&naive)
        .earliest()
        .or_else(|| tz.from_local_datetime(&(naive + TimeDelta::hours(1))).earliest())
        .map_or_else(|| naive.and_utc(), |at| at.with_timezone(&Utc))
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

fn hours_of<'a>(plays: impl Iterator<Item = &'a Play>) -> f64 {
    plays.map(|p| p.seconds.max(0) as f64).sum::<f64>() / 3600.0
}

/// How one song is told apart from another: its artist and title, ignoring case, so the
/// library copy and the outside copy of a song are one song.
fn song_key(play: &Play) -> String {
    format!(
        "{}\u{1f}{}",
        play.artist.trim().to_lowercase(),
        play.title.trim().to_lowercase()
    )
}

fn artist_key(play: &Play) -> String {
    play.artist.trim().to_lowercase()
}

/// The app a play is counted under: Octo's apps by platform ("Octo for Android"), any other
/// app by name without its version ("Symfonium 12.3.0" is Symfonium).
pub fn app_name(label: &str, platform: &str) -> String {
    let label = label.trim();
    if label.is_empty() {
        return UNKNOWN_APP.to_string();
    }
    let lower = label.to_lowercase();
    if lower == "octo" || lower.starts_with("octo ") {
        return match platform_name(platform) {
            Some(name) => format!("Octo for {name}"),
            None => "Octo".to_string(),
        };
    }
    match label.rsplit_once(' ') {
        Some((name, version)) if version.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
            name.to_string()
        }
        _ => label.to_string(),
    }
}

fn platform_name(platform: &str) -> Option<&'static str> {
    match platform {
        "android" => Some("Android"),
        "iphone" => Some("iPhone"),
        "windows" => Some("Windows"),
        "macos" => Some("macOS"),
        "linux" => Some("Linux"),
        _ => None,
    }
}

/// The page for some plays: `rows` holds every play on record of the people shown, each with
/// whose it is.
pub fn rollup<Tz: TimeZone>(rows: &[(&str, &Play)], now: DateTime<Utc>, days: i64, tz: &Tz) -> Rollup {
    let w = window(now, days, tz);
    let local_day = |at: DateTime<Utc>| at.with_timezone(tz).date_naive();
    let in_range: Vec<(&str, &Play)> = rows
        .iter()
        .copied()
        .filter(|(_, p)| p.played_at >= w.start && p.played_at <= w.now)
        .collect();
    let plays: Vec<&Play> = in_range.iter().map(|(_, p)| *p).collect();
    let before: Vec<&Play> = rows
        .iter()
        .map(|(_, p)| *p)
        .filter(|p| p.played_at >= w.before && p.played_at < w.start)
        .collect();

    let hours = hours_of(plays.iter().copied());
    let hours_before = hours_of(before.iter().copied());
    let first_play = rows.iter().map(|(_, p)| p.played_at).min();
    let change = change_of(first_play, &w, days, hours, hours_before, tz);

    // Hours per day, and minutes per hour of the week.
    let mut by_day: HashMap<NaiveDate, f64> = HashMap::new();
    let mut grid = vec![vec![0.0f64; 24]; 7];
    for play in &plays {
        let local = play.played_at.with_timezone(tz);
        *by_day.entry(local.date_naive()).or_default() += play.seconds.max(0) as f64 / 3600.0;
        grid[local.weekday().num_days_from_monday() as usize][local.hour() as usize] +=
            play.seconds.max(0) as f64 / 60.0;
    }
    let per_day: Vec<DayHours> = (0..days)
        .map(|offset| {
            let day = w.first_day + TimeDelta::days(offset);
            DayHours {
                day: day.format("%Y-%m-%d").to_string(),
                hours: round(by_day.get(&day).copied().unwrap_or(0.0), 2),
            }
        })
        .collect();
    let grid = grid
        .into_iter()
        .map(|row| row.into_iter().map(|m| round(m, 1)).collect())
        .collect();

    let unique_songs = plays.iter().map(|p| song_key(p)).collect::<HashSet<_>>().len();
    let artists = plays.iter().map(|p| artist_key(p)).collect::<HashSet<_>>().len();
    let library_share = if plays.is_empty() {
        0.0
    } else {
        round(
            plays.iter().filter(|p| p.is_local).count() as f64 / plays.len() as f64,
            3,
        )
    };

    let mut day_set: HashSet<NaiveDate> = HashSet::new();
    for play in &plays {
        day_set.insert(local_day(play.played_at));
    }
    let (best_streak, current_streak) = streaks(&day_set, w.today);
    let (genre_list, untagged) = genres(&plays);

    Rollup {
        days,
        hours: round(hours, 2),
        plays: plays.len(),
        unique_songs,
        artists,
        library_share,
        hours_before: round(hours_before, 2),
        change,
        first_play,
        per_day,
        grid,
        top_artists: top_artists(&plays),
        top_songs: top_songs(&plays),
        genres: genre_list,
        untagged_hours: round(untagged, 2),
        longest_session: longest_session(&in_range),
        best_streak,
        current_streak,
        hearted: hearted(&plays),
        apps: apps(&plays),
    }
}

/// A change is shown only when the range before was on record for most of its length; else
/// the day listening went on record.
fn change_of<Tz: TimeZone>(
    first_play: Option<DateTime<Utc>>,
    w: &Window,
    days: i64,
    hours: f64,
    hours_before: f64,
    tz: &Tz,
) -> Change {
    let since = first_play.map(|at| at.with_timezone(tz).date_naive().format("%Y-%m-%d").to_string());
    let allowed_gap = TimeDelta::seconds(((1.0 - ON_RECORD_FOR_CHANGE) * (days * 86_400) as f64) as i64);
    let on_record = first_play.is_some_and(|first| first <= w.before + allowed_gap);
    if on_record && hours_before > 0.0 {
        Change {
            kind: "change",
            percent: Some(((hours - hours_before) / hours_before * 100.0).round() as i64),
            since,
        }
    } else {
        Change {
            kind: "new",
            percent: None,
            since,
        }
    }
}

/// The longest run of days in a row with plays, and the run that reaches today (or yesterday,
/// when nothing has played yet today).
fn streaks(days: &HashSet<NaiveDate>, today: NaiveDate) -> (usize, usize) {
    let mut best = 0;
    for day in days {
        if days.contains(&(*day - TimeDelta::days(1))) {
            continue;
        }
        let mut run = 1;
        while days.contains(&(*day + TimeDelta::days(run))) {
            run += 1;
        }
        best = best.max(run as usize);
    }
    let mut end = today;
    if !days.contains(&end) {
        end -= TimeDelta::days(1);
    }
    let mut current = 0;
    while days.contains(&(end - TimeDelta::days(current))) {
        current += 1;
    }
    (best, current as usize)
}

fn top_artists(plays: &[&Play]) -> Vec<TopArtist> {
    struct Tally<'a> {
        name: &'a str,
        plays: usize,
        seconds: i64,
        songs: HashMap<String, (usize, &'a Play)>,
        first: usize,
    }
    let mut tallies: HashMap<String, Tally> = HashMap::new();
    for (index, play) in plays.iter().enumerate() {
        let tally = tallies.entry(artist_key(play)).or_insert_with(|| Tally {
            name: play.artist.trim(),
            plays: 0,
            seconds: 0,
            songs: HashMap::new(),
            first: index,
        });
        tally.plays += 1;
        tally.seconds += play.seconds.max(0);
        let song = tally.songs.entry(song_key(play)).or_insert((0, play));
        song.0 += 1;
        song.1 = play;
    }
    let mut list: Vec<Tally> = tallies.into_values().collect();
    list.sort_by(|a, b| {
        b.plays
            .cmp(&a.plays)
            .then(b.seconds.cmp(&a.seconds))
            .then(a.first.cmp(&b.first))
    });
    list.into_iter()
        .take(TOP)
        .map(|tally| {
            let cover = tally
                .songs
                .values()
                .max_by(|a, b| a.0.cmp(&b.0).then(a.1.played_at.cmp(&b.1.played_at)))
                .map(|(_, play)| *play);
            TopArtist {
                name: tally.name.to_string(),
                plays: tally.plays,
                minutes: round(tally.seconds as f64 / 60.0, 1),
                song_id: cover.map(|p| p.song_id.clone()).unwrap_or_default(),
                cover_id: cover.map(|p| p.cover_id.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

fn top_songs(plays: &[&Play]) -> Vec<TopSong> {
    let mut tallies: HashMap<String, (usize, i64, &Play, usize)> = HashMap::new();
    for (index, play) in plays.iter().enumerate() {
        let tally = tallies.entry(song_key(play)).or_insert((0, 0, play, index));
        tally.0 += 1;
        tally.1 += play.seconds.max(0);
        // The newest play's ids and cover stand for the song.
        tally.2 = play;
    }
    let mut list: Vec<(usize, i64, &Play, usize)> = tallies.into_values().collect();
    list.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.3.cmp(&b.3)));
    list.into_iter()
        .take(TOP)
        .map(|(count, seconds, play, _)| TopSong {
            title: play.title.clone(),
            artist: play.artist.clone(),
            album: play.album.clone(),
            plays: count,
            minutes: round(seconds as f64 / 60.0, 1),
            local: play.is_local,
            song_id: play.song_id.clone(),
            cover_id: play.cover_id.clone(),
        })
        .collect()
}

/// The genres by hours (the top seven, each under the spelling with the most hours), and the
/// hours of plays with no genre.
fn genres(plays: &[&Play]) -> (Vec<GenreHours>, f64) {
    let mut groups: HashMap<String, HashMap<String, i64>> = HashMap::new();
    let mut untagged = 0i64;
    for play in plays {
        match play.genre.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
            Some(genre) => {
                *groups
                    .entry(genre_key(genre))
                    .or_default()
                    .entry(genre.to_string())
                    .or_default() += play.seconds.max(0);
            }
            None => untagged += play.seconds.max(0),
        }
    }
    let mut list: Vec<(String, i64)> = groups
        .into_values()
        .map(|spellings| {
            let total = spellings.values().sum::<i64>();
            let name = spellings
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                .map(|(name, _)| name)
                .unwrap_or_default();
            (name, total)
        })
        .filter(|(_, seconds)| *seconds > 0)
        .collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let shown = list
        .into_iter()
        .take(TOP_GENRES)
        .map(|(name, seconds)| GenreHours {
            name,
            hours: round(seconds as f64 / 3600.0, 2),
        })
        .collect();
    (shown, untagged as f64 / 3600.0)
}

/// The longest stretch of one person's plays with no gap of 20 minutes or more.
fn longest_session(rows: &[(&str, &Play)]) -> Option<Session> {
    let mut by_person: HashMap<&str, Vec<&Play>> = HashMap::new();
    for (who, play) in rows {
        by_person.entry(who).or_default().push(play);
    }
    let several = by_person.len() > 1;
    let mut best: Option<Session> = None;
    for (who, mut plays) in by_person {
        plays.sort_by_key(|p| p.played_at);
        let mut start = 0;
        while start < plays.len() {
            let mut end_at = plays[start].ended_at();
            let mut next = start + 1;
            while next < plays.len() && plays[next].played_at - end_at < SESSION_GAP {
                end_at = end_at.max(plays[next].ended_at());
                next += 1;
            }
            let minutes = (end_at - plays[start].played_at).num_minutes();
            let longer = best.as_ref().is_none_or(|b| {
                minutes > b.minutes || (minutes == b.minutes && plays[start].played_at > b.start)
            });
            if longer && minutes > 0 {
                best = Some(Session {
                    minutes,
                    start: plays[start].played_at,
                    plays: next - start,
                    who: several.then(|| who.to_string()),
                });
            }
            start = next;
        }
    }
    best
}

/// Songs hearted while they played, newest first, each once.
fn hearted(plays: &[&Play]) -> Vec<Hearted> {
    let mut seen = HashSet::new();
    let mut list = Vec::new();
    for play in plays.iter().rev().filter(|p| p.hearted) {
        if !seen.insert(song_key(play)) {
            continue;
        }
        list.push(Hearted {
            title: play.title.clone(),
            artist: play.artist.clone(),
            local: play.is_local,
            song_id: play.song_id.clone(),
            cover_id: play.cover_id.clone(),
            at: play.played_at,
        });
        if list.len() == HEARTED_SHOWN {
            break;
        }
    }
    list
}

/// Plays by app, the most used first. A play written with no platform takes the one its app's
/// name says: the app's label is its `c`, and an Octo app's label keeps its User-Agent's
/// parentheses.
fn apps(plays: &[&Play]) -> Vec<AppShare> {
    let mut tallies: HashMap<(String, String), usize> = HashMap::new();
    for play in plays {
        let platform = if play.platform.is_empty() || play.platform == UNKNOWN_PLATFORM {
            platform_of(&play.app, &play.app).to_string()
        } else {
            play.platform.clone()
        };
        *tallies
            .entry((app_name(&play.app, &platform), platform))
            .or_default() += 1;
    }
    let total = plays.len().max(1) as f64;
    let mut list: Vec<AppShare> = tallies
        .into_iter()
        .map(|((name, platform), count)| AppShare {
            name,
            platform,
            plays: count,
            share: round(count as f64 / total, 3),
        })
        .collect();
    list.sort_by(|a, b| b.plays.cmp(&a.plays).then(a.name.cmp(&b.name)));
    list
}

/// One person's card: hours in the range, their top genre and hours per day.
pub fn person<Tz: TimeZone>(name: &str, rows: &[Play], now: DateTime<Utc>, days: i64, tz: &Tz) -> Person {
    let w = window(now, days, tz);
    let plays: Vec<&Play> = rows
        .iter()
        .filter(|p| p.played_at >= w.start && p.played_at <= w.now)
        .collect();
    let mut by_day: HashMap<NaiveDate, f64> = HashMap::new();
    for play in &plays {
        *by_day
            .entry(play.played_at.with_timezone(tz).date_naive())
            .or_default() += play.seconds.max(0) as f64 / 3600.0;
    }
    Person {
        name: name.to_string(),
        hours: round(hours_of(plays.iter().copied()), 2),
        plays: plays.len(),
        top_genre: genres(&plays).0.into_iter().next().map(|g| g.name),
        per_day: (0..days)
            .map(|offset| {
                round(
                    by_day
                        .get(&(w.first_day + TimeDelta::days(offset)))
                        .copied()
                        .unwrap_or(0.0),
                    2,
                )
            })
            .collect(),
    }
}

#[cfg(test)]
#[path = "rollups_tests.rs"]
mod tests;
