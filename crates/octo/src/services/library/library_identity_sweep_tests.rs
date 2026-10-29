//! The library tag check over a fake library, fingerprint and edit path: it gives each song an
//! identity, keeps what disagrees, takes stray spaces off through the journal, gets out of a
//! download's way, and carries on after a restart.

use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use octo_core::fingerprint::acoust_id_client::{AcoustIdLookup, AcoustIdRelease, AcoustIdResult};
use octo_core::settings::{AppSettings, LibraryActionSettings, SubsonicSettings};

use super::*;
use crate::services::library::library_tag_edits;
use crate::services::test_support::mp3;
use octo_media::tags::KeptIdentityTags;

pub(crate) const STUDIO: &str = "28982738-0000-4000-8000-000000000001";
pub(crate) const LIVE: &str = "eca90222-0000-4000-8000-000000000002";
pub(crate) const INSTRUMENTAL: &str = "3efde39a-0000-4000-8000-000000000003";
pub(crate) const PROMO: &str = "89e29cc2-0000-4000-8000-000000000004";
pub(crate) const PROMO_TRACK: &str = "7a7a7a7a-0000-4000-8000-000000000005";
const SO_WHAT: &str = "50505050-0000-4000-8000-000000000006";

/// A recording AcoustID names, with its length and the releases it is on.
fn recording(id: &str, title: &str, ms: i64, releases: Vec<AcoustIdRelease>) -> AcoustIdRecording {
    let mut found = AcoustIdRecording::new(id, title, vec!["My Chemical Romance".to_string()], None, None);
    found.duration_ms = Some(ms);
    found.duration_seconds = Some((ms / 1000) as i32);
    found.releases = releases;
    found
}

/// A confirmed answer for `id`, decided as said, with these recordings in the lookup.
pub(crate) fn confirmed(
    id: &str,
    kind: RecordingKind,
    by_length: bool,
    sole: bool,
    all: Vec<AcoustIdRecording>,
) -> VerificationResult {
    VerificationResult {
        verdict: VerificationVerdict::Confirmed,
        score: 0.98,
        recording_id: Some(id.to_string()),
        kind: Some(kind),
        by_length,
        sole,
        // As the verifier says it: one recording alone, or one the length decided, is a known
        // take.
        kind_certain: by_length || sole,
        lookup: Some(AcoustIdLookup::new(
            true,
            None,
            vec![AcoustIdResult::new(0.98, all)],
        )),
        ..Default::default()
    }
}

/// Black Parade: studio audio, the exact length deciding it among three takes; the
/// instrumental is the only one on the promo, as the promo's track.
fn parade_heard() -> VerificationResult {
    let promo = AcoustIdRelease {
        release_id: Some(PROMO.into()),
        release_track_id: Some(PROMO_TRACK.into()),
        title: Some("The Black Parade (instrumentals)".into()),
        ..Default::default()
    };
    confirmed(
        STUDIO,
        RecordingKind::Studio,
        true,
        false,
        vec![
            recording(STUDIO, "Welcome to the Black Parade", 311_106, Vec::new()),
            recording(LIVE, "Welcome to the Black Parade", 305_426, Vec::new()),
            recording(
                INSTRUMENTAL,
                "Welcome to the Black Parade (instrumental)",
                309_684,
                vec![promo],
            ),
        ],
    )
}

type Answers = HashMap<String, VerificationResult>;

/// Answers by file name; a file it was not told about is fine and plain.
#[derive(Default)]
struct FakeVerifier {
    not_ready: AtomicBool,
    asked: Mutex<Vec<String>>,
    answers: Mutex<Answers>,
    failures_left: AtomicI32,
    /// Files replaced while their fingerprint is looked up, by name.
    replaced_meanwhile: Mutex<Vec<String>>,
}

impl FakeVerifier {
    fn answering(answers: Vec<(&str, VerificationResult)>) -> Arc<Self> {
        let verifier = FakeVerifier::default();
        *verifier.answers.lock() = answers
            .into_iter()
            .map(|(name, answer)| (name.to_string(), answer))
            .collect();
        Arc::new(verifier)
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().clone()
    }
}

#[async_trait]
impl IReviewSweepVerifier for FakeVerifier {
    fn is_ready(&self) -> bool {
        !self.not_ready.load(Ordering::SeqCst)
    }

    async fn verify(&self, path: &str, _: Option<&str>, _: Option<&str>) -> VerificationResult {
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        self.asked.lock().push(name.clone());
        if self.replaced_meanwhile.lock().contains(&name) {
            replace(Path::new(path));
        }
        if self.failures_left.load(Ordering::SeqCst) > 0 {
            self.failures_left.fetch_sub(1, Ordering::SeqCst);
            return VerificationResult {
                reason: InconclusiveReason::LookupFailed,
                ..Default::default()
            };
        }
        self.answers
            .lock()
            .get(&name)
            .cloned()
            .unwrap_or_else(|| confirmed(SO_WHAT, RecordingKind::Studio, false, true, Vec::new()))
    }

    async fn isrc_agrees(&self, _: &VerificationResult, _: &str) -> Option<bool> {
        None
    }
}

#[derive(Default)]
struct FakeActivity {
    busy: AtomicBool,
}

impl IAcquisitionActivity for FakeActivity {
    fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
}

/// Songs whose ids are their file names, in a temp folder, unless moved.
struct FakeLibrary {
    dir: PathBuf,
    songs: Mutex<Vec<SweepSong>>,
    moved: Mutex<HashMap<String, PathBuf>>,
}

#[async_trait]
impl IdentitySweepLibrary for FakeLibrary {
    async fn songs(&self) -> Option<Vec<SweepSong>> {
        Some(self.songs.lock().clone())
    }

    fn locate(&self, song: &SweepSong) -> Option<String> {
        let path = self
            .moved
            .lock()
            .get(&song.id)
            .cloned()
            .unwrap_or_else(|| self.dir.join(&song.id));
        path.is_file().then(|| path.to_string_lossy().into_owned())
    }
}

