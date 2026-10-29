//! The library tag check: every library song's file is fingerprinted in AcoustID's spare lane,
//! the one recording its audio is gets decided, and its tags are held against that recording
//! (`octo_core::tagging::tag_audit`). What disagrees waits in Library health for a person;
//! stray spaces are taken off here, through the journaled edit path. Its place and every
//! song's answer are kept in `<config>/identity-sweep.json`, so a restart carries on.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use futures::FutureExt;
use indexmap::IndexMap;
use octo_core::common::Clock;
use octo_core::common::dotnet::{self, is_blank};
use octo_core::fingerprint::acoust_id_client::AcoustIdRecording;
use octo_core::fingerprint::recording_identity::{self, Identity, RecordingFacts, RecordingKind};
use octo_core::fingerprint::verification::{InconclusiveReason, VerificationResult, VerificationVerdict};
use octo_core::json::datetime;
use octo_core::library::generated_playlist_service::dotnet_ticks;
use octo_core::settings::SettingsStore;
use octo_core::tagging::tag_audit::{
    self, AuditRule, FileTags, HeardAs, NamedRecording, OtherAlbumArtists, OtherNames, TagAudit, TakeCopy,
};
use octo_media::tags::TagFile;
use octo_media::tags::credit_names::StrayAlbumArtists;
use octo_media::tags::field_names;
use octo_media::tags::tag_writer_extras::{self as extras, TagFields};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::library_edit_service::{LibraryEditOutcome, LibraryEditService};
use super::library_review_sweep_worker::{IReviewSweepVerifier, LibraryReviewSweepWorker};
use super::library_snapshot::LibrarySnapshot;
use super::navidrome_song_path_resolver::{Candidate, NavidromeSongPathResolver, PathSource};
use super::notice_queue::set_aside;
use super::quality_upgrade_worker::compare_ordinal;
use super::version_check::{self, KeptCover, KeptVersion, VersionInput, VersionSources};
use crate::services::common::IAcquisitionActivity;
use crate::services::family::library_index::row_full_path;
use crate::services::family::{LibraryScope, ScopeResolver};
use crate::services::fingerprint::MusicBrainzClient;
use crate::services::state_file;

// ---- what the check needs, behind seams so tests need no Navidrome, fpcalc or MusicBrainz ----

/// One library song as Navidrome lists it, with what proves which file it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepSong {
    pub id: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    /// The path Navidrome reports, its library's root, and the size and suffix it reports,
    /// which the file must match.
    pub path: String,
    pub library_path: Option<String>,
    pub size: i64,
    pub suffix: String,
}

/// The library's songs, and the file each one is.
#[async_trait]
pub trait IdentitySweepLibrary: Send + Sync {
    /// Every song the check looks at, or None when Navidrome cannot be read.
    async fn songs(&self) -> Option<Vec<SweepSong>>;
    /// The song's file on this server, proved by its size, or None.
    fn locate(&self, song: &SweepSong) -> Option<String>;
}

/// The journaled edit path the check writes stray spaces through.
#[async_trait]
pub trait IdentitySweepEdits: Send + Sync {
    /// Why files may not be changed for this user now, or None when they may.
    fn refusal(&self, user: &str) -> Option<String>;
    /// Writes the changes, unless the file's stamp is no longer `expected_stamp`: a file
    /// replaced since its tags were read keeps the new tags.
    async fn retag(
        &self,
        id: &str,
        user: &str,
        changes: IndexMap<String, Option<String>>,
        expected_stamp: Option<String>,
    ) -> LibraryEditOutcome;
}

/// The tracks of one release, each as its release track id and its recording id.
#[async_trait]
pub trait ReleaseTracks: Send + Sync {
    async fn tracks(&self, release_id: &str) -> Option<Vec<(String, String)>>;
}

/// What the music database says one recording is, asked only for a recording a file's tags
/// name that the fingerprint does not.
#[async_trait]
pub trait RecordingFactsLookup: Send + Sync {
    async fn facts(&self, recording_id: &str) -> Option<RecordingFacts>;
}

/// Navidrome's song list from the library snapshot, each file proved the way a library action
/// proves it. With Family on, only the shared library.
pub struct SnapshotSweepLibrary {
    pub snapshot: Arc<LibrarySnapshot>,
    pub resolver: Arc<NavidromeSongPathResolver>,
    pub scope: Option<ScopeResolver>,
}

#[async_trait]
impl IdentitySweepLibrary for SnapshotSweepLibrary {
    async fn songs(&self) -> Option<Vec<SweepSong>> {
        let index = self.snapshot.current().await?;
        let view = self
            .scope
            .as_ref()
            .filter(|scope| scope.keeps_shared())
            .map(ScopeResolver::shared_view);
        Some(
            index
                .rows()
                .iter()
                .filter(|row| {
                    view.as_ref().is_none_or(|view| {
                        let full = row_full_path(row.library_path.as_deref(), &row.path);
                        view.allows(&LibraryScope::Shared, Some(&row.id), Some(&full))
                    })
                })
                .map(|row| SweepSong {
                    id: row.id.clone(),
                    artist: row.artist.clone(),
                    title: row.title.clone(),
                    album: row.album.clone().unwrap_or_default(),
                    path: row.path.clone(),
                    library_path: row.library_path.clone(),
                    size: row.size,
                    suffix: row.suffix.clone(),
                })
                .collect(),
        )
    }

    fn locate(&self, song: &SweepSong) -> Option<String> {
        let candidate = Candidate {
            id: song.id.clone(),
            raw_path: Some(song.path.clone()),
            library_path: song.library_path.clone(),
            size: song.size,
            title: song.title.clone(),
            artist: song.artist.clone(),
            album: song.album.clone(),
            suffix: song.suffix.clone(),
            duration: None,
            source: PathSource::NativeApi,
            missing: false,
            album_artist: None,
        };
        self.resolver
            .verify(&candidate, &self.resolver.music_root(), true)
            .map(|file| file.absolute_path)
    }
}

/// The edit service the apps' Library health uses: every write journaled, so it can be undone.
pub struct EditServiceSweepEdits(pub Arc<LibraryEditService>);

#[async_trait]
impl IdentitySweepEdits for EditServiceSweepEdits {
    fn refusal(&self, user: &str) -> Option<String> {
        self.0.refusal(Some(user), true)
    }

    async fn retag(
        &self,
        id: &str,
        user: &str,
        changes: IndexMap<String, Option<String>>,
        expected_stamp: Option<String>,
    ) -> LibraryEditOutcome {
        self.0
            .retag_unchanged(id, user, changes, expected_stamp.as_deref())
            .await
    }
}

/// A release's tracks from MusicBrainz, on its background lane.
pub struct MusicBrainzReleaseTracks(pub Arc<MusicBrainzClient>);

#[async_trait]
impl ReleaseTracks for MusicBrainzReleaseTracks {
    async fn tracks(&self, release_id: &str) -> Option<Vec<(String, String)>> {
        let details = self.0.lookup_release_in_background(release_id).await.ok()??;
        Some(
            details
                .tracks
                .iter()
                .filter_map(|track| {
                    Some((
                        track.release_track_id.clone().filter(|id| !is_blank(id))?,
                        track.recording_id.clone().filter(|id| !is_blank(id))?,
                    ))
                })
                .collect(),
        )
    }
}

