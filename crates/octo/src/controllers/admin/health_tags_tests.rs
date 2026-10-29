//! Library health's "Tags that disagree with the audio" through the assembled app: the finding
//! in the report beside the app's checks, its preview and fix steps, and the tag check's own
//! status, pause, start and start over.

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use futures::FutureExt;
use indexmap::IndexMap;
use octo_core::settings::{AppSettings, LibraryActionSettings};
use serde_json::{Value, json};

use super::test_app::{Reply, app, send};
use crate::app::{AppState, TestServices};
use crate::http::pipeline::App;
use crate::services::library::LibrarySnapshot;
use crate::services::library::library_identity_sweep::{CheckedSong, KeptFinding, stamp_of};
use crate::services::library::library_snapshot::ReadPage;
use crate::services::library::quality_upgrade_worker::LibrarySongRow;
use crate::services::test_support::mp3;

const STUDIO: &str = "28982738-0000-4000-8000-000000000001";

struct Setup {
    _dir: tempfile::TempDir,
    state: AppState,
    app: App,
    token: String,
}

/// A library of two songs, with the tag check's answers for both: one sure finding that can
/// be fixed, and one it found but cannot say what to write for.
fn setup(actions: LibraryActionSettings) -> Setup {
    let dir = tempfile::tempdir().expect("a temp dir");
    let music = dir.path().join("music");
    std::fs::create_dir_all(&music).expect("a music folder");
    let parade = music.join("parade.mp3");
    let guess = music.join("guess.mp3");
    std::fs::write(&parade, mp3()).expect("written");
    std::fs::write(&guess, mp3()).expect("written");

    let rows = [
        LibrarySongRow {
            id: "song-parade".into(),
            path: "parade.mp3".into(),
            title: "Welcome to the Black Parade".into(),
            artist: "My Chemical Romance".into(),
            album: Some("The Black Parade (instrumentals)".into()),
            album_id: Some("al-1".into()),
            ..Default::default()
        },
        LibrarySongRow {
            id: "song-guess".into(),
            path: "guess.mp3".into(),
            title: "Teenagers".into(),
            artist: "My Chemical Romance".into(),
            album: Some("The Black Parade".into()),
            album_id: Some("al-2".into()),
            ..Default::default()
        },
    ];
    let read_page: ReadPage = Arc::new(move |start, count| {
        let page: Vec<LibrarySongRow> = rows
            .iter()
            .skip(start as usize)
            .take(count as usize)
            .cloned()
            .collect();
        let held = page.len();
        async move { Some((page, held)) }.boxed()
    });
    let snapshot = Arc::new(LibrarySnapshot::with(
        read_page,
        octo_core::common::Clock::system(),
    ));
    let state = AppState::for_tests_with(
        AppSettings {
            library_actions: actions,
            ..Default::default()
        },
        TestServices {
            config_dir: Some(dir.path().join("config")),
            library_snapshot: Some(snapshot),
            ..Default::default()
        },
    );
    let kept = |path: &std::path::Path, title: &str, album: &str| CheckedSong {
        stamp: stamp_of(&path.to_string_lossy()).expect("a stamp"),
        path: path.to_string_lossy().into_owned(),
        artist: "My Chemical Romance".into(),
        title: title.into(),
        album: album.into(),
        ..CheckedSong::default()
    };
    state.identity_sweep_store.update(|s| {
        let mut changes = IndexMap::new();
        changes.insert("album".to_string(), Some("The Black Parade".to_string()));
        changes.insert("recordingId".to_string(), Some(STUDIO.to_string()));
        changes.insert("releaseTrackId".to_string(), None);
        s.songs.insert(
            "song-parade".into(),
            CheckedSong {
                recording_id: Some(STUDIO.into()),
                kind: Some("Studio".into()),
                kind_certain: true,
                sure: true,
                findings: vec![KeptFinding {
                    rule: "album".into(),
                    words: "The album names an instrumental.".into(),
                }],
                changes,
                ..kept(
                    &parade,
                    "Welcome to the Black Parade",
                    "The Black Parade (instrumentals)",
                )
            },
        );
        s.songs.insert(
            "song-guess".into(),
            CheckedSong {
                kind: Some("Studio".into()),
                findings: vec![KeptFinding {
                    rule: "ids".into(),
                    words: "The release track id is a track of a live recording.".into(),
                }],
                ..kept(&guess, "Teenagers", "The Black Parade")
            },
        );
        s.total = 2;
    });
    let token = state.browse_sessions.create("admin");
    Setup {
        app: app(state.clone()),
        state,
        token,
        _dir: dir,
    }
}

fn may_fix() -> LibraryActionSettings {
    LibraryActionSettings {
        enabled: true,
        dry_run: false,
        allowed_users: vec!["admin".into()],
        ..Default::default()
    }
}

