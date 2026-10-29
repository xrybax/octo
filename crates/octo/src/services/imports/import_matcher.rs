//! Matches imported songs against the library, with `LibraryIndex`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use futures::future::BoxFuture;
use octo_core::common::Clock;
use octo_core::common::dotnet::is_blank;
use octo_core::common::romanize::{has_non_ascii_letters, romanized_key};
use octo_core::common::song_identity::{SongIdentity, SongRef};
use octo_core::common::version_gap::{self, Against};
use octo_core::common::{live_version, version_variant};
use parking_lot::Mutex;
use tracing::{debug, warn};

use super::ImportTrack;
use super::import_keys::ImportKeys;
use super::text_list::{Orientation, TextLine, choose_orientation};
use crate::services::family::library_index::row_full_path;
use crate::services::family::{LibraryScope, ScopeResolver};
use crate::services::library::NavidromePlaylistApi;
use crate::services::library::duplicate_scan_worker::is_lossless_file;
use crate::services::library::library_ownership::LibraryOwnership;
use crate::services::library::quality_upgrade_worker::LibrarySongRow;

/// One page of the library; None when Navidrome could not be read. Tests answer from a list.
pub type ReadPage =
    Arc<dyn Fn(i32, i32) -> BoxFuture<'static, Option<(Vec<LibrarySongRow>, usize)>> + Send + Sync>;

/// One recording an ISRC names in the music database: its id, its title and its length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsrcRecording {
    pub id: String,
    pub title: String,
    pub seconds: Option<i32>,
}

/// The recordings the music database lists for an ISRC; None when it could not be asked.
pub type IsrcRecordings = Arc<dyn Fn(String) -> BoxFuture<'static, Option<Vec<IsrcRecording>>> + Send + Sync>;

/// Whether the library already has each imported song, the way Octo decides it everywhere else:
/// one ISRC, or one MatchKey (the primary artist and the title, one version, so a live take or a
/// remix is its own song) and a length within LibraryOwnership's 8 seconds when both lengths are
/// known. A song those miss gets a second look through SongIdentity.Same among the songs with
/// its title, its title in Latin letters, or a name of its credit, which forgives a credit
/// written another way ("A, B" against "B feat. A", "Fonsi, Luis", an accent, a renamed artist)
/// and a title written in another script ("Камин" and "Kamin").
///
/// The whole library is read once, through Navidrome's own song list, and kept a few minutes, so
/// a list of thousands is matched in one pass rather than a search per song. With Family on, a
/// person's list is matched only against the songs they can see: the shared library and their
/// own, never another member's.
pub struct ImportMatcher {
    gate: tokio::sync::Mutex<()>,
    index: Mutex<Option<(Arc<LibraryIndex>, DateTime<Utc>)>>,
    problem: Mutex<Option<String>>,
    read_page: Mutex<ReadPage>,
    clock: Mutex<Clock>,
    scope: Option<ScopeResolver>,
    /// The music database's recordings for each ISRC asked about, kept while Octo runs.
    isrc_recordings: Mutex<HashMap<String, Vec<IsrcRecording>>>,
    ask_isrc: Mutex<Option<IsrcRecordings>>,
}

impl ImportMatcher {
    pub const KEEP_INDEX_FOR: TimeDelta = TimeDelta::minutes(10);
    const PAGE_SIZE: i32 = 1000;

    pub const CANNOT_READ_LIBRARY: &str = "Octo could not read your Navidrome library, so it cannot tell which songs you have. Octo needs a Navidrome admin sign-in, set under Music server.";

    /// Over Navidrome's song list.
    pub fn new(navidrome: Arc<NavidromePlaylistApi>) -> Self {
        Self::with_pages(Arc::new(move |start, count| {
            let navidrome = navidrome.clone();
            Box::pin(async move { navidrome.list_songs(start, count).await })
        }))
    }

    /// Over any source of pages.
    pub fn with_pages(read_page: ReadPage) -> Self {
        ImportMatcher {
            gate: tokio::sync::Mutex::new(()),
            index: Mutex::new(None),
            problem: Mutex::new(None),
            read_page: Mutex::new(read_page),
            clock: Mutex::new(Clock::system()),
            scope: None,
            isrc_recordings: Mutex::new(HashMap::new()),
            ask_isrc: Mutex::new(None),
        }
    }