/// A recording's facts from MusicBrainz, on its background lane.
pub struct MusicBrainzRecordingFacts(pub Arc<MusicBrainzClient>);

#[async_trait]
impl RecordingFactsLookup for MusicBrainzRecordingFacts {
    async fn facts(&self, recording_id: &str) -> Option<RecordingFacts> {
        self.0.recording_facts(recording_id, true).await.ok()?
    }
}

// ---- what is kept ----------------------------------------------------------------------------

/// One thing found about one song, as kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct KeptFinding {
    pub rule: String,
    pub words: String,
}

/// One song as the check last found it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct CheckedSong {
    /// The file's size and last write when checked; a file changed since is checked again.
    pub stamp: String,
    pub path: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    /// The recording the audio was found to be, and its kind; None when nothing was decided.
    pub recording_id: Option<String>,
    pub kind: Option<String>,
    /// The kind is known, not only the likeliest pick among takes that share the fingerprint.
    /// Only a known kind is said in words or held against another copy.
    pub kind_certain: bool,
    /// The exact length or a single match decided it.
    pub sure: bool,
    /// AcoustID never answered for it: what else was found is kept, and the song is checked
    /// again next pass.
    pub retry: bool,
    pub findings: Vec<KeptFinding>,
    /// The tags that would put the findings right: a value, or None to take a tag off.
    pub changes: IndexMap<String, Option<String>>,
    /// The album artist's other names were looked for. A song checked before they were is read
    /// again for them alone, without its audio.
    pub names_checked: bool,
    /// What the version check found: the file is not the version of its song its tags say.
    pub version: Option<KeptVersion>,
    /// The version check ran on this file. A song checked before it existed is checked again.
    pub version_checked: bool,
    /// What the version check found of the file's cover: another artist's release's.
    pub cover: Option<KeptCover>,
    #[serde(with = "datetime::utc_option")]
    pub checked_utc: Option<DateTime<Utc>>,
}

fn one() -> i32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct IdentitySweepState {
    #[serde(default)]
    pub paused: bool,
    /// The id of the last song dealt with this pass; empty at its start.
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub cursor: String,
    /// Every song checked, by its Navidrome id.
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub songs: BTreeMap<String, CheckedSong>,
    #[serde(default = "one")]
    pub pass: i32,
    #[serde(default)]
    pub total: i32,
    /// Songs whose stray spaces the check took off by itself.
    #[serde(default)]
    pub spaces_fixed: i32,
    /// Songs whose album artist the check took off its other names by itself.
    #[serde(default)]
    pub names_fixed: i32,
    #[serde(default, with = "datetime::utc_option")]
    pub last_checked_utc: Option<DateTime<Utc>>,
    #[serde(default, with = "datetime::utc_option")]
    pub pass_finished_utc: Option<DateTime<Utc>>,
    #[serde(default = "datetime::min_value", with = "datetime::utc")]
    pub next_pass_utc: DateTime<Utc>,
}

impl Default for IdentitySweepState {
    fn default() -> Self {
        IdentitySweepState {
            paused: false,
            cursor: String::new(),
            songs: BTreeMap::new(),
            pass: 1,
            total: 0,
            spaces_fixed: 0,
            names_fixed: 0,
            last_checked_utc: None,
            pass_finished_utc: None,
            next_pass_utc: datetime::min_value(),
        }
    }
}

struct StoreInner {
    state: IdentitySweepState,
    dirty: bool,
    /// One more on every change, so what is worked out from the state knows when it is stale.
    version: u64,
}

/// identity-sweep.json, written at most every [`IdentitySweepStore::FLUSH_INTERVAL`]: it holds
/// a line per library song, so a write per song would rewrite it thousands of times a pass.
/// [`IdentitySweepStore::run_flusher`] picks up what is left every 5 s and on shutdown, and
/// `Drop` flushes once more.
pub struct IdentitySweepStore {
    path: Option<PathBuf>,
    clock: Clock,
    inner: Mutex<StoreInner>,
    flush_lock: Mutex<()>,
    last_write: Mutex<Option<DateTime<Utc>>>,
}

impl IdentitySweepStore {
    pub const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

    /// A store kept in `path` (None keeps it in memory), on the system clock.
    pub fn new(path: Option<PathBuf>) -> Self {
        Self::with_clock(path, Clock::system())
    }

    /// A file that cannot be read is set aside as `<file>.corrupt-<ticks>` and the check
    /// starts over.
    pub fn with_clock(path: Option<PathBuf>, clock: Clock) -> Self {
        let path = path.filter(|p| !dotnet::is_blank(&p.to_string_lossy()));
        let mut state = IdentitySweepState::default();
        if let Some(path) = &path
            && path.exists()
        {
            match Self::load(path) {
                Ok(loaded) => state = loaded,
                Err(e) => {
                    warn!("library tag check state could not be read ({e}); starting over");
                    set_aside(path);
                }
            }
        }
        IdentitySweepStore {
            path,
            clock,
            inner: Mutex::new(StoreInner {
                state,
                dirty: false,
                version: 0,
            }),
            flush_lock: Mutex::new(()),
            last_write: Mutex::new(None),
        }
    }

    fn load(path: &Path) -> anyhow::Result<IdentitySweepState> {
        let text = state_file::read_all_text(path)?;
        Ok(serde_json::from_str::<Option<IdentitySweepState>>(&text)?.unwrap_or_default())
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn read<T>(&self, read: impl FnOnce(&IdentitySweepState) -> T) -> T {
        read(&self.inner.lock().state)
    }

    /// Reads the state with its version, which moves on with every change.
    pub fn read_versioned<T>(&self, read: impl FnOnce(&IdentitySweepState) -> T) -> (u64, T) {
        let inner = self.inner.lock();
        (inner.version, read(&inner.state))
    }

    /// Changes the state in memory; it reaches the disk now when nothing was written in the
    /// last few seconds, otherwise on the next flush.
    pub fn update(&self, change: impl FnOnce(&mut IdentitySweepState)) {
        {
            let mut inner = self.inner.lock();
            change(&mut inner.state);
            inner.dirty = true;
            inner.version += 1;
        }
        if self.path.is_none() {
            return;
        }
        let now = self.clock.now();
        let due = self.last_write.lock().is_none_or(|last| {
            now - last >= TimeDelta::from_std(Self::FLUSH_INTERVAL).expect("five seconds")
        });
        if due {
            self.flush();
        }
    }

    /// Writes the state if it changed, the snapshot taken under the same lock as the write.
    pub fn flush(&self) -> bool {
        let Some(path) = self.path.as_deref() else {
            return true;
        };
        let _flushing = self.flush_lock.lock();
        let json = {
            let mut inner = self.inner.lock();
            if !inner.dirty {
                return true;
            }
            inner.dirty = false;
            octo_core::json::to_string(&inner.state)
        };
        *self.last_write.lock() = Some(self.clock.now());
        match state_file::save_atomic(path, &json) {
            Ok(()) => true,
            Err(e) => {
                self.inner.lock().dirty = true;
                warn!("library tag check state could not be written: {e}");
                false
            }
        }
    }

    /// The 5-second flush timer, and the last flush on shutdown, as a worker.
    pub async fn run_flusher(self: Arc<Self>, token: CancellationToken) -> anyhow::Result<()> {
        let store = self.clone();
        state_file::flush_every(
            Self::FLUSH_INTERVAL,
            token,
            Arc::new(move || {
                store.flush();
            }),
        )
        .await
    }
}

impl Drop for IdentitySweepStore {
    fn drop(&mut self) {
        self.flush();
    }
}

// ---- what Library health shows ---------------------------------------------------------------

/// One song whose tags disagree with its audio, for Library health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagFinding {
    pub id: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub path: String,
    /// The file's size and last write when checked; a fix is refused once it differs.
    pub stamp: String,
    pub findings: Vec<(AuditRule, String)>,
    /// The tags "Use the audio's tags" writes; empty when nothing can be written for it.
    pub changes: IndexMap<String, Option<String>>,
    pub sure: bool,
    /// What the audio is, in words: "a live recording".
    pub heard: Option<String>,
}

