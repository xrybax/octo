//! Keeping people's marks when a set of copies is fixed: which copy keeps its id, the identity
//! tags carried onto the best audio, the move itself through a real quarantine and journal over a
//! faked Navidrome, playlist entries moved over in place, Undo, and the automatic put back when
//! Navidrome does not keep the moved song.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use futures::FutureExt;
use indexmap::IndexMap;
use octo_core::settings::{
    AppSettings, LibraryAction, LibraryActionDefinition, LibraryActionSettings, SubsonicSettings,
};
use octo_media::tags::TagFile;
use octo_media::tags::tag_writer_extras::{self as extras, TagFields};
use serde_json::{Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::*;
use crate::app::{AppState, TestServices};
use crate::services::health::ServerSongFields;
use crate::services::health::health_fixes::{self as fixes, FixStep, fix_actions};
use crate::services::health::library_health::{self as health, HealthCheck};
use crate::services::health::library_health_service::{HealthRun, LibraryHealthService, Row};
use crate::services::library::LibrarySongRow;
use crate::services::library::library_tag_edits as tag_edits;
use crate::services::library::{LibraryActionRequest, LibraryActionState};

// ---- which copy keeps its id ----------------------------------------------------------------

fn at(day: u32) -> Option<DateTime<Utc>> {
    Some(Utc.with_ymd_and_hms(2026, 1, day, 12, 0, 0).unwrap())
}

fn copies(list: &[(&str, Option<DateTime<Utc>>)]) -> Vec<CopyRef> {
    list.iter()
        .map(|(id, created)| CopyRef {
            id: id.to_string(),
            created: *created,
        })
        .collect()
}

fn map<V: Copy>(pairs: &[(&str, V)]) -> HashMap<String, V> {
    pairs.iter().map(|(id, value)| (id.to_string(), *value)).collect()
}

#[test]
fn a_favorite_rating_and_plays_score_with_plays_capped_at_fifty() {
    let score = |starred, rating, plays| {
        marks_score(&SongMarks {
            starred,
            rating,
            plays,
        })
    };
    assert_eq!(score(false, 0, 0), 0);
    assert_eq!(score(true, 0, 0), 3);
    assert_eq!(score(false, 4, 0), 2);
    assert_eq!(score(true, 5, 12), 17);
    assert_eq!(score(false, 0, 900), 50);
}

#[test]
fn the_copy_with_the_most_marks_keeps_its_id_over_playlists_and_age() {
    let set = copies(&[("new", at(9)), ("marked", at(5)), ("old", at(1))]);
    let pick = pick_target(&set, &map(&[("marked", 3)]), &map(&[("old", 4)])).expect("a pick");
    assert_eq!(pick.target, "marked");
    assert_eq!(pick.reason, TargetReason::Marks);
}

#[test]
fn without_marks_the_copy_in_the_most_playlists_keeps_its_id() {
    let set = copies(&[("a", at(1)), ("b", at(5))]);
    let pick = pick_target(&set, &HashMap::new(), &map(&[("b", 2), ("a", 1)])).expect("a pick");
    assert_eq!(pick.target, "b");
    assert_eq!(pick.reason, TargetReason::Playlists);
}

#[test]
fn with_no_marks_anywhere_the_oldest_copy_keeps_its_id() {
    let set = copies(&[("new", at(9)), ("unknown", None), ("old", at(1))]);
    let pick = pick_target(&set, &HashMap::new(), &HashMap::new()).expect("a pick");
    assert_eq!(pick.target, "old");
    assert_eq!(pick.reason, TargetReason::Oldest);
}

#[test]
fn copies_nothing_tells_apart_keep_the_sets_own_order() {
    let set = copies(&[("first", None), ("second", None)]);
    let pick = pick_target(&set, &map(&[("first", 3), ("second", 3)]), &HashMap::new()).expect("a pick");
    assert_eq!(pick.target, "first");
    assert_eq!(pick.reason, TargetReason::Marks);
    let none = pick_target(&set, &HashMap::new(), &HashMap::new()).expect("a pick");
    assert_eq!(
        (none.target.as_str(), none.reason),
        ("first", TargetReason::First)
    );
}

#[test]
fn only_the_same_extension_keeps_the_path() {
    assert!(same_extension("/m/a/Song.flac", "/m/b/Song (1).FLAC"));
    assert!(!same_extension("/m/a/Song.flac", "/m/a/Song.mp3"));
    assert!(!same_extension("/m/a/Song", "/m/a/Song"));
}

#[test]
fn smart_and_synced_playlists_are_not_editable() {
    let kinds = crate::services::library::navidrome_playlist_api::parse_playlist_kinds(&json!([
        {"id": "p1", "name": "Mine", "ownerName": "bob", "rules": null, "sync": false},
        {"id": "p2", "name": "Smart", "ownerName": "bob", "rules": {"all": [{"is": {"loved": true}}]}},
        {"id": "p3", "name": "Synced", "ownerName": "bob", "sync": true},
    ]));
    let editable: Vec<(&str, bool)> = kinds.iter().map(|k| (k.id.as_str(), k.editable())).collect();
    assert_eq!(editable, [("p1", true), ("p2", false), ("p3", false)]);
}

// ---- the identity carried onto the best audio ------------------------------------------------

struct Tagged<'a> {
    title: &'a str,
    artist: &'a str,
    album: &'a str,
    album_artist: &'a str,
    genre: &'a str,
    track: u32,
    album_id: Option<&'a str>,
    artist_id: Option<&'a str>,
}

fn write_flac(path: &Path, tags: &Tagged<'_>) {
    std::fs::create_dir_all(path.parent().expect("a folder")).expect("folders");
    let bytes = if path.extension().is_some_and(|e| e == "mp3") {
        crate::services::test_support::mp3()
    } else {
        crate::services::test_support::flac()
    };
    std::fs::write(path, bytes).expect("written");
    let mut file = TagFile::open(path).expect("opens");
    file.set_title(Some(tags.title));
    file.set_performers(&[tags.artist.to_string()]);
    file.set_album(Some(tags.album));
    file.set_album_artists(&[tags.album_artist.to_string()]);
    file.set_genres(&[tags.genre.to_string()]);
    file.set_track(tags.track);
    if let Some(id) = tags.album_id {
        extras::set_text(&mut file, TagFields::ALBUM_ID, Some(id));
    }
    if let Some(id) = tags.artist_id {
        extras::set_text(&mut file, TagFields::ARTIST_ID, Some(id));
    }
    file.save().expect("saved");
}

fn tag(path: &Path, name: &str) -> Option<String> {
    tag_edits::read(path)
        .expect("the tags read")
        .get(name)
        .cloned()
        .flatten()
}

const MARKED: Tagged<'static> = Tagged {
    title: "Teardrop",
    artist: "Massive Attack",
    album: "Mezzanine",
    album_artist: "Massive Attack",
    genre: "marked copy",
    track: 3,
    album_id: Some("a1b2c3d4-0000-4000-8000-000000000001"),
    artist_id: Some("10adbe5e-a2c0-4bf3-8249-2b4cbf6e6ca8"),
};

const BEST: Tagged<'static> = Tagged {
    title: "Teardrop (2019 Remaster)",
    artist: "Massive Attack feat. Elizabeth Fraser",
    album: "Mezzanine (Deluxe)",
    album_artist: "Massive Attack",
    genre: "best copy",
    track: 7,
    album_id: Some("ffffffff-0000-4000-8000-00000000000f"),
    artist_id: None,
};

#[test]
fn the_marked_copys_identity_goes_onto_the_best_audio_and_its_other_tags_stay() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (marked, best) = (dir.path().join("marked.flac"), dir.path().join("best.flac"));
    write_flac(&marked, &MARKED);
    write_flac(&best, &BEST);

    carry_identity(&marked, &best, None).expect("carried");

    assert_eq!(tag(&best, "title").as_deref(), Some("Teardrop"));
    assert_eq!(tag(&best, "artist").as_deref(), Some("Massive Attack"));
    assert_eq!(tag(&best, "album").as_deref(), Some("Mezzanine"));
    assert_eq!(tag(&best, "track").as_deref(), Some("3"));
    let file = TagFile::open(&best).expect("opens");
    assert_eq!(
        extras::read_text(&file, TagFields::ALBUM_ID).as_deref(),
        MARKED.album_id
    );
    assert_eq!(
        extras::read_text(&file, TagFields::ARTIST_ID).as_deref(),
        MARKED.artist_id
    );
    assert_eq!(
        KeptIdentityTags::pid_inputs(&KeptIdentityTags::read(&best, None).expect("reads")),
        KeptIdentityTags::pid_inputs(&KeptIdentityTags::read(&marked, None).expect("reads"))
    );
    // Not part of what Navidrome files the song by: the best copy's own.
    assert_eq!(tag(&best, "genre").as_deref(), Some("best copy"));
}