    /// Asks the music database which recordings an imported song's ISRC names, so a library
    /// file tagged with one of those recordings is found when it carries no ISRC itself.
    pub fn with_isrc_recordings(self, ask: IsrcRecordings) -> Self {
        *self.ask_isrc.lock() = Some(ask);
        self
    }

    /// How many ISRCs one match asks the music database about, one a second: a long list is
    /// asked about over several matches.
    pub const ISRC_ASKS_PER_MATCH: usize = 20;

    /// Asks about the ISRCs of songs no library file carries, up to [`Self::ISRC_ASKS_PER_MATCH`]
    /// that were not asked about before.
    async fn learn_isrcs(&self, index: &LibraryIndex, tracks: &[ImportTrack]) {
        let Some(ask) = self.ask_isrc.lock().clone() else {
            return;
        };
        let wanted: Vec<String> = {
            let known = self.isrc_recordings.lock();
            let mut wanted: Vec<String> = Vec::new();
            for track in tracks {
                for isrc in SongIdentity::isrcs(track.isrc.iter()) {
                    if !known.contains_key(&isrc)
                        && !index.by_isrc.contains_key(&isrc)
                        && !wanted.contains(&isrc)
                    {
                        wanted.push(isrc);
                    }
                }
            }
            wanted.truncate(Self::ISRC_ASKS_PER_MATCH);
            wanted
        };
        for isrc in wanted {
            match ask(isrc.clone()).await {
                Some(recordings) => {
                    self.isrc_recordings.lock().insert(isrc, recordings);
                }
                None => break,
            }
        }
    }

    /// What the music database said about the track's ISRCs, as far as it was asked.
    fn known_recordings(&self, track: &ImportTrack) -> Vec<IsrcRecording> {
        let known = self.isrc_recordings.lock();
        SongIdentity::isrcs(track.isrc.iter())
            .into_iter()
            .filter_map(|isrc| known.get(&isrc).cloned())
            .flatten()
            .collect()
    }

    /// Matches each person's lists against their own scope of the library.
    pub fn with_library_scope(mut self, scope: ScopeResolver) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn set_read_page(&self, read_page: ReadPage) {
        *self.read_page.lock() = read_page;
    }

    pub fn set_clock(&self, clock: Clock) {
        *self.clock.lock() = clock;
    }

    fn now(&self) -> DateTime<Utc> {
        self.clock.lock().now()
    }

    /// Why the last match could not read the library, in words, or None when it could.
    pub fn problem(&self) -> Option<String> {
        self.problem.lock().clone()
    }

    /// Read the library again on the next match: something was just added to it.
    pub fn forget(&self) {
        *self.index.lock() = None;
    }

    /// Sets each track's LibraryId to the library's song for it, or None. False when the library
    /// could not be read, in which case the tracks keep what an earlier match found.
    pub async fn match_tracks(&self, tracks: &mut [ImportTrack]) -> bool {
        let Some(index) = self.library_index().await else {
            return false;
        };
        self.learn_isrcs(&index, tracks).await;
        for track in tracks.iter_mut() {
            let recordings = self.known_recordings(track);
            track.library_id = index
                .find_known(track, &recordings, &|_| true)
                .map(|row| row.id.clone());
        }
        true
    }

    /// [`Self::match_tracks`] for one person's lists: only songs that person can see count as
    /// theirs, so a song in another member's own library is still missing for them.
    pub async fn match_tracks_for(&self, owner: &str, tracks: &mut [ImportTrack]) -> bool {
        let Some(index) = self.library_index().await else {
            return false;
        };
        self.learn_isrcs(&index, tracks).await;
        let known: Vec<Vec<IsrcRecording>> =
            tracks.iter().map(|track| self.known_recordings(track)).collect();
        self.within(owner, |visible| {
            for (track, recordings) in tracks.iter_mut().zip(&known) {
                track.library_id = index
                    .find_known(track, recordings, visible)
                    .map(|row| row.id.clone());
            }
        });
        true
    }

    /// Playlist reads reuse the library index and already learned ISRCs. They never start
    /// catalog lookups, source refreshes or downloads on a player's request, and read the library
    /// only when `read_if_stale` says a song they miss has landed: false when no index read in
    /// the last [`Self::KEEP_INDEX_FOR`] is at hand otherwise.
    pub async fn match_streaming_tracks_for(
        &self,
        owner: &str,
        tracks: &mut [ImportTrack],
        read_if_stale: bool,
    ) -> bool {
        let fresh = self
            .index
            .lock()
            .clone()
            .filter(|(_, at)| self.now() - *at < Self::KEEP_INDEX_FOR)
            .map(|(index, _)| index);
        let index = match fresh {
            Some(index) => index,
            None if read_if_stale => match self.library_index().await {
                Some(index) => index,
                None => return false,
            },
            None => return false,
        };
        self.within(owner, |visible| {
            for track in tracks {
                track.library_id = index
                    .find_known(track, &self.known_recordings(track), visible)
                    .map(|row| row.id.clone());
            }
        });
        true
    }