/// One song the version check found is not the version its tags say, for Library health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRow {
    pub id: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub path: String,
    /// The file's size and last write when checked; a retag is refused once it differs.
    pub stamp: String,
    pub version: KeptVersion,
}

/// One song whose cover the version check found is another artist's release's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverRow {
    pub id: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub path: String,
    pub stamp: String,
    pub cover: KeptCover,
}

/// What the dashboard shows of the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySweepStatus {
    pub state: String,
    pub reason: Option<String>,
    pub paused: bool,
    pub checked: usize,
    pub total: i32,
    pub pass: i32,
    pub found: usize,
    pub sure: usize,
    pub spaces_fixed: i32,
    pub names_fixed: i32,
    pub per_hour: i32,
    pub last_checked_utc: Option<DateTime<Utc>>,
    pub pass_finished_utc: Option<DateTime<Utc>>,
}

/// What the check is built from, apart from the clock.
pub struct LibraryIdentitySweepParts {
    pub store: Arc<IdentitySweepStore>,
    pub verifier: Arc<dyn IReviewSweepVerifier>,
    pub activity: Arc<dyn IAcquisitionActivity>,
    pub library: Arc<dyn IdentitySweepLibrary>,
    /// The edit path stray spaces go through; None leaves them for a person.
    pub edits: Option<Arc<dyn IdentitySweepEdits>>,
    /// A release's tracks, asked only for a file whose ids may be another recording's.
    pub releases: Option<Arc<dyn ReleaseTracks>>,
    /// What a recording is, asked only for one a file's ids name that the fingerprint does not.
    pub recordings: Option<Arc<dyn RecordingFactsLookup>>,
    /// What the version check asks of the music database, the catalog and AcoustID; None
    /// leaves the version check off.
    pub versions: Option<Arc<dyn VersionSources>>,
    /// The library action and Subsonic settings, read at use.
    pub settings: Arc<SettingsStore>,
}

/// Gives every library song an identity, a few an hour, only while nothing is downloading, and
/// holds its tags against it. It never changes anything but stray spaces; the rest is listed in
/// Library health for a person to fix.
pub struct LibraryIdentitySweep {
    store: Arc<IdentitySweepStore>,
    verifier: Arc<dyn IReviewSweepVerifier>,
    activity: Arc<dyn IAcquisitionActivity>,
    library: Arc<dyn IdentitySweepLibrary>,
    edits: Option<Arc<dyn IdentitySweepEdits>>,
    releases: Option<Arc<dyn ReleaseTracks>>,
    recordings: Option<Arc<dyn RecordingFactsLookup>>,
    versions: Option<Arc<dyn VersionSources>>,
    settings: Arc<SettingsStore>,
    clock: Clock,

    songs: Mutex<Option<Arc<Vec<SweepSong>>>>,
    generation: AtomicI32,
    lookup_failures: AtomicI32,
    hold: Mutex<(String, Option<String>)>,
    /// Every song with something found, as last worked out from the state, and the state's
    /// version it was worked out from.
    listed: Mutex<Option<(u64, Arc<Vec<TagFinding>>)>>,
}

/// How one song's check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checked {
    /// Checked and kept, whatever it found.
    Done,
    /// AcoustID did not answer; the song is tried again.
    Again,
}

impl LibraryIdentitySweep {
    pub const MAX_LOOKUP_FAILURES_PER_SONG: i32 = 3;
    pub const IDLE_CHECK: Duration = Duration::from_secs(60);
    pub const BUSY_CHECK: Duration = Duration::from_secs(30);
    /// After a full pass, how long before the library is listed again for new or changed songs.
    pub const PASS_INTERVAL: TimeDelta = TimeDelta::hours(6);

    const DONE: &'static str = "Every song has been checked. New or changed ones are checked later.";

    pub fn new(parts: LibraryIdentitySweepParts, clock: Clock) -> Self {
        LibraryIdentitySweep {
            store: parts.store,
            verifier: parts.verifier,
            activity: parts.activity,
            library: parts.library,
            edits: parts.edits,
            releases: parts.releases,
            recordings: parts.recordings,
            versions: parts.versions,
            settings: parts.settings,
            clock,
            songs: Mutex::new(None),
            generation: AtomicI32::new(0),
            lookup_failures: AtomicI32::new(0),
            hold: Mutex::new(("Off".to_string(), None)),
            listed: Mutex::new(None),
        }
    }

    pub fn store(&self) -> &Arc<IdentitySweepStore> {
        &self.store
    }

