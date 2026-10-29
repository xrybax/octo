//! The Test's words for each thing that can be missing, and the client's calls, against a
//! stand-in SpotiFLAC.

use std::sync::Arc;
use std::time::Duration;

use octo_core::settings::{AppSettings, SettingsStore, SpotiflacSettings};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

fn client(settings: SpotiflacSettings) -> SpotiflacClient {
    SpotiflacClient::with_poll_interval(
        Arc::new(SettingsStore::for_tests(AppSettings {
            spotiflac: settings,
            ..Default::default()
        })),
        Duration::from_millis(5),
    )
}

fn connection(url: &str, token: &str) -> Connection {
    Connection {
        url: url.to_string(),
        token: token.to_string(),
    }
}

async fn spotiflac(info: serde_json::Value, extensions: serde_json::Value) -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(info))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/extensions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(extensions))
        .mount(&server)
        .await;
    server
}

fn info(authenticated: bool, multiuser: bool) -> serde_json::Value {
    json!({"name": "SpotiFLAC", "version": "5.0.5", "api_version": "v1", "multiuser": multiuser, "authenticated": authenticated})
}

fn two_extensions() -> serde_json::Value {
    json!({"providers": [{"provider": "one", "installed": true}, {"provider": "two", "installed": true}], "totals": {}})
}

fn texts(report: &TestReport, state: &str) -> Vec<String> {
    report
        .checks
        .iter()
        .filter(|check| check.state == state)
        .map(|check| check.text.clone())
        .collect()
}

#[tokio::test]
async fn a_test_with_no_address_says_to_type_one() {
    let music = tempfile::tempdir().expect("a folder");
    let report = client(SpotiflacSettings::default())
        .test(&connection("", ""), false, None, &music.path().to_string_lossy())
        .await;
    assert!(!report.ok);
    assert_eq!(
        report.message,
        "No address yet. Type the address of your SpotiFLAC server, for example http://spotiflac:8000."
    );
}

#[tokio::test]
async fn a_test_that_reaches_nothing_says_where_it_looked() {
    let music = tempfile::tempdir().expect("a folder");
    let report = client(SpotiflacSettings::default())
        .test(
            &connection("http://127.0.0.1:1", ""),
            true,
            None,
            &music.path().to_string_lossy(),
        )
        .await;
    assert!(!report.ok);
    assert!(
        report
            .message
            .starts_with("Octo could not reach SpotiFLAC at http://127.0.0.1:1."),
        "{}",
        report.message
    );
}

#[tokio::test]
async fn a_wrong_token_is_named() {
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Missing or invalid ?token="))
        .mount(&server)
        .await;
    let music = tempfile::tempdir().expect("a folder");
    let report = client(SpotiflacSettings::default())
        .test(
            &connection(&server.uri(), "wrong"),
            true,
            None,
            &music.path().to_string_lossy(),
        )
        .await;
    assert!(!report.ok);
    assert!(
        report.message.contains("refused the web token"),
        "{}",
        report.message
    );
}

#[tokio::test]
async fn an_address_that_is_not_spotiflac_is_named() {
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>Navidrome</html>"))
        .mount(&server)
        .await;
    let music = tempfile::tempdir().expect("a folder");
    let report = client(SpotiflacSettings::default())
        .test(
            &connection(&server.uri(), ""),
            true,
            None,
            &music.path().to_string_lossy(),
        )
        .await;
    assert!(!report.ok);
    assert!(
        report.message.contains("answers, but not as SpotiFLAC's web API"),
        "{}",
        report.message
    );
}

#[tokio::test]
async fn no_extensions_says_what_to_bring() {
    let server = spotiflac(info(false, false), json!({"providers": [], "totals": {}})).await;
    let music = tempfile::tempdir().expect("a folder");
    let report = client(SpotiflacSettings::default())
        .test(
            &connection(&server.uri(), ""),
            true,
            None,
            &music.path().to_string_lossy(),
        )
        .await;
    assert!(!report.ok);
    assert!(
        report
            .message
            .starts_with("SpotiFLAC has no extensions installed"),
        "{}",
        report.message
    );
    assert!(
        report.message.contains("SPOTIFLAC_REGISTRIES"),
        "{}",
        report.message
    );
    assert_eq!(report.extensions, Some(0));
    assert!(
        texts(&report, "warn")
            .iter()
            .any(|text| text.contains("no web token")),
        "{:?}",
        report.checks
    );
}