/// The same tags in a newer file, as a replace leaves it.
fn replace(path: &Path) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("opens")
        .set_modified(SystemTime::now() + Duration::from_secs(300))
        .expect("the time set");
}

/// Writes through the same tag writer the edit service uses, and keeps who asked for what.
struct FakeEdits {
    dir: PathBuf,
    asked: Mutex<Vec<(String, String, IndexMap<String, Option<String>>)>>,
}

#[async_trait]
impl IdentitySweepEdits for FakeEdits {
    fn refusal(&self, _: &str) -> Option<String> {
        None
    }

    async fn retag(
        &self,
        id: &str,
        user: &str,
        changes: IndexMap<String, Option<String>>,
        expected_stamp: Option<String>,
    ) -> LibraryEditOutcome {
        self.asked
            .lock()
            .push((id.to_string(), user.to_string(), changes.clone()));
        let path = self.dir.join(id);
        if expected_stamp.is_some() && stamp_of(&path.to_string_lossy()) != expected_stamp {
            return LibraryEditOutcome::new("skipped", "changed since");
        }
        let written = library_tag_edits::write(&path, &changes);
        LibraryEditOutcome::new(if written.changed { "applied" } else { "skipped" }, "")
    }
}

struct Setup {
    dir: tempfile::TempDir,
    library: Arc<FakeLibrary>,
    edits: Arc<FakeEdits>,
    activity: Arc<FakeActivity>,
    versions: Option<Arc<dyn VersionSources>>,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp dir");
        Setup {
            library: Arc::new(FakeLibrary {
                dir: dir.path().to_path_buf(),
                songs: Mutex::new(Vec::new()),
                moved: Mutex::new(HashMap::new()),
            }),
            edits: Arc::new(FakeEdits {
                dir: dir.path().to_path_buf(),
                asked: Mutex::new(Vec::new()),
            }),
            activity: Arc::new(FakeActivity::default()),
            versions: None,
            dir,
        }
    }

    fn state_path(&self) -> PathBuf {
        self.dir.path().join(".state").join("identity-sweep.json")
    }

    /// An MP3 named `id` with these tags, listed in the library.
    fn song(&self, id: &str, title: &str, album: &str, ids: &[(&str, &str)]) {
        let path = self.dir.path().join(id);
        std::fs::write(&path, mp3()).expect("written");
        let mut file = TagFile::open(&path).expect("opens");
        file.set_title(Some(title));
        file.set_performers(&["My Chemical Romance".to_string()]);
        file.set_album(Some(album));
        for (field, value) in ids {
            match *field {
                "recording" => extras::set_recording_id(&mut file, value),
                "track" => extras::set_release_track_id(&mut file, Some(value)),
                "release" => extras::set_text(&mut file, TagFields::ALBUM_ID, Some(value)),
                _ => unreachable!(),
            }
        }
        file.save().expect("saved");
        self.library.songs.lock().push(SweepSong {
            id: id.to_string(),
            artist: "My Chemical Romance".into(),
            title: title.trim().to_string(),
            album: album.trim().to_string(),
            ..SweepSong::default()
        });
    }

    fn sweep(&self, verifier: Arc<FakeVerifier>, actions: LibraryActionSettings) -> LibraryIdentitySweep {
        let store = Arc::new(IdentitySweepStore::new(Some(self.state_path())));
        let settings = Arc::new(SettingsStore::for_tests(AppSettings {
            library_actions: actions,
            subsonic: SubsonicSettings {
                admin_username: Some("bob".into()),
                ..Default::default()
            },
            ..Default::default()
        }));
        LibraryIdentitySweep::new(
            LibraryIdentitySweepParts {
                store,
                verifier,
                activity: self.activity.clone(),
                library: self.library.clone(),
                edits: Some(self.edits.clone()),
                releases: None,
                recordings: None,
                versions: self.versions.clone(),
                settings,
            },
            Clock::system(),
        )
    }
}

fn actions(per_hour: i32) -> LibraryActionSettings {
    LibraryActionSettings {
        enabled: true,
        dry_run: false,
        tag_check_per_hour: per_hour,
        allowed_users: vec!["bob".into()],
        ..Default::default()
    }
}

fn rules(finding: &TagFinding) -> Vec<AuditRule> {
    finding.findings.iter().map(|(rule, _)| *rule).collect()
}

async fn ticks(sweep: &LibraryIdentitySweep, count: usize) {
    for _ in 0..count {
        sweep.tick().await;
    }
}

#[tokio::test]
async fn a_pass_gives_every_song_an_identity_and_keeps_what_disagrees() {
    let setup = Setup::new();
    setup.song(
        "a-parade.mp3",
        "Welcome to the Black Parade",
        "The Black Parade (instrumentals)",
        &[("recording", STUDIO), ("track", PROMO_TRACK), ("release", PROMO)],
    );
    setup.song("b-fine.mp3", "Teenagers", "The Black Parade", &[]);
    setup.song(
        "c-so-what.mp3",
        "So What",
        "A Muse In Her Feelings (Instrumentals)",
        &[],
    );
    let verifier = FakeVerifier::answering(vec![("a-parade.mp3", parade_heard())]);
    let sweep = setup.sweep(verifier.clone(), actions(360));

    ticks(&sweep, 4).await;

    assert_eq!(verifier.asked(), ["a-parade.mp3", "b-fine.mp3", "c-so-what.mp3"]);
    let status = sweep.status();
    assert_eq!((status.checked, status.total, status.pass), (3, 3, 2));
    assert_eq!(status.state, "Done");
    let found = sweep.findings();
    assert_eq!(found.len(), 2);
    let parade = found.iter().find(|f| f.id == "a-parade.mp3").expect("the parade");
    assert_eq!(rules(parade), [AuditRule::Album, AuditRule::Ids]);
    assert!(parade.sure);
    assert_eq!(parade.heard.as_deref(), Some("the studio recording"));
    assert_eq!(
        parade.changes.get("album"),
        Some(&Some("The Black Parade".to_string()))
    );
    // The album is renamed, so the promo's ids come off with it: left on, they would keep the
    // song on the promo's album.
    assert_eq!(parade.changes.get("releaseTrackId"), Some(&None));
    assert_eq!(parade.changes.get("releaseId"), Some(&None));
    let so_what = found.iter().find(|f| f.id == "c-so-what.mp3").expect("so what");
    assert_eq!(rules(so_what), [AuditRule::Album]);
    assert_eq!(
        so_what.changes.get("recordingId"),
        Some(&Some(SO_WHAT.to_string()))
    );
    // Each song's identity is kept, the fine one too.
    let kept = sweep
        .store()
        .read(|s| s.songs.get("b-fine.mp3").cloned())
        .expect("kept");
    assert_eq!(kept.recording_id.as_deref(), Some(SO_WHAT));
    assert!(kept.findings.is_empty());
    assert_eq!(status.found, 2);
    assert_eq!(status.sure, 2);
}

