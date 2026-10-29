//! The listening log: every completed play of each person, for their Listening page.
//!
//! One file of JSON lines per person under `<config>/listening/` (`<user>.jsonl`): a play per
//! line, and a heart that came after its play was written as a line of its own
//! (`{"heart":...}`) that reading folds into the play. `listening.json` beside them keeps the
//! names as first seen, whether the old radio history was copied in, and the genre answers
//! already found for artists and albums.
//!
//! A play is timed by when the next one starts. Each person's current play waits in memory
//! from its now-playing (or its scrobble) until that person's next play starts, or until its
//! length and ten more minutes pass with nothing new; only then is it written, and only when
//! an app reported it as played. A heart while it waits marks it.
//!
//! Nothing here touches the disk under the lock: the files are written in order by one
//! writer thread, and a person's file is read only the first time their plays are needed.
//! Plays are kept for `Listening:KeepDays`; the daily prune also folds the heart lines in.
//! Nothing is written while `Listening:Enabled` is off.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use octo_core::common::Clock;
use octo_core::last_fm::last_fm_radio_state_store::user_key;
use octo_core::models::radio::LastFmRadioPlay;
use octo_core::settings::SettingsStore;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::genres::AlbumLookup;
use crate::services::state_file;

/// The app a play is filed under when nothing says which app played it.
pub const UNKNOWN_APP: &str = "Unknown app";
/// The platform of a play whose app did not say.
pub const UNKNOWN_PLATFORM: &str = "unknown";
/// The meta file beside the people's logs.
pub const META_FILE: &str = "listening.json";

/// The longest a play with no known length is counted for.
pub const UNTIMED_CAP: TimeDelta = TimeDelta::minutes(15);
/// How long past its length a play waits for the next one before it is written as played whole.
pub const SETTLE_AFTER: TimeDelta = TimeDelta::minutes(10);
/// A heart this long before a play started still counts for it.
const HEART_LEAD: TimeDelta = TimeDelta::seconds(30);
/// A heart this long after a play ended still counts for it.
const HEART_TRAIL: TimeDelta = TimeDelta::minutes(2);
/// How long a heart waits for its play to be written.
const HEART_WAIT: TimeDelta = TimeDelta::hours(2);
/// The most hearts kept waiting for one person.
const HEARTS_PER_PERSON: usize = 50;

/// One completed play.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Play {
    pub song_id: String,
    /// The id the song's cover is asked for by: the library's cover id, or the song's own id.
    pub cover_id: String,
    pub artist: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    pub title: String,
    pub album: String,
    /// The genre, from the song, its album or its artist; None when none of them has one.
    pub genre: Option<String>,
    /// How long it played, in seconds.
    pub seconds: i64,
    pub is_local: bool,
    /// The song was hearted while it played.
    pub hearted: bool,
    /// The app, as the Family page names a device's app ("Octo 1.9.0 (Android)").
    pub app: String,
    /// android, iphone, windows, macos, linux or unknown.
    pub platform: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// When the play started.
    pub played_at: DateTime<Utc>,
    /// scrobble, internet-radio or backfill.
    pub source: String,
}

impl Default for Play {
    fn default() -> Self {
        Play {
            song_id: String::new(),
            cover_id: String::new(),
            artist: String::new(),
            album_artist: None,
            title: String::new(),
            album: String::new(),
            genre: None,
            seconds: 0,
            is_local: false,
            hearted: false,
            app: UNKNOWN_APP.to_string(),
            platform: UNKNOWN_PLATFORM.to_string(),
            device_id: None,
            played_at: DateTime::<Utc>::UNIX_EPOCH,
            source: "scrobble".to_string(),
        }
    }
}

impl Play {
    /// When the play ended.
    pub fn ended_at(&self) -> DateTime<Utc> {
        self.played_at + TimeDelta::seconds(self.seconds.max(0))
    }

    /// Whether a heart at `at` came while this play was on.
    fn heart_counts(&self, at: DateTime<Utc>) -> bool {
        at >= self.played_at - HEART_LEAD && at <= self.ended_at() + HEART_TRAIL
    }
}

/// A heart that came after its play was written, as its own line of the person's log.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HeartLine {
    heart: String,
    at: DateTime<Utc>,
}

