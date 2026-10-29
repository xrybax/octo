//! DownloadAlbumOnSongStar: a heart on an outside song also queues its album down the
//! album-heart chain, with the album id the song row shows the client.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use http_body_util::BodyExt;
use octo_core::settings::{DownloadSource, HeartDownloadSource, HeartDownloadStep};
use octo_core::soulseek::soulseek_metadata_service::{RoutingKind, SoulseekRouting};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer};

use super::*;
use crate::app::{AppState, TestServices};
use crate::controllers::subsonic::test_app::{NAVIDROME_OK, navidrome_json, settings};
use crate::services::common::test_fakes::{FakeDownloads, until};
use crate::services::common::{AcquisitionRequest, TrackAcquisitionQueue};

struct Fixture {
    state: AppState,
    _navidrome: MockServer,
    downloads: Arc<FakeDownloads>,
    song: String,
}

impl Fixture {
    /// Teardrop, on Mezzanine. YouTube only, so the chains start at once rather than waiting
    /// out Soulseek.
    async fn new(album_on_song_star: bool, album_hearts: bool) -> Fixture {
        let navidrome = MockServer::start().await;
        Mock::given(path("/rest/ping"))
            .respond_with(navidrome_json(NAVIDROME_OK))
            .mount(&navidrome)
            .await;
        let mut app = settings(&navidrome.uri());
        app.subsonic.download_album_on_song_star = album_on_song_star;
        app.subsonic.heart_download_sources = vec![HeartDownloadStep {
            source: HeartDownloadSource::YouTube,
            song_enabled: Some(true),
            album_enabled: Some(album_hearts),
            ..Default::default()
        }];
        let downloads = Arc::new(FakeDownloads::default());
        let state = AppState::for_tests_with(
            app,
            TestServices {
                download_service: Some(downloads.clone()),
                ..Default::default()
            },
        );
        let song = register_song(&state, "Teardrop", Some("Mezzanine"));
        Fixture {
            state,
            _navidrome: navidrome,
            downloads,
            song,
        }
    }

    async fn star(&self, id: &str) -> serde_json::Value {
        self.star_all(&[id]).await
    }

    /// One star carrying every id.
    async fn star_all(&self, ids: &[&str]) -> serde_json::Value {
        let ids: String = ids.iter().map(|id| format!("id={id}&")).collect();
        let request = Request::builder()
            .uri(format!(
                "/rest/star.view?{ids}u=winters&t=tok&s=salt&v=1.16.1&c=test&f=json"
            ))
            .body(Body::empty())
            .expect("a request");
        let response = star(State(self.state.clone()), request).await.expect("answered");
        let body = response.into_body().collect().await.expect("read").to_bytes();
        serde_json::from_slice(&body).expect("JSON")
    }

    /// The songs queued, each finished as it is taken.
    async fn queued(&self) -> Vec<String> {
        let mut ids = Vec::new();
        while let Some(request) = next_queued(&self.state.track_acquisition_queue).await {
            self.state.track_acquisition_queue.release(&request);
            request
                .completion
                .try_set_result(format!("/music/{}.flac", request.external_id));
            ids.push(request.external_id.clone());
        }
        ids
    }

    /// The albums the download service was asked for, as (provider, album id, source).
    fn albums(&self) -> Vec<(String, String, DownloadSource)> {
        self.downloads
            .album_calls
            .lock()
            .iter()
            .map(|call| (call.0.clone(), call.1.clone(), call.2))
            .collect()
    }
}

fn register_song(state: &AppState, title: &str, album: Option<&str>) -> String {
    state.external_id_registry.register(SoulseekRouting {
        kind: RoutingKind::Song,
        artist: Some("Massive Attack".into()),
        title: Some(title.into()),
        album: album.map(str::to_string),
        ..Default::default()
    })
}

/// The album id a song row on `album` shows the client.
fn album_id(state: &AppState, album: &str) -> String {
    state.external_id_registry.register(SoulseekRouting {
        kind: RoutingKind::Album,
        artist: Some("Massive Attack".into()),
        album: Some(album.into()),
        ..Default::default()
    })
}

async fn next_queued(queue: &TrackAcquisitionQueue) -> Option<Arc<AcquisitionRequest>> {
    tokio::time::timeout(
        Duration::from_millis(500),
        queue.dequeue(&CancellationToken::new()),
    )
    .await
    .ok()
    .flatten()
}