    /// The worker loop until the host stops, then a last flush.
    pub async fn run(self: Arc<Self>, stopping: CancellationToken) -> anyhow::Result<()> {
        // Never alongside startup, which already works the disk and Navidrome.
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(180)) => {}
            _ = stopping.cancelled() => {
                self.store.flush();
                return Ok(());
            }
        }
        while !stopping.is_cancelled() {
            // A tick that panics must never stop the worker.
            let tick = tokio::select! {
                tick = std::panic::AssertUnwindSafe(self.tick()).catch_unwind() => tick,
                _ = stopping.cancelled() => break,
            };
            let wait = tick.unwrap_or_else(|_| {
                error!("Library tag check failed");
                Self::IDLE_CHECK
            });
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = stopping.cancelled() => break,
            }
        }
        self.store.flush();
        Ok(())
    }

    /// One song checked, or why not; answers how long to wait before the next.
    pub async fn tick(&self) -> Duration {
        let per_hour = self
            .settings
            .current()
            .library_actions
            .effective_tag_check_per_hour();
        if per_hour == 0 {
            return self.hold(
                "Off",
                Some("Set how many songs an hour to check under Library actions."),
                Self::IDLE_CHECK,
            );
        }
        if self.store.read(|s| s.paused) {
            return self.hold("Paused", Some("Paused from the dashboard."), Self::IDLE_CHECK);
        }
        if !self.verifier.is_ready() {
            return self.hold(
                "Paused",
                Some("AcoustID is not set up: turn on download verification and add a key."),
                Self::IDLE_CHECK,
            );
        }
        if self.activity.is_busy() {
            return self.hold(
                "Waiting",
                Some("Waiting for a download to finish."),
                Self::BUSY_CHECK,
            );
        }
        let interval = Duration::from_secs_f64(3600.0 / f64::from(per_hour));
        let generation = self.generation.load(Ordering::SeqCst);

        self.read_names_again(generation).await;

        // A song with findings whose file changed since (a fix, an undo, an edit) goes first.
        if let Some((id, song)) = self.changed_since() {
            return match self.check(generation, &id, &song, false).await {
                Checked::Done => self.hold("Running", None, interval),
                Checked::Again => self.hold(
                    "Waiting",
                    Some("AcoustID did not answer; trying that song again."),
                    interval,
                ),
            };
        }

        let now = self.clock.now();
        if self.songs.lock().is_none() && self.store.read(|s| s.cursor.is_empty() && s.next_pass_utc > now) {
            return self.hold("Done", Some(Self::DONE), Self::IDLE_CHECK);
        }
        if self.songs.lock().is_none() {
            let Some(mut listed) = self.library.songs().await else {
                return self.hold(
                    "Paused",
                    Some("Octo could not read your library from Navidrome."),
                    Self::IDLE_CHECK,
                );
            };
            if generation != self.generation.load(Ordering::SeqCst) {
                return Duration::ZERO;
            }
            listed.sort_by(|a, b| compare_ordinal(&a.id, &b.id));
            listed.dedup_by(|a, b| a.id == b.id);
            let total = i32::try_from(listed.len()).unwrap_or(i32::MAX);
            *self.songs.lock() = Some(Arc::new(listed));
            self.record(generation, |s| s.total = total);
        }
        let Some(songs) = self.songs.lock().clone() else {
            return Duration::ZERO;
        };

        let cursor = self.store.read(|s| s.cursor.clone());
        let mut passed = cursor.clone();
        let mut target: Option<(SweepSong, String)> = None;
        for song in songs.iter().skip(first_after(&songs, &cursor)) {
            let Some(path) = self.library.locate(song) else {
                passed = song.id.clone();
                continue;
            };
            let stamp = stamp_of(&path);
            let versions_on = self.versions.is_some();
            let seen = self.store.read(|s| {
                s.songs.get(&song.id).map(|kept| {
                    (
                        kept.stamp.clone(),
                        kept.path.clone(),
                        kept.retry || (versions_on && !kept.version_checked),
                    )
                })
            });
            let unchanged = matches!((&stamp, &seen), (Some(stamp), Some((seen, _, false))) if stamp == seen);
            if stamp.is_none() || unchanged {
                // The same file somewhere else (a move or a rename keeps its size and last
                // write): its answer holds, at the place it is now.
                if let Some((_, kept_path, _)) = &seen
                    && unchanged
                    && *kept_path != path
                {
                    let id = song.id.clone();
                    let moved = path.clone();
                    self.record(generation, |s| {
                        if let Some(kept) = s.songs.get_mut(&id) {
                            kept.path = moved;
                        }
                    });
                }
                passed = song.id.clone();
                continue;
            }
            target = Some((song.clone(), path));
            break;
        }

        let Some((song, path)) = target else {
            let present: HashSet<&str> = songs.iter().map(|song| song.id.as_str()).collect();
            self.record(generation, |s| {
                s.songs.retain(|id, _| present.contains(id.as_str()));
                s.cursor = String::new();
                s.pass += 1;
                s.pass_finished_utc = Some(now);
                s.next_pass_utc = now + Self::PASS_INTERVAL;
            });
            *self.songs.lock() = None;
            info!("Library tag check finished a pass over {} song(s)", songs.len());
            return self.hold("Done", Some(Self::DONE), Self::IDLE_CHECK);
        };

        if passed != cursor {
            self.record(generation, |s| s.cursor = passed.clone());
        }
        let located = SweepSong { path, ..song };
        match self.check(generation, &located.id.clone(), &located, true).await {
            Checked::Done => self.hold("Running", None, interval),
            Checked::Again => self.hold(
                "Waiting",
                Some("AcoustID did not answer; trying that song again."),
                interval,
            ),
        }
    }

    /// How many songs checked before other names were looked for are read again for them in one
    /// tick.
    const NAMES_BATCH: usize = 50;

    /// Reads songs checked before other names of the album artist, title, album, track and disc
    /// numbers and ids were looked for again, for them alone: copies of a field's own value are
    /// taken off as stray spaces are, the rest join the song's findings. A file changed since is
    /// left to its own full check.
    async fn read_names_again(&self, generation: i32) {
        let pending: Vec<(String, String, String)> = self.store.read(|s| {
            s.songs
                .iter()
                .filter(|(_, kept)| !kept.names_checked)
                .take(Self::NAMES_BATCH)
                .map(|(id, kept)| (id.clone(), kept.path.clone(), kept.stamp.clone()))
                .collect()
        });
        if pending.is_empty() {
            return;
        }
        let read = tokio::task::spawn_blocking(move || {
            pending
                .into_iter()
                .map(|(id, path, stamp)| {
                    let same = !path.is_empty() && stamp_of(&path).is_some_and(|now| now == stamp);
                    let other = same
                        .then(|| TagFile::open(&path).ok().as_ref().and_then(other_names_of))
                        .flatten();
                    (id, stamp, other)
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        for (id, stamp, other) in read {
            let Some((other_album_artists, other_names)) = other else {
                self.record(generation, |s| {
                    if let Some(kept) = s.songs.get_mut(&id) {
                        kept.names_checked = true;
                    }
                });
                continue;
            };
            let Some(kept) = self.store.read(|s| s.songs.get(&id).cloned()) else {
                continue;
            };
            let tags = FileTags {
                title: kept.title.trim().to_string(),
                album: kept.album.trim().to_string(),
                other_album_artists,
                other_names,
                ..FileTags::default()
            };
            let found = tag_audit::other_names_alone(&tags);
            let fixed = self.put_right(&id, &tags, &found, Some(stamp)).await;
            // What was not put right by itself, the names that say something else, waits.
            let (left, written) = match fixed {
                Some((now, _)) => (
                    tag_audit::other_names_alone(&now),
                    Some(stamp_of(&kept.path).unwrap_or_default()),
                ),
                None => (found, None),
            };
            self.record(generation, |s| {
                let Some(kept) = s.songs.get_mut(&id) else {
                    return;
                };
                kept.names_checked = true;
                if let Some(stamp) = written {
                    kept.stamp = stamp;
                    s.names_fixed += 1;
                }
                kept.findings.extend(left.findings.iter().map(|f| KeptFinding {
                    rule: f.rule.name().to_string(),
                    words: f.words.clone(),
                }));
                kept.changes.extend(left.changes.clone());
                if left.found() {
                    kept.sure &= left.sure;
                }
            });
        }
    }

    /// A song with findings whose file is not the one checked any more.
    fn changed_since(&self) -> Option<(String, SweepSong)> {
        let with_findings: Vec<(String, CheckedSong)> = self.store.read(|s| {
            s.songs
                .iter()
                .filter(|(_, kept)| !kept.findings.is_empty())
                .map(|(id, kept)| (id.clone(), kept.clone()))
                .collect()
        });
        with_findings.into_iter().find_map(|(id, kept)| {
            let stamp = stamp_of(&kept.path)?;
            (stamp != kept.stamp).then(|| {
                let song = SweepSong {
                    id: id.clone(),
                    artist: kept.artist.clone(),
                    title: kept.title.clone(),
                    album: kept.album.clone(),
                    path: kept.path.clone(),
                    ..SweepSong::default()
                };
                (id, song)
            })
        })
    }

    /// Checks one song whose file is at `song.path`. `in_order` moves the pass's place to it.
    async fn check(&self, generation: i32, id: &str, song: &SweepSong, in_order: bool) -> Checked {
        let path = song.path.clone();
        // The file as its tags are read: a write later is refused once the file is not this one.
        let read_stamp = stamp_of(&path);
        let read = {
            let path = path.clone();
            tokio::task::spawn_blocking(move || read_tags(&path))
                .await
                .ok()
                .flatten()
        };
        let now = self.clock.now();
        let Some((artist, tags)) = read else {
            // A file whose tags cannot be read is kept as checked, so it is not read again until
            // it changes.
            let stamp = stamp_of(&path).unwrap_or_default();
            let kept = CheckedSong {
                stamp,
                path: path.clone(),
                artist: song.artist.clone(),
                title: song.title.clone(),
                album: song.album.clone(),
                checked_utc: Some(now),
                names_checked: true,
                ..CheckedSong::default()
            };
            self.finish(generation, id, kept, in_order, Fixed::default());
            return Checked::Done;
        };
        let artist = if is_blank(&artist) {
            song.artist.clone()
        } else {
            artist
        };
        let title = if is_blank(&tags.title) {
            song.title.clone()
        } else {
            tags.title.trim().to_string()
        };

        let verdict = self.verifier.verify(&path, Some(&artist), Some(&title)).await;
        if generation != self.generation.load(Ordering::SeqCst) {
            return Checked::Done;
        }
        if verdict.verdict == VerificationVerdict::Inconclusive
            && verdict.reason == InconclusiveReason::LookupFailed
            && self.lookup_failures.fetch_add(1, Ordering::SeqCst) + 1 < Self::MAX_LOOKUP_FAILURES_PER_SONG
        {
            return Checked::Again;
        }
        let lookup_failed = verdict.verdict == VerificationVerdict::Inconclusive
            && verdict.reason == InconclusiveReason::LookupFailed;
        self.lookup_failures.store(0, Ordering::SeqCst);

        let heard = self.heard_as(&verdict, &tags).await;
        let mut found = tag_audit::audit(&tags, heard.as_ref());
        let mut tags = tags;
        let mut fixed = Fixed::default();
        if let Some((written, now)) = self.put_right(id, &tags, &found, read_stamp.clone()).await {
            tags = written;
            found = tag_audit::audit(&tags, heard.as_ref());
            fixed = now;
        }
        let (version, cover) = self
            .check_version(id, &path, &artist, &tags, &verdict, read_stamp.as_deref())
            .await;
        let kept = CheckedSong {
            // The file whose tags were read; after the check's own write, the file as written, so
            // the next pass sees it as checked. A file replaced meanwhile is not the one these
            // answers are about, so it is checked again.
            stamp: if fixed.any() {
                stamp_of(&path).unwrap_or_default()
            } else {
                read_stamp.unwrap_or_default()
            },
            path,
            artist,
            title: tags.title.clone(),
            album: tags.album.clone(),
            recording_id: heard.as_ref().map(|h| h.identity.recording_id.clone()),
            kind: heard.as_ref().map(|h| kind_name(h.identity.kind).to_string()),
            kind_certain: heard.as_ref().is_some_and(|h| h.identity.kind_certain),
            sure: found.sure,
            retry: lookup_failed && heard.is_none(),
            findings: found
                .findings
                .iter()
                .map(|f| KeptFinding {
                    rule: f.rule.name().to_string(),
                    words: f.words.clone(),
                })
                .collect(),
            changes: if found.found() {
                found.changes.clone()
            } else {
                IndexMap::new()
            },
            checked_utc: Some(now),
            names_checked: true,
            version_checked: self.versions.is_some(),
            version,
            cover,
        };
        if let Some(version) = &kept.version {
            info!(
                "Version check: '{} - {}' {}",
                kept.artist, kept.title, version.words
            );
        }
        if found.found() {
            info!(
                "Library tag check: '{} - {}' {}",
                kept.artist,
                kept.title,
                found
                    .findings
                    .iter()
                    .map(|f| f.words.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        // A lookup that failed every time is not an answer. The song is kept as it is now, marked
        // to be tried again next pass: kept, so a file changed since it was last checked is not
        // picked first again and again; marked, so the next pass does not take it as done.
        self.finish(generation, id, kept, in_order, fixed);
        Checked::Done
    }

    /// The version check on one file whose tags were just read, when it is on. A file kept as it
    /// is stays kept while it is the same file and the same thing is found.
    async fn check_version(
        &self,
        id: &str,
        path: &str,
        artist: &str,
        tags: &FileTags,
        verdict: &VerificationResult,
        read_stamp: Option<&str>,
    ) -> (Option<KeptVersion>, Option<KeptCover>) {
        let Some(sources) = self.versions.as_ref() else {
            return (None, None);
        };
        let input = {
            let (path, artist, title, album, recording) = (
                path.to_string(),
                artist.to_string(),
                tags.title.trim().to_string(),
                tags.album.trim().to_string(),
                tags.recording_id.clone(),
            );
            match tokio::task::spawn_blocking(move || {
                VersionInput::read(&path, &artist, &title, &album, recording)
            })
            .await
            {
                Ok(input) => input,
                Err(_) => return (None, None),
            }
        };
        let threshold = if verdict.threshold > 0.0 {
            verdict.threshold
        } else {
            self.settings.current().soulseek.effective_min_score_fraction()
        };
        let facts = version_check::gather(sources.as_ref(), &input, verdict, threshold).await;
        let mut found = octo_core::tagging::version_audit::judge(&facts)
            .map(|finding| version_check::kept_of(&finding, &input));
        let mut cover = version_check::check_cover(sources.as_ref(), &input).await;
        let before = self.store.read(|s| s.songs.get(id).cloned());
        if let Some(before) = before
            && read_stamp.is_some_and(|stamp| stamp == before.stamp)
        {
            if let Some(found) = found.as_mut()
                && before
                    .version
                    .as_ref()
                    .is_some_and(|v| v.kept && v.words == found.words)
            {
                found.kept = true;
            }
            if let Some(cover) = cover.as_mut()
                && before
                    .cover
                    .as_ref()
                    .is_some_and(|c| c.kept && c.words == cover.words)
            {
                cover.kept = true;
            }
        }
        (found, cover)
    }

    /// The songs whose cover is another artist's release's, as the version check last found
    /// them, left out once kept or once their file changed.
    pub fn cover_findings(&self) -> Vec<CoverRow> {
        let mut rows: Vec<CoverRow> = self.store.read(|s| {
            s.songs
                .iter()
                .filter_map(|(id, song)| {
                    let cover = song.cover.clone().filter(|c| !c.kept)?;
                    Some(CoverRow {
                        id: id.clone(),
                        artist: song.artist.clone(),
                        title: song.title.clone(),
                        album: song.album.clone(),
                        path: song.path.clone(),
                        stamp: song.stamp.clone(),
                        cover,
                    })
                })
                .collect()
        });
        rows.retain(|row| stamp_of(&row.path).is_some_and(|stamp| stamp == row.stamp));
        rows.sort_by(|a, b| {
            compare_ordinal(&sort_key(&a.artist), &sort_key(&b.artist))
                .then_with(|| compare_ordinal(&sort_key(&a.title), &sort_key(&b.title)))
        });
        rows
    }

    /// Keeps these songs' covers as they are until their files change. Answers how many.
    pub fn keep_covers(&self, ids: &[String]) -> usize {
        let mut kept = 0;
        self.store.update(|s| {
            for id in ids {
                if let Some(cover) = s.songs.get_mut(id).and_then(|song| song.cover.as_mut())
                    && !cover.kept
                {
                    cover.kept = true;
                    kept += 1;
                }
            }
        });
        self.store.flush();
        kept
    }

    /// Checks one song now, whatever the pace, for "Check now" from the dashboard or an app:
    /// its tags against its audio and its version. The song as it was found, or why it could not
    /// be checked.
    pub async fn check_now(&self, id: &str) -> Result<CheckedSong, String> {
        if !self.verifier.is_ready() {
            return Err("AcoustID is not set up: turn on download verification and add a key.".to_string());
        }
        let listed = match self.songs.lock().clone() {
            Some(songs) => songs.iter().find(|song| song.id == id).cloned(),
            None => None,
        };
        let song = match listed {
            Some(song) => Some(song),
            None => self
                .library
                .songs()
                .await
                .ok_or_else(|| "Octo could not read your library from Navidrome.".to_string())?
                .into_iter()
                .find(|song| song.id == id),
        };
        let song = song.ok_or_else(|| "That song is not in the library.".to_string())?;
        let path = self
            .library
            .locate(&song)
            .ok_or_else(|| "Octo could not find that song's file.".to_string())?;
        let generation = self.generation.load(Ordering::SeqCst);
        let located = SweepSong { path, ..song };
        if self.check(generation, id, &located, false).await == Checked::Again {
            return Err("AcoustID did not answer; try again in a minute.".to_string());
        }
        self.store
            .read(|s| s.songs.get(id).cloned())
            .ok_or_else(|| "The check was started over meanwhile.".to_string())
    }

    /// The library songs a search names, for Check one song now: the one with this id, or every
    /// song whose title and artist hold each word of the search, ignoring case and accents. None
    /// when Navidrome cannot be read.
    pub async fn find_songs(&self, query: &str, limit: usize) -> Option<Vec<SweepSong>> {
        let cached = self.songs.lock().clone();
        let songs = match cached {
            Some(songs) => songs.as_ref().clone(),
            None => self.library.songs().await?,
        };
        if let Some(exact) = songs.iter().find(|song| song.id == query.trim()) {
            return Some(vec![exact.clone()]);
        }
        let words: Vec<String> = query
            .split_whitespace()
            .map(octo_core::common::SongIdentity::key)
            .filter(|word| !word.is_empty())
            .collect();
        if words.is_empty() {
            return Some(Vec::new());
        }
        let mut found: Vec<SweepSong> = songs
            .into_iter()
            .filter(|song| {
                let text = octo_core::common::SongIdentity::key(&format!("{} {}", song.title, song.artist));
                words.iter().all(|word| text.contains(word.as_str()))
            })
            .collect();
        found.sort_by(|a, b| {
            compare_ordinal(&sort_key(&a.artist), &sort_key(&b.artist))
                .then_with(|| compare_ordinal(&sort_key(&a.title), &sort_key(&b.title)))
        });
        found.truncate(limit);
        Some(found)
    }

    /// Keeps these songs as they are: their version findings are not shown again until their
    /// files change. Answers how many were kept.
    pub fn keep_versions(&self, ids: &[String]) -> usize {
        let mut kept = 0;
        self.store.update(|s| {
            for id in ids {
                if let Some(version) = s.songs.get_mut(id).and_then(|song| song.version.as_mut())
                    && !version.kept
                {
                    version.kept = true;
                    kept += 1;
                }
            }
        });
        self.store.flush();
        kept
    }

    /// The songs that are not the version their tags say, as the version check last found them,
    /// left out once kept or once their file changed.
    pub fn version_findings(&self) -> Vec<VersionRow> {
        let mut rows: Vec<VersionRow> = self.store.read(|s| {
            s.songs
                .iter()
                .filter_map(|(id, song)| {
                    let version = song.version.clone().filter(|v| !v.kept)?;
                    Some(VersionRow {
                        id: id.clone(),
                        artist: song.artist.clone(),
                        title: song.title.clone(),
                        album: song.album.clone(),
                        path: song.path.clone(),
                        stamp: song.stamp.clone(),
                        version,
                    })
                })
                .collect()
        });
        rows.retain(|row| stamp_of(&row.path).is_some_and(|stamp| stamp == row.stamp));
        rows.sort_by(|a, b| {
            compare_ordinal(&sort_key(&a.artist), &sort_key(&b.artist))
                .then_with(|| compare_ordinal(&sort_key(&a.title), &sort_key(&b.title)))
                .then_with(|| compare_ordinal(&a.id, &b.id))
        });
        rows
    }

    /// Whether the version check is on.
    pub fn checks_versions(&self) -> bool {
        self.versions.is_some()
    }

    fn finish(&self, generation: i32, id: &str, kept: CheckedSong, in_order: bool, fixed: Fixed) {
        let now = self.clock.now();
        self.record(generation, |s| {
            if in_order {
                s.cursor = id.to_string();
            }
            s.songs.insert(id.to_string(), kept);
            s.last_checked_utc = Some(now);
            if fixed.spaces {
                s.spaces_fixed += 1;
            }
            if fixed.names {
                s.names_fixed += 1;
            }
        });
    }

    /// What the fingerprint said, held for the audit: the recording it decided, whether that
    /// is sure, and which recordings the file's own ids belong to.
    async fn heard_as(&self, verdict: &VerificationResult, tags: &FileTags) -> Option<HeardAs> {
        let identity = Identity::of(Some(verdict))?;
        let recordings: Vec<&AcoustIdRecording> = verdict
            .lookup
            .iter()
            .flat_map(|lookup| lookup.results.iter())
            .flat_map(|result| result.recordings.iter())
            .collect();
        let named = |id: &str| {
            recordings
                .iter()
                .find(|recording| dotnet::eq_ignore_case(&recording.recording_id, id))
                .copied()
        };
        let written = match tags
            .recording_id
            .as_deref()
            .filter(|id| !is_blank(id) && !dotnet::eq_ignore_case(id, &identity.recording_id))
        {
            Some(id) => self.named_recording(id, named(id)).await,
            None => None,
        };

        let track = tags.release_track_id.as_deref().filter(|id| !is_blank(id));
        let release = tags.release_id.as_deref().filter(|id| !is_blank(id));
        let mut track_owner: Option<(String, NamedRecording)> = track.and_then(|track| {
            recordings.iter().find_map(|recording| {
                recording
                    .releases
                    .iter()
                    .any(|r| {
                        r.release_track_id
                            .as_deref()
                            .is_some_and(|t| dotnet::eq_ignore_case(t, track))
                    })
                    .then(|| (recording.recording_id.clone(), listed_recording(recording)))
            })
        });
        let own = named(&identity.recording_id);
        let mut on_release: Option<Option<String>> = release.and_then(|release| {
            own.and_then(|own| {
                own.releases
                    .iter()
                    .find(|r| {
                        r.release_id
                            .as_deref()
                            .is_some_and(|id| dotnet::eq_ignore_case(id, release))
                    })
                    .and_then(|r| r.release_track_id.clone())
                    .map(Some)
            })
        });
        // What AcoustID's answer left open is asked of the release itself, only for a file whose
        // ids could be another recording's.
        let ids_in_doubt = track.is_some()
            || tags
                .recording_id
                .as_deref()
                .is_some_and(|id| !dotnet::eq_ignore_case(id, &identity.recording_id));
        let mut owner_to_ask: Option<String> = None;
        if let (Some(release), Some(releases), true) = (release, &self.releases, ids_in_doubt)
            && (on_release.is_none() || (track.is_some() && track_owner.is_none()))
            && let Some(listed) = releases.tracks(release).await
        {
            if on_release.is_none() {
                on_release = Some(
                    listed
                        .iter()
                        .find(|(_, recording)| dotnet::eq_ignore_case(recording, &identity.recording_id))
                        .map(|(track, _)| track.clone()),
                );
            }
            if track_owner.is_none()
                && let Some(track) = track
                && let Some((_, owner)) = listed.iter().find(|(id, _)| dotnet::eq_ignore_case(id, track))
            {
                owner_to_ask = Some(owner.clone());
            }
        }
        // The track's recording, when only the release named it: what the fingerprint or the
        // music database knows of it, and nothing assumed when neither knows.
        if let Some(owner) = owner_to_ask
            && !dotnet::eq_ignore_case(&owner, &identity.recording_id)
            && let Some(known) = self.named_recording(&owner, named(&owner)).await
        {
            track_owner = Some((owner, known));
        }
        Some(HeardAs {
            sure: verdict.by_length || verdict.sole,
            file_ms: (verdict.duration_seconds > 0).then(|| i64::from(verdict.duration_seconds) * 1000),
            identity,
            written,
            track_owner,
            on_release,
        })
    }

    /// What is known of a recording a file's tags name: the fingerprint's word when it names
    /// it, else the music database's. None when neither can say.
    async fn named_recording(&self, id: &str, listed: Option<&AcoustIdRecording>) -> Option<NamedRecording> {
        if let Some(recording) = listed {
            return Some(listed_recording(recording));
        }
        let facts = self.recordings.as_ref()?.facts(id).await?;
        let bare = AcoustIdRecording::new(id, facts.title.clone(), Vec::<String>::new(), None, None);
        Some(NamedRecording {
            listed: false,
            kind: Some(recording_identity::kind_of(&bare, Some(&facts))),
            length_ms: facts.length_ms.filter(|ms| *ms > 0),
        })
    }

    /// Puts right what needs no one to decide, through the journaled edit path, when library
    /// actions may change files for the library keeper: stray spaces, and other names of the
    /// album artist or another field that only repeat its value. The file's tags afterwards and
    /// what was put right, or None when nothing was written.
    async fn put_right(
        &self,
        id: &str,
        tags: &FileTags,
        found: &TagAudit,
        read_stamp: Option<String>,
    ) -> Option<(FileTags, Fixed)> {
        let changes = found.automatic_changes(tags);
        if changes.is_empty() {
            return None;
        }
        let edits = self.edits.as_ref()?;
        let settings = self.settings.current();
        if settings.library_actions.dry_run {
            return None;
        }
        let user = LibraryReviewSweepWorker::keeper(
            &settings.library_actions,
            settings.subsonic.admin_username.as_deref(),
        )?;
        if edits.refusal(&user).is_some() {
            return None;
        }
        let what = Fixed::of(&changes);
        let outcome = edits.retag(id, &user, changes.clone(), read_stamp).await;
        if outcome.state != "applied" {
            info!(
                "Library tag check could not put right {} on {}: {}",
                what.words(),
                tags.title.trim(),
                outcome.detail.as_deref().unwrap_or(&outcome.state)
            );
            return None;
        }
        info!(
            "Library tag check put right {} on '{}'",
            what.words(),
            tags.title.trim()
        );
        let mut written = tags.clone();
        for (field, value) in changes {
            let value = value.unwrap_or_default();
            match field.as_str() {
                tag_audit::audit_fields::TITLE => written.title = value,
                tag_audit::audit_fields::ALBUM => written.album = value,
                tag_audit::audit_fields::ALBUM_ARTIST_NAMES => written.other_album_artists = None,
                tag_audit::audit_fields::OTHER_NAMES => written.other_names.clear(),
                _ => {}
            }
        }
        Some((written, what))
    }

    fn record(&self, generation: i32, change: impl FnOnce(&mut IdentitySweepState)) {
        self.store.update(|s| {
            if generation == self.generation.load(Ordering::SeqCst) {
                change(s);
            }
        });
    }

    fn hold(&self, state: &str, reason: Option<&str>, wait: Duration) -> Duration {
        *self.hold.lock() = (state.to_string(), reason.map(str::to_string));
        wait
    }

    pub fn set_paused(&self, paused: bool) {
        self.store.update(|s| s.paused = paused);
        self.store.flush();
    }

    /// Forgets every answer and starts a new pass from the first song.
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.songs.lock() = None;
        self.store.update(|s| {
            let paused = s.paused;
            *s = IdentitySweepState {
                paused,
                ..IdentitySweepState::default()
            };
        });
        self.store.flush();
    }

    /// The songs whose tags disagree with their audio, as the check last found them. A song
    /// whose file changed since is left out until it is checked again.
    pub fn findings(&self) -> Vec<TagFinding> {
        self.listed()
            .iter()
            .filter(|finding| stamp_of(&finding.path).is_some_and(|stamp| stamp == finding.stamp))
            .cloned()
            .collect()
    }

    /// Every song with something found, whatever its file is now. Worked out again only when
    /// the state changed: it reads every kept song and every copy beside it.
    fn listed(&self) -> Arc<Vec<TagFinding>> {
        let version = self.store.read_versioned(|_| ()).0;
        if let Some((at, listed)) = self.listed.lock().as_ref()
            && *at == version
        {
            return Arc::clone(listed);
        }
        let (version, listed) = self.store.read_versioned(|s| Arc::new(list_findings(&s.songs)));
        *self.listed.lock() = Some((version, Arc::clone(&listed)));
        listed
    }

    pub fn status(&self) -> IdentitySweepStatus {
        self.status_of(&self.findings())
    }

    /// The findings and the status, with every file looked at once.
    pub fn findings_and_status(&self) -> (Vec<TagFinding>, IdentitySweepStatus) {
        let findings = self.findings();
        let status = self.status_of(&findings);
        (findings, status)
    }

    fn status_of(&self, findings: &[TagFinding]) -> IdentitySweepStatus {
        let settings = self.settings.current();
        let (state, reason) = self.hold.lock().clone();
        self.store.read(|s| IdentitySweepStatus {
            state,
            reason,
            paused: s.paused,
            checked: s.songs.values().filter(|kept| !kept.retry).count(),
            total: s.total,
            pass: s.pass,
            found: findings.len(),
            sure: findings
                .iter()
                .filter(|f| f.sure && !f.changes.is_empty())
                .count(),
            spaces_fixed: s.spaces_fixed,
            names_fixed: s.names_fixed,
            per_hour: settings.library_actions.effective_tag_check_per_hour(),
            last_checked_utc: s.last_checked_utc,
            pass_finished_utc: s.pass_finished_utc,
        })
    }
}

impl version_check::VersionChecker for LibraryIdentitySweep {
    fn check<'a>(&'a self, navidrome_id: &'a str) -> futures::future::BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            if !self.checks_versions() {
                return Err("The version check is not set up on this server.".to_string());
            }
            let song = self.check_now(navidrome_id).await?;
            let name = format!("'{} - {}'", song.artist, song.title);
            Ok(match song.version.as_ref().filter(|version| !version.kept) {
                Some(version) => format!("{name}: {}", version.words),
                None => format!("{name} is the version its tags say."),
            })
        })
    }
}

/// What the check put right by itself on one song.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Fixed {
    spaces: bool,
    names: bool,
}