    /// Which way round a plain list's lines are written, judged by which reading names more of
    /// the songs `owner` can see. Artist - Title when the library cannot be read.
    pub async fn orientation_for(&self, owner: &str, lines: &[TextLine]) -> Orientation {
        let Some(index) = self.library_index().await else {
            return Orientation::ArtistTitle;
        };
        self.within(owner, |visible| {
            choose_orientation(lines, |artist, title| {
                let track = ImportTrack {
                    key: ImportKeys::for_words(Some(artist), Some(title)),
                    artist: artist.to_string(),
                    title: title.to_string(),
                    ..Default::default()
                };
                index.knows_where(&track, visible)
            })
        })
    }

    /// Runs `work` with the test for the songs `owner` can see: every song, unless Family keeps
    /// members' libraries apart.
    fn within<R>(&self, owner: &str, work: impl FnOnce(&dyn Fn(&LibrarySongRow) -> bool) -> R) -> R {
        let view = self
            .scope
            .as_ref()
            .map(ScopeResolver::view)
            .filter(|view| view.apart());
        let Some(view) = view else {
            return work(&|_| true);
        };
        let scope = LibraryScope::Viewer(owner.to_string());
        let visible = |row: &LibrarySongRow| {
            let full = row_full_path(row.library_path.as_deref(), &row.path);
            view.allows(&scope, Some(&row.id), Some(&full))
        };
        work(&visible)
    }

    async fn library_index(&self) -> Option<Arc<LibraryIndex>> {
        let _gate = self.gate.lock().await;
        if let Some((fresh, at)) = self.index.lock().clone()
            && self.now() - at < Self::KEEP_INDEX_FOR
        {
            return Some(fresh);
        }
        let mut rows = Vec::new();
        let mut start = 0;
        loop {
            let read = self.read_page.lock().clone();
            let Some((page, count)) = read(start, Self::PAGE_SIZE).await else {
                warn!("Could not read the library to match imported songs");
                *self.problem.lock() = Some(Self::CANNOT_READ_LIBRARY.to_string());
                return None;
            };
            rows.extend(page);
            if (count as i32) < Self::PAGE_SIZE {
                break;
            }
            start += Self::PAGE_SIZE;
        }
        let count = rows.len();
        let index = Arc::new(LibraryIndex::new(rows));
        *self.index.lock() = Some((index.clone(), self.now()));
        *self.problem.lock() = None;
        debug!("Read {count} library songs to match imported songs against");
        Some(index)
    }
}

/// The library by ISRC, MatchKey, title, title in Latin letters and credited name, for matching
/// many songs at once.
pub struct LibraryIndex {
    rows: Vec<LibrarySongRow>,
    by_isrc: HashMap<String, Vec<usize>>,
    /// By MusicBrainz recording id, lowercased.
    by_recording: HashMap<String, Vec<usize>>,
    by_key: HashMap<String, Vec<usize>>,
    by_title: HashMap<String, Vec<usize>>,
    /// Titles with letters outside ASCII, by their Latin spelling.
    by_cross_key: HashMap<String, Vec<usize>>,
    /// Each credited name, by its key and its Latin spelling.
    by_name: HashMap<String, Vec<usize>>,
}

impl LibraryIndex {
    /// The most songs one lookup reads from the credited-name buckets: a name on thousands of
    /// songs stops there.
    pub const NAME_SCAN_CAP: usize = 2_000;