#[tokio::test]
async fn a_song_heart_fetches_only_the_song_by_default() {
    let fixture = Fixture::new(false, true).await;

    let reply = fixture.star(&fixture.song).await;

    assert_eq!(reply["subsonic-response"]["status"], "ok", "{reply}");
    assert_eq!(fixture.queued().await, vec![fixture.song.clone()]);
    assert!(fixture.albums().is_empty());
}

#[tokio::test]
async fn a_song_heart_also_fetches_the_rest_of_its_album_when_asked() {
    let fixture = Fixture::new(true, true).await;

    let reply = fixture.star(&fixture.song).await;

    assert_eq!(reply["subsonic-response"]["status"], "ok", "{reply}");
    // The song goes first; the album waits until it is in and shows in the library.
    let request = next_queued(&fixture.state.track_acquisition_queue)
        .await
        .expect("the song is queued");
    assert_eq!(request.external_id, fixture.song);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(fixture.albums().is_empty(), "the album waits for the song");
    fixture.state.track_acquisition_queue.release(&request);
    request
        .completion
        .try_set_result(format!("/music/{}.flac", request.external_id));
    fixture
        .state
        .acquisition_tracker
        .complete("soulseek", &fixture.song, Some("nd-1"));
    until(|| !fixture.albums().is_empty()).await;
    assert_eq!(
        fixture.albums(),
        [(
            "soulseek".to_string(),
            album_id(&fixture.state, "Mezzanine"),
            DownloadSource::YouTube
        )]
    );
}

#[tokio::test]
async fn a_song_heart_leaves_the_album_alone_when_no_source_takes_album_hearts() {
    let fixture = Fixture::new(true, false).await;

    fixture.star(&fixture.song).await;

    assert_eq!(fixture.queued().await, vec![fixture.song.clone()]);
    assert!(fixture.albums().is_empty());
}

#[tokio::test]
async fn the_album_is_the_one_the_song_row_names() {
    let fixture = Fixture::new(true, true).await;
    let single = register_song(&fixture.state, "Unfinished Sympathy", None);

    assert_eq!(
        album_of_song(&fixture.state, &fixture.song),
        Some(album_id(&fixture.state, "Mezzanine"))
    );
    // A song with no album name is shown as a single named after itself.
    assert_eq!(
        album_of_song(&fixture.state, &single),
        Some(album_id(&fixture.state, "Unfinished Sympathy"))
    );
    assert_eq!(album_of_song(&fixture.state, "never-registered"), None);
}

/// Lands a queued song and closes its row, as the worker and the library would.
fn land(fixture: &Fixture, request: &Arc<AcquisitionRequest>) {
    fixture.state.track_acquisition_queue.release(request);
    request
        .completion
        .try_set_result(format!("/music/{}.flac", request.external_id));
    fixture
        .state
        .acquisition_tracker
        .complete("soulseek", &request.external_id, Some("nd-1"));
}

#[tokio::test]
async fn songs_of_one_album_hearted_together_bring_the_album_once() {
    let fixture = Fixture::new(true, true).await;
    let angel = register_song(&fixture.state, "Angel", Some("Mezzanine"));

    let reply = fixture.star_all(&[&fixture.song, &angel]).await;

    assert_eq!(reply["subsonic-response"]["status"], "ok", "{reply}");
    let queue = &fixture.state.track_acquisition_queue;
    let first = next_queued(queue).await.expect("a song is queued");
    let second = next_queued(queue).await.expect("both songs are queued");
    let mut songs = vec![first.external_id.clone(), second.external_id.clone()];
    songs.sort();
    let mut hearted = vec![fixture.song.clone(), angel.clone()];
    hearted.sort();
    assert_eq!(songs, hearted);

    // The album waits for both songs, then is walked once.
    land(&fixture, &first);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        fixture.albums().is_empty(),
        "the album waits for every hearted song"
    );
    land(&fixture, &second);
    until(|| !fixture.albums().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        fixture.albums(),
        [(
            "soulseek".to_string(),
            album_id(&fixture.state, "Mezzanine"),
            DownloadSource::YouTube
        )]
    );
}

#[tokio::test]
async fn a_song_hearted_with_its_album_brings_the_album_once() {
    let fixture = Fixture::new(true, true).await;
    let album = album_id(&fixture.state, "Mezzanine");

    let reply = fixture.star_all(&[&fixture.song, &album]).await;

    assert_eq!(reply["subsonic-response"]["status"], "ok", "{reply}");
    let request = next_queued(&fixture.state.track_acquisition_queue)
        .await
        .expect("the song is queued");
    land(&fixture, &request);
    until(|| !fixture.albums().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fixture.albums().len(), 1, "{:?}", fixture.albums());
}