impl Fixed {
    fn of(changes: &IndexMap<String, Option<String>>) -> Fixed {
        let names = [
            tag_audit::audit_fields::ALBUM_ARTIST_NAMES,
            tag_audit::audit_fields::OTHER_NAMES,
        ]
        .into_iter()
        .filter(|field| changes.contains_key(*field))
        .count();
        Fixed {
            spaces: changes.len() > names,
            names: names > 0,
        }
    }

    fn any(self) -> bool {
        self.spaces || self.names
    }

    fn words(self) -> &'static str {
        match (self.spaces, self.names) {
            (true, true) => "the stray spaces and the tags' other names",
            (false, true) => "the tags' other names",
            _ => "the stray spaces",
        }
    }
}

/// Every kept song with something found, the takes beside each other among them, by artist and
/// title.
fn list_findings(songs: &BTreeMap<String, CheckedSong>) -> Vec<TagFinding> {
    // Only a take that is known is held against another copy.
    let copies: Vec<TakeCopy> = songs
        .iter()
        .filter(|(_, kept)| kept.kind_certain)
        .filter_map(|(id, kept)| {
            Some(TakeCopy {
                id: id.clone(),
                artist: kept.artist.clone(),
                title: kept.title.clone(),
                album: kept.album.clone(),
                kind: kind_parse(kept.kind.as_deref()?)?,
            })
        })
        .collect();
    let beside: HashMap<String, String> = tag_audit::takes_beside(&copies).into_iter().collect();
    let mut found: Vec<TagFinding> = Vec::new();
    for (id, kept) in songs {
        let mut findings: Vec<(AuditRule, String)> = kept
            .findings
            .iter()
            .filter_map(|f| Some((AuditRule::parse(&f.rule)?, f.words.clone())))
            .collect();
        if let Some(words) = beside.get(id) {
            findings.push((AuditRule::Takes, words.clone()));
        }
        if findings.is_empty() {
            continue;
        }
        found.push(TagFinding {
            heard: kept
                .kind
                .as_deref()
                .filter(|_| kept.kind_certain)
                .and_then(kind_parse)
                .map(|k| k.described().to_string()),
            id: id.clone(),
            artist: kept.artist.clone(),
            title: kept.title.clone(),
            album: kept.album.clone(),
            path: kept.path.clone(),
            stamp: kept.stamp.clone(),
            findings,
            changes: kept.changes.clone(),
            sure: kept.sure,
        });
    }
    found.sort_by(|a, b| {
        compare_ordinal(&sort_key(&a.artist), &sort_key(&b.artist))
            .then_with(|| compare_ordinal(&sort_key(&a.title), &sort_key(&b.title)))
            .then_with(|| compare_ordinal(&a.id, &b.id))
    });
    found
}

