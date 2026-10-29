//! getAnimatedArt, the artwork on getAlbum's album, the extension listing and the relay, through
//! the app.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use octo_core::common::Clock;
use octo_core::settings::{AppSettings, SettingsStore};
use serde_json::{Value, json};
use wiremock::matchers::{any, header, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::test_app::*;
use super::with_album_field;
use crate::app::AppState;
use crate::services::charts::AppleMusicPages;
use crate::services::cover_art::ITunesCoverArtLookup;
use crate::services::cover_art::animated_art::{AnimatedArt, Clip};

const SQUARE: &str = "/itunes-assets/HLSMusic1/v4/aa/P1_default.m3u8";
const QUERY: &str = "f=json&u=alice&t=good&s=salt&v=1.16.1&c=Octo";

async fn navidrome() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(navidrome_json(NAVIDROME_OK))
        .mount(&server)
        .await;
    server
}

fn with_mode(url: &str, mode: &str) -> AppSettings {
    let mut settings = settings(url);
    settings.metadata.animated_art = mode.into();
    settings
}

fn clip(video: &str) -> Clip {
    Clip {
        video: format!("https://mvod.itunes.apple.com{video}"),
        poster: Some("https://is1-ssl.mzstatic.com/image/thumb/x/1200x1200bb.jpg".into()),
        color: Some("0a0b0c".into()),
        width: 2000,
        height: 2000,
    }
}

/// The app's covers, nothing asked of Apple, the relay reading from `video` when given.
fn covers(settings: Arc<SettingsStore>, video: Option<&str>) -> Arc<AnimatedArt> {
    let itunes = Arc::new(ITunesCoverArtLookup::with_base_url(
        None,
        "http://127.0.0.1:9",
        Clock::system(),
    ));
    let pages = Arc::new(AppleMusicPages::with_base_url(
        "http://127.0.0.1:9",
        Duration::ZERO,
    ));
    let covers = AnimatedArt::new(settings, itunes, pages, None);
    Arc::new(match video {
        Some(origin) => covers.with_video_origin(origin),
        None => covers,
    })
}

fn state_for(mode: &str, navidrome: &MockServer, video: Option<&str>) -> AppState {
    state_with(with_mode(&navidrome.uri(), mode), |inner| {
        inner.animated_art = covers(inner.settings.clone(), video);
    })
}

#[test]
fn the_field_goes_into_the_album_object_and_nowhere_else() {
    let body = br#"{"subsonic-response":{"status":"ok","album":{"id":"a1","song":[{"id":"s1","album":"Melodrama"}]}}}"#;
    let out = with_album_field(body.to_vec(), &json!({"square": {"video": "v"}}));
    let read: Value = serde_json::from_slice(&out).expect("still JSON");
    assert_eq!(
        read["subsonic-response"]["album"]["animatedArt"]["square"]["video"],
        "v"
    );
    assert_eq!(
        read["subsonic-response"]["album"]["song"][0]["album"],
        "Melodrama"
    );

    let empty = with_album_field(br#"{"album":{}}"#.to_vec(), &json!({}));
    assert_eq!(empty, br#"{"album":{"animatedArt":{}}}"#);

    let none = br#"{"subsonic-response":{"status":"ok","song":{"album":"x"}}}"#;
    assert_eq!(with_album_field(none.to_vec(), &json!({})), none.to_vec());
}

#[tokio::test]
async fn get_animated_art_answers_a_kept_cover() {
    let navidrome = navidrome().await;
    let state = state_for("relay", &navidrome, None);
    let token = state
        .animated_art
        .keep("Lorde", "Melodrama", Some(clip(SQUARE)), None);

    let reply = get(
        &app(&state),
        &format!("/rest/getAnimatedArt?artist=Lorde&album=Melodrama&{QUERY}"),
    )
    .await;
    let answer = reply.json();
    let cover = &answer["subsonic-response"]["animatedArt"];
    assert_eq!(
        cover["square"]["video"],
        json!(format!("animated/{token}/square.m3u8"))
    );
    assert_eq!(cover["square"]["color"], "0a0b0c");
    assert!(cover.get("tall").is_none());
}

#[tokio::test]
async fn get_animated_art_says_when_covers_are_off() {
    let navidrome = navidrome().await;
    let state = state_for("off", &navidrome, None);
    let reply = get(
        &app(&state),
        &format!("/rest/getAnimatedArt?artist=Lorde&album=Melodrama&{QUERY}"),
    )
    .await;
    let answer = reply.json();
    assert_eq!(answer["subsonic-response"]["status"], "failed");
    assert_eq!(
        answer["subsonic-response"]["error"]["message"],
        "Animated art is off on this server"
    );
}

#[tokio::test]
async fn the_extension_is_listed_while_covers_are_on() {
    let navidrome = navidrome().await;
    for (mode, listed) in [("direct", true), ("relay", true), ("off", false)] {
        let state = state_for(mode, &navidrome, None);
        let answer = get(&app(&state), &format!("/rest/getOpenSubsonicExtensions?{QUERY}"))
            .await
            .json();
        let names: Vec<String> = answer["subsonic-response"]["openSubsonicExtensions"]
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|e| e["name"].as_str().map(str::to_string))
            .collect();
        assert_eq!(names.iter().any(|n| n == "octoAnimatedArt"), listed, "{mode}");
    }
}

#[tokio::test]
async fn the_relay_serves_the_playlist_rewritten_and_the_video_with_its_range() {
    let navidrome = navidrome().await;
    let video = MockServer::start().await;
    Mock::given(path(SQUARE))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\nseg1.m4s\n"),
        )
        .mount(&video)
        .await;
    Mock::given(path("/itunes-assets/HLSMusic1/v4/aa/seg1.m4s"))
        .and(header("range", "bytes=2-5"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 2-5/10")
                .insert_header("content-type", "video/mp4")
                .set_body_bytes(vec![2, 3, 4, 5]),
        )
        .mount(&video)
        .await;
    let state = state_for("relay", &navidrome, Some(&video.uri()));
    let token = state
        .animated_art
        .keep("Lorde", "Melodrama", Some(clip(SQUARE)), None);
    let app = app(&state);

    let playlist = get(&app, &format!("/animated/{token}/square.m3u8")).await;
    assert_eq!(playlist.status, StatusCode::OK);
    assert_eq!(
        playlist.header("content-type"),
        Some("application/vnd.apple.mpegurl")
    );
    let text = String::from_utf8(playlist.body.to_vec()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines[1].starts_with("#EXT-X-MAP:URI=\"") && lines[1].ends_with(".mp4\""),
        "{text}"
    );
    let segment = lines[2].to_string();
    assert!(segment.ends_with(".m4s"), "{text}");

    let part = send(
        &app,
        Method::GET,
        &format!("/animated/{token}/{segment}"),
        &[("Range", "bytes=2-5")],
        Body::empty(),
    )
    .await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.header("content-range"), Some("bytes 2-5/10"));
    assert_eq!(part.body.as_ref(), [2, 3, 4, 5]);
    assert!(
        part.header("cache-control")
            .is_some_and(|c| c.contains("immutable"))
    );

    // No such token, and nothing at all in direct mode.
    assert_eq!(
        get(&app, "/animated/nope/square.m3u8").await.status,
        StatusCode::NOT_FOUND
    );
    let direct = state_for("direct", &navidrome, Some(&video.uri()));
    let token = direct
        .animated_art
        .keep("Lorde", "Melodrama", Some(clip(SQUARE)), None);
    assert_eq!(
        get(
            &super::super::test_app::app(&direct),
            &format!("/animated/{token}/square.m3u8")
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}