/// The promo as MusicBrainz lists it: the instrumental only.
struct PromoTracks {
    asked: Mutex<Vec<String>>,
}

#[async_trait]
impl ReleaseTracks for PromoTracks {
    async fn tracks(&self, release_id: &str) -> Option<Vec<(String, String)>> {
        self.asked.lock().push(release_id.to_string());
        (release_id == PROMO).then(|| vec![(PROMO_TRACK.to_string(), INSTRUMENTAL.to_string())])
    }
}

#[tokio::test]
async fn a_release_the_audio_is_not_on_loses_every_id_it_gave() {
    let setup = Setup::new();
    setup.song(
        "a-parade.mp3",
        "Welcome to the Black Parade",
        "The Black Parade (instrumentals)",
        &[("recording", STUDIO), ("track", PROMO_TRACK), ("release", PROMO)],
    );
    setup.song("b-fine.mp3", "Teenagers", "The Black Parade", &[]);
    let releases = Arc::new(PromoTracks {
        asked: Mutex::new(Vec::new()),
    });
    let mut sweep = setup.sweep(
        FakeVerifier::answering(vec![("a-parade.mp3", parade_heard())]),
        actions(360),
    );
    sweep.releases = Some(releases.clone());
    ticks(&sweep, 3).await;

    // Asked once, for the file with ids; the file with none needs no release.
    assert_eq!(*releases.asked.lock(), [PROMO]);
    let parade = sweep
        .findings()
        .into_iter()
        .find(|f| f.id == "a-parade.mp3")
        .expect("found");
    assert_eq!(parade.changes.get("releaseTrackId"), Some(&None));
    assert_eq!(parade.changes.get("releaseId"), Some(&None));
    assert_eq!(parade.changes.get("recordingId"), None);
}

#[tokio::test]
async fn a_restart_carries_on_from_the_song_it_reached() {
    let setup = Setup::new();
    for id in ["a.mp3", "b.mp3", "c.mp3"] {
        setup.song(id, "Teenagers", "The Black Parade", &[]);
    }
    let first = FakeVerifier::answering(Vec::new());
    {
        let sweep = setup.sweep(first.clone(), actions(360));
        ticks(&sweep, 2).await;
        sweep.store().flush();
    }
    assert_eq!(first.asked(), ["a.mp3", "b.mp3"]);

    let second = FakeVerifier::answering(Vec::new());
    let sweep = setup.sweep(second.clone(), actions(360));
    ticks(&sweep, 3).await;
    assert_eq!(second.asked(), ["c.mp3"]);
    assert_eq!(sweep.status().checked, 3);
}

#[tokio::test]
async fn it_waits_while_a_download_runs_and_does_nothing_when_off_or_paused() {
    let setup = Setup::new();
    setup.song("a.mp3", "Teenagers", "The Black Parade", &[]);
    let verifier = FakeVerifier::answering(Vec::new());

    let off = setup.sweep(verifier.clone(), actions(0));
    off.tick().await;
    assert_eq!(off.status().state, "Off");

    let sweep = setup.sweep(verifier.clone(), actions(60));
    setup.activity.busy.store(true, Ordering::SeqCst);
    assert_eq!(sweep.tick().await, LibraryIdentitySweep::BUSY_CHECK);
    assert_eq!(sweep.status().state, "Waiting");

    setup.activity.busy.store(false, Ordering::SeqCst);
    sweep.set_paused(true);
    sweep.tick().await;
    assert_eq!(sweep.status().state, "Paused");

    verifier.not_ready.store(true, Ordering::SeqCst);
    sweep.set_paused(false);
    sweep.tick().await;
    assert!(sweep.status().reason.unwrap_or_default().contains("AcoustID"));
    assert!(verifier.asked().is_empty());

    verifier.not_ready.store(false, Ordering::SeqCst);
    // One song an hour is one a minute's worth of waiting, a minute apart at 60.
    assert_eq!(sweep.tick().await, Duration::from_secs(60));
    assert_eq!(verifier.asked(), ["a.mp3"]);
}

#[tokio::test]
async fn stray_spaces_are_taken_off_through_the_journaled_edit_path() {
    let setup = Setup::new();
    setup.song("a.mp3", "Teenagers ", "The Black Parade", &[]);
    let sweep = setup.sweep(FakeVerifier::answering(Vec::new()), actions(360));
    sweep.tick().await;

    let asked = setup.edits.asked.lock().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, "bob");
    assert_eq!(asked[0].2.get("title"), Some(&Some("Teenagers".to_string())));
    let file = TagFile::open(setup.dir.path().join("a.mp3")).expect("opens");
    assert_eq!(file.title().as_deref(), Some("Teenagers"));
    assert!(sweep.findings().is_empty());
    assert_eq!(sweep.status().spaces_fixed, 1);
    // The write is the file as checked: it is not checked again.
    assert_eq!(sweep.tick().await, LibraryIdentitySweep::IDLE_CHECK);
}