// ---- Navidrome, faked ------------------------------------------------------------------------

#[derive(Clone)]
struct FakeSong {
    relative: String,
    size: u64,
    missing: bool,
    title: String,
    starred: bool,
    rating: i32,
    plays: i64,
    created: String,
}

#[derive(Clone)]
struct FakePlaylist {
    id: String,
    name: String,
    smart: bool,
    tracks: Vec<String>,
    comment: String,
}

#[derive(Default)]
struct FakeNavidrome {
    library_path: String,
    songs: Mutex<HashMap<String, FakeSong>>,
    playlists: Mutex<Vec<FakePlaylist>>,
    scans: std::sync::atomic::AtomicUsize,
    /// Scans leave the sizes alone: Navidrome never shows the moved file.
    blind: std::sync::atomic::AtomicBool,
}

impl FakeNavidrome {
    /// A scan: every song's size as its file has it now, or missing.
    fn scan(&self) {
        self.scans.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.blind.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        for song in self.songs.lock().values_mut() {
            match std::fs::metadata(Path::new(&self.library_path).join(&song.relative)) {
                Ok(metadata) => {
                    song.size = metadata.len();
                    song.missing = false;
                }
                Err(_) => song.missing = true,
            }
        }
    }

    fn tracks(&self, playlist: &str) -> Vec<String> {
        self.playlists
            .lock()
            .iter()
            .find(|p| p.id == playlist)
            .map(|p| p.tracks.clone())
            .unwrap_or_default()
    }
}

struct Answers(Arc<FakeNavidrome>);

fn json_answer(value: &Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(value.to_string(), "application/json")
}