    pub fn new(rows: impl IntoIterator<Item = LibrarySongRow>) -> Self {
        let mut index = LibraryIndex {
            rows: Vec::new(),
            by_isrc: HashMap::new(),
            by_recording: HashMap::new(),
            by_key: HashMap::new(),
            by_title: HashMap::new(),
            by_cross_key: HashMap::new(),
            by_name: HashMap::new(),
        };
        for row in rows {
            if is_blank(&row.title) {
                continue;
            }
            let at = index.rows.len();
            for isrc in SongIdentity::isrcs(&row.isrcs) {
                add(&mut index.by_isrc, isrc, at);
            }
            if let Some(recording) = row.recording_id.as_deref().filter(|id| !is_blank(id)) {
                add(&mut index.by_recording, recording.trim().to_ascii_lowercase(), at);
            }
            add(
                &mut index.by_key,
                SongIdentity::match_key(&row.artist, &row.title),
                at,
            );
            let title = SongIdentity::parse_title(&row.title, Some(&row.artist));
            if has_non_ascii_letters(&title.keyed) {
                add(&mut index.by_cross_key, romanized_key(&title.keyed), at);
            }
            add(&mut index.by_title, title.key, at);
            for key in name_keys(&row.artist) {
                add(&mut index.by_name, key, at);
            }
            index.rows.push(row);
        }
        index
    }

    pub fn count(&self) -> usize {
        self.rows.len()
    }

    /// The library's best copy of this song, or None.
    pub fn find(&self, track: &ImportTrack) -> Option<&LibrarySongRow> {
        self.find_where(track, &|_| true)
    }

    /// The best copy among the songs `visible` keeps, or None. Looked for by ISRC first (the
    /// closest length among the songs carrying it), then by MatchKey at a length within the
    /// ownership tolerance, then among the songs with the same title, the same title in Latin
    /// letters, or a credited name in common, each accepted only by [`SongIdentity::same`] with
    /// [`SongIdentity::owned`]: another script and a credit written another way count, a typo
    /// does not. Within each step the best fit by [`SongIdentity::score`] wins.
    pub fn find_where(
        &self,
        track: &ImportTrack,
        visible: &dyn Fn(&LibrarySongRow) -> bool,
    ) -> Option<&LibrarySongRow> {
        self.find_known(track, &[], visible)
    }

    /// [`Self::find_where`] with the recordings the music database lists for the song's ISRC:
    /// a file tagged with one of them is the song even when it carries no ISRC, and their
    /// length stands in for a list that gave none. Every step takes only a file of the version
    /// the song names: its title says no other version (a remix by someone else, a sped-up
    /// copy, the plain song for a remix), and for the steps by name, its album names no other
    /// version either ("Let's Get Lost" on "Let's Get Lost Remixes" is not the original).
    pub fn find_known(
        &self,
        track: &ImportTrack,
        recordings: &[IsrcRecording],
        visible: &dyn Fn(&LibrarySongRow) -> bool,
    ) -> Option<&LibrarySongRow> {
        for isrc in SongIdentity::isrcs(track.isrc.iter()) {
            if let Some(coded) = self.by_isrc.get(&isrc)
                && let Some(hit) = best(
                    self.rows_of(coded)
                        .filter(|row| visible(row))
                        .filter(|row| !version_gap::other_version(&track.title, &row.title, Against::Title)),
                    track,
                    None,
                )
            {
                return Some(hit);
            }
        }

        let mut by_recording: Vec<usize> = Vec::new();
        for recording in recordings {
            if let Some(found) = self.by_recording.get(&recording.id.trim().to_ascii_lowercase()) {
                by_recording.extend(found);
            }
        }
        dedupe(&mut by_recording);
        if let Some(hit) = best(
            self.rows_of(&by_recording)
                .filter(|row| visible(row))
                .filter(|row| !version_gap::other_version(&track.title, &row.title, Against::Title)),
            track,
            None,
        ) {
            return Some(hit);
        }

        let seconds = track.seconds.filter(|s| *s > 0).or_else(|| {
            recordings
                .iter()
                .find(|recording| {
                    !version_gap::other_version(&track.title, &recording.title, Against::Recording)
                })
                .and_then(|recording| recording.seconds)
        });
        let wanted =
            SongRef::new(track.title.clone(), track.artist.clone()).with_seconds(seconds.map(f64::from));
        if let Some(same) = self
            .by_key
            .get(&SongIdentity::match_key(&track.artist, &track.title))
            && let Some(exact) = best(
                self.rows_of(same)
                    .filter(|row| visible(row))
                    .filter(|row| version_agrees(track, row))
                    .filter(|row| length_fits(seconds, row.duration)),
                track,
                Some(&wanted),
            )
        {
            return Some(exact);
        }

        let title = SongIdentity::parse_title(&track.title, Some(&track.artist));
        if title.key.is_empty() {
            return None;
        }
        let options = SongIdentity::owned(LibraryOwnership::DURATION_TOLERANCE_SECONDS);
        let accepted = |row: &&LibrarySongRow| {
            visible(row)
                && version_agrees(track, row)
                && SongIdentity::same(&wanted, &song_ref(row), Some(&options)).is_same()
        };

        if let Some(titled) = self.by_title.get(&title.key)
            && let Some(hit) = best(self.rows_of(titled).filter(accepted), track, Some(&wanted))
        {
            return Some(hit);
        }

        let mut crossed: Vec<usize> = Vec::new();
        if let Some(found) = self.by_cross_key.get(&title.key) {
            crossed.extend(found);
        }
        if has_non_ascii_letters(&title.keyed) {
            let latin = romanized_key(&title.keyed);
            for bucket in [self.by_title.get(&latin), self.by_cross_key.get(&latin)]
                .into_iter()
                .flatten()
            {
                crossed.extend(bucket);
            }
        }
        dedupe(&mut crossed);
        if let Some(hit) = best(self.rows_of(&crossed).filter(accepted), track, Some(&wanted)) {
            return Some(hit);
        }

        let mut named: Vec<usize> = Vec::new();
        for key in name_keys(&track.artist) {
            if let Some(found) = self.by_name.get(&key) {
                named.extend(found);
            }
        }
        dedupe(&mut named);
        if named.len() > Self::NAME_SCAN_CAP {
            debug!(
                "'{}' is credited on {} library songs; matching '{}' read the first {}",
                track.artist,
                named.len(),
                track.title,
                Self::NAME_SCAN_CAP
            );
            named.truncate(Self::NAME_SCAN_CAP);
        }
        best(self.rows_of(&named).filter(accepted), track, Some(&wanted))
    }

