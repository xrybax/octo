use std::sync::Arc;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use octo_core::common::Clock;
use octo_core::models::radio::LastFmRadioPlay;
use octo_core::settings::{AppSettings, ListeningSettings, SettingsStore};
use parking_lot::Mutex;

use super::*;

/// A log in a fresh folder on a clock the test moves.
struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    settings: Arc<SettingsStore>,
    now: Arc<Mutex<DateTime<Utc>>>,
    log: ListeningLog,
}

fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 20, 18, 0, 0).unwrap()
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("listening");
        let settings = Arc::new(SettingsStore::for_tests(AppSettings::default()));
        let now = Arc::new(Mutex::new(start()));
        let log = Self::open(&path, &settings, &now);
        Fixture {
            _dir: dir,
            path,
            settings,
            now,
            log,
        }
    }

    fn open(path: &Path, settings: &Arc<SettingsStore>, now: &Arc<Mutex<DateTime<Utc>>>) -> ListeningLog {
        let clock = Arc::clone(now);
        ListeningLog::with_clock(path, Arc::clone(settings), Clock::new(move || *clock.lock()))
    }

    /// The same folder read again, as after a restart, once every write is on disk.
    fn reopened(&self) -> ListeningLog {
        self.log.flush();
        Self::open(&self.path, &self.settings, &self.now)
    }

    fn now(&self) -> DateTime<Utc> {
        *self.now.lock()
    }

    fn advance(&self, by: TimeDelta) {
        *self.now.lock() += by;
    }

    fn set(&self, listening: ListeningSettings) {
        self.settings.set(AppSettings {
            listening,
            ..Default::default()
        });
    }

    fn files(&self) -> Vec<String> {
        self.log.flush();
        let mut names: Vec<String> = std::fs::read_dir(&self.path)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn now_playing(&self, id: &str) -> Vec<Settled> {
        self.log.now_playing("winters", id, self.now())
    }

    fn completed(&self, id: &str, length: Option<i64>, sent: Option<DateTime<Utc>>) -> Vec<Settled> {
        self.log.completed("winters", draft(id), length, sent, self.now())
    }

    /// Writes plays that ended, each with its own first genre, and gives (song, seconds).
    fn write(&self, settled: Vec<Settled>) -> Vec<(String, i64)> {
        settled
            .into_iter()
            .filter_map(|s| {
                let done = (s.play.song_id.clone(), s.play.seconds);
                let genre = s.genres.first().cloned();
                self.log.write(s, genre).then_some(done)
            })
            .collect()
    }
}

fn play(id: &str, at: DateTime<Utc>, seconds: i64) -> Play {
    Play {
        song_id: id.into(),
        cover_id: id.into(),
        artist: "Drake".into(),
        title: format!("Song {id}"),
        album: "Album".into(),
        seconds,
        played_at: at,
        ..Default::default()
    }
}

fn draft(id: &str) -> Draft {
    Draft {
        play: play(id, DateTime::<Utc>::UNIX_EPOCH, 0),
        genres: vec!["Rap".into()],
        album: None,
    }
}

fn seconds_of(settled: &[Settled]) -> Vec<(&str, i64)> {
    settled
        .iter()
        .map(|s| (s.play.song_id.as_str(), s.play.seconds))
        .collect()
}