/// A recording the fingerprint names, as the audit holds it.
fn listed_recording(recording: &AcoustIdRecording) -> NamedRecording {
    NamedRecording {
        listed: true,
        kind: Some(recording_identity::kind_of(recording, None)),
        length_ms: recording.duration_ms.filter(|ms| *ms > 0).or_else(|| {
            recording
                .duration_seconds
                .filter(|s| *s > 0)
                .map(|s| i64::from(s) * 1000)
        }),
    }
}

/// The file's tags the check reads, and its artist; None when it cannot be opened.
fn read_tags(path: &str) -> Option<(String, FileTags)> {
    let mut file = TagFile::open(path).ok()?;
    let artist = file.performers().join("; ");
    let recording_id = extras::read_recording_id(&mut file);
    Some((
        artist,
        FileTags {
            title: file.title().unwrap_or_default(),
            album: file.album().unwrap_or_default(),
            recording_id,
            release_track_id: extras::read_text(&file, TagFields::RELEASE_TRACK_ID),
            release_id: extras::read_text(&file, TagFields::ALBUM_ID),
            release_group_id: extras::read_text(&file, TagFields::RELEASE_GROUP_ID),
            other_album_artists: other_album_artists_of(&file),
            other_names: other_fields_of(&file),
        },
    ))
}