    /// Whether the library has this song by its MatchKey or by its title, among the songs
    /// `visible` keeps: a quick test for reading a whole list, without the slower lookups.
    pub fn knows_where(&self, track: &ImportTrack, visible: &dyn Fn(&LibrarySongRow) -> bool) -> bool {
        if self
            .by_key
            .get(&SongIdentity::match_key(&track.artist, &track.title))
            .is_some_and(|same| self.rows_of(same).any(visible))
        {
            return true;
        }
        let title = SongIdentity::parse_title(&track.title, Some(&track.artist)).key;
        if title.is_empty() {
            return false;
        }
        let wanted = SongRef::new(track.title.clone(), track.artist.clone());
        let options = SongIdentity::owned(LibraryOwnership::DURATION_TOLERANCE_SECONDS);
        self.by_title.get(&title).is_some_and(|titled| {
            self.rows_of(titled).any(|row| {
                visible(row) && SongIdentity::same(&wanted, &song_ref(row), Some(&options)).is_same()
            })
        })
    }

    fn rows_of<'a, 'b>(&'a self, at: &'b [usize]) -> impl Iterator<Item = &'a LibrarySongRow> + use<'a, 'b> {
        at.iter().map(move |&i| &self.rows[i])
    }
}

/// Whether a library file is the version an imported song names, by the file's own title and
/// album: no version marker or remixer differs between the two titles, and an album that names
/// a version ("Remixes", "Sped Up", "Live at ...") names one the song's title or its own album
/// asks for, or the file's title says.
pub fn version_agrees(track: &ImportTrack, row: &LibrarySongRow) -> bool {
    if version_gap::other_version(&track.title, &row.title, Against::Title) {
        return false;
    }
    let Some(album) = row.album.as_deref().filter(|album| !is_blank(album)) else {
        return true;
    };
    let artist = Some(row.artist.as_str());
    let mut named = version_variant::in_folder(album, artist);
    if live_version::mentions(Some(&version_variant::without_artist(album, artist))) {
        named.insert("live".to_string());
    }
    if named.is_empty() {
        return true;
    }
    let mut allowed = version_variant::requested(Some(&track.title), track.album.as_deref());
    allowed.extend(version_variant::requested(Some(&row.title), None));
    if track
        .album
        .as_deref()
        .is_some_and(|own| live_version::mentions(Some(own)))
    {
        allowed.insert("live".to_string());
    }
    named.iter().all(|version| allowed.contains(version))
}

fn song_ref(row: &LibrarySongRow) -> SongRef {
    SongRef::new(row.title.clone(), row.artist.clone())
        .with_seconds(row.duration.filter(|d| *d > 0).map(f64::from))
}