impl Respond for Answers {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let fake = &self.0;
        let path = request.url.path().to_string();
        let method = request.method.as_str().to_string();
        let query: HashMap<String, Vec<String>> =
            request.url.query_pairs().fold(HashMap::new(), |mut all, (k, v)| {
                all.entry(k.into_owned()).or_default().push(v.into_owned());
                all
            });
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        if path == "/auth/login" {
            return json_answer(&json!({
                "token": "admin-jwt", "isAdmin": true, "username": "admin",
                "subsonicSalt": "salt", "subsonicToken": "token",
            }));
        }
        if let Some(endpoint) = path.strip_prefix("/rest/") {
            // The admin's own stars and ratings, as Navidrome's Subsonic API sets them.
            let first = |name: &str| query.get(name).and_then(|values| values.first().cloned());
            if endpoint == "updatePlaylist" {
                let mut playlists = fake.playlists.lock();
                let Some(playlist) =
                    first("playlistId").and_then(|id| playlists.iter_mut().find(|p| p.id == id))
                else {
                    return json_answer(
                        &json!({"subsonic-response": {"status": "failed", "error": {"code": 70}}}),
                    );
                };
                playlist.comment = first("comment").unwrap_or_default();
                return json_answer(&json!({"subsonic-response": {"status": "ok"}}));
            }
            let mut songs = fake.songs.lock();
            let Some(song) = first("id").and_then(|id| songs.get_mut(&id)) else {
                return json_answer(
                    &json!({"subsonic-response": {"status": "failed", "error": {"code": 70}}}),
                );
            };
            match endpoint {
                "setRating" => song.rating = first("rating").and_then(|r| r.parse().ok()).unwrap_or(0),
                "star" => song.starred = true,
                "unstar" => song.starred = false,
                _ => return ResponseTemplate::new(404),
            }
            return json_answer(&json!({"subsonic-response": {"status": "ok"}}));
        }
        if let Some(id) = path.strip_prefix("/api/song/") {
            let songs = fake.songs.lock();
            let Some(song) = songs.get(id) else {
                return ResponseTemplate::new(404);
            };
            return json_answer(&json!({
                "id": id, "path": song.relative, "libraryPath": fake.library_path, "size": song.size,
                "suffix": "flac", "title": song.title, "artist": "Massive Attack", "album": "Mezzanine",
                "duration": 2, "missing": song.missing, "starred": song.starred, "rating": song.rating,
                "playCount": song.plays, "createdAt": song.created,
            }));
        }
        if path == "/api/playlist" && method == "GET" {
            let list: Vec<Value> = fake
                .playlists
                .lock()
                .iter()
                .map(|p| {
                    json!({
                        "id": p.id, "name": p.name, "ownerName": "alice", "sync": false,
                        "rules": if p.smart { json!({"all": [{"is": {"loved": true}}]}) } else { Value::Null },
                    })
                })
                .collect();
            return json_answer(&Value::Array(list));
        }
        let Some(rest) = path.strip_prefix("/api/playlist/") else {
            return ResponseTemplate::new(404);
        };
        let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
        let mut playlists = fake.playlists.lock();
        let Some(playlist) = playlists.iter_mut().find(|p| p.id == parts[0]) else {
            return ResponseTemplate::new(404);
        };
        match (method.as_str(), parts.as_slice()) {
            ("GET", [_, "tracks"]) => {
                let start: usize = query.get("_start").and_then(|v| v[0].parse().ok()).unwrap_or(0);
                let end: usize = query
                    .get("_end")
                    .and_then(|v| v[0].parse().ok())
                    .unwrap_or(usize::MAX);
                let rows: Vec<Value> = playlist
                    .tracks
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(end.saturating_sub(start))
                    .map(|(i, id)| json!({"id": i + 1, "mediaFileId": id}))
                    .collect();
                json_answer(&Value::Array(rows))
            }
            ("POST", [_, "tracks"]) => {
                if playlist.smart {
                    return ResponseTemplate::new(400);
                }
                let ids: Vec<String> = body["ids"]
                    .as_array()
                    .map(|ids| {
                        ids.iter()
                            .filter_map(|id| id.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let added = ids.len();
                playlist.tracks.extend(ids);
                json_answer(&json!({"added": added}))
            }
            ("DELETE", [_, "tracks"]) => {
                let mut positions: Vec<usize> = query
                    .get("id")
                    .map(|ids| ids.iter().filter_map(|id| id.parse().ok()).collect())
                    .unwrap_or_default();
                positions.sort_unstable_by(|a, b| b.cmp(a));
                for position in positions {
                    if position >= 1 && position <= playlist.tracks.len() {
                        playlist.tracks.remove(position - 1);
                    }
                }
                json_answer(&json!({}))
            }
            ("PUT", [_, "tracks", position]) => {
                // Navidrome's Reorder: the track lands at insert_before, the ones between shift.
                let last = playlist.tracks.len();
                let Ok(position) = position.parse::<usize>() else {
                    return ResponseTemplate::new(400);
                };
                let Some(new) = body["insert_before"]
                    .as_str()
                    .and_then(|n| n.parse::<usize>().ok())
                else {
                    return ResponseTemplate::new(400);
                };
                if position < 1 || position > last {
                    return ResponseTemplate::new(404);
                }
                let new = new.clamp(1, last);
                let track = playlist.tracks.remove(position - 1);
                playlist.tracks.insert(new - 1, track);
                json_answer(&json!({"id": position.to_string()}))
            }
            _ => ResponseTemplate::new(404),
        }
    }
}

struct Factory {
    state: AppState,
    navidrome: Arc<FakeNavidrome>,
    /// The library as Library health reads it.
    rows: Arc<Mutex<Vec<LibrarySongRow>>>,
    root: PathBuf,
    _server: MockServer,
    _dir: tempfile::TempDir,
}

impl Factory {
    async fn new() -> Factory {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path().join("music");
        std::fs::create_dir_all(&root).expect("the music folder");
        let navidrome = Arc::new(FakeNavidrome {
            library_path: root.to_string_lossy().into_owned(),
            ..Default::default()
        });
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(Answers(Arc::clone(&navidrome)))
            .mount(&server)
            .await;
        let settings = AppSettings {
            subsonic: SubsonicSettings {
                url: Some(server.uri()),
                auto_detect_download_path: false,
                admin_username: Some("admin".into()),
                admin_password: Some("admin-password".into()),
                ..Default::default()
            },
            library_actions: LibraryActionSettings {
                enabled: true,
                dry_run: false,
                allowed_users: vec!["alice".into()],
                actions: vec![LibraryActionDefinition {
                    action: LibraryAction::Delete,
                    enabled: true,
                    ..Default::default()
                }],
                quarantine_retention_days: 14,
                ..Default::default()
            },
            ..Default::default()
        };
        let rows: Arc<Mutex<Vec<LibrarySongRow>>> = Arc::default();
        let read = Arc::clone(&rows);
        let read_page: crate::services::library::library_snapshot::ReadPage =
            Arc::new(move |start: i32, count: i32| {
                let all = read.lock().clone();
                let page: Vec<LibrarySongRow> = all
                    .iter()
                    .skip(start.max(0) as usize)
                    .take(count.max(0) as usize)
                    .cloned()
                    .collect();
                let total = all.len();
                async move { Some((page, total)) }.boxed()
            });
        let state = AppState::for_tests_with(
            settings,
            TestServices {
                library_snapshot: Some(Arc::new(crate::services::library::LibrarySnapshot::with(
                    read_page,
                    octo_core::common::Clock::system(),
                ))),
                config_dir: Some(dir.path().join("config")),
                raw_settings: vec![("Library:DownloadPath".into(), root.to_string_lossy().into_owned())],
                ..Default::default()
            },
        );
        let scanned = Arc::clone(&navidrome);
        state.library_action_executor.configure(move |seams| {
            seams.history_poll = Duration::from_millis(5);
            seams.history_attempts = 3;
            seams.force_scan = Some(Arc::new(move || {
                scanned.scan();
                async { true }.boxed()
            }));
        });
        state.library_health.set_settle_for(chrono::TimeDelta::zero());
        Factory {
            state,
            navidrome,
            rows,
            root,
            _server: server,
            _dir: dir,
        }
    }

    fn keep(&self) -> Arc<KeepMarks> {
        Arc::clone(
            self.state
                .library_health
                .keep_marks()
                .expect("keeping marks is set up"),
        )
    }

    /// A FLAC copy Navidrome knows by `id`, and its row as the library read lists it.
    fn copy(
        &self,
        id: &str,
        relative: &str,
        tags: &Tagged<'_>,
        bits: i32,
        created: u32,
        starred: bool,
    ) -> Row {
        let path = self.root.join(relative);
        write_flac(&path, tags);
        let size = std::fs::metadata(&path).expect("written").len();
        let created = at(created).expect("a day");
        self.navidrome.songs.lock().insert(
            id.to_string(),
            FakeSong {
                relative: relative.to_string(),
                size,
                missing: false,
                title: tags.title.to_string(),
                starred,
                rating: 0,
                plays: 0,
                created: created.to_rfc3339(),
            },
        );
        Arc::new(LibrarySongRow {
            id: id.into(),
            path: relative.into(),
            size: size as i64,
            suffix: Path::new(relative)
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default(),
            bit_rate: if bits > 16 { 2000 } else { 900 },
            title: "Teardrop".into(),
            artist: "Massive Attack".into(),
            duration: Some(330),
            seconds: Some(330.0),
            album: Some("Mezzanine".into()),
            album_id: Some("al".into()),
            album_artist: Some("Massive Attack".into()),
            bit_depth: Some(bits),
            sample_rate: Some(if bits > 16 { 96000 } else { 44100 }),
            added_utc: Some(created),
            ..Default::default()
        })
    }

    fn playlist(&self, id: &str, name: &str, smart: bool, tracks: &[&str]) {
        self.navidrome.playlists.lock().push(FakePlaylist {
            id: id.into(),
            name: name.into(),
            smart,
            tracks: tracks.iter().map(|t| t.to_string()).collect(),
            comment: String::new(),
        });
    }

    /// The plan for the one set these copies make, the way the dashboard's preview makes it.
    async fn plan(&self, rows: &[Row]) -> (fixes::DuplicateFix<Row>, Vec<FixStep>) {
        let fields = ServerSongFields::new(rows.iter().map(|row| &**row));
        let report = health::check(rows, &fields);
        assert_eq!(report.duplicates.len(), 1, "one set of copies");
        let set = &report.duplicates[0];
        let keep = self.keep();
        let uses = keep.playlist_uses().await.unwrap_or_default();
        let pick = keep.pick_for(set, &uses).await;
        let fix = plan(set, &fields, pick.as_ref());
        let steps = fixes::duplicate_steps(&fix, &fields, None);
        (fix, steps)
    }

    async fn run(&self, steps: Vec<FixStep>) -> Arc<HealthRun> {
        let health = &self.state.library_health;
        assert!(
            health
                .start_fix("Fixing copies", Some(HealthCheck::Duplicates), steps, "alice")
                .is_none()
        );
        self.finished().await
    }

    async fn undo(&self) -> Arc<HealthRun> {
        assert_eq!(self.state.library_health.start_undo("alice"), None);
        self.finished().await
    }

    async fn finished(&self) -> Arc<HealthRun> {
        for _ in 0..1000 {
            if let Some(run) = self.state.library_health.run()
                && !run.is_running()
            {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the run never finished");
    }
}

fn trashed(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![root.join(".octo-trash")];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|e| e == "flac") {
                found.push(path);
            }
        }
    }
    found
}

// ---- the move --------------------------------------------------------------------------------

#[tokio::test]
async fn fixing_keeps_the_best_sound_in_the_starred_copy_its_id_and_playlist_places() {
    let f = Factory::new().await;
    // A: the best sound, never marked, added last. B: starred by the admin, old, 16-bit.
    let a = f.copy(
        "A",
        "Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac",
        &BEST,
        24,
        20,
        false,
    );
    let b = f.copy(
        "B",
        "Massive Attack/Mezzanine/03 Teardrop.flac",
        &MARKED,
        16,
        1,
        true,
    );
    f.playlist("p1", "Evening", false, &["x", "A", "y", "A"]);
    f.playlist("p2", "Loved", true, &["A"]);

    let (fix, steps) = f.plan(&[a, b]).await;
    assert_eq!(fix.keep.id, "B");
    assert_eq!(fix.audio.as_ref().map(|a| a.id.as_str()), Some("A"));
    assert!(fix.why.contains("starred, rated or played"), "{}", fix.why);
    assert_eq!(steps[0].action, fix_actions::MOVE_SOUND);
    assert_eq!((steps[0].id.as_str(), steps[0].param("from")), ("B", Some("A")));

    let run = f.run(steps).await;
    let outcome = run.progress().outcome.expect("an outcome");
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);

    let b_path = f.root.join("Massive Attack/Mezzanine/03 Teardrop.flac");
    let a_path = f.root.join("Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac");
    // B's path holds A's audio under B's identity; A's own file is in the trash.
    assert_eq!(tag(&b_path, "genre").as_deref(), Some("best copy"));
    assert_eq!(tag(&b_path, "title").as_deref(), Some("Teardrop"));
    assert_eq!(tag(&b_path, "album").as_deref(), Some("Mezzanine"));
    assert!(!a_path.exists());
    assert_eq!(
        trashed(&f.root).len(),
        2,
        "A, and B's own file until the trash is swept"
    );
    // B's id still answers at its path, at the new size.
    assert!(
        f.state
            .navidrome_song_path_resolver
            .shows_at("B", &b_path.to_string_lossy())
            .await
    );
    // Every place A had in a playlist is B's now, in the same place; a smart one is left alone.
    assert_eq!(f.navidrome.tracks("p1"), ["x", "B", "y", "B"]);
    assert_eq!(f.navidrome.tracks("p2"), ["A"]);
    assert!(outcome.summary().contains("Loved"), "{}", outcome.summary());
    let journal = f.state.library_action_journal.recent(20);
    assert!(journal.iter().any(|e| e.navidrome_id == "B"
        && e.action == LibraryAction::BetterQuality
        && e.state == LibraryActionState::Applied
        && e.history_kept == Some(true)));
    assert!(journal.iter().any(|e| e.navidrome_id == "A"
        && e.action == LibraryAction::Delete
        && e.copy
        && e.state == LibraryActionState::Applied));
}