/// The album artist written again under other names the library server reads beside it.
fn other_album_artists_of(file: &TagFile) -> Option<OtherAlbumArtists> {
    StrayAlbumArtists::of(file).map(|stray| OtherAlbumArtists {
        album_artist: stray.album_artist.join("; "),
        names: stray.names(),
        values: stray.values(),
        copies: stray.copies,
    })
}

/// The title, album, track and disc numbers and ids written again under other names the library
/// server reads beside them.
fn other_fields_of(file: &TagFile) -> Vec<OtherNames> {
    field_names::strays(file)
        .into_iter()
        .map(|stray| {
            let mut names: Vec<String> = Vec::new();
            let mut values: Vec<String> = Vec::new();
            for field in &stray.fields {
                names.push(field.name.clone());
                for value in &field.values {
                    if !values.contains(value) {
                        values.push(value.clone());
                    }
                }
            }
            OtherNames {
                field: stray.words.to_string(),
                own: stray.own,
                names,
                values,
                copies: stray.copies,
            }
        })
        .collect()
}

/// Every other name a file holds, of the album artist and of the other fields; None when it
/// holds none.
fn other_names_of(file: &TagFile) -> Option<(Option<OtherAlbumArtists>, Vec<OtherNames>)> {
    let album_artists = other_album_artists_of(file);
    let fields = other_fields_of(file);
    (album_artists.is_some() || !fields.is_empty()).then_some((album_artists, fields))
}