/// The keys a credit's names are found by: each name and guest, as a key and, when it has
/// letters outside ASCII, in Latin letters. Names that credit nobody are left out.
fn name_keys(artist: &str) -> Vec<String> {
    let credit = SongIdentity::parse_artists(artist);
    let mut keys: Vec<String> = Vec::new();
    for name in credit.names.iter().chain(&credit.featured) {
        if SongIdentity::is_placeholder_artist(name) {
            continue;
        }
        let mut push = |key: String| {
            if !key.is_empty() && !keys.contains(&key) {
                keys.push(key);
            }
        };
        push(SongIdentity::key(name));
        if has_non_ascii_letters(name) {
            push(romanized_key(name));
        }
    }
    keys
}

fn dedupe(at: &mut Vec<usize>) {
    let mut seen = HashSet::new();
    at.retain(|i| seen.insert(*i));
}

fn add(map: &mut HashMap<String, Vec<usize>>, key: String, at: usize) {
    if key.is_empty() {
        return;
    }
    let bucket = map.entry(key).or_default();
    if bucket.last() != Some(&at) {
        bucket.push(at);
    }
}

fn length_fits(wanted: Option<i32>, found: Option<i32>) -> bool {
    match (wanted.filter(|w| *w > 0), found.filter(|f| *f > 0)) {
        (Some(wanted), Some(found)) => (wanted - found).abs() <= LibraryOwnership::DURATION_TOLERANCE_SECONDS,
        _ => true,
    }
}

/// With `wanted`, the best fit by [`SongIdentity::score`] first; then the closest in length,
/// the same album, lossless, and the highest bitrate.
fn best<'a>(
    rows: impl Iterator<Item = &'a LibrarySongRow>,
    track: &ImportTrack,
    wanted: Option<&SongRef>,
) -> Option<&'a LibrarySongRow> {
    let album_key = track.album.as_deref().map(SongIdentity::key);
    let mut rows: Vec<(&LibrarySongRow, f64)> = rows
        .map(|row| {
            let fit = wanted.map_or(0.0, |wanted| SongIdentity::score(wanted, &song_ref(row)));
            (row, fit)
        })
        .collect();
    // Stable, as OrderBy and its ThenBys are.
    rows.sort_by(|(a, fit_a), (b, fit_b)| {
        let distance = |row: &LibrarySongRow| match (
            track.seconds.filter(|s| *s > 0),
            row.duration.filter(|d| *d > 0),
        ) {
            (Some(wanted), Some(found)) => (wanted - found).abs(),
            _ => 0,
        };
        let same_album = |row: &LibrarySongRow| {
            album_key
                .as_ref()
                .is_some_and(|key| SongIdentity::key(row.album.as_deref().unwrap_or("")) == *key)
        };
        fit_b
            .total_cmp(fit_a)
            .then_with(|| distance(a).cmp(&distance(b)))
            .then_with(|| same_album(b).cmp(&same_album(a)))
            .then_with(|| {
                is_lossless_file(&b.suffix, b.bit_rate).cmp(&is_lossless_file(&a.suffix, a.bit_rate))
            })
            .then_with(|| b.bit_rate.cmp(&a.bit_rate))
    });
    rows.into_iter().next().map(|(row, _)| row)
}

/// [`IsrcRecordings`] over MusicBrainz's code lookup, in the order it lists the recordings. A
/// timeout or an unreadable answer is None, so the next match asks again.
pub fn isrc_recordings_from(
    music_brainz: Arc<crate::services::fingerprint::music_brainz_client::MusicBrainzClient>,
) -> IsrcRecordings {
    Arc::new(move |isrc: String| {
        let music_brainz = music_brainz.clone();
        Box::pin(async move {
            let doc = music_brainz.lookup_isrc(&isrc).await.ok()?;
            Some(doc.map(|doc| isrc_recordings_of(&doc)).unwrap_or_default())
        })
    })
}

/// The recordings of a MusicBrainz code lookup's answer.
pub fn isrc_recordings_of(doc: &serde_json::Value) -> Vec<IsrcRecording> {
    doc.get("recordings")
        .and_then(serde_json::Value::as_array)
        .map(|recordings| {
            recordings
                .iter()
                .filter_map(|recording| {
                    let id = recording.get("id")?.as_str()?.to_string();
                    let title = recording
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    let seconds = recording
                        .get("length")
                        .and_then(serde_json::Value::as_i64)
                        .filter(|ms| *ms > 0)
                        .map(|ms| ((ms + 500) / 1000) as i32);
                    Some(IsrcRecording {
                        id,
                        title: title.to_string(),
                        seconds,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "import_matcher_tests.rs"]
mod tests;