#[tokio::test]
async fn in_rehearsal_stray_spaces_are_only_listed() {
    let setup = Setup::new();
    setup.song("a.mp3", "Teenagers ", "The Black Parade", &[]);
    let sweep = setup.sweep(
        FakeVerifier::answering(Vec::new()),
        LibraryActionSettings {
            dry_run: true,
            ..actions(360)
        },
    );
    sweep.tick().await;
    assert!(setup.edits.asked.lock().is_empty());
    let found = sweep.findings();
    assert_eq!(found.len(), 1);
    assert_eq!(rules(&found[0]), [AuditRule::Spaces]);
    assert_eq!(
        found[0].changes.get("title"),
        Some(&Some("Teenagers".to_string()))
    );
}

#[tokio::test]
async fn a_file_changed_since_leaves_its_finding_and_is_checked_again_first() {
    let setup = Setup::new();
    setup.song("a.mp3", "So What", "A Muse In Her Feelings (Instrumentals)", &[]);
    setup.song("b.mp3", "Teenagers", "The Black Parade", &[]);
    let verifier = FakeVerifier::answering(Vec::new());
    let sweep = setup.sweep(verifier.clone(), actions(360));
    ticks(&sweep, 3).await;
    assert_eq!(sweep.findings().len(), 1);
    assert_eq!(sweep.status().state, "Done");

    // A fix writes the plain album; until the song is checked again it is not listed.
    let path = setup.dir.path().join("a.mp3");
    let mut changes = IndexMap::new();
    changes.insert("album".to_string(), Some("A Muse In Her Feelings".to_string()));
    assert!(library_tag_edits::write(&path, &changes).changed);
    assert!(sweep.findings().is_empty());

    sweep.tick().await;
    assert_eq!(verifier.asked(), ["a.mp3", "b.mp3", "a.mp3"]);
    assert!(sweep.findings().is_empty());
    let kept = sweep
        .store()
        .read(|s| s.songs.get("a.mp3").cloned())
        .expect("kept");
    assert_eq!(kept.album, "A Muse In Her Feelings");
}

#[tokio::test]
async fn a_lookup_that_fails_is_tried_again_then_left_for_the_next_pass() {
    let setup = Setup::new();
    setup.song("a.mp3", "Teenagers", "The Black Parade", &[]);
    setup.song("b.mp3", "Teenagers", "The Black Parade", &[]);
    let verifier = FakeVerifier::answering(Vec::new());
    verifier.failures_left.store(3, Ordering::SeqCst);
    let sweep = setup.sweep(verifier.clone(), actions(360));
    ticks(&sweep, 2).await;
    assert_eq!(sweep.status().state, "Waiting");
    sweep.tick().await;
    sweep.tick().await;
    assert_eq!(verifier.asked(), ["a.mp3", "a.mp3", "a.mp3", "b.mp3"]);
    // The song AcoustID never answered for is kept to be tried again, and not counted as checked.
    let kept = sweep
        .store()
        .read(|s| s.songs.get("a.mp3").cloned())
        .expect("kept");
    assert!(kept.retry);
    assert_eq!(kept.recording_id, None);
    assert!(sweep.store().read(|s| !s.songs["b.mp3"].retry));
    assert_eq!(sweep.status().checked, 1);

    // The next pass asks again for it alone.
    sweep.tick().await;
    sweep.store().update(|s| s.next_pass_utc = datetime::min_value());
    ticks(&sweep, 2).await;
    assert_eq!(verifier.asked(), ["a.mp3", "a.mp3", "a.mp3", "b.mp3", "a.mp3"]);
    assert!(sweep.store().read(|s| !s.songs["a.mp3"].retry));
    assert_eq!(sweep.status().checked, 2);
}

#[tokio::test]
async fn a_changed_song_whose_lookup_keeps_failing_does_not_hold_up_the_pass() {
    let setup = Setup::new();
    setup.song("a.mp3", "So What", "A Muse In Her Feelings (Instrumentals)", &[]);
    setup.song("b.mp3", "Teenagers", "The Black Parade", &[]);
    setup.song("c.mp3", "Teenagers", "The Black Parade", &[]);
    let verifier = FakeVerifier::answering(Vec::new());
    let sweep = setup.sweep(verifier.clone(), actions(360));
    sweep.tick().await;
    assert_eq!(sweep.findings().len(), 1);

    // The file changes, and AcoustID stops answering for good.
    replace(&setup.dir.path().join("a.mp3"));
    verifier.failures_left.store(i32::MAX, Ordering::SeqCst);
    ticks(&sweep, 3).await;
    assert_eq!(verifier.asked(), ["a.mp3", "a.mp3", "a.mp3", "a.mp3"]);

    // Its new file is kept, to be tried again next pass, and the pass moves on.
    assert!(sweep.store().read(|s| s.songs["a.mp3"].retry));
    verifier.failures_left.store(0, Ordering::SeqCst);
    ticks(&sweep, 2).await;
    assert_eq!(
        verifier.asked(),
        ["a.mp3", "a.mp3", "a.mp3", "a.mp3", "b.mp3", "c.mp3"]
    );
}

#[tokio::test]
async fn a_song_with_stray_spaces_whose_lookup_failed_is_heard_again_next_pass() {
    let setup = Setup::new();
    setup.song("a.mp3", "Teenagers ", "The Black Parade", &[]);
    let verifier = FakeVerifier::answering(Vec::new());
    verifier.failures_left.store(3, Ordering::SeqCst);
    let sweep = setup.sweep(
        verifier.clone(),
        LibraryActionSettings {
            dry_run: true,
            ..actions(360)
        },
    );
    ticks(&sweep, 4).await;
    let found = sweep.findings();
    assert_eq!(found.len(), 1);
    assert_eq!(rules(&found[0]), [AuditRule::Spaces]);
    assert!(sweep.store().read(|s| s.songs["a.mp3"].retry));

    sweep.store().update(|s| s.next_pass_utc = datetime::min_value());
    ticks(&sweep, 2).await;
    assert_eq!(verifier.asked(), ["a.mp3", "a.mp3", "a.mp3", "a.mp3"]);
    let kept = sweep
        .store()
        .read(|s| s.songs.get("a.mp3").cloned())
        .expect("kept");
    assert!(!kept.retry);
    assert_eq!(kept.recording_id.as_deref(), Some(SO_WHAT));
}