#[tokio::test]
async fn a_multi_user_spotiflac_is_refused() {
    let server = spotiflac(info(true, true), two_extensions()).await;
    let music = tempfile::tempdir().expect("a folder");
    let report = client(SpotiflacSettings::default())
        .test(
            &connection(&server.uri(), "t"),
            true,
            None,
            &music.path().to_string_lossy(),
        )
        .await;
    assert!(!report.ok);
    assert!(report.message.contains("--web-multiuser"), "{}", report.message);
}

#[tokio::test]
async fn a_ready_spotiflac_passes_and_the_folder_is_made() {
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/info"))
        .and(header("cookie", "spotiflac_web_token=s3cret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(info(true, false)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/extensions"))
        .and(header("cookie", "spotiflac_web_token=s3cret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(two_extensions()))
        .mount(&server)
        .await;
    let music = tempfile::tempdir().expect("a folder");
    let root = music.path().to_string_lossy().into_owned();
    let report = client(SpotiflacSettings::default())
        .test(
            &connection(&server.uri(), "s3cret"),
            true,
            Some("/downloads"),
            &root,
        )
        .await;
    assert!(report.ok, "{:?}", report.checks);
    assert_eq!(report.message, "SpotiFLAC is ready.");
    assert_eq!(report.version.as_deref(), Some("5.0.5"));
    assert_eq!(report.extensions, Some(2));
    assert!(music.path().join(SPOTIFLAC_FOLDER).is_dir());
    assert!(
        texts(&report, "ok")
            .iter()
            .any(|text| text.contains("/downloads/.octo-incoming/spotiflac on SpotiFLAC's side")),
        "{:?}",
        report.checks
    );
}

#[tokio::test]
async fn a_ready_spotiflac_with_the_switch_off_says_so() {
    let server = spotiflac(info(true, false), two_extensions()).await;
    let music = tempfile::tempdir().expect("a folder");
    let report = client(SpotiflacSettings::default())
        .test(
            &connection(&server.uri(), "t"),
            false,
            None,
            &music.path().to_string_lossy(),
        )
        .await;
    assert!(report.ok);
    assert_eq!(report.message, "SpotiFLAC is ready, with a warning below.");
    assert!(
        texts(&report, "warn")
            .iter()
            .any(|text| text.contains("The switch is off")),
        "{:?}",
        report.checks
    );
}

#[tokio::test]
async fn a_download_is_asked_with_the_quality_the_extensions_and_the_folder() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/downloads"))
        .and(wiremock::matchers::body_json(json!({
            "url": "https://open.spotify.com/track/abc",
            "quality": "HI_RES_LOSSLESS",
            "services": ["ext:one", "ext:two"],
            "output_dir": "/downloads/.octo-incoming/spotiflac/j1",
        })))
        .respond_with(
            ResponseTemplate::new(202)
                .set_body_json(json!({"id": "job-1", "status": "queued", "created_at": 1.0})),
        )
        .mount(&server)
        .await;
    let settings = SpotiflacSettings {
        enabled: true,
        url: server.uri(),
        quality: "hi_res_lossless".into(),
        services: "ext:one, ext:two".into(),
        ..Default::default()
    };
    let job = client(settings.clone())
        .queue(
            &connection(&server.uri(), ""),
            "https://open.spotify.com/track/abc",
            &settings,
            "/downloads/.octo-incoming/spotiflac/j1",
        )
        .await
        .expect("queued");
    assert_eq!(job.id, "job-1");
}

#[tokio::test]
async fn spotiflacs_own_error_words_come_through() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/downloads"))
        .respond_with(ResponseTemplate::new(429).set_body_json(
            json!({"error": "Too many downloads already queued.", "detail": "50 queued, limit 50."}),
        ))
        .mount(&server)
        .await;
    let settings = SpotiflacSettings::default();
    let error = client(settings.clone())
        .queue(&connection(&server.uri(), ""), "https://x/y", &settings, "/d")
        .await
        .expect_err("refused");
    assert_eq!(
        error.to_string(),
        "SpotiFLAC said: Too many downloads already queued. 50 queued, limit 50."
    );
}

#[tokio::test]
async fn songs_asked_for_at_once_keep_to_the_setting() {
    let client = Arc::new(client(SpotiflacSettings {
        at_once: 1,
        ..Default::default()
    }));
    let first = client.slot().await;
    let waiting = {
        let client = client.clone();
        tokio::spawn(async move {
            let _second = client.slot().await;
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiting.is_finished(), "the second song waits for the first");
    drop(first);
    tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("the second song starts once the first is done")
        .expect("no panic");
}