#[tokio::test]
async fn undo_puts_both_copies_and_their_playlist_places_back() {
    let f = Factory::new().await;
    let a = f.copy(
        "A",
        "Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac",
        &BEST,
        24,
        20,
        false,
    );
    let b = f.copy(
        "B",
        "Massive Attack/Mezzanine/03 Teardrop.flac",
        &MARKED,
        16,
        1,
        true,
    );
    f.playlist("p1", "Evening", false, &["x", "A", "y"]);
    let (_, steps) = f.plan(&[a, b]).await;
    f.run(steps).await;
    assert_eq!(f.navidrome.tracks("p1"), ["x", "B", "y"]);

    let undo = f.undo().await;
    let outcome = undo.progress().outcome.expect("an outcome");
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);

    let b_path = f.root.join("Massive Attack/Mezzanine/03 Teardrop.flac");
    let a_path = f.root.join("Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac");
    assert_eq!(tag(&b_path, "genre").as_deref(), Some("marked copy"));
    assert_eq!(tag(&a_path, "genre").as_deref(), Some("best copy"));
    assert_eq!(tag(&a_path, "title").as_deref(), Some("Teardrop (2019 Remaster)"));
    assert_eq!(f.navidrome.tracks("p1"), ["x", "A", "y"]);
}

#[tokio::test]
async fn a_move_navidrome_does_not_keep_is_put_back_by_itself() {
    let f = Factory::new().await;
    let a = f.copy(
        "A",
        "Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac",
        &BEST,
        24,
        20,
        false,
    );
    let b = f.copy(
        "B",
        "Massive Attack/Mezzanine/03 Teardrop.flac",
        &MARKED,
        16,
        1,
        true,
    );
    f.playlist("p1", "Evening", false, &["A"]);
    let (_, steps) = f.plan(&[a, b]).await;
    f.navidrome.blind.store(true, std::sync::atomic::Ordering::SeqCst);

    let run = f.run(steps).await;

    let outcome = run.progress().outcome.expect("an outcome");
    assert_eq!(outcome.failed.len(), 1, "{:?}", outcome.failed);
    assert_eq!(outcome.failed[0].0.action, fix_actions::MOVE_SOUND);
    assert!(
        outcome.failed[0].1.contains("put back"),
        "{}",
        outcome.failed[0].1
    );
    let b_path = f.root.join("Massive Attack/Mezzanine/03 Teardrop.flac");
    let a_path = f.root.join("Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac");
    assert_eq!(tag(&b_path, "genre").as_deref(), Some("marked copy"));
    assert_eq!(tag(&a_path, "genre").as_deref(), Some("best copy"));
    // Playlist places move only once Navidrome keeps the song.
    assert_eq!(f.navidrome.tracks("p1"), ["A"]);
    assert!(f.navidrome.scans.load(std::sync::atomic::Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn a_marked_copy_in_another_format_is_kept_as_it_is() {
    let f = Factory::new().await;
    let a = f.copy(
        "A",
        "Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac",
        &BEST,
        24,
        20,
        false,
    );
    // Starred, but not a FLAC: moving the FLAC in would change its path.
    let b = f.copy(
        "B",
        "Massive Attack/Mezzanine/03 Teardrop.mp3",
        &MARKED,
        16,
        1,
        true,
    );

    let (fix, steps) = f.plan(&[a, b]).await;

    assert_eq!(fix.keep.id, "B");
    assert!(fix.audio.is_none());
    assert!(fix.why.contains("different format"), "{}", fix.why);
    assert!(steps.iter().all(|step| step.action != fix_actions::MOVE_SOUND));
    assert_eq!(
        steps
            .iter()
            .filter(|s| s.action == fix_actions::REMOVE)
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["A"]
    );
}

#[tokio::test]
async fn an_older_copy_nobody_marked_in_another_format_does_not_cost_the_best_sound() {
    let f = Factory::new().await;
    let a = f.copy(
        "A",
        "Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac",
        &BEST,
        24,
        20,
        false,
    );
    let b = f.copy(
        "B",
        "Massive Attack/Mezzanine/03 Teardrop.mp3",
        &MARKED,
        16,
        1,
        false,
    );

    let (fix, steps) = f.plan(&[a, b]).await;

    assert_eq!(fix.keep.id, "A");
    assert!(fix.audio.is_none());
    assert_eq!(
        steps
            .iter()
            .map(|s| (s.action.as_str(), s.id.as_str()))
            .collect::<Vec<_>>(),
        [(fix_actions::REMOVE, "B")]
    );
}

#[tokio::test]
async fn with_no_marks_anywhere_the_best_sound_moves_into_the_oldest_copy() {
    let f = Factory::new().await;
    let a = f.copy(
        "A",
        "Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac",
        &BEST,
        24,
        20,
        false,
    );
    let b = f.copy(
        "B",
        "Massive Attack/Mezzanine/03 Teardrop.flac",
        &MARKED,
        16,
        1,
        false,
    );

    let (fix, steps) = f.plan(&[a, b]).await;

    assert_eq!(fix.keep.id, "B");
    assert_eq!(fix.audio.as_ref().map(|a| a.id.as_str()), Some("A"));
    assert!(fix.why.contains("oldest"), "{}", fix.why);
    assert_eq!(steps[0].action, fix_actions::MOVE_SOUND);
}

#[tokio::test]
async fn when_the_best_copy_holds_the_marks_the_fix_is_as_before() {
    let f = Factory::new().await;
    let a = f.copy(
        "A",
        "Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac",
        &BEST,
        24,
        20,
        true,
    );
    let b = f.copy(
        "B",
        "Massive Attack/Mezzanine/03 Teardrop.flac",
        &MARKED,
        16,
        1,
        false,
    );

    let (fix, steps) = f.plan(&[a, b]).await;

    assert_eq!(fix.keep.id, "A");
    assert!(fix.audio.is_none());
    assert_eq!(
        steps
            .iter()
            .map(|s| (s.action.as_str(), s.id.as_str()))
            .collect::<Vec<_>>(),
        [(fix_actions::REMOVE, "B")]
    );
}

// ---- the steps and their words ---------------------------------------------------------------

#[test]
fn a_move_step_names_the_copy_it_takes_the_sound_from_and_undoes_as_a_move_back() {
    let step = FixStep::move_sound(
        "B",
        "Teardrop",
        "A",
        &[fixes::TagChange::new("year", None, "1998")],
    );
    assert_eq!(
        (step.param("from"), step.param("year")),
        (Some("A"), Some("1998"))
    );
    assert_eq!(
        step.undo_step(),
        Some(FixStep::with(
            fix_actions::MOVE_BACK,
            "B",
            "Teardrop",
            IndexMap::from([("from".to_string(), "A".to_string())])
        ))
    );
    assert_eq!(LibraryHealthService::invalid(std::slice::from_ref(&step)), None);
    let no_copy = FixStep::new(fix_actions::MOVE_SOUND, "B", "Teardrop");
    assert!(LibraryHealthService::invalid(&[no_copy]).is_some());
    let odd = FixStep::with(
        fix_actions::MOVE_SOUND,
        "B",
        "Teardrop",
        IndexMap::from([
            ("from".to_string(), "A".to_string()),
            ("mood".to_string(), "sad".to_string()),
        ]),
    );
    assert_eq!(
        LibraryHealthService::invalid(&[odd]).as_deref(),
        Some("Unknown tag \"mood\".")
    );
}

#[test]
fn the_keep_marks_words_are_plain_and_use_no_dash() {
    let mut lines = vec![
        crate::services::health::health_words::MARKS_PICKED_IN_PREVIEW.to_string(),
        FORMAT_DIFFERS.to_string(),
        CHECKING.to_string(),
    ];
    for reason in [
        TargetReason::Marks,
        TargetReason::Playlists,
        TargetReason::Oldest,
        TargetReason::First,
    ] {
        lines.push(crate::services::health::health_words::kept_marks_moved(
            "FLAC, 24-bit, 96 kHz",
            reason,
        ));
        lines.push(crate::services::health::health_words::kept_marks_as_is(
            "MP3, 320 kbps",
            reason,
        ));
    }
    lines.push(
        fixes::FixOutcome {
            done: vec![
                FixStep::move_sound("B", "Teardrop", "A", &[]),
                FixStep::new(fix_actions::MOVE_BACK, "C", "Angel"),
            ],
            ..Default::default()
        }
        .summary(),
    );
    for line in &lines {
        assert!(
            !(line.contains('\u{2014}') || line.contains('\u{2013}') || line.contains(" - ")),
            "{line}"
        );
    }
    assert_eq!(
        lines.last().map(String::as_str),
        Some("Moved the best sound into 1 song, put back the copies of 1 song.")
    );
}

#[tokio::test]
async fn moving_the_sound_waits_while_a_job_writes_the_music_folder() {
    let f = Factory::new().await;
    f.copy(
        "A",
        "Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac",
        &BEST,
        24,
        20,
        false,
    );
    f.copy(
        "B",
        "Massive Attack/Mezzanine/03 Teardrop.flac",
        &MARKED,
        16,
        1,
        true,
    );
    let b_path = f.root.join("Massive Attack/Mezzanine/03 Teardrop.flac");
    let tidy = f
        .state
        .library_write_gate
        .begin_job("Tidy artists")
        .await
        .expect("free");

    let keep = f.keep();
    let moving = tokio::spawn(async move { keep.move_sound("B", "A", "alice", &IndexMap::new()).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!moving.is_finished(), "the sound moved during a job");
    assert_eq!(tag(&b_path, "genre").as_deref(), Some("marked copy"));

    drop(tidy);
    let answer = moving.await.expect("joined");
    assert_eq!(answer.state, "applied", "{answer:?}");
    assert_eq!(tag(&b_path, "genre").as_deref(), Some("best copy"));
}

#[tokio::test]
async fn a_move_navidrome_does_not_keep_is_never_put_back_over_newer_content() {
    let f = Factory::new().await;
    f.copy(
        "A",
        "Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac",
        &BEST,
        24,
        20,
        false,
    );
    f.copy(
        "B",
        "Massive Attack/Mezzanine/03 Teardrop.flac",
        &MARKED,
        16,
        1,
        true,
    );
    let b_path = f.root.join("Massive Attack/Mezzanine/03 Teardrop.flac");
    let keep = f.keep();
    let answer = keep.move_sound("B", "A", "alice", &IndexMap::new()).await;
    assert_eq!(answer.state, "applied", "{answer:?}");

    // Something newer lands at the path before the run's check, which Navidrome then fails.
    std::fs::write(&b_path, b"newer").expect("written");
    f.navidrome.blind.store(true, std::sync::atomic::Ordering::SeqCst);
    let (failed, _) = keep.finish_run(&["B".to_string()], &[]).await;

    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].1.contains("changed since"), "{}", failed[0].1);
    assert_eq!(std::fs::read(&b_path).expect("still there"), b"newer");
}

#[test]
fn the_startup_sweep_removes_the_hidden_copies_a_stopped_move_left() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let album = dir.path().join("Massive Attack/Mezzanine");
    std::fs::create_dir_all(&album).expect("made");
    for name in [
        "03 Teardrop.flac",
        ".octo-keep-0a1b.flac",
        ".octo-keep-back-2c3d.flac",
        ".hidden-but-not-ours.flac",
    ] {
        std::fs::write(album.join(name), b"x").expect("written");
    }

    assert_eq!(sweep_staging_under(dir.path()), 2);

    let mut left: Vec<String> = std::fs::read_dir(&album)
        .expect("listed")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, [".hidden-but-not-ours.flac", "03 Teardrop.flac"]);
}