#[tokio::test]
async fn stray_spaces_are_not_written_over_a_file_replaced_while_it_was_checked() {
    let setup = Setup::new();
    setup.song("a.mp3", "Teenagers ", "The Black Parade", &[]);
    let verifier = FakeVerifier::answering(Vec::new());
    verifier.replaced_meanwhile.lock().push("a.mp3".into());
    let sweep = setup.sweep(verifier.clone(), actions(360));
    sweep.tick().await;

    assert_eq!(setup.edits.asked.lock().len(), 1);
    let file = TagFile::open(setup.dir.path().join("a.mp3")).expect("opens");
    assert_eq!(file.title().as_deref(), Some("Teenagers "));
    assert_eq!(sweep.status().spaces_fixed, 0);
    // What was found is about the file before the replace: not listed, and checked again first.
    assert!(sweep.findings().is_empty());
    verifier.replaced_meanwhile.lock().clear();
    sweep.tick().await;
    assert_eq!(verifier.asked(), ["a.mp3", "a.mp3"]);
    let file = TagFile::open(setup.dir.path().join("a.mp3")).expect("opens");
    assert_eq!(file.title().as_deref(), Some("Teenagers"));
    assert_eq!(sweep.status().spaces_fixed, 1);
}

#[tokio::test]
async fn a_moved_file_keeps_its_answer_at_its_new_place() {
    let setup = Setup::new();
    setup.song("a.mp3", "So What", "A Muse In Her Feelings (Instrumentals)", &[]);
    let verifier = FakeVerifier::answering(Vec::new());
    let sweep = setup.sweep(verifier.clone(), actions(360));
    ticks(&sweep, 2).await;
    assert_eq!(sweep.findings().len(), 1);

    // Moved: same size and last write, another folder.
    let folder = setup.dir.path().join("moved");
    std::fs::create_dir_all(&folder).expect("a folder");
    let moved = folder.join("a.mp3");
    std::fs::rename(setup.dir.path().join("a.mp3"), &moved).expect("moved");
    setup.library.moved.lock().insert("a.mp3".into(), moved.clone());
    assert!(sweep.findings().is_empty());

    sweep.store().update(|s| s.next_pass_utc = datetime::min_value());
    ticks(&sweep, 2).await;
    let found = sweep.findings();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, moved.to_string_lossy());
    // Its answer still holds: the file was not looked up again.
    assert_eq!(verifier.asked(), ["a.mp3"]);
}

#[tokio::test]
async fn a_take_that_was_only_the_likelier_pick_is_kept_but_never_said() {
    let setup = Setup::new();
    setup.song(
        "live.mp3",
        "Welcome to the Black Parade",
        "The Black Parade Is Dead!",
        &[],
    );
    setup.song(
        "studio.mp3",
        "Welcome to the Black Parade",
        "The Black Parade",
        &[],
    );
    let mut guess = confirmed(LIVE, RecordingKind::Live, false, false, Vec::new());
    guess.kind_certain = false;
    let verifier = FakeVerifier::answering(vec![
        ("live.mp3", guess),
        (
            "studio.mp3",
            confirmed(STUDIO, RecordingKind::Studio, true, false, Vec::new()),
        ),
    ]);
    let sweep = setup.sweep(verifier, actions(360));
    ticks(&sweep, 3).await;

    let kept = sweep
        .store()
        .read(|s| s.songs.get("live.mp3").cloned())
        .expect("kept");
    assert_eq!(kept.kind.as_deref(), Some("Live"));
    assert!(!kept.kind_certain);
    // Neither the title nor the copy beside it is held against a take that was only likely.
    assert!(sweep.findings().is_empty(), "{:?}", sweep.findings());
}

/// What MusicBrainz says of the album's own recording of the studio take.
struct AlbumRecording {
    kind_note: Option<&'static str>,
}

#[async_trait]
impl RecordingFactsLookup for AlbumRecording {
    async fn facts(&self, recording_id: &str) -> Option<RecordingFacts> {
        (recording_id == ALBUM_RECORDING).then(|| RecordingFacts {
            recording_id: recording_id.to_string(),
            title: "Welcome to the Black Parade".into(),
            disambiguation: self.kind_note.map(str::to_string),
            length_ms: Some(311_000),
            releases: Vec::new(),
        })
    }
}

const ALBUM_RECORDING: &str = "a1a1a1a1-0000-4000-8000-000000000007";

#[tokio::test]
async fn a_recording_id_acoustid_does_not_list_is_left_alone_unless_musicbrainz_rules_it_out() {
    for (note, wrong) in [(None, false), (Some("live, 2006-10-22"), true)] {
        let setup = Setup::new();
        setup.song(
            "a-parade.mp3",
            "Welcome to the Black Parade",
            "The Black Parade",
            &[
                ("recording", ALBUM_RECORDING),
                ("release", "r-parade"),
                ("track", "t-parade"),
            ],
        );
        let mut sweep = setup.sweep(
            FakeVerifier::answering(vec![("a-parade.mp3", parade_heard())]),
            actions(360),
        );
        sweep.recordings = Some(Arc::new(AlbumRecording { kind_note: note }));
        ticks(&sweep, 2).await;
        let found = sweep.findings();
        if !wrong {
            assert!(found.is_empty(), "{found:?}");
            continue;
        }
        assert_eq!(found.len(), 1);
        assert_eq!(rules(&found[0]), [AuditRule::Ids]);
        assert!(found[0].findings[0].1.contains("a live recording"));
        assert_eq!(
            found[0].changes.get("recordingId"),
            Some(&Some(STUDIO.to_string()))
        );
        // The song stays on its album: its release ids are not touched.
        assert_eq!(found[0].changes.get("releaseId"), None);
        assert_eq!(found[0].changes.get("releaseTrackId"), None);
    }
}