/// The platform a play came from: the app's name when it says (an iPhone app's Mac build names
/// the Mac in its agent), then the User-Agent's own words in its parentheses
/// (`Octo/1.9.0 (Android)` is android; Android is checked before Linux, since Android's own agents
/// name both).
pub fn platform_of(user_agent: &str, client: &str) -> &'static str {
    let app = client.trim().to_ascii_lowercase();
    let lower = user_agent.to_ascii_lowercase();
    let on_a_mac = lower.contains("macintosh") || lower.contains("mac os") || lower.contains("macos");
    if IPHONE_APPS.iter().any(|name| app.starts_with(name)) {
        return if on_a_mac { "macos" } else { "iphone" };
    }
    if ANDROID_APPS.iter().any(|name| app.starts_with(name)) {
        return "android";
    }
    let inside = user_agent
        .split_once('(')
        .map(|(_, rest)| rest.split(')').next().unwrap_or(rest))
        .unwrap_or("")
        .to_ascii_lowercase();
    if inside.contains("android") {
        "android"
    } else if inside.contains("iphone") || inside.contains("ipad") || inside.contains("ios") {
        "iphone"
    } else if inside.contains("windows") {
        "windows"
    } else if inside.contains("mac") {
        "macos"
    } else if inside.contains("linux") {
        "linux"
    } else {
        UNKNOWN_PLATFORM
    }
}

/// Players that only run on iPhone and iPad, by the name they send as `c`. play:Sub sends
/// `playSub_` and the device's model.
const IPHONE_APPS: [&str; 5] = ["amperfy", "playsub", "play:sub", "isub", "arpeggi"];
/// Players that only run on Android, by the name they send as `c`.
const ANDROID_APPS: [&str; 6] = ["dsub", "ultrasonic", "symfonium", "tempo", "subtracks", "tempus"];

/// What a play is written from: the song as its scrobble described it, and where its genre
/// may be found.
#[derive(Clone)]
pub struct Draft {
    pub play: Play,
    /// The song's own genres, first one first.
    pub genres: Vec<String>,
    /// The library album to ask when the song has none.
    pub album: Option<AlbumLookup>,
}

/// A play that has ended, to be written once its genre is known.
pub struct Settled {
    pub user: String,
    pub play: Play,
    pub genres: Vec<String>,
    pub album: Option<AlbumLookup>,
    /// The person's clear count when it ended: a play from before a clear is never written.
    generation: u64,
}

/// A person's current play.
struct Pending {
    /// The person, as the request named them.
    user: String,
    song_id: String,
    start: DateTime<Utc>,
    /// The song's length in seconds, once a scrobble said it.
    length: Option<i64>,
    /// The song as the completed scrobble described it; None while no app has reported the
    /// play as played, and then it is never written.
    draft: Option<Draft>,
    hearted: bool,
}

impl Pending {
    /// How long the play can last: its length, else the cap for a play of no known length.
    fn limit(&self) -> TimeDelta {
        self.length.map_or(UNTIMED_CAP, TimeDelta::seconds)
    }

    /// Whether it would have ended by `at`.
    fn over_by(&self, at: DateTime<Utc>) -> bool {
        at - self.start >= self.limit()
    }
}

/// What is kept beside the logs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct Meta {
    version: i32,
    /// The radio history was copied in once.
    backfilled: bool,
    /// Each person's name as first seen, by key.
    names: HashMap<String, String>,
    /// The genre found for an artist (by key), or "" when it has none.
    artist_genres: HashMap<String, String>,
    /// The genre of a library album (by id), or "" when it has none.
    album_genres: HashMap<String, String>,
}

/// One write for the writer thread.
enum Op {
    Append(PathBuf, String),
    Rewrite(PathBuf, String),
    Remove(PathBuf),
    Flush(mpsc::SyncSender<()>),
}

#[derive(Default)]
struct Inner {
    meta: Meta,
    /// The plays of each person read so far, by key, oldest first.
    users: HashMap<String, Vec<Play>>,
    /// Each person's current play.
    pending: HashMap<String, Pending>,
    /// Hearts no play has taken yet, by person: song id and when.
    hearts: HashMap<String, Vec<(String, DateTime<Utc>)>>,
    /// How often each person's plays were cleared.
    generations: HashMap<String, u64>,
    /// People whose file holds heart lines, for the prune to fold in.
    untidy: HashSet<String>,
}

pub struct ListeningLog {
    dir: PathBuf,
    settings: Arc<SettingsStore>,
    clock: Clock,
    inner: Mutex<Inner>,
    writer: mpsc::Sender<Op>,
}

impl ListeningLog {
    pub fn new(dir: impl Into<PathBuf>, settings: Arc<SettingsStore>) -> Self {
        Self::with_clock(dir, settings, Clock::system())
    }