impl Setup {
    async fn get(&self, uri: &str) -> Reply {
        let cookie = format!("octo_browse={}", self.token);
        send(&self.app, Method::GET, uri, &[("Cookie", &cookie)], None).await
    }

    async fn post(&self, uri: &str, body: Value) -> Reply {
        let cookie = format!("octo_browse={}", self.token);
        send(
            &self.app,
            Method::POST,
            uri,
            &[("X-Octo-Admin", "1"), ("Cookie", &cookie)],
            Some(&body.to_string()),
        )
        .await
    }
}

#[tokio::test]
async fn the_report_lists_the_tag_finding_beside_the_apps_checks() {
    let setup = setup(may_fix());
    let reply = setup.get("/api/admin/health").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let report = reply.json();
    assert_eq!(report["clean"], false);
    let tags = report["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["check"] == "tags")
        .expect("the tags finding")
        .clone();
    assert_eq!(tags["title"], "Tags that disagree with the audio");
    assert_eq!(tags["count"], 2);
    assert_eq!(tags["countLabel"], "2 songs");
    assert_eq!(tags["sure"], 1);
    assert_eq!(tags["fixAllLabel"], "Fix the sure one");
    assert_eq!(tags["useLabel"], "Use the audio's tags");
    // By artist, then title.
    assert_eq!(tags["songs"], json!(["song-guess", "song-parade"]));
    let parade = &tags["items"][1];
    assert_eq!(parade["id"], "song-parade");
    assert_eq!(parade["sure"], true);
    assert_eq!(parade["heard"], "the studio recording");
    assert_eq!(parade["rules"][0]["rule"], "album");
    assert_eq!(parade["rules"][0]["label"], "Album names another take");
    assert_eq!(
        parade["changes"][0]["words"],
        "Album: The Black Parade, was The Black Parade (instrumentals)"
    );
    assert_eq!(parade["changes"][2]["words"], "Release track id: taken off");
    assert_eq!(tags["items"][0]["changes"], json!([]));
    assert_eq!(
        report["songs"]["song-parade"]["title"],
        "Welcome to the Black Parade"
    );
    assert_eq!(report["tagCheck"]["found"], 2);
    assert_eq!(report["tagCheck"]["sure"], 1);
    assert!(
        report["overview"]
            .as_str()
            .unwrap_or("")
            .contains("could be better")
    );
    // No dashes of any length in what the page shows.
    assert!(!reply.text().contains('\u{2014}') && !reply.text().contains('\u{2013}'));
}

#[tokio::test]
async fn fix_all_sure_ones_previews_only_the_sure_songs_and_its_steps_are_accepted() {
    let setup = setup(may_fix());
    let preview = setup
        .post("/api/admin/health/preview", json!({ "check": "tags" }))
        .await;
    assert_eq!(preview.status, StatusCode::OK, "{}", preview.text());
    let preview = preview.json();
    assert_eq!(preview["title"], "Use the audio's tags");
    assert_eq!(preview["canFix"], true);
    assert_eq!(preview["lines"].as_array().map(Vec::len), Some(1));
    let step = &preview["steps"][0];
    assert_eq!(step["action"], "retag");
    assert_eq!(step["id"], "song-parade");
    // The file the tags were worked out from goes with them, so a file replaced since is left.
    let stamp = setup
        .state
        .identity_sweep_store
        .read(|s| s.songs["song-parade"].stamp.clone());
    assert_eq!(
        step["with"],
        json!({ "album": "The Black Parade", "recordingId": STUDIO, "releaseTrackId": "", "fileStamp": stamp })
    );

    // The steps go through the same apply as every fix, which checks each tag it writes.
    let apply = setup
        .post(
            "/api/admin/health/apply",
            json!({ "label": preview["label"], "check": "tags", "steps": preview["steps"] }),
        )
        .await;
    assert_eq!(apply.status, StatusCode::ACCEPTED, "{}", apply.text());
    let bad = setup
        .post(
            "/api/admin/health/apply",
            json!({ "check": "tags", "steps": [{ "action": "retag", "id": "song-parade", "with": { "recordingId": "not-an-id" } }] }),
        )
        .await;
    assert!(
        bad.status == StatusCode::BAD_REQUEST || bad.status == StatusCode::CONFLICT,
        "{}",
        bad.text()
    );
}

#[test]
fn a_file_stamp_is_no_tag_and_alone_is_nothing_to_write() {
    use crate::services::health::LibraryHealthService;
    use crate::services::health::health_fixes::{FixStep, fix_actions};

    let step = |with: &[(&str, &str)]| {
        FixStep::with(
            fix_actions::RETAG,
            "song-parade",
            "Welcome to the Black Parade",
            with.iter()
                .map(|(tag, value)| (tag.to_string(), value.to_string()))
                .collect(),
        )
    };
    assert_eq!(
        LibraryHealthService::invalid(&[step(&[("album", "The Black Parade"), ("fileStamp", "1234:5678")])]),
        None
    );
    assert!(
        LibraryHealthService::invalid(&[step(&[("fileStamp", "1234:5678")])])
            .is_some_and(|why| why.starts_with("No tags to write"))
    );
}

#[tokio::test]
async fn one_song_is_previewed_by_its_id_and_a_song_with_nothing_to_write_waits() {
    let setup = setup(may_fix());
    let one = setup
        .post(
            "/api/admin/health/preview",
            json!({ "check": "tags", "keys": ["song-parade"] }),
        )
        .await;
    assert_eq!(one.status, StatusCode::OK);
    assert_eq!(one.json()["go"], "Write these tags");

    let waits = setup
        .post(
            "/api/admin/health/preview",
            json!({ "check": "tags", "keys": ["song-guess"] }),
        )
        .await;
    assert_eq!(waits.status, StatusCode::BAD_REQUEST);
    assert!(waits.text().contains("waits for you"));

    let gone = setup
        .post(
            "/api/admin/health/preview",
            json!({ "check": "tags", "keys": ["nope"] }),
        )
        .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn with_library_actions_off_the_preview_says_why_and_offers_no_fix() {
    let setup = setup(LibraryActionSettings::default());
    let preview = setup
        .post("/api/admin/health/preview", json!({ "check": "tags" }))
        .await;
    assert_eq!(preview.status, StatusCode::OK);
    let preview = preview.json();
    assert_eq!(preview["canFix"], false);
    assert!(
        preview["why"]
            .as_str()
            .unwrap_or("")
            .contains("Library actions are off")
    );
}

#[tokio::test]
async fn a_file_changed_since_it_was_checked_leaves_the_report() {
    let setup = setup(may_fix());
    let path = setup
        .state
        .identity_sweep_store
        .read(|s| s.songs["song-parade"].path.clone());
    std::fs::write(&path, [mp3(), vec![0u8; 16]].concat()).expect("rewritten");
    let report = setup.get("/api/admin/health").await.json();
    let tags = report["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["check"] == "tags")
        .cloned()
        .expect("the other song is still listed");
    assert_eq!(tags["songs"], json!(["song-guess"]));
    assert_eq!(tags["fixAllLabel"], Value::Null);
    assert!(tags["noneSure"].as_str().is_some());
}

#[tokio::test]
async fn the_tag_check_pauses_starts_and_starts_over() {
    let setup = setup(may_fix());
    let status = setup.get("/api/admin/health/tags").await;
    assert_eq!(status.status, StatusCode::OK);
    assert_eq!(status.json()["tagCheck"]["checked"], 2);
    assert_eq!(status.json()["tagCheck"]["perHour"], 60);

    let paused = setup.post("/api/admin/health/tags/pause", json!({})).await;
    assert_eq!(paused.status, StatusCode::ACCEPTED);
    assert_eq!(paused.json()["tagCheck"]["paused"], true);
    let started = setup.post("/api/admin/health/tags/start", json!({})).await;
    assert_eq!(started.status, StatusCode::ACCEPTED);
    assert_eq!(started.json()["tagCheck"]["paused"], false);

    let reset = setup.post("/api/admin/health/tags/reset", json!({})).await;
    assert_eq!(reset.status, StatusCode::ACCEPTED);
    assert_eq!(reset.json()["tagCheck"]["checked"], 0);
    let report = setup.get("/api/admin/health").await.json();
    assert!(
        report["checks"]
            .as_array()
            .is_none_or(|checks| checks.iter().all(|check| check["check"] != "tags"))
    );
}

#[tokio::test]
async fn the_tag_check_cannot_start_while_it_is_set_to_none_an_hour() {
    let setup = setup(LibraryActionSettings {
        tag_check_per_hour: 0,
        ..may_fix()
    });
    let started = setup.post("/api/admin/health/tags/start", json!({})).await;
    assert_eq!(started.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_tag_check_answers_only_a_signed_in_admin() {
    let setup = setup(may_fix());
    let anonymous = send(&setup.app, Method::GET, "/api/admin/health/tags", &[], None).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    let no_header = send(
        &setup.app,
        Method::POST,
        "/api/admin/health/tags/reset",
        &[("Cookie", &format!("octo_browse={}", setup.token))],
        None,
    )
    .await;
    assert_eq!(no_header.status, StatusCode::FORBIDDEN);
    assert_eq!(setup.state.identity_sweep_store.read(|s| s.songs.len()), 2);
}