#[tokio::test]
async fn the_findings_are_worked_out_again_only_when_the_state_changes() {
    let setup = Setup::new();
    setup.song("a.mp3", "So What", "A Muse In Her Feelings (Instrumentals)", &[]);
    let sweep = setup.sweep(FakeVerifier::answering(Vec::new()), actions(360));
    ticks(&sweep, 2).await;
    let first = sweep.listed();
    assert!(Arc::ptr_eq(&first, &sweep.listed()));
    let (findings, status) = sweep.findings_and_status();
    assert_eq!((findings.len(), status.found), (1, 1));
    assert!(Arc::ptr_eq(&first, &sweep.listed()));

    sweep.reset();
    assert!(!Arc::ptr_eq(&first, &sweep.listed()));
    assert!(sweep.findings().is_empty());
}

#[tokio::test]
async fn a_live_copy_titled_plainly_beside_the_studio_copy_is_found() {
    let setup = Setup::new();
    setup.song(
        "live.mp3",
        "Welcome to the Black Parade",
        "The Black Parade Is Dead!",
        &[],
    );
    setup.song(
        "studio.mp3",
        "Welcome to the Black Parade",
        "The Black Parade",
        &[],
    );
    let verifier = FakeVerifier::answering(vec![
        (
            "live.mp3",
            confirmed(LIVE, RecordingKind::Live, true, false, Vec::new()),
        ),
        (
            "studio.mp3",
            confirmed(STUDIO, RecordingKind::Studio, true, false, Vec::new()),
        ),
    ]);
    let sweep = setup.sweep(verifier, actions(360));
    ticks(&sweep, 3).await;

    let found = sweep.findings();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "live.mp3");
    assert_eq!(rules(&found[0]), [AuditRule::Title, AuditRule::Takes]);
    assert_eq!(
        found[0].changes.get("title"),
        Some(&Some("Welcome to the Black Parade (Live)".to_string()))
    );
    assert!(found[0].findings[1].1.contains("'The Black Parade'"));
}

#[tokio::test]
async fn start_over_forgets_every_answer() {
    let setup = Setup::new();
    setup.song("a.mp3", "So What", "A Muse In Her Feelings (Instrumentals)", &[]);
    let verifier = FakeVerifier::answering(Vec::new());
    let sweep = setup.sweep(verifier.clone(), actions(360));
    ticks(&sweep, 2).await;
    assert_eq!(sweep.findings().len(), 1);
    sweep.reset();
    assert!(sweep.findings().is_empty());
    assert_eq!(sweep.status().checked, 0);
    sweep.tick().await;
    assert_eq!(verifier.asked(), ["a.mp3", "a.mp3"]);
}

#[test]
fn a_state_file_that_cannot_be_read_is_set_aside() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("identity-sweep.json");
    std::fs::write(&path, "not json").expect("written");
    let store = IdentitySweepStore::new(Some(path.clone()));
    assert!(store.read(|s| s.songs.is_empty()));
    assert!(!path.exists());
    store.update(|s| s.total = 4);
    store.flush();
    assert_eq!(IdentitySweepStore::new(Some(path)).read(|s| s.total), 4);
}

#[test]
fn kind_names_read_back() {
    for kind in [
        RecordingKind::Studio,
        RecordingKind::Live,
        RecordingKind::Instrumental,
    ] {
        assert_eq!(kind_parse(kind_name(kind)), Some(kind));
    }
    assert_eq!(kind_name(RecordingKind::Live), "Live");
    assert_eq!(kind_parse("Unknown"), None);
}

/// The song's album artist is `own`, written again under the other names given.
fn spell_album_artist(path: &Path, own: &str, others: &[(&str, &str)]) {
    let mut file = TagFile::open(path).expect("opens");
    file.set_album_artists(&[own.to_string()]);
    for (name, value) in others {
        extras::set_multi_value(&mut file, name, &[value.to_string()]);
    }
    file.save().expect("saved");
}

fn album_artist_keys(path: &Path) -> Vec<(String, Vec<String>)> {
    let view = KeptIdentityTags::navidrome_view(&TagFile::open(path).expect("opens"));
    view.into_iter()
        .filter(|(key, _)| key.starts_with("album") && key.contains("artist"))
        .collect()
}

#[tokio::test]
async fn the_album_artist_written_again_under_other_names_is_taken_off_through_the_journal() {
    let setup = Setup::new();
    setup.song("a.mp3", "industry plant", "industry plant", &[]);
    let path = setup.dir.path().join("a.mp3");
    spell_album_artist(
        &path,
        "miki",
        &[
            ("ALBUM ARTIST", "Miki"),
            ("ALBUM_ARTIST", "Miki"),
            ("ALBUMARTISTS", "Miki"),
            ("ALBUM_ARTISTS", "Miki"),
        ],
    );
    let sweep = setup.sweep(FakeVerifier::answering(Vec::new()), actions(360));
    sweep.tick().await;

    let asked = setup.edits.asked.lock().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, "bob");
    assert_eq!(asked[0].2.get("albumArtistNames"), Some(&None));
    assert_eq!(
        album_artist_keys(&path),
        [("albumartist".to_string(), vec!["miki".to_string()])]
    );
    assert!(sweep.findings().is_empty());
    let status = sweep.status();
    assert_eq!((status.names_fixed, status.spaces_fixed), (1, 0));
    // The write is the file as checked: it is not checked again.
    assert_eq!(sweep.tick().await, LibraryIdentitySweep::IDLE_CHECK);
}

#[tokio::test]
async fn another_album_artist_under_another_name_waits_for_a_person() {
    let setup = Setup::new();
    setup.song("a.mp3", "Stronger", "Graduation", &[]);
    let path = setup.dir.path().join("a.mp3");
    spell_album_artist(&path, "Ye", &[("ALBUM ARTIST", "Kanye West")]);
    let sweep = setup.sweep(FakeVerifier::answering(Vec::new()), actions(360));
    sweep.tick().await;

    assert!(setup.edits.asked.lock().is_empty());
    let found = sweep.findings();
    assert_eq!(found.len(), 1);
    assert_eq!(rules(&found[0]), [AuditRule::AlbumArtistNames]);
    assert!(!found[0].sure);
    assert_eq!(found[0].changes.get("albumArtistNames"), Some(&None));

    // The fix Library health sends keeps the file's own album artist.
    assert!(library_tag_edits::write(&path, &found[0].changes).changed);
    assert_eq!(
        album_artist_keys(&path),
        [("albumartist".to_string(), vec!["Ye".to_string()])]
    );
}