    /// The log on another clock, for tests.
    pub fn with_clock(dir: impl Into<PathBuf>, settings: Arc<SettingsStore>, clock: Clock) -> Self {
        let dir = dir.into();
        let mut inner = Inner::default();
        if let Ok(Some(text)) = state_file::read_text(&dir.join(META_FILE)) {
            match serde_json::from_str::<Meta>(&text) {
                Ok(meta) => inner.meta = meta,
                Err(e) => warn!("{META_FILE} could not be read, starting over: {e}"),
            }
        }
        let (writer, writes) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("listening-writer".into())
            .spawn(move || write_in_order(writes));
        if let Err(e) = spawned {
            warn!("The listening writer could not start: {e}");
        }
        ListeningLog {
            dir,
            settings,
            clock,
            inner: Mutex::new(inner),
            writer,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// Whether plays are being written down.
    pub fn recording(&self) -> bool {
        self.settings.current().listening.enabled
    }

    fn keep(&self) -> TimeDelta {
        TimeDelta::days(self.settings.current().listening.effective_keep_days())
    }

    fn send(&self, op: Op) {
        if self.writer.send(op).is_err() {
            warn!("The listening writer has stopped; a write was lost");
        }
    }

    /// Waits until every write asked for so far is on disk. Blocking.
    pub fn flush(&self) {
        let (done, wait) = mpsc::sync_channel(1);
        self.send(Op::Flush(done));
        let _ = wait.recv_timeout(Duration::from_secs(30));
    }

    // ---- plays as they happen -------------------------------------------------------------

    /// `user` started playing `song_id` at `at`. The play before it ends here.
    pub fn now_playing(&self, user: &str, song_id: &str, at: DateTime<Utc>) -> Vec<Settled> {
        let key = user_key(user);
        if key.is_empty() || song_id.is_empty() || !self.recording() {
            return Vec::new();
        }
        let at = at.min(self.now());
        let mut inner = self.inner.lock();
        // The app saying again that the same song is playing is the same play.
        if inner
            .pending
            .get(&key)
            .is_some_and(|p| p.song_id == song_id && !p.over_by(at))
        {
            return Vec::new();
        }
        let generation = generation(&inner, &key);
        let mut settled = Vec::new();
        if let Some(previous) = inner.pending.remove(&key) {
            settled.extend(settle(previous, at, generation));
        }
        inner.pending.insert(
            key,
            Pending {
                user: user.trim().to_string(),
                song_id: song_id.to_string(),
                start: at,
                length: None,
                draft: None,
                hearted: false,
            },
        );
        settled
    }

    /// An app reported `user`'s play of the draft's song as played, at `at`. `length` is the
    /// song's, in seconds; `sent_start` is the time the app sent with it, which is when the
    /// play started. The play is written once it ends.
    pub fn completed(
        &self,
        user: &str,
        draft: Draft,
        length: Option<i64>,
        sent_start: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> Vec<Settled> {
        let key = user_key(user);
        let song_id = draft.play.song_id.clone();
        if key.is_empty() || song_id.is_empty() || !self.recording() {
            return Vec::new();
        }
        let at = at.min(self.now());
        let length = length.filter(|l| *l > 0);
        let mut inner = self.inner.lock();
        if let Some(current) = inner.pending.get_mut(&key)
            && current.song_id == song_id
            && (current.draft.is_none() || !current.over_by(at))
        {
            current.length = length.or(current.length);
            current.draft = Some(draft);
            return Vec::new();
        }
        // A play with no now-playing before it: it started when the app says, else as long ago
        // as the song lasts. A time in the future is taken as now.
        let start = sent_start
            .map(|sent| sent.min(at))
            .unwrap_or_else(|| at - TimeDelta::seconds(length.unwrap_or(0)));
        let generation = generation(&inner, &key);
        let mut settled = Vec::new();
        if let Some(previous) = inner.pending.remove(&key) {
            let end = start.max(previous.start);
            settled.extend(settle(previous, end, generation));
        }
        inner.pending.insert(
            key,
            Pending {
                user: user.trim().to_string(),
                song_id,
                start,
                length,
                draft: Some(draft),
                hearted: false,
            },
        );
        settled
    }

    /// A play that is over as it is told (the radio stream's songs): to be written as it is.
    pub fn finished(&self, user: &str, play: Play, genres: Vec<String>) -> Option<Settled> {
        let key = user_key(user);
        if key.is_empty() || !self.recording() {
            return None;
        }
        let generation = generation(&self.inner.lock(), &key);
        Some(Settled {
            user: user.to_string(),
            play,
            genres,
            album: None,
            generation,
        })
    }

    /// The plays that have waited their length and ten more minutes with nothing new after
    /// them: each played whole.
    pub fn settle_expired(&self) -> Vec<Settled> {
        let now = self.now();
        let mut inner = self.inner.lock();
        let due: Vec<String> = inner
            .pending
            .iter()
            .filter(|(_, p)| now - p.start >= p.limit() + SETTLE_AFTER)
            .map(|(key, _)| key.clone())
            .collect();
        let mut settled = Vec::new();
        for key in due {
            let generation = generation(&inner, &key);
            if let Some(play) = inner.pending.remove(&key) {
                let end = play.start + play.limit();
                settled.extend(settle(play, end, generation));
            }
        }
        settled
    }

    /// Every current play, ended now (on shutdown): each as long as it has played so far.
    pub fn settle_all(&self) -> Vec<Settled> {
        let now = self.now();
        let mut inner = self.inner.lock();
        let keys: Vec<String> = inner.pending.keys().cloned().collect();
        let mut settled = Vec::new();
        for key in keys {
            let generation = generation(&inner, &key);
            if let Some(play) = inner.pending.remove(&key) {
                settled.extend(settle(play, now, generation));
            }
        }
        settled
    }

    /// `user` hearted these songs at `at`: the current play of one is marked, and so is a play
    /// of one written a moment ago; otherwise the heart waits for its play to be written.
    pub fn heart(&self, user: &str, song_ids: &[String], at: DateTime<Utc>) {
        let key = user_key(user);
        if key.is_empty() || song_ids.is_empty() || !self.recording() {
            return;
        }
        let mut lines = Vec::new();
        {
            let mut inner = self.inner.lock();
            for id in song_ids.iter().filter(|id| !id.is_empty()) {
                if let Some(current) = inner.pending.get_mut(&key).filter(|p| &p.song_id == id) {
                    current.hearted = true;
                    continue;
                }
                let written = inner.users.get_mut(&key).and_then(|rows| {
                    rows.iter_mut()
                        .rev()
                        .take_while(|row| row.ended_at() + HEART_TRAIL >= at)
                        .find(|row| &row.song_id == id && row.heart_counts(at))
                });
                match written {
                    Some(row) => {
                        if !row.hearted {
                            row.hearted = true;
                            lines.push(HeartLine {
                                heart: id.clone(),
                                at,
                            });
                        }
                    }
                    None => {
                        let waiting = inner.hearts.entry(key.clone()).or_default();
                        waiting.retain(|(_, when)| at - *when <= HEART_WAIT);
                        waiting.push((id.clone(), at));
                        if waiting.len() > HEARTS_PER_PERSON {
                            let extra = waiting.len() - HEARTS_PER_PERSON;
                            waiting.drain(..extra);
                        }
                    }
                }
            }
            if !lines.is_empty() {
                inner.untidy.insert(key.clone());
            }
        }
        for line in lines {
            if let Ok(text) = serde_json::to_string(&line) {
                self.send(Op::Append(self.user_path(&key), text));
            }
        }
    }

    /// Writes a play that has ended. False when recording is off, the play names no song, or
    /// the person's plays were cleared after it ended. May read the person's file: blocking.
    pub fn write(&self, settled: Settled, genre: Option<String>) -> bool {
        let Settled {
            user,
            mut play,
            generation: then,
            ..
        } = settled;
        let key = user_key(&user);
        if key.is_empty()
            || play.title.trim().is_empty()
            || play.artist.trim().is_empty()
            || !self.recording()
        {
            return false;
        }
        if genre.is_some() {
            play.genre = genre;
        }
        self.ensure_loaded(&key);
        let mut meta_text = None;
        let line = {
            let mut inner = self.inner.lock();
            if generation(&inner, &key) != then {
                return false;
            }
            if let Some(waiting) = inner.hearts.get_mut(&key) {
                let before = waiting.len();
                waiting.retain(|(id, at)| !(id == &play.song_id && play.heart_counts(*at)));
                if waiting.len() != before {
                    play.hearted = true;
                }
            }
            if !inner.meta.names.contains_key(&key) {
                inner.meta.names.insert(key.clone(), user.trim().to_string());
                meta_text = meta_json(&inner.meta);
            }
            let line = match serde_json::to_string(&play) {
                Ok(line) => line,
                Err(e) => {
                    warn!("A play of {} could not be written down: {e}", play.title);
                    return false;
                }
            };
            let rows = inner.users.entry(key.clone()).or_default();
            let at = rows.partition_point(|row| row.played_at <= play.played_at);
            rows.insert(at, play);
            line
        };
        if let Some(text) = meta_text {
            self.send(Op::Rewrite(self.dir.join(META_FILE), text));
        }
        self.send(Op::Append(self.user_path(&key), line));
        true
    }

    /// Writes a play at once, for a play already over: [`Self::finished`] then [`Self::write`].
    pub fn record(&self, user: &str, play: Play) -> bool {
        let genre = play.genre.clone();
        match self.finished(user, play, Vec::new()) {
            Some(settled) => self.write(settled, genre),
            None => false,
        }
    }

    // ---- reading --------------------------------------------------------------------------

    /// Everyone with plays on record, as (key, name), by name. Reads every file: blocking.
    pub fn people(&self) -> Vec<(String, String)> {
        self.load_everyone();
        let inner = self.inner.lock();
        let mut people: Vec<(String, String)> = inner
            .users
            .iter()
            .filter(|(_, rows)| !rows.is_empty())
            .map(|(key, _)| {
                let name = inner.meta.names.get(key).cloned().unwrap_or_else(|| key.clone());
                (key.clone(), name)
            })
            .collect();
        people.sort_by_key(|a| a.1.to_lowercase());
        people
    }

    /// The name a person is shown by.
    pub fn name_of(&self, user: &str) -> String {
        let key = user_key(user);
        let inner = self.inner.lock();
        inner
            .meta
            .names
            .get(&key)
            .cloned()
            .unwrap_or_else(|| user.trim().to_string())
    }

    /// A copy of one person's plays within the days kept, oldest first. Blocking.
    pub fn plays_of(&self, user: &str) -> Vec<Play> {
        let key = user_key(user);
        self.ensure_loaded(&key);
        let since = self.now() - self.keep();
        let inner = self.inner.lock();
        inner
            .users
            .get(&key)
            .map(|rows| {
                rows.iter()
                    .filter(|row| row.played_at >= since)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A copy of everyone's plays within the days kept, by key. Blocking.
    pub fn all_plays(&self) -> Vec<(String, Vec<Play>)> {
        self.load_everyone();
        let since = self.now() - self.keep();
        let inner = self.inner.lock();
        inner
            .users
            .iter()
            .map(|(key, rows)| {
                (
                    key.clone(),
                    rows.iter()
                        .filter(|row| row.played_at >= since)
                        .cloned()
                        .collect(),
                )
            })
            .collect()
    }

    // ---- upkeep ---------------------------------------------------------------------------

    /// Removes one person's plays, or everyone's, with their current play, waiting hearts and
    /// name; a play of theirs still being worked out is never written. Returns how many plays
    /// went. Blocking.
    pub fn clear(&self, user: Option<&str>) -> usize {
        let keys: Vec<String> = match user {
            Some(user) => vec![user_key(user)],
            None => {
                self.load_everyone();
                let inner = self.inner.lock();
                let mut keys: HashSet<String> = inner.users.keys().cloned().collect();
                keys.extend(inner.pending.keys().cloned());
                keys.extend(inner.meta.names.keys().cloned());
                keys.into_iter().collect()
            }
        };
        for key in &keys {
            self.ensure_loaded(key);
        }
        let mut removed = 0;
        let meta_text = {
            let mut inner = self.inner.lock();
            for key in &keys {
                // Kept as read and empty, so the file is never read back before it is gone.
                removed += inner
                    .users
                    .insert(key.clone(), Vec::new())
                    .map_or(0, |rows| rows.len());
                inner.pending.remove(key);
                inner.hearts.remove(key);
                inner.untidy.remove(key);
                inner.meta.names.remove(key);
                *inner.generations.entry(key.clone()).or_default() += 1;
            }
            meta_json(&inner.meta)
        };
        for key in &keys {
            self.send(Op::Remove(self.user_path(key)));
        }
        if let Some(text) = meta_text {
            self.send(Op::Rewrite(self.dir.join(META_FILE), text));
        }
        removed
    }

    /// Whether the radio history has been copied in.
    pub fn backfilled(&self) -> bool {
        self.inner.lock().meta.backfilled
    }

    /// Copies the radio's play history in, once ever, while recording is on: completed
    /// scrobbles and radio stream plays (never the radio's own starter seeds), each as played
    /// by an unknown app, as long as it is within the days kept. Returns how many plays were
    /// copied; 0 once it has run, and 0 without running while recording is off. Blocking.
    pub fn backfill(&self, history: Vec<(String, Vec<LastFmRadioPlay>)>) -> usize {
        if !self.recording() || self.backfilled() {
            return 0;
        }
        let since = self.now() - self.keep();
        for (user, _) in &history {
            self.ensure_loaded(&user_key(user));
        }
        let mut rewrites = Vec::new();
        let mut copied = 0;
        let meta_text = {
            let mut inner = self.inner.lock();
            if inner.meta.backfilled {
                return 0;
            }
            for (user, plays) in history {
                let key = user_key(&user);
                if key.is_empty() {
                    continue;
                }
                let rows = inner.users.entry(key.clone()).or_default();
                let mut added = 0;
                for play in plays {
                    if !matches!(play.source.as_str(), "scrobble" | "internet-radio")
                        || play.played_at_utc < since
                        || play.artist.trim().is_empty()
                        || play.title.trim().is_empty()
                    {
                        continue;
                    }
                    let seconds = i64::from(play.duration.unwrap_or(0).max(0));
                    // A scrobble is dated by the time the app sent, when the play started; the
                    // radio stream dates its songs when they ended.
                    let played_at = if play.source == "internet-radio" {
                        play.played_at_utc - TimeDelta::seconds(seconds)
                    } else {
                        play.played_at_utc
                    };
                    if rows.iter().any(|row| {
                        row.song_id == play.song_id
                            && (row.played_at - played_at).abs() <= TimeDelta::minutes(5)
                    }) {
                        continue;
                    }
                    rows.push(Play {
                        cover_id: play.song_id.clone(),
                        song_id: play.song_id,
                        artist: play.artist,
                        title: play.title,
                        album: play.album.unwrap_or_default(),
                        genre: play.genre.filter(|g| !g.trim().is_empty()),
                        seconds,
                        is_local: play.is_local,
                        played_at,
                        source: "backfill".to_string(),
                        ..Default::default()
                    });
                    added += 1;
                }
                if added == 0 {
                    continue;
                }
                rows.sort_by_key(|row| row.played_at);
                rewrites.push((key.clone(), lines_of(rows)));
                inner.untidy.remove(&key);
                inner
                    .meta
                    .names
                    .entry(key)
                    .or_insert_with(|| user.trim().to_string());
                copied += added;
            }
            inner.meta.backfilled = true;
            meta_json(&inner.meta)
        };
        for (key, text) in rewrites {
            self.send(Op::Rewrite(self.user_path(&key), text));
        }
        if let Some(text) = meta_text {
            self.send(Op::Rewrite(self.dir.join(META_FILE), text));
        }
        copied
    }

    /// Removes plays older than the days kept and folds the heart lines into their plays,
    /// file by file. Returns how many plays went. Blocking.
    pub fn prune(&self) -> usize {
        self.load_everyone();
        let since = self.now() - self.keep();
        let mut removed = 0;
        let mut writes = Vec::new();
        {
            let mut inner = self.inner.lock();
            let untidy = std::mem::take(&mut inner.untidy);
            for (key, rows) in &mut inner.users {
                let old = rows.partition_point(|row| row.played_at < since);
                if old == 0 && !untidy.contains(key) {
                    continue;
                }
                rows.drain(..old);
                removed += old;
                writes.push((key.clone(), (!rows.is_empty()).then(|| lines_of(rows))));
            }
        }
        for (key, text) in writes {
            match text {
                Some(text) => self.send(Op::Rewrite(self.user_path(&key), text)),
                None => self.send(Op::Remove(self.user_path(&key))),
            }
        }
        removed
    }

    /// The genre already found for an artist: Some(None) when it is known to have none.
    pub fn artist_genre(&self, artist: &str) -> Option<Option<String>> {
        let inner = self.inner.lock();
        inner
            .meta
            .artist_genres
            .get(&artist_key(artist))
            .map(|g| (!g.is_empty()).then(|| g.clone()))
    }

    pub fn remember_artist_genre(&self, artist: &str, genre: Option<&str>) {
        let key = artist_key(artist);
        if key.is_empty() {
            return;
        }
        let text = {
            let mut inner = self.inner.lock();
            inner
                .meta
                .artist_genres
                .insert(key, genre.unwrap_or("").to_string());
            meta_json(&inner.meta)
        };
        if let Some(text) = text {
            self.send(Op::Rewrite(self.dir.join(META_FILE), text));
        }
    }

    /// The genre already read for a library album: Some(None) when it has none.
    pub fn album_genre(&self, album_id: &str) -> Option<Option<String>> {
        let inner = self.inner.lock();
        inner
            .meta
            .album_genres
            .get(album_id)
            .map(|g| (!g.is_empty()).then(|| g.clone()))
    }

    pub fn remember_album_genre(&self, album_id: &str, genre: Option<&str>) {
        if album_id.is_empty() {
            return;
        }
        let text = {
            let mut inner = self.inner.lock();
            inner
                .meta
                .album_genres
                .insert(album_id.to_string(), genre.unwrap_or("").to_string());
            meta_json(&inner.meta)
        };
        if let Some(text) = text {
            self.send(Op::Rewrite(self.dir.join(META_FILE), text));
        }
    }

    /// The artists of plays that have no genre yet and no answer kept. Blocking.
    pub fn artists_missing_genres(&self) -> Vec<String> {
        self.load_everyone();
        let inner = self.inner.lock();
        let mut seen = HashSet::new();
        let mut artists = Vec::new();
        for rows in inner.users.values() {
            for row in rows.iter().filter(|row| row.genre.is_none()) {
                let key = artist_key(&row.artist);
                if !inner.meta.artist_genres.contains_key(&key) && seen.insert(key) {
                    artists.push(row.artist.clone());
                }
            }
        }
        artists
    }

    /// Gives every play with no genre the genre kept for its artist. Returns how many changed.
    pub fn fill_genres_from_artists(&self) -> usize {
        let mut changed = 0;
        let mut writes = Vec::new();
        {
            let mut inner = self.inner.lock();
            let genres = inner.meta.artist_genres.clone();
            for (key, rows) in &mut inner.users {
                let mut touched = false;
                for row in rows.iter_mut().filter(|row| row.genre.is_none()) {
                    if let Some(genre) = genres.get(&artist_key(&row.artist)).filter(|g| !g.is_empty()) {
                        row.genre = Some(genre.clone());
                        touched = true;
                        changed += 1;
                    }
                }
                if touched {
                    writes.push((key.clone(), lines_of(rows)));
                }
            }
            for (key, _) in &writes {
                inner.untidy.remove(key);
            }
        }
        for (key, text) in writes {
            self.send(Op::Rewrite(self.user_path(&key), text));
        }
        changed
    }

    /// Looks up on Last.fm the artists of plays with no genre, keeping each answer. A lookup
    /// that fails keeps nothing and stops the rest until the next run. Returns how many
    /// artists were answered.
    pub async fn look_up_missing_genres(
        self: &Arc<Self>,
        last_fm: &crate::services::last_fm::LastFmService,
    ) -> usize {
        let log = Arc::clone(self);
        let artists = tokio::task::spawn_blocking(move || log.artists_missing_genres())
            .await
            .unwrap_or_default();
        let mut answered = 0;
        for artist in artists {
            if !self.recording() {
                break;
            }
            match super::genres::artist_genre_from_last_fm(last_fm, &artist).await {
                Ok(genre) => {
                    self.remember_artist_genre(&artist, genre.as_deref());
                    answered += 1;
                }
                Err(_) => break,
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        answered
    }

    /// The work in the background, while recording is on: the radio history copied in once,
    /// the genres of plays without one looked up by artist on Last.fm, old plays pruned once a
    /// day, and plays left waiting with nothing after them written. On shutdown every current
    /// play is written as long as it has played.
    pub async fn run(
        self: Arc<Self>,
        history: Arc<crate::services::last_fm::last_fm_radio_state_store::LastFmRadioStateStore>,
        last_fm: Arc<crate::services::last_fm::LastFmService>,
        stopping: CancellationToken,
    ) -> anyhow::Result<()> {
        let mut daily: Option<DateTime<Utc>> = None;
        loop {
            if self.recording() && daily.is_none_or(|at| self.now() - at >= TimeDelta::hours(24)) {
                daily = Some(self.now());
                self.daily(&history, &last_fm).await;
            }
            let expired = self.settle_expired();
            super::write_settled(Arc::clone(&self), Arc::clone(&last_fm), expired);
            tokio::select! {
                () = stopping.cancelled() => break,
                () = tokio::time::sleep(Duration::from_secs(60)) => {}
            }
        }
        let log = Arc::clone(&self);
        let _ = tokio::task::spawn_blocking(move || {
            for settled in log.settle_all() {
                let genre = super::genres::resolve_kept(&log, &settled);
                log.write(settled, genre);
            }
            log.flush();
        })
        .await;
        Ok(())
    }

    async fn daily(
        self: &Arc<Self>,
        history: &Arc<crate::services::last_fm::last_fm_radio_state_store::LastFmRadioStateStore>,
        last_fm: &crate::services::last_fm::LastFmService,
    ) {
        let log = Arc::clone(self);
        let history = Arc::clone(history);
        let copied = tokio::task::spawn_blocking(move || {
            if log.backfilled() {
                return 0;
            }
            let people: Vec<(String, Vec<LastFmRadioPlay>)> = history
                .known_users()
                .into_iter()
                .map(|user| {
                    let plays = history.get_user(&user).plays;
                    (user, plays)
                })
                .collect();
            log.backfill(people)
        })
        .await
        .unwrap_or(0);
        if copied > 0 {
            info!("Listening: copied {copied} plays from the radio history");
        }
        self.look_up_missing_genres(last_fm).await;
        let log = Arc::clone(self);
        let (filled, removed) =
            tokio::task::spawn_blocking(move || (log.fill_genres_from_artists(), log.prune()))
                .await
                .unwrap_or((0, 0));
        if filled > 0 {
            debug!("Listening: {filled} plays took their artist's genre");
        }
        if removed > 0 {
            info!("Listening: removed {removed} plays older than the days kept");
        }
    }

    // ---- files ----------------------------------------------------------------------------

    fn user_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.jsonl", file_stem(key)))
    }

    /// Reads a person's file the first time their plays are needed. Blocking; the lock is
    /// never held while the file is read.
    fn ensure_loaded(&self, key: &str) {
        if key.is_empty() || self.inner.lock().users.contains_key(key) {
            return;
        }
        let (rows, untidy) = read_log(&self.user_path(key));
        let mut inner = self.inner.lock();
        if !inner.users.contains_key(key) {
            inner.users.insert(key.to_string(), rows);
            if untidy {
                inner.untidy.insert(key.to_string());
            }
        }
    }

    /// Reads the file of everyone on disk not read yet.
    fn load_everyone(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let keys: Vec<String> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".jsonl").and_then(key_of_stem)
            })
            .collect();
        for key in keys {
            self.ensure_loaded(&key);
        }
    }
}

/// The person's clear count.
fn generation(inner: &Inner, key: &str) -> u64 {
    inner.generations.get(key).copied().unwrap_or(0)
}

/// A current play ended at `end`: written only when an app reported it as played, timed to
/// its end and never past its length (or the cap for a play of no known length).
fn settle(pending: Pending, end: DateTime<Utc>, generation: u64) -> Option<Settled> {
    let limit = pending.limit();
    let draft = pending.draft?;
    let elapsed = TimeDelta::seconds((end - pending.start).num_seconds().max(0));
    let mut play = draft.play;
    play.seconds = elapsed.min(limit).num_seconds();
    play.played_at = pending.start;
    play.hearted |= pending.hearted;
    Some(Settled {
        user: pending.user,
        play,
        genres: draft.genres,
        album: draft.album,
        generation,
    })
}

/// The plays as lines of a person's file.
fn lines_of(rows: &[Play]) -> String {
    let mut text = String::new();
    for row in rows {
        if let Ok(line) = serde_json::to_string(row) {
            text.push_str(&line);
            text.push('\n');
        }
    }
    text
}

fn meta_json(meta: &Meta) -> Option<String> {
    let mut meta = meta.clone();
    meta.version = 1;
    match serde_json::to_string_pretty(&meta) {
        Ok(text) => Some(text),
        Err(e) => {
            warn!("{META_FILE} could not be written: {e}");
            None
        }
    }
}

/// A person's plays from their file, oldest first, each heart line folded into the latest play
/// of its song that it came during; and whether the file had heart lines to fold. A line torn
/// by a crash mid-append is left out.
fn read_log(path: &Path) -> (Vec<Play>, bool) {
    let Ok(Some(text)) = state_file::read_text(path) else {
        return (Vec::new(), false);
    };
    let mut rows: Vec<Play> = Vec::new();
    let mut hearts: Vec<HeartLine> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("heart").is_some() {
            if let Ok(heart) = serde_json::from_value::<HeartLine>(value) {
                hearts.push(heart);
            }
        } else if let Ok(play) = serde_json::from_value::<Play>(value) {
            rows.push(play);
        }
    }
    rows.sort_by_key(|row| row.played_at);
    for heart in &hearts {
        if let Some(row) = rows
            .iter_mut()
            .rev()
            .find(|row| row.song_id == heart.heart && row.heart_counts(heart.at))
        {
            row.hearted = true;
        }
    }
    (rows, !hearts.is_empty())
}

/// The writer thread: every write in the order it was asked for.
fn write_in_order(writes: mpsc::Receiver<Op>) {
    for op in writes {
        let result = match &op {
            Op::Append(path, line) => append(path, line),
            Op::Rewrite(path, text) => state_file::save_atomic(path, text),
            Op::Remove(path) => match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
            Op::Flush(done) => {
                let _ = done.send(());
                Ok(())
            }
        };
        if let Err(e) = result {
            warn!("The listening log could not be written: {e}");
        }
    }
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()
}

/// How an artist is looked up in the genre answers: trimmed and lower-cased.
fn artist_key(artist: &str) -> String {
    artist.trim().to_lowercase()
}

/// A person's key as a file name: letters, digits, `.`, `_` and `-` as they are, every other
/// byte as `%XX`.
fn file_stem(key: &str) -> String {
    let mut stem = String::new();
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') || (byte == b'.' && !stem.is_empty()) {
            stem.push(byte as char);
        } else {
            stem.push_str(&format!("%{byte:02X}"));
        }
    }
    stem
}

/// The key a file name stands for.
fn key_of_stem(stem: &str) -> Option<String> {
    let bytes = stem.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = stem.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let key = String::from_utf8(out).ok()?;
    (!key.is_empty()).then_some(key)
}

#[cfg(test)]
#[path = "listening_log_tests.rs"]
mod tests;