#[tokio::test]
async fn moving_the_sound_waits_for_no_library_action_on_either_copy() {
    let f = Factory::new().await;
    f.copy(
        "A",
        "Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac",
        &BEST,
        24,
        20,
        false,
    );
    f.copy(
        "B",
        "Massive Attack/Mezzanine/03 Teardrop.flac",
        &MARKED,
        16,
        1,
        true,
    );
    let a_path = f.root.join("Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac");
    let b_path = f.root.join("Massive Attack/Mezzanine/03 Teardrop.flac");
    let keep = f.keep();

    // A library action is running on the best-sounding copy.
    let action = f
        .state
        .library_action_executor
        .claim_songs(&[&a_path.to_string_lossy()])
        .expect("free");
    let answer = keep.move_sound("B", "A", "alice", &IndexMap::new()).await;
    assert_eq!(answer.state, "skipped", "{answer:?}");
    assert_eq!(
        answer.detail.as_deref(),
        Some(crate::services::library::LibraryActionExecutor::BUSY_TEXT)
    );
    assert_eq!(tag(&b_path, "genre").as_deref(), Some("marked copy"));
    assert!(a_path.exists());

    drop(action);
    let answer = keep.move_sound("B", "A", "alice", &IndexMap::new()).await;
    assert_eq!(answer.state, "applied", "{answer:?}");
    // The move held both copies only while it ran.
    assert!(
        f.state
            .library_action_executor
            .claim_songs(&[&a_path.to_string_lossy(), &b_path.to_string_lossy()])
            .is_some()
    );
}

// ---- every rated copy kept -------------------------------------------------------------------

/// A copy as the library read lists it, with no file behind it.
fn row(id: &str, relative: &str, bits: i32, day: u32) -> Row {
    Arc::new(LibrarySongRow {
        id: id.into(),
        path: relative.into(),
        size: 1000,
        suffix: Path::new(relative)
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default(),
        bit_rate: if bits > 16 { 2000 } else { 900 },
        title: "Teardrop".into(),
        artist: "Massive Attack".into(),
        duration: Some(330),
        seconds: Some(330.0),
        album: Some("Mezzanine".into()),
        album_id: Some("al".into()),
        album_artist: Some("Massive Attack".into()),
        bit_depth: Some(bits),
        sample_rate: Some(if bits > 16 { 96000 } else { 44100 }),
        added_utc: at(day),
        ..Default::default()
    })
}

fn starred() -> SongMarks {
    SongMarks {
        starred: true,
        ..Default::default()
    }
}

fn rated(rating: i32) -> SongMarks {
    SongMarks {
        rating,
        ..Default::default()
    }
}

fn played(plays: i64) -> SongMarks {
    SongMarks {
        plays,
        ..Default::default()
    }
}

fn kim() -> Reader {
    Reader::Member("kim".into())
}

/// Every reader's marks on each copy: the admin's and kim's, none where not named.
fn set_marks(rows: &[Row], given: &[(&str, Reader, Option<SongMarks>)]) -> SetMarks {
    SetMarks(
        rows.iter()
            .map(|copy| {
                let reads = [Reader::Admin, kim()]
                    .into_iter()
                    .map(|reader| {
                        let marks = given
                            .iter()
                            .find(|(id, who, _)| *id == copy.id && *who == reader)
                            .map_or(Some(SongMarks::default()), |(_, _, marks)| *marks);
                        ReaderMarks { reader, marks }
                    })
                    .collect();
                (copy.id.clone(), reads)
            })
            .collect(),
    )
}

/// What a plan keeps first, whose sound moves in, what else stays and what goes.
type RatedShape = (String, Option<String>, Vec<String>, Vec<String>);

/// The plan for the one set `rows` make with every rated copy kept, or None when nothing would
/// go.
fn rated_plan(rows: &[Row], marks: &SetMarks, picked: Option<&str>) -> Option<RatedShape> {
    let fields = ServerSongFields::new(rows.iter().map(|row| &**row));
    let report = health::check(rows, &fields);
    assert_eq!(report.duplicates.len(), 1, "one set of copies");
    let set = &report.duplicates[0];
    let picked = picked.and_then(|id| set.copies.iter().find(|copy| copy.id == id));
    let fix = plan_keeping_rated(set, &fields, marks, &PlaylistUses::default(), picked)?;
    let ids = |copies: &[Row]| {
        let mut ids: Vec<String> = copies.iter().map(|copy| copy.id.clone()).collect();
        ids.sort();
        ids
    };
    Some((
        fix.keep.id.clone(),
        fix.audio.as_ref().map(|audio| audio.id.clone()),
        ids(&fix.also_kept),
        ids(&fix.remove),
    ))
}

const A_BEST: &str = "Massive Attack/Mezzanine (Deluxe)/07 Teardrop.flac";
const B_FLAC: &str = "Massive Attack/Mezzanine/03 Teardrop.flac";
const C_FLAC: &str = "Massive Attack/Singles/01 Teardrop.flac";