#[test]
fn a_play_is_kept_across_a_restart_under_the_name_first_seen() {
    let f = Fixture::new();
    assert!(f.log.record("Winters", play("a", start(), 200)));
    assert!(
        f.log
            .record("winters", play("b", start() + TimeDelta::minutes(4), 180))
    );
    let again = f.reopened();
    assert_eq!(
        again.people(),
        vec![("winters".to_string(), "Winters".to_string())]
    );
    let plays = again.plays_of("WINTERS");
    assert_eq!(
        plays.iter().map(|p| p.song_id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(f.files(), ["listening.json", "winters.jsonl"]);
}

#[test]
fn a_play_without_a_song_or_a_person_is_not_written() {
    let f = Fixture::new();
    assert!(!f.log.record("", play("a", start(), 10)));
    let mut nameless = play("a", start(), 10);
    nameless.title = " ".into();
    assert!(!f.log.record("winters", nameless));
    assert!(f.log.people().is_empty());
}

#[test]
fn a_client_that_scrobbles_halfway_is_timed_to_the_next_song() {
    let f = Fixture::new();
    assert!(f.now_playing("a").is_empty());
    f.advance(TimeDelta::seconds(100));
    // Played at 50%: nothing is written yet.
    assert!(f.completed("a", Some(200), Some(start())).is_empty());
    f.advance(TimeDelta::seconds(100));
    let ended = f.now_playing("b");
    assert_eq!(seconds_of(&ended), [("a", 200)]);
    assert_eq!(ended[0].play.played_at, start());
    assert_eq!(ended[0].user, "winters");
    assert_eq!(f.write(ended), [("a".to_string(), 200)]);
    assert_eq!(f.log.plays_of("winters")[0].genre.as_deref(), Some("Rap"));
}

#[test]
fn a_client_that_scrobbles_only_at_the_start_is_timed_to_the_next_play() {
    let f = Fixture::new();
    assert!(f.completed("a", Some(200), Some(start())).is_empty());
    f.advance(TimeDelta::seconds(150));
    let ended = f.completed("b", Some(180), Some(f.now()));
    assert_eq!(seconds_of(&ended), [("a", 150)]);
    // The last one, with nothing after it, is written whole once its length and ten minutes
    // have passed.
    f.advance(TimeDelta::seconds(179));
    assert!(f.log.settle_expired().is_empty());
    f.advance(SETTLE_AFTER + TimeDelta::seconds(1));
    assert_eq!(seconds_of(&f.log.settle_expired()), [("b", 180)]);
}

#[test]
fn a_skipped_song_counts_only_what_played() {
    let f = Fixture::new();
    f.now_playing("a");
    f.advance(TimeDelta::seconds(20));
    f.completed("a", Some(300), Some(start()));
    f.advance(TimeDelta::seconds(20));
    assert_eq!(seconds_of(&f.now_playing("b")), [("a", 40)]);
}

#[test]
fn a_paused_song_never_counts_past_its_length() {
    let f = Fixture::new();
    f.now_playing("a");
    f.advance(TimeDelta::seconds(30));
    f.completed("a", Some(200), Some(start()));
    f.advance(TimeDelta::minutes(20));
    assert_eq!(seconds_of(&f.now_playing("b")), [("a", 200)]);
    // Paused and never followed: written whole once it has waited long enough.
    f.completed("b", Some(240), None);
    f.advance(TimeDelta::seconds(240) + SETTLE_AFTER);
    assert_eq!(seconds_of(&f.log.settle_expired()), [("b", 240)]);
}

#[test]
fn a_song_no_app_reported_as_played_is_not_written() {
    let f = Fixture::new();
    f.now_playing("a");
    f.advance(TimeDelta::seconds(5));
    assert!(f.now_playing("b").is_empty());
    f.advance(UNTIMED_CAP + SETTLE_AFTER);
    assert!(f.log.settle_expired().is_empty());
}

#[test]
fn a_song_of_no_known_length_counts_at_most_fifteen_minutes() {
    let f = Fixture::new();
    f.now_playing("a");
    f.completed("a", None, None);
    f.advance(TimeDelta::minutes(40));
    assert_eq!(
        seconds_of(&f.now_playing("b")),
        [("a", UNTIMED_CAP.num_seconds())]
    );
}

#[test]
fn the_same_song_said_again_mid_play_is_the_same_play() {
    let f = Fixture::new();
    f.now_playing("a");
    f.advance(TimeDelta::seconds(30));
    assert!(f.now_playing("a").is_empty());
    f.completed("a", Some(200), None);
    f.advance(TimeDelta::seconds(170));
    // Played to the end and started again: a second play.
    let ended = f.now_playing("a");
    assert_eq!(seconds_of(&ended), [("a", 200)]);
    assert_eq!(ended[0].play.played_at, start());
}

#[test]
fn on_shutdown_each_current_play_counts_what_has_played() {
    let f = Fixture::new();
    f.now_playing("a");
    f.completed("a", Some(300), None);
    f.advance(TimeDelta::seconds(120));
    assert_eq!(seconds_of(&f.log.settle_all()), [("a", 120)]);
    assert!(f.log.settle_all().is_empty());
}

#[test]
fn a_time_in_the_future_is_taken_as_now() {
    let f = Fixture::new();
    f.completed("a", Some(200), Some(start() + TimeDelta::hours(3)));
    f.advance(TimeDelta::seconds(60));
    let ended = f.now_playing("b");
    assert_eq!(ended[0].play.played_at, start());
    assert_eq!(ended[0].play.seconds, 60);
}

#[test]
fn recording_off_writes_nothing() {
    let f = Fixture::new();
    f.set(ListeningSettings {
        enabled: false,
        ..Default::default()
    });
    f.now_playing("a");
    assert!(f.completed("a", Some(100), None).is_empty());
    assert!(f.log.settle_all().is_empty());
    assert!(!f.log.record("winters", play("a", start(), 100)));
    f.log.heart("winters", &["a".into()], f.now());
    assert!(f.log.people().is_empty());
    assert!(f.files().is_empty(), "{:?}", f.files());
}

#[test]
fn a_heart_while_the_song_plays_marks_its_play() {
    let f = Fixture::new();
    f.now_playing("a");
    f.advance(TimeDelta::seconds(60));
    f.log.heart("winters", &["a".into()], f.now());
    f.completed("a", Some(180), None);
    f.advance(TimeDelta::seconds(120));
    let ended = f.now_playing("b");
    assert!(ended[0].play.hearted);
    f.write(ended);
    assert!(f.reopened().plays_of("winters")[0].hearted);
}

#[test]
fn a_heart_just_after_a_written_play_is_appended_not_rewritten() {
    let f = Fixture::new();
    for n in 0..500 {
        assert!(f.log.record(
            "winters",
            play(&format!("s{n}"), start() + TimeDelta::minutes(n), 60)
        ));
    }
    let file = f.path.join("winters.jsonl");
    f.log.flush();
    let before = std::fs::read_to_string(&file).unwrap();
    *f.now.lock() = start() + TimeDelta::minutes(499) + TimeDelta::seconds(90);
    f.log.heart("winters", &["s499".into()], f.now());
    f.log.flush();
    let after = std::fs::read_to_string(&file).unwrap();
    assert!(after.starts_with(&before), "the file was rewritten");
    let added = &after[before.len()..];
    assert_eq!(added.lines().count(), 1);
    assert!(added.contains("\"heart\":\"s499\""), "{added}");
    // Read again, the heart is on its play; the daily prune folds the line in.
    assert!(f.reopened().plays_of("winters").last().unwrap().hearted);
    f.log.prune();
    f.log.flush();
    let folded = std::fs::read_to_string(&file).unwrap();
    assert!(!folded.contains("\"heart\":"), "the heart line was not folded in");
    assert!(f.reopened().plays_of("winters").last().unwrap().hearted);
}

#[test]
fn a_heart_waits_for_its_play_per_person() {
    let f = Fixture::new();
    f.log.heart("winters", &["a".into()], f.now());
    // Someone else's many hearts never push it out.
    let many: Vec<String> = (0..80).map(|n| format!("x{n}")).collect();
    f.log.heart("alex", &many, f.now());
    f.log.heart("alex", &["a".into()], f.now());
    f.advance(TimeDelta::seconds(100));
    assert!(f.log.record("winters", play("a", start(), 100)));
    assert!(f.log.plays_of("winters")[0].hearted);
    // Long after its play, a heart does not count.
    f.advance(TimeDelta::hours(1));
    assert!(
        f.log
            .record("winters", play("b", start() + TimeDelta::minutes(2), 60))
    );
    f.log.heart("winters", &["b".into()], f.now());
    assert!(!f.log.plays_of("winters")[1].hearted);
}

#[test]
fn plays_older_than_the_days_kept_are_pruned() {
    let f = Fixture::new();
    f.set(ListeningSettings {
        keep_days: 7,
        ..Default::default()
    });
    f.log
        .record("winters", play("old", start() - TimeDelta::days(6), 100));
    f.log.record("winters", play("new", start(), 100));
    f.log
        .record("alex", play("alex-old", start() - TimeDelta::days(6), 100));
    f.advance(TimeDelta::days(2));
    // Past the days kept they are gone from the page at once, and from the files once a day.
    assert_eq!(f.log.plays_of("winters").len(), 1);
    assert_eq!(f.log.prune(), 2);
    assert!(f.log.plays_of("alex").is_empty());
    assert_eq!(f.reopened().plays_of("winters")[0].song_id, "new");
    assert_eq!(f.files(), ["listening.json", "winters.jsonl"]);
}

#[test]
fn clearing_removes_plays_names_and_plays_still_in_flight() {
    let f = Fixture::new();
    f.log.record("winters", play("a", start(), 100));
    f.log.record("Alex", play("b", start(), 100));
    f.log.record("Alex", play("c", start(), 100));
    // Alex's current play ends, and is being worked out, when the history is cleared.
    f.log.now_playing("Alex", "d", f.now());
    f.log.completed("Alex", draft("d"), Some(100), None, f.now());
    f.advance(TimeDelta::seconds(100));
    let in_flight = f.log.now_playing("Alex", "e", f.now());
    f.log.completed("Alex", draft("e"), Some(100), None, f.now());
    assert_eq!(in_flight.len(), 1);
    assert_eq!(f.log.clear(Some("alex")), 2);
    assert!(f.write(in_flight).is_empty());
    assert!(f.log.settle_all().is_empty(), "the current play was dropped too");
    assert_eq!(f.log.name_of("alex"), "alex");
    let again = f.reopened();
    assert_eq!(
        again.people(),
        vec![("winters".to_string(), "winters".to_string())]
    );
    assert_eq!(f.log.clear(None), 1);
    assert!(f.reopened().people().is_empty());
}

fn radio_play(id: &str, source: &str, at: DateTime<Utc>) -> LastFmRadioPlay {
    LastFmRadioPlay {
        song_id: id.into(),
        artist: "Kavinsky".into(),
        title: format!("Song {id}"),
        album: Some("OutRun".into()),
        genre: Some("Electro".into()),
        duration: Some(240),
        is_local: true,
        source: source.into(),
        played_at_utc: at,
        ..Default::default()
    }
}

#[test]
fn the_radio_history_is_copied_in_once_without_the_starter_seeds() {
    let f = Fixture::new();
    let history = vec![(
        "Winters".to_string(),
        vec![
            radio_play("scrobbled", "scrobble", start() - TimeDelta::days(2)),
            radio_play("streamed", "internet-radio", start() - TimeDelta::days(1)),
            radio_play("seed", "bootstrap-starter", start() - TimeDelta::days(1)),
            radio_play("ancient", "scrobble", start() - TimeDelta::days(120)),
        ],
    )];
    // Not while recording is off, and it is not marked as done.
    f.set(ListeningSettings {
        enabled: false,
        ..Default::default()
    });
    assert_eq!(f.log.backfill(history.clone()), 0);
    assert!(!f.log.backfilled());
    f.set(ListeningSettings::default());
    assert_eq!(f.log.backfill(history.clone()), 2);
    let plays = f.log.plays_of("winters");
    assert_eq!(
        plays.iter().map(|p| p.song_id.as_str()).collect::<Vec<_>>(),
        ["scrobbled", "streamed"]
    );
    let scrobbled = &plays[0];
    assert_eq!(scrobbled.app, UNKNOWN_APP);
    assert_eq!(scrobbled.platform, UNKNOWN_PLATFORM);
    assert_eq!(scrobbled.source, "backfill");
    assert_eq!(scrobbled.seconds, 240);
    assert_eq!(scrobbled.genre.as_deref(), Some("Electro"));
    // A scrobble is already dated at its start; a radio stream song at its end.
    assert_eq!(scrobbled.played_at, start() - TimeDelta::days(2));
    assert_eq!(
        plays[1].played_at,
        start() - TimeDelta::days(1) - TimeDelta::seconds(240)
    );
    // Never again, even after a restart.
    assert_eq!(f.reopened().backfill(history), 0);
    assert_eq!(f.reopened().plays_of("winters").len(), 2);
}

#[test]
fn genre_answers_are_kept_including_none() {
    let f = Fixture::new();
    assert_eq!(f.log.artist_genre("Drake"), None);
    f.log.remember_artist_genre("Drake", Some("Rap"));
    f.log.remember_artist_genre("Nobody", None);
    f.log.remember_album_genre("al-1", Some("Electro"));
    let again = f.reopened();
    assert_eq!(again.artist_genre(" drake "), Some(Some("Rap".into())));
    assert_eq!(again.artist_genre("Nobody"), Some(None));
    assert_eq!(again.album_genre("al-1"), Some(Some("Electro".into())));
}

#[test]
fn copied_plays_take_their_artists_genre() {
    let f = Fixture::new();
    let mut bare = radio_play("a", "scrobble", start() - TimeDelta::days(1));
    bare.genre = None;
    f.log.backfill(vec![("winters".into(), vec![bare])]);
    assert_eq!(f.log.artists_missing_genres(), ["Kavinsky"]);
    f.log.remember_artist_genre("Kavinsky", Some("Synthwave"));
    assert!(f.log.artists_missing_genres().is_empty());
    assert_eq!(f.log.fill_genres_from_artists(), 1);
    assert_eq!(
        f.reopened().plays_of("winters")[0].genre.as_deref(),
        Some("Synthwave")
    );
}

/// A Last.fm that answers `status` with `body` to every call.
async fn last_fm_answering(
    status: u16,
    body: &str,
) -> (wiremock::MockServer, crate::services::last_fm::LastFmService) {
    let server = wiremock::MockServer::builder().start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(
            wiremock::ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json"),
        )
        .mount(&server)
        .await;
    let mut settings = AppSettings::default();
    settings.last_fm.api_key = "0123456789abcdef0123456789abcdef".into();
    let service = crate::services::last_fm::LastFmService::with_base_url(
        Arc::new(SettingsStore::for_tests(settings)),
        &format!("{}/2.0/", server.uri()),
    );
    (server, service)
}

#[tokio::test]
async fn a_failed_last_fm_lookup_is_never_kept_as_no_genre() {
    let f = Fixture::new();
    let log = Arc::new(Fixture::open(&f.path, &f.settings, &f.now));
    let mut bare = radio_play("a", "scrobble", start() - TimeDelta::days(1));
    bare.genre = None;
    log.backfill(vec![("winters".into(), vec![bare])]);

    let (_down, failing) = last_fm_answering(500, "{}").await;
    assert_eq!(log.look_up_missing_genres(&failing).await, 0);
    assert_eq!(log.artist_genre("Kavinsky"), None, "a failure was kept");
    let resolved = super::super::genres::resolve(&log, &failing, &[], None, "Kavinsky").await;
    assert_eq!(resolved, None);
    assert_eq!(log.artist_genre("Kavinsky"), None, "a failure was kept");

    let (_up, answering) = last_fm_answering(
        200,
        r#"{"toptags":{"tag":[{"name":"seen live"},{"name":"synthwave"}]}}"#,
    )
    .await;
    assert_eq!(log.look_up_missing_genres(&answering).await, 1);
    assert_eq!(log.artist_genre("Kavinsky"), Some(Some("Synthwave".into())));
}

#[test]
fn a_line_torn_by_a_crash_is_left_out() {
    let f = Fixture::new();
    f.log.record("winters", play("a", start(), 100));
    f.log.flush();
    let file = f.path.join("winters.jsonl");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("{\"songId\":\"b\",\"art");
    std::fs::write(&file, text).unwrap();
    assert_eq!(f.reopened().plays_of("winters").len(), 1);
}

#[test]
fn any_name_is_a_safe_file_name() {
    for key in ["winters", "a.b", ".hidden", "../up", "名前", "x y/z"] {
        let stem = file_stem(key);
        assert!(!stem.contains('/') && !stem.starts_with('.'), "{key}: {stem}");
        assert_eq!(key_of_stem(&stem).as_deref(), Some(key));
    }
}

#[test]
fn the_platform_is_read_from_the_user_agents_parentheses() {
    assert_eq!(platform_of("Octo/1.9.0 (Android)", ""), "android");
    assert_eq!(platform_of("Octo/1.9.0 (iPhone)", ""), "iphone");
    assert_eq!(platform_of("Octo/1.9.0 (Windows)", ""), "windows");
    assert_eq!(
        platform_of("Symfonium/12.3.0 (Linux; Android 14; Pixel 8)", ""),
        "android"
    );
    assert_eq!(
        platform_of("Feishin/0.12 (Macintosh; Intel Mac OS X 14)", ""),
        "macos"
    );
    assert_eq!(platform_of("Feishin/0.12 (X11; Linux x86_64)", ""), "linux");
    assert_eq!(platform_of("Octo", ""), UNKNOWN_PLATFORM);
    assert_eq!(platform_of("", ""), UNKNOWN_PLATFORM);
}

#[test]
fn an_app_that_runs_on_one_platform_names_it() {
    // A bare Apple networking agent: what Amperfy, play:Sub and iSub send.
    let apple = "Amperfy/1.4 CFNetwork/1490 Darwin/23.2.0";
    for app in ["Amperfy", "playSub_iPhone15Pro", "play:Sub", "iSub", "Arpeggi"] {
        assert_eq!(platform_of(apple, app), "iphone", "{app}");
        assert_eq!(platform_of("", app), "iphone", "{app}");
    }
    for app in ["DSub", "Ultrasonic", "Symfonium", "Tempo", "Tempus", "subtracks"] {
        assert_eq!(platform_of("okhttp/4.12.0", app), "android", "{app}");
    }
    // A bare Apple networking agent with no app name could be a Mac app too.
    assert_eq!(platform_of("CFNetwork/1490 Darwin/23.2.0", ""), UNKNOWN_PLATFORM);
    // An iPhone app's Mac build says so in its agent.
    assert_eq!(
        platform_of("Amperfy/1.4 (Macintosh; Mac OS X 15.1) CFNetwork/1568", "Amperfy"),
        "macos"
    );
    // The app's own name wins over a User-Agent that says otherwise.
    assert_eq!(
        platform_of("Mozilla/5.0 (X11; Linux x86_64)", "Symfonium"),
        "android"
    );
    // An app that runs anywhere is read from its User-Agent.
    assert_eq!(
        platform_of("Feishin/0.12 (X11; Linux x86_64)", "Feishin"),
        "linux"
    );
    assert_eq!(platform_of("okhttp/4.12.0", "Feishin"), UNKNOWN_PLATFORM);
    // substreamer runs on both, so its own name says nothing.
    assert_eq!(platform_of("okhttp/4.12.0", "substreamer8"), UNKNOWN_PLATFORM);
}