/// The song as a check made before the album artist's other names were looked for kept it.
fn checked_before_names(sweep: &LibraryIdentitySweep, setup: &Setup, id: &str, title: &str) {
    let path = setup.dir.path().join(id);
    let stamp = stamp_of(&path.to_string_lossy()).expect("a stamp");
    sweep.store().update(|s| {
        s.songs.insert(
            id.to_string(),
            CheckedSong {
                stamp,
                path: path.to_string_lossy().into_owned(),
                artist: "My Chemical Romance".into(),
                title: title.into(),
                album: title.into(),
                recording_id: Some(STUDIO.into()),
                checked_utc: Some(Utc::now()),
                ..CheckedSong::default()
            },
        );
    });
}

#[tokio::test]
async fn a_song_checked_before_is_read_again_for_the_album_artists_other_names_alone() {
    let setup = Setup::new();
    setup.song("a.mp3", "industry plant", "industry plant", &[]);
    let path = setup.dir.path().join("a.mp3");
    spell_album_artist(
        &path,
        "miki",
        &[("ALBUM ARTIST", "Miki"), ("ALBUMARTISTS", "Miki")],
    );
    let verifier = FakeVerifier::answering(Vec::new());
    let sweep = setup.sweep(verifier.clone(), actions(360));
    checked_before_names(&sweep, &setup, "a.mp3", "industry plant");

    sweep.tick().await;
    // Taken off without listening to it again.
    assert!(verifier.asked().is_empty());
    let asked = setup.edits.asked.lock().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].2.get("albumArtistNames"), Some(&None));
    assert_eq!(
        album_artist_keys(&path),
        [("albumartist".to_string(), vec!["miki".to_string()])]
    );
    assert_eq!(sweep.status().names_fixed, 1);
    let kept = sweep.store().read(|s| s.songs["a.mp3"].clone());
    assert!(kept.names_checked);
    assert_eq!(kept.recording_id.as_deref(), Some(STUDIO));
    // The file as written is the one checked: the pass does not listen to it again.
    ticks(&sweep, 2).await;
    assert!(verifier.asked().is_empty());
    assert_eq!(setup.edits.asked.lock().len(), 1);
}

#[tokio::test]
async fn in_rehearsal_a_song_checked_before_lists_its_album_artists_other_names() {
    let setup = Setup::new();
    setup.song("a.mp3", "industry plant", "industry plant", &[]);
    let path = setup.dir.path().join("a.mp3");
    spell_album_artist(&path, "miki", &[("ALBUM ARTIST", "Miki")]);
    let sweep = setup.sweep(
        FakeVerifier::answering(Vec::new()),
        LibraryActionSettings {
            dry_run: true,
            ..actions(360)
        },
    );
    checked_before_names(&sweep, &setup, "a.mp3", "industry plant");
    sweep.tick().await;
    assert!(setup.edits.asked.lock().is_empty());
    let found = sweep.findings();
    assert_eq!(found.len(), 1);
    assert_eq!(rules(&found[0]), [AuditRule::AlbumArtistNames]);
    assert_eq!(found[0].changes.get("albumArtistNames"), Some(&None));
    // Read once: the next tick does not add it again.
    sweep.tick().await;
    assert_eq!(rules(&sweep.findings()[0]), [AuditRule::AlbumArtistNames]);
}

// ---- the version check ----------------------------------------------------------------------

/// The music database's recordings of "Teenagers", every version.
struct TeenagersCatalog;

#[async_trait]
impl VersionSources for TeenagersCatalog {
    async fn song_recordings(
        &self,
        _: &str,
        _: &str,
    ) -> Option<Vec<octo_core::tagging::version_audit::KnownRecording>> {
        Some(vec![
            octo_core::tagging::version_audit::KnownRecording::new("teen", "Teenagers", Some(161_000)),
            octo_core::tagging::version_audit::KnownRecording::new(
                "robot",
                "Teenagers (Robot remix)",
                Some(203_000),
            ),
        ])
    }

    async fn isrc_entry(&self, _: &str) -> Option<octo_core::tagging::version_audit::IsrcEntry> {
        None
    }

    async fn recording(&self, _: &str) -> Option<octo_core::tagging::version_audit::KnownRecording> {
        None
    }

    async fn tracks_of(&self, _: &str) -> Option<Vec<String>> {
        Some(Vec::new())
    }

    async fn resampled_lookup(&self, _: &str, _: f64) -> Option<AcoustIdLookup> {
        None
    }
}

#[tokio::test]
async fn the_version_check_lists_a_remix_tagged_as_the_song_and_keep_puts_it_away() {
    let mut setup = Setup::new();
    setup.versions = Some(Arc::new(TeenagersCatalog));
    setup.song("a-teenagers.mp3", "Teenagers", "The Black Parade", &[]);
    setup.song("b-fine.mp3", "Mama", "The Black Parade", &[]);
    let robot = recording("robot", "Teenagers (Robot remix)", 203_000, Vec::new());
    let heard = confirmed("robot", RecordingKind::Remix, false, true, vec![robot]);
    let verifier = FakeVerifier::answering(vec![("a-teenagers.mp3", heard)]);
    let sweep = setup.sweep(verifier, actions(360));

    ticks(&sweep, 3).await;

    let rows = sweep.version_findings();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row.id, "a-teenagers.mp3");
    assert!(row.version.heard);
    assert_eq!(row.version.is_id.as_deref(), Some("robot"));
    assert_eq!(
        row.version.words,
        "Tagged as 'Teenagers', but the fingerprint is 'Teenagers (Robot remix)' (3:23.00)."
    );
    assert_eq!(
        row.version.retag.get("title"),
        Some(&Some("Teenagers (Robot remix)".to_string()))
    );
    // Every song checked has the version check behind it.
    assert!(
        sweep
            .store()
            .read(|s| s.songs.values().all(|song| song.version_checked))
    );

    // Checked again now, the same file says the same thing.
    let again = sweep.check_now("a-teenagers.mp3").await.expect("checked");
    assert_eq!(
        again.version.as_ref().map(|v| v.words.as_str()),
        Some(row.version.words.as_str())
    );

    assert_eq!(sweep.keep_versions(&["a-teenagers.mp3".to_string()]), 1);
    assert!(sweep.version_findings().is_empty());
    // Kept stays kept while it is the same file and the same finding.
    sweep.check_now("a-teenagers.mp3").await.expect("checked");
    assert!(sweep.version_findings().is_empty());

    let found = sweep.find_songs("teen", 10).await.expect("listed");
    assert_eq!(
        found.iter().map(|song| song.id.as_str()).collect::<Vec<_>>(),
        ["a-teenagers.mp3"]
    );
}