#[test]
fn with_one_rated_copy_the_best_sound_moves_into_it_and_the_unrated_copies_go() {
    let rows = [
        row("A", A_BEST, 24, 20),
        row("B", B_FLAC, 16, 1),
        row("C", C_FLAC, 16, 5),
    ];
    // C has the most plays, but only B is rated.
    let marks = set_marks(
        &rows,
        &[
            ("B", Reader::Admin, Some(rated(4))),
            ("C", Reader::Admin, Some(played(40))),
        ],
    );
    let (keep, audio, also, remove) = rated_plan(&rows, &marks, None).expect("a plan");
    assert_eq!(keep, "B");
    assert_eq!(audio.as_deref(), Some("A"));
    assert!(also.is_empty());
    assert_eq!(remove, ["C"]);
}

#[test]
fn every_rated_copy_stays_and_only_the_unmarked_ones_go() {
    let rows = [
        row("A", A_BEST, 24, 20),
        row("B", B_FLAC, 16, 1),
        row("C", C_FLAC, 16, 5),
    ];
    // A, the best sound, is starred by a Family member; B is rated by the admin.
    let marks = set_marks(
        &rows,
        &[
            ("A", kim(), Some(starred())),
            ("B", Reader::Admin, Some(rated(2))),
        ],
    );
    let (keep, audio, also, remove) = rated_plan(&rows, &marks, None).expect("a plan");
    assert_eq!((keep.as_str(), audio), ("A", None));
    assert_eq!(also, ["B"]);
    assert_eq!(remove, ["C"]);
}

#[test]
fn two_rated_copies_keep_the_best_sound_in_the_most_marked_one() {
    let rows = [
        row("A", A_BEST, 24, 20),
        row("B", B_FLAC, 16, 1),
        row("C", C_FLAC, 16, 5),
    ];
    let marks = set_marks(
        &rows,
        &[
            ("B", Reader::Admin, Some(rated(5))),
            ("B", kim(), Some(starred())),
            ("C", kim(), Some(rated(3))),
        ],
    );
    let (keep, audio, also, remove) = rated_plan(&rows, &marks, None).expect("a plan");
    assert_eq!((keep.as_str(), audio.as_deref()), ("B", Some("A")));
    assert_eq!(also, ["C"]);
    assert!(
        remove.is_empty(),
        "the best sound's own file is the only one that goes"
    );
}

#[test]
fn a_set_where_every_copy_is_rated_is_left_alone() {
    let rows = [row("A", A_BEST, 24, 20), row("B", B_FLAC, 16, 1)];
    let marks = set_marks(
        &rows,
        &[
            ("A", Reader::Admin, Some(starred())),
            ("B", kim(), Some(rated(1))),
        ],
    );
    assert_eq!(rated_plan(&rows, &marks, None), None);
}

#[test]
fn with_no_rated_copy_the_fix_is_the_one_that_keeps_peoples_marks() {
    let rows = [row("A", A_BEST, 24, 20), row("B", B_FLAC, 16, 1)];
    let marks = set_marks(&rows, &[("B", Reader::Admin, Some(played(12)))]);
    let (keep, audio, also, remove) = rated_plan(&rows, &marks, None).expect("a plan");
    assert_eq!((keep.as_str(), audio.as_deref()), ("B", Some("A")));
    assert!(also.is_empty() && remove.is_empty());
}

#[test]
fn a_copy_whose_marks_could_not_be_read_stays() {
    let rows = [
        row("A", A_BEST, 24, 20),
        row("B", B_FLAC, 16, 1),
        row("C", C_FLAC, 16, 5),
    ];
    let marks = set_marks(&rows, &[("C", kim(), None)]);
    let (keep, audio, also, remove) = rated_plan(&rows, &marks, None).expect("a plan");
    // Nothing known marks C, so the best sound is kept in its own copy and C stays beside it.
    assert_eq!((keep.as_str(), audio), ("A", None));
    assert_eq!(also, ["C"]);
    assert_eq!(remove, ["B"]);
}

#[test]
fn a_copy_a_person_picked_is_kept_first_and_the_rated_ones_stay_too() {
    let rows = [
        row("A", A_BEST, 24, 20),
        row("B", B_FLAC, 16, 1),
        row("C", C_FLAC, 16, 5),
    ];
    let marks = set_marks(&rows, &[("B", Reader::Admin, Some(rated(3)))]);
    let (keep, audio, also, remove) = rated_plan(&rows, &marks, Some("C")).expect("a plan");
    assert_eq!((keep.as_str(), audio), ("C", None));
    assert_eq!(also, ["B"]);
    assert_eq!(remove, ["A"]);
}

#[test]
fn the_steps_go_only_while_the_copy_that_goes_is_unrated_and_name_the_copy_that_stays() {
    let rows = [
        row("A", A_BEST, 24, 20),
        row("B", B_FLAC, 16, 1),
        row("C", C_FLAC, 16, 5),
    ];
    let fields = ServerSongFields::new(rows.iter().map(|row| &**row));
    let report = health::check(&rows, &fields);
    let marks = set_marks(&rows, &[("B", Reader::Admin, Some(rated(4)))]);
    let fix = plan_keeping_rated(
        &report.duplicates[0],
        &fields,
        &marks,
        &PlaylistUses::default(),
        None,
    )
    .expect("a plan");
    let mut steps = fixes::duplicate_steps(&fix, &fields, None);
    fixes::only_unrated(&mut steps);
    let shape: Vec<(&str, &str, Option<&str>, Option<&str>)> = steps
        .iter()
        .map(|step| {
            (
                step.action.as_str(),
                step.id.as_str(),
                step.param(INTO).or(step.param("from")),
                step.param(UNRATED_ONLY),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            (fix_actions::MOVE_SOUND, "B", Some("A"), Some("true")),
            (fix_actions::REMOVE, "C", Some("B"), Some("true")),
        ]
    );
    // The move's own check lets the flag through, and a move with no tags to write is valid.
    assert_eq!(LibraryHealthService::invalid(&steps), None);
    let bare = FixStep::move_sound("B", "Teardrop", "A", &[]);
    assert_eq!(LibraryHealthService::invalid(&[bare]), None);
}

#[test]
fn carrying_marks_gives_the_higher_rating_and_any_star_and_never_plays() {
    let from = [
        ReaderMarks::read(
            Reader::Admin,
            SongMarks {
                starred: true,
                rating: 5,
                plays: 30,
            },
        ),
        ReaderMarks::read(kim(), rated(2)),
        ReaderMarks {
            reader: Reader::Member("lee".into()),
            marks: None,
        },
    ];
    let into = [
        ReaderMarks::read(Reader::Admin, rated(3)),
        ReaderMarks::read(kim(), rated(4)),
    ];
    let (ratings, stars) = carry_plan(&from, &into);
    assert_eq!(ratings, [(Reader::Admin, 3, 5)]);
    assert_eq!(stars, [Reader::Admin]);
    // Nothing is written over marks on the kept copy that could not be read.
    let unread = [
        ReaderMarks {
            reader: Reader::Admin,
            marks: None,
        },
        ReaderMarks {
            reader: kim(),
            marks: None,
        },
    ];
    assert_eq!(carry_plan(&from, &unread), (Vec::new(), Vec::new()));
}

#[test]
fn a_remove_that_waits_for_no_rating_says_why_it_kept_a_copy() {
    assert_eq!(kept_because(&[ReaderMarks::read(Reader::Admin, played(9))]), None);
    assert_eq!(
        kept_because(&[ReaderMarks::read(kim(), rated(1))]),
        Some(RATED_SINCE)
    );
    assert_eq!(
        kept_because(&[ReaderMarks {
            reader: Reader::Admin,
            marks: None
        }]),
        Some(MARKS_UNREAD)
    );
}

#[test]
fn the_keep_rated_words_are_plain_and_use_no_dash() {
    use crate::services::health::health_words as words;
    let lines = [
        words::KEEP_RATED_LABEL.to_string(),
        words::KEEP_RATED_MEANING.to_string(),
        words::KEEP_RATED_MORE.to_string(),
        words::RATED_KEPT_IN_PREVIEW.to_string(),
        words::ALL_RATED.to_string(),
        words::also_keeps_rated(1),
        words::also_keeps_rated(2),
        words::skipped_rated(3),
        RATED_SINCE.to_string(),
        MARKS_UNREAD.to_string(),
        words::fix_meaning(HealthCheck::Duplicates).to_string(),
    ];
    for line in &lines {
        assert!(
            !(line.contains('\u{2014}') || line.contains('\u{2013}') || line.contains(" - ")),
            "{line}"
        );
        assert!(!line.to_lowercase().contains("lossless"), "{line}");
        assert!(!line.contains("favourite"), "{line}");
    }
    assert_eq!(
        words::also_keeps_rated(1),
        "Also keeps 1 other copy someone starred or rated, as it is."
    );
    assert_eq!(
        words::marks_words(1, 4, 2, 2, false).as_deref(),
        Some("Starred, rated 4 stars by 2 people")
    );
    assert_eq!(words::marks_words(0, 0, 0, 0, false), None);
    assert_eq!(
        words::marks_words(0, 0, 0, 0, true).as_deref(),
        Some("Some marks could not be read, so it stays")
    );
}

// ---- marks carried over, through a real run ---------------------------------------------------

/// Family members' marks, kept here.
#[derive(Default)]
struct FakeMembers {
    marks: Mutex<HashMap<(String, String), SongMarks>>,
}

impl FakeMembers {
    fn set(&self, member: &str, song: &str, marks: SongMarks) {
        self.marks
            .lock()
            .insert((member.to_string(), song.to_string()), marks);
    }

    fn get(&self, member: &str, song: &str) -> SongMarks {
        self.marks
            .lock()
            .get(&(member.to_string(), song.to_string()))
            .copied()
            .unwrap_or_default()
    }
}

impl MemberMarks for FakeMembers {
    fn marks_of<'a>(&'a self, song_id: &'a str) -> BoxFuture<'a, Vec<(String, Option<SongMarks>)>> {
        Box::pin(async move { vec![("kim".to_string(), Some(self.get("kim", song_id)))] })
    }

    fn set_rating<'a>(&'a self, member: &'a str, song_id: &'a str, rating: i32) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let mut marks = self.get(member, song_id);
            marks.rating = rating;
            self.set(member, song_id, marks);
            true
        })
    }

    fn set_starred<'a>(&'a self, member: &'a str, song_id: &'a str, starred: bool) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let mut marks = self.get(member, song_id);
            marks.starred = starred;
            self.set(member, song_id, marks);
            true
        })
    }
}