/// The file's size and last write, or None when it is not there.
pub(crate) fn stamp_of(path: &str) -> Option<String> {
    let metadata = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let ticks = metadata
        .modified()
        .map(|modified| dotnet_ticks(DateTime::<Utc>::from(modified)))
        .unwrap_or(0);
    Some(format!("{}:{ticks}", metadata.len()))
}

/// Where the cursor's song is, or the place after it in id order.
fn first_after(songs: &[SweepSong], cursor: &str) -> usize {
    if cursor.is_empty() {
        return 0;
    }
    match songs.binary_search_by(|song| compare_ordinal(&song.id, cursor)) {
        Ok(index) => index + 1,
        Err(index) => index,
    }
}

fn sort_key(text: &str) -> String {
    dotnet::to_lower_invariant(text.trim())
}

/// What the state file keeps for a studio take, which has no title word.
const STUDIO_NAME: &str = "Studio";

/// A kind's name as the state file keeps it: the word its title carries ("Live").
pub fn kind_name(kind: RecordingKind) -> &'static str {
    kind.title_word().unwrap_or(STUDIO_NAME)
}

/// The kind a kept name stands for, read the way a title's bracketed take is read, so every
/// kind with a title word reads back.
pub fn kind_parse(name: &str) -> Option<RecordingKind> {
    if name == STUDIO_NAME {
        return Some(RecordingKind::Studio);
    }
    RecordingKind::named_in(&format!("Song ({name})"))
        .into_iter()
        .find(|kind| kind.title_word() == Some(name))
}

#[cfg(test)]
#[path = "library_identity_sweep_tests.rs"]
mod tests;