#[tokio::test]
async fn songs_checked_before_the_version_check_are_checked_again() {
    let setup = Setup::new();
    setup.song("a-teenagers.mp3", "Teenagers", "The Black Parade", &[]);
    let verifier = FakeVerifier::answering(Vec::new());
    let sweep = setup.sweep(verifier.clone(), actions(360));
    ticks(&sweep, 2).await;
    assert_eq!(verifier.asked().len(), 1);
    assert!(
        sweep
            .store()
            .read(|s| s.songs.values().all(|song| !song.version_checked))
    );
    drop(sweep);

    let mut setup = setup;
    setup.versions = Some(Arc::new(TeenagersCatalog));
    let sweep = setup.sweep(verifier.clone(), actions(360));
    sweep.store().update(|s| s.next_pass_utc = datetime::min_value());
    ticks(&sweep, 2).await;
    assert_eq!(
        verifier.asked().len(),
        2,
        "checked again once the version check is on"
    );
    assert!(
        sweep
            .store()
            .read(|s| s.songs.values().all(|song| song.version_checked))
    );
}

/// The song is on track `own`, written again under the other names given.
fn number_track(path: &Path, own: u32, others: &[(&str, &str)]) {
    let mut file = TagFile::open(path).expect("opens");
    file.set_track(own);
    for (name, value) in others {
        extras::set_multi_value(&mut file, name, &[value.to_string()]);
    }
    file.save().expect("saved");
}

fn track_keys(path: &Path) -> Vec<(String, Vec<String>)> {
    let view = KeptIdentityTags::navidrome_view(&TagFile::open(path).expect("opens"));
    view.into_iter()
        .filter(|(key, _)| key.starts_with("track") || key == "trck")
        .collect()
}

#[tokio::test]
async fn a_track_number_written_again_under_another_name_is_taken_off_through_the_journal() {
    let setup = Setup::new();
    setup.song("a.mp3", "Don't Start Now", "Future Nostalgia", &[]);
    let path = setup.dir.path().join("a.mp3");
    number_track(&path, 2, &[("TRACK", "02"), ("TRACKNUMBER", "2/11")]);
    let sweep = setup.sweep(FakeVerifier::answering(Vec::new()), actions(360));
    sweep.tick().await;

    let asked = setup.edits.asked.lock().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].2.get("otherNames"), Some(&None));
    assert!(track_keys(&path).is_empty(), "{:?}", track_keys(&path));
    assert_eq!(TagFile::open(&path).expect("opens").track(), 2);
    assert!(sweep.findings().is_empty());
    assert_eq!(sweep.status().names_fixed, 1);
    assert_eq!(sweep.tick().await, LibraryIdentitySweep::IDLE_CHECK);
}

#[tokio::test]
async fn another_track_number_under_another_name_waits_for_a_person() {
    let setup = Setup::new();
    setup.song("a.mp3", "Don't Start Now", "Future Nostalgia", &[]);
    let path = setup.dir.path().join("a.mp3");
    number_track(&path, 2, &[("TRACK", "3")]);
    let sweep = setup.sweep(FakeVerifier::answering(Vec::new()), actions(360));
    sweep.tick().await;

    assert!(setup.edits.asked.lock().is_empty());
    let found = sweep.findings();
    assert_eq!(found.len(), 1);
    assert_eq!(rules(&found[0]), [AuditRule::OtherNames]);
    assert!(!found[0].sure);
    assert_eq!(found[0].changes.get("otherNames"), Some(&None));

    assert!(library_tag_edits::write(&path, &found[0].changes).changed);
    assert!(track_keys(&path).is_empty());
    assert_eq!(TagFile::open(&path).expect("opens").track(), 2);
}

#[tokio::test]
async fn a_song_checked_before_loses_its_copies_and_lists_a_track_number_that_says_another() {
    let setup = Setup::new();
    setup.song("a.mp3", "industry plant", "industry plant", &[]);
    let path = setup.dir.path().join("a.mp3");
    spell_album_artist(&path, "miki", &[("ALBUM ARTIST", "miki")]);
    number_track(&path, 2, &[("TRACK", "3")]);
    let verifier = FakeVerifier::answering(Vec::new());
    let sweep = setup.sweep(verifier.clone(), actions(360));
    checked_before_names(&sweep, &setup, "a.mp3", "industry plant");

    sweep.tick().await;
    assert!(verifier.asked().is_empty());
    let asked = setup.edits.asked.lock().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].2.get("albumArtistNames"), Some(&None));
    assert!(!asked[0].2.contains_key("otherNames"));
    assert_eq!(
        album_artist_keys(&path),
        [("albumartist".to_string(), vec!["miki".to_string()])]
    );
    // The track number that says another waits, from the file as written.
    let found = sweep.findings();
    assert_eq!(found.len(), 1);
    assert_eq!(rules(&found[0]), [AuditRule::OtherNames]);
    assert_eq!(found[0].changes.get("otherNames"), Some(&None));
    assert_eq!(sweep.status().names_fixed, 1);
    ticks(&sweep, 2).await;
    assert!(verifier.asked().is_empty());
    assert_eq!(setup.edits.asked.lock().len(), 1);
}