impl Factory {
    fn members(&self) -> Arc<FakeMembers> {
        let members = Arc::new(FakeMembers::default());
        self.keep()
            .use_members(Arc::clone(&members) as Arc<dyn MemberMarks>);
        members
    }

    /// The admin's own rating and plays on a song.
    fn mark(&self, id: &str, rating: i32, plays: i64) {
        let mut songs = self.navidrome.songs.lock();
        let song = songs.get_mut(id).expect("a song");
        song.rating = rating;
        song.plays = plays;
    }

    /// The admin's own star and rating on a song.
    fn admin(&self, id: &str) -> (bool, i32) {
        let songs = self.navidrome.songs.lock();
        let song = songs.get(id).expect("a song");
        (song.starred, song.rating)
    }

    /// The plan for the one set `rows` make with every rated copy kept, and its steps.
    async fn rated_steps(&self, rows: &[Row]) -> Option<(fixes::DuplicateFix<Row>, Vec<FixStep>)> {
        let fields = ServerSongFields::new(rows.iter().map(|row| &**row));
        let report = health::check(rows, &fields);
        let set = &report.duplicates[0];
        let keep = self.keep();
        let uses = keep.playlist_uses().await.unwrap_or_default();
        let marks = keep.read_set(set).await;
        let fix = plan_keeping_rated(set, &fields, &marks, &uses, None)?;
        let mut steps = fixes::duplicate_steps(&fix, &fields, None);
        fixes::only_unrated(&mut steps);
        Some((fix, steps))
    }
}

#[tokio::test]
async fn a_removed_copys_stars_ratings_and_playlist_places_move_to_the_kept_one_and_undo_takes_them_off() {
    let f = Factory::new().await;
    let members = f.members();
    // A: the best sound, starred and rated 2 by the admin, played often. B: rated 4 by the
    // admin, starred by kim, in a playlist.
    let a = f.copy("A", A_BEST, &BEST, 24, 1, true);
    let b = f.copy("B", B_FLAC, &MARKED, 16, 20, false);
    f.mark("A", 2, 30);
    f.mark("B", 4, 0);
    members.set("kim", "B", starred());
    f.playlist("p1", "Evening", false, &["x", "B", "y"]);

    let (fix, steps) = f.plan(&[a, b]).await;
    assert_eq!((fix.keep.id.as_str(), fix.audio.is_none()), ("A", true));
    assert_eq!(
        steps
            .iter()
            .map(|s| (s.action.as_str(), s.id.as_str(), s.param(INTO)))
            .collect::<Vec<_>>(),
        [(fix_actions::REMOVE, "B", Some("A"))]
    );

    let run = f.run(steps).await;
    let outcome = run.progress().outcome.expect("an outcome");
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert!(!f.root.join(B_FLAC).exists());
    // The higher rating and kim's star are on the kept copy; B's place in the playlist is A's.
    assert_eq!(f.admin("A"), (true, 4));
    assert!(members.get("kim", "A").starred);
    assert_eq!(f.navidrome.tracks("p1"), ["x", "A", "y"]);
    // B's own marks are left as they were, for when it comes back.
    assert_eq!(f.admin("B"), (false, 4));
    assert!(members.get("kim", "B").starred);

    let undo = f.undo().await;
    let outcome = undo.progress().outcome.expect("an outcome");
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert!(f.root.join(B_FLAC).exists());
    assert_eq!(f.admin("A"), (true, 2));
    assert!(!members.get("kim", "A").starred);
    assert_eq!(f.navidrome.tracks("p1"), ["x", "B", "y"]);
}

#[tokio::test]
async fn undo_leaves_a_mark_someone_changed_since_the_fix() {
    let f = Factory::new().await;
    let a = f.copy("A", A_BEST, &BEST, 24, 1, true);
    let b = f.copy("B", B_FLAC, &MARKED, 16, 20, false);
    f.mark("A", 2, 30);
    f.mark("B", 4, 0);
    let (_, steps) = f.plan(&[a, b]).await;
    f.run(steps).await;
    assert_eq!(f.admin("A"), (true, 4));

    // Someone rates the kept copy 5 after the fix.
    f.mark("A", 5, 30);
    f.undo().await;

    assert_eq!(f.admin("A"), (true, 5));
}

#[tokio::test]
async fn moving_the_sound_carries_the_other_copys_rating_and_undo_takes_it_off() {
    let f = Factory::new().await;
    let members = f.members();
    // B is the marked copy (starred, played); A, the best sound, is rated by kim.
    let a = f.copy("A", A_BEST, &BEST, 24, 20, false);
    let b = f.copy("B", B_FLAC, &MARKED, 16, 1, true);
    f.mark("B", 0, 20);
    members.set("kim", "A", rated(3));

    let (fix, steps) = f.plan(&[a, b]).await;
    assert_eq!(
        (fix.keep.id.as_str(), fix.audio.as_ref().map(|a| a.id.as_str())),
        ("B", Some("A"))
    );
    f.run(steps).await;
    assert_eq!(members.get("kim", "B").rating, 3);

    f.undo().await;
    assert_eq!(members.get("kim", "B").rating, 0);
    assert_eq!(members.get("kim", "A").rating, 3);
}

#[tokio::test]
async fn with_every_rated_copy_kept_a_rated_copy_never_goes() {
    let f = Factory::new().await;
    let members = f.members();
    let a = f.copy("A", A_BEST, &BEST, 24, 20, false);
    let b = f.copy("B", B_FLAC, &MARKED, 16, 1, false);
    let c = f.copy("C", C_FLAC, &MARKED, 16, 5, false);
    // A, the best sound, is rated by kim; B by the admin; C by nobody, though often played.
    members.set("kim", "A", rated(5));
    f.mark("B", 3, 0);
    f.mark("C", 0, 45);

    let (fix, steps) = f.rated_steps(&[a, b, c]).await.expect("a plan");
    assert_eq!(fix.keep.id, "A");
    assert_eq!(
        fix.also_kept
            .iter()
            .map(|copy| copy.id.as_str())
            .collect::<Vec<_>>(),
        ["B"]
    );
    assert_eq!(steps.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["C"]);

    let run = f.run(steps).await;
    let outcome = run.progress().outcome.expect("an outcome");
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert!(f.root.join(A_BEST).exists());
    assert!(f.root.join(B_FLAC).exists());
    assert!(!f.root.join(C_FLAC).exists());
}

#[tokio::test]
async fn a_copy_rated_after_the_preview_is_kept_when_the_fix_runs() {
    let f = Factory::new().await;
    let members = f.members();
    let a = f.copy("A", A_BEST, &BEST, 24, 20, false);
    let b = f.copy("B", B_FLAC, &MARKED, 16, 1, false);
    let c = f.copy("C", C_FLAC, &MARKED, 16, 5, false);
    f.mark("A", 4, 0);

    let (_, steps) = f.rated_steps(&[a, b, c]).await.expect("a plan");
    assert_eq!(steps.len(), 2, "{steps:?}");
    // kim stars B between the preview and the fix.
    members.set("kim", "B", starred());

    let run = f.run(steps).await;
    let outcome = run.progress().outcome.expect("an outcome");
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert_eq!(
        outcome
            .unchanged
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["B"]
    );
    assert!(f.root.join(B_FLAC).exists());
    assert!(!f.root.join(C_FLAC).exists());
    assert!(
        outcome
            .summary()
            .contains("Kept 1 copy someone starred or rated since the preview"),
        "{}",
        outcome.summary()
    );
}

#[tokio::test]
async fn a_set_where_every_copy_is_rated_has_no_steps() {
    let f = Factory::new().await;
    let members = f.members();
    let a = f.copy("A", A_BEST, &BEST, 24, 20, true);
    let b = f.copy("B", B_FLAC, &MARKED, 16, 1, false);
    members.set("kim", "B", rated(1));
    assert!(f.rated_steps(&[a, b]).await.is_none());
}

// ---- Fix duplicates from any app -------------------------------------------------------------

impl Factory {
    /// Keep every rated copy on, ratings on, and Fix duplicates on (with `star`, if any) beside
    /// Delete.
    fn fix_from_apps(&self, star: Option<i32>) {
        let mut settings = (*self.state.settings.current()).clone();
        let actions = &mut settings.library_actions;
        actions.keep_rated_copies = true;
        actions.ratings_enabled = true;
        actions.duplicates_enabled = true;
        actions.actions = vec![
            LibraryActionDefinition {
                action: LibraryAction::Delete,
                enabled: true,
                ..Default::default()
            },
            LibraryActionDefinition {
                action: LibraryAction::WrongVersion,
                enabled: false,
                rating: Some(0),
                ..Default::default()
            },
            LibraryActionDefinition {
                action: LibraryAction::FixDuplicates,
                enabled: true,
                rating: star,
                ..Default::default()
            },
        ];
        self.state.settings.set(settings);
    }

    /// The action playlist worker over this state, for one sweep.
    fn playlist_worker(&self) -> crate::services::library::LibraryActionPlaylistWorker {
        crate::services::library::LibraryActionPlaylistWorker::new(
            Arc::clone(&self.state.library_action_executor),
            Arc::clone(&self.state.library_action_journal),
            Arc::clone(&self.state.library_action_quarantine),
            Arc::clone(&self.state.navidrome_song_path_resolver),
            self.state.navidrome_identity.clone(),
            Arc::clone(&self.state.navidrome_playlist_api),
            Arc::clone(&self.state.settings),
        )
    }

    /// The library read as Library health sees it: these rows.
    fn library(&self, rows: &[Row]) {
        *self.rows.lock() = rows.iter().map(|row| (**row).clone()).collect();
    }
}

#[tokio::test]
async fn adding_a_copy_to_the_fix_duplicates_playlist_fixes_the_song_and_keeps_rated_copies() {
    let f = Factory::new().await;
    let members = f.members();
    f.fix_from_apps(None);
    let a = f.copy("A", A_BEST, &BEST, 24, 20, false);
    let b = f.copy("B", B_FLAC, &MARKED, 16, 1, false);
    let c = f.copy("C", C_FLAC, &MARKED, 16, 5, false);
    // kim gave B a heart; nobody marked A or C.
    members.set("kim", "B", starred());
    f.library(&[a, b, c]);
    let fix = "\u{1F6E0} Fix duplicates";
    f.playlist("fix", fix, false, &["C"]);

    f.playlist_worker()
        .sweep(&tokio_util::sync::CancellationToken::new())
        .await;
    let run = f.finished().await;

    let outcome = run.progress().outcome.expect("an outcome");
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    // B, the hearted copy, stays and takes the best sound; C goes; A's own file goes.
    assert!(f.root.join(B_FLAC).exists());
    assert_eq!(tag(&f.root.join(B_FLAC), "genre").as_deref(), Some("best copy"));
    assert!(!f.root.join(C_FLAC).exists());
    assert!(!f.root.join(A_BEST).exists());
    // The song left the action playlist, which says what adding a song to it does.
    assert!(f.navidrome.tracks("fix").is_empty());
    let comment = f.navidrome.playlists.lock()[0].comment.clone();
    assert!(
        comment.contains("anyone gave a heart or a star rating stays"),
        "{comment}"
    );
}

#[tokio::test]
async fn the_fix_duplicates_star_takes_its_own_rating_off_so_it_never_keeps_the_copy() {
    let f = Factory::new().await;
    let members = f.members();
    f.fix_from_apps(Some(3));
    let a = f.copy("A", A_BEST, &BEST, 24, 20, false);
    let b = f.copy("B", B_FLAC, &MARKED, 16, 1, false);
    members.set("kim", "A", rated(4));
    // Three stars on B ask for the fix; they are not a mark that keeps B.
    f.mark("B", 3, 0);
    f.library(&[a, b]);

    let sign_in: [(String, String); 3] = [
        ("u".into(), "alice".into()),
        ("t".into(), "token".into()),
        ("s".into(), "salt".into()),
    ];
    let request = LibraryActionRequest::new(LibraryAction::FixDuplicates, "B", "alice").with_credential(
        octo_subsonic::SubsonicCredential::from(sign_in.iter().map(|(k, v)| (k, v))),
    );
    let outcome = f
        .state
        .library_action_executor
        .apply(request)
        .await
        .expect("an outcome");
    assert_eq!(outcome.state, LibraryActionState::Applied, "{outcome:?}");
    f.finished().await;

    assert_eq!(f.admin("B").1, 0);
    assert!(!f.root.join(B_FLAC).exists());
    assert!(f.root.join(A_BEST).exists());
}

#[tokio::test]
async fn fix_duplicates_on_a_song_whose_copies_are_all_rated_keeps_them_and_answers_the_question() {
    let f = Factory::new().await;
    let members = f.members();
    f.fix_from_apps(None);
    let a = f.copy("A", A_BEST, &BEST, 24, 20, true);
    let b = f.copy("B", B_FLAC, &MARKED, 16, 1, false);
    members.set("kim", "B", rated(2));
    f.library(&[a, b]);

    let outcome = f
        .state
        .library_action_executor
        .apply(LibraryActionRequest::new(
            LibraryAction::FixDuplicates,
            "A",
            "alice",
        ))
        .await
        .expect("an outcome");

    assert_eq!(outcome.state, LibraryActionState::Applied, "{outcome:?}");
    assert!(
        outcome
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("Every copy is starred or rated"))
    );
    assert!(f.state.library_health.run().is_none(), "nothing ran");
    assert!(f.root.join(A_BEST).exists() && f.root.join(B_FLAC).exists());
}

#[tokio::test]
async fn fix_duplicates_needs_delete_on_and_a_second_copy() {
    let f = Factory::new().await;
    f.fix_from_apps(None);
    let a = f.copy("A", A_BEST, &BEST, 24, 20, false);
    f.library(&[a]);
    let ask = || LibraryActionRequest::new(LibraryAction::FixDuplicates, "A", "alice");

    let alone = f
        .state
        .library_action_executor
        .apply(ask())
        .await
        .expect("an outcome");
    assert_eq!(alone.state, LibraryActionState::Skipped);
    assert_eq!(
        alone.detail.as_deref(),
        Some("Octo does not see another copy of this song.")
    );

    let mut settings = (*f.state.settings.current()).clone();
    settings
        .library_actions
        .actions
        .retain(|action| action.action != LibraryAction::Delete);
    f.state.settings.set(settings);
    let off = f
        .state
        .library_action_executor
        .apply(ask())
        .await
        .expect("an outcome");
    assert_eq!(off.state, LibraryActionState::Skipped);
    assert!(
        off.detail
            .as_deref()
            .is_some_and(|d| d.contains("turn on the Delete action"))
    );
}
