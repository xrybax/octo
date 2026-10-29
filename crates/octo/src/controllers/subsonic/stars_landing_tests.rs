//! A heart from another app on an outside song, through its download landing, the way an app
//! like Arpeggi uses it: what a refresh reads after it (getStarred2, getSong, the song inside a
//! list), the heart taken back, the heart sent again once the song is in the library, and the
//! heart sent several times while the song downloads.

use std::sync::Arc;
use std::time::Duration;

use octo_core::settings::{AppSettings, SoulseekSettings};
use octo_core::soulseek::soulseek_metadata_service::{RoutingKind, SoulseekRouting};
use parking_lot::Mutex;
use serde_json::{Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request as MockRequest, Respond, ResponseTemplate};

use crate::app::AppState;
use crate::controllers::subsonic::test_app::*;
use crate::http::pipeline::App;

/// The library id the downloaded song gets in Navidrome.
const LANDED: &str = "nd-teardrop";

/// Navidrome as an app's refresh meets it: the token "good" signs anyone in, stars and unstars
/// change which library songs are favorites, and getSong and getStarred2 say so.
struct LandingNavidrome {
    starred: Arc<Mutex<Vec<String>>>,
}

impl Respond for LandingNavidrome {
    fn respond(&self, request: &MockRequest) -> ResponseTemplate {
        let path = request.url.path().trim_end_matches(".view").to_string();
        let query = query_of(request);
        if query.get("t").map(String::as_str) != Some("good") {
            return navidrome_json(NAVIDROME_WRONG_PASSWORD);
        }
        let ids: Vec<String> = request
            .url
            .query_pairs()
            .filter(|(k, _)| k == "id")
            .map(|(_, v)| v.into_owned())
            .collect();
        let envelope = |body: Value| {
            let mut response = json!({"status": "ok", "version": "1.16.1", "type": "navidrome"});
            if let (Some(response), Value::Object(body)) = (response.as_object_mut(), body) {
                response.extend(body);
            }
            navidrome_json(&json!({ "subsonic-response": response }).to_string())
        };
        let song = |id: &str, starred: bool| {
            let mut song =
                json!({"id": id, "title": "Teardrop", "artist": "Massive Attack", "albumId": "al-1"});
            if starred {
                song["starred"] = json!("2026-10-14T22:00:00.000Z");
            }
            song
        };
        let mut starred = self.starred.lock();
        if path.ends_with("/rest/star") {
            for id in ids {
                if !starred.contains(&id) {
                    starred.push(id);
                }
            }
            return envelope(json!({}));
        }
        if path.ends_with("/rest/unstar") {
            starred.retain(|id| !ids.contains(id));
            return envelope(json!({}));
        }
        if path.ends_with("/rest/getSong") {
            let id = ids.first().cloned().unwrap_or_default();
            let on = starred.contains(&id);
            return envelope(json!({ "song": song(&id, on) }));
        }
        if path.ends_with("/rest/getStarred2") {
            let songs: Vec<Value> = starred.iter().map(|id| song(id, true)).collect();
            return envelope(json!({ "starred2": { "song": songs } }));
        }
        envelope(json!({}))
    }
}

struct Landing {
    state: AppState,
    app: App,
    navidrome: MockServer,
    _directory: tempfile::TempDir,
}

impl Landing {
    async fn new(favorite_downloads: bool) -> Landing {
        let navidrome = MockServer::builder().start().await;
        Mock::given(any())
            .respond_with(LandingNavidrome {
                starred: Arc::default(),
            })
            .mount(&navidrome)
            .await;
        let directory = tempfile::tempdir().expect("a temp dir");
        let mut settings = AppSettings {
            soulseek: SoulseekSettings {
                base_url: Some("http://127.0.0.1:1".into()),
                ..Default::default()
            },
            ..settings(&navidrome.uri())
        };
        settings.subsonic.star_downloads_for_requester = favorite_downloads;
        let state = state_with(settings, |_| {});
        state
            .settings
            .set_raw("Library:DownloadPath", Some(&directory.path().to_string_lossy()));
        Landing {
            app: app(&state),
            state,
            navidrome,
            _directory: directory,
        }
    }

    fn teardrop(&self) -> String {
        self.state.external_id_registry.register(SoulseekRouting {
            kind: RoutingKind::Song,
            artist: Some("Massive Attack".into()),
            title: Some("Teardrop".into()),
            album: Some("Mezzanine".into()),
            duration: Some(330),
            ..Default::default()
        })
    }

    async fn star(&self, sign_in: &str, id: &str) {
        let reply = get(&self.app, &format!("/rest/star.view?{sign_in}&f=json&id={id}")).await;
        assert_eq!(reply.envelope()["status"], "ok", "{}", reply.text());
    }

    async fn unstar(&self, sign_in: &str, id: &str) {
        let reply = get(&self.app, &format!("/rest/unstar.view?{sign_in}&f=json&id={id}")).await;
        assert_eq!(reply.envelope()["status"], "ok", "{}", reply.text());
    }

    /// The outside song's download lands and Navidrome shows it as [`LANDED`].
    fn land(&self, id: &str) {
        let (_, provider, external_id) = self.state.local_library.parse_song_id(id);
        self.state.acquisition_tracker.complete(
            &provider.expect("a provider"),
            &external_id.expect("an external id"),
            Some(LANDED),
        );
    }

    async fn starred2_ids(&self, sign_in: &str) -> Vec<String> {
        let reply = get(&self.app, &format!("/rest/getStarred2.view?{sign_in}&f=json")).await;
        reply.envelope()["starred2"]["song"]
            .as_array()
            .map(|songs| {
                songs
                    .iter()
                    .filter_map(|song| song["id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// What getSong says of `id` for this sign-in: its `starred` time, if any.
    async fn song_starred(&self, sign_in: &str, id: &str) -> Option<String> {
        let reply = get(&self.app, &format!("/rest/getSong.view?{sign_in}&f=json&id={id}")).await;
        let envelope = reply.envelope();
        assert_eq!(envelope["status"], "ok", "{}", reply.text());
        envelope["song"]["starred"].as_str().map(str::to_string)
    }

    /// The ids of every call Navidrome received at `endpoint`, one list per call, with who sent it.
    async fn calls_to(&self, endpoint: &str) -> Vec<(String, Vec<String>)> {
        self.navidrome
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| {
                r.url
                    .path()
                    .trim_end_matches(".view")
                    .ends_with(&format!("/rest/{endpoint}"))
            })
            .map(|r| {
                let user = query_of(r).get("u").cloned().unwrap_or_default();
                let ids = r
                    .url
                    .query_pairs()
                    .filter(|(k, _)| k == "id")
                    .map(|(_, v)| v.into_owned())
                    .collect();
                (user, ids)
            })
            .collect()
    }

    async fn until_starred(&self, count: usize) {
        for _ in 0..500 {
            if self.calls_to("star").await.len() >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "Navidrome never got {count} star(s): {:?}",
            self.calls_to("star").await
        );
    }
}

fn arpeggi(user: &str) -> String {
    auth(user, "good", "Arpeggi")
}

/// With downloads made favorites, the heart shows while the song downloads (in getStarred2 and
/// on the song itself), lands as the library copy's favorite for whoever hearted it, and the id
/// the app hearted keeps reading as a favorite after a refresh.
#[tokio::test]
async fn a_heart_from_another_app_is_a_favorite_through_the_landing() {
    let t = Landing::new(true).await;
    let song = t.teardrop();
    let alice = arpeggi("alice");

    t.star(&alice, &song).await;
    assert_eq!(t.starred2_ids(&alice).await, std::slice::from_ref(&song));
    assert!(t.song_starred(&alice, &song).await.is_some());

    t.land(&song);
    t.until_starred(1).await;
    assert_eq!(
        t.calls_to("star").await,
        [("alice".to_string(), vec![LANDED.to_string()])]
    );
    assert_eq!(t.starred2_ids(&alice).await, [LANDED]);
    assert!(
        t.song_starred(&alice, &song).await.is_some(),
        "the hearted id still reads as a favorite"
    );
    assert!(t.song_starred(&alice, LANDED).await.is_some());

    let xml = get(&t.app, &format!("/rest/getSong.view?{alice}&id={song}")).await;
    assert!(xml.text().contains(r#"starred=""#), "{}", xml.text());
}

/// Nobody else sees someone's heart on an outside song.
#[tokio::test]
async fn someone_elses_heart_is_not_marked() {
    let t = Landing::new(true).await;
    let song = t.teardrop();
    t.star(&arpeggi("alice"), &song).await;

    assert!(t.song_starred(&arpeggi("carol"), &song).await.is_none());
    assert!(t.starred2_ids(&arpeggi("carol")).await.is_empty());
}

/// Taking the heart back once the song landed takes the library copy's favorite back too.
#[tokio::test]
async fn taking_the_heart_back_after_it_landed_unstars_the_library_copy() {
    let t = Landing::new(true).await;
    let song = t.teardrop();
    let alice = arpeggi("alice");
    t.star(&alice, &song).await;
    t.land(&song);
    t.until_starred(1).await;
    for _ in 0..100 {
        if t.song_starred(&alice, &song).await.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    t.unstar(&alice, &song).await;

    assert_eq!(
        t.calls_to("unstar").await,
        [("alice".to_string(), vec![LANDED.to_string()])]
    );
    assert!(t.starred2_ids(&alice).await.is_empty());
    assert!(t.song_starred(&alice, &song).await.is_none());
}

/// With downloads not made favorites (the default), a heart on a song you do not have only
/// fetches it: it is not listed as a favorite, and nothing is favorited when it lands.
#[tokio::test]
async fn with_download_favorites_off_a_heart_only_fetches_the_song() {
    let t = Landing::new(false).await;
    let song = t.teardrop();
    let alice = arpeggi("alice");

    t.star(&alice, &song).await;
    assert!(t.starred2_ids(&alice).await.is_empty());
    assert!(t.song_starred(&alice, &song).await.is_none());

    t.land(&song);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(t.calls_to("star").await.is_empty());
    assert_eq!(t.state.star_on_arrival.held(), 0);
}

/// Once the song is in the library, a heart on the id the app still has is a heart on a song you
/// have: Navidrome favorites the library copy at once, nothing is fetched again, and a refresh
/// shows the heart, whatever the download setting says.
#[tokio::test]
async fn a_heart_sent_again_after_the_song_landed_is_a_favorite_at_once() {
    for favorite_downloads in [false, true] {
        let t = Landing::new(favorite_downloads).await;
        let song = t.teardrop();
        let alice = arpeggi("alice");
        t.star(&alice, &song).await;
        t.land(&song);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let before = t.calls_to("star").await.len();

        t.star(&alice, &song).await;

        let stars = t.calls_to("star").await;
        assert_eq!(stars.len(), before + 1, "{stars:?}");
        assert_eq!(stars.last().expect("a star").1, [LANDED], "{favorite_downloads}");
        assert_eq!(t.starred2_ids(&alice).await, [LANDED], "{favorite_downloads}");
        assert!(
            t.song_starred(&alice, &song).await.is_some(),
            "{favorite_downloads}"
        );
        assert_eq!(t.state.star_on_arrival.held(), 0, "nothing new is fetched");
    }
}

/// Hearts sent again and again while the song downloads end as exactly one favorite of the
/// library copy, never none.
#[tokio::test]
async fn hearts_sent_again_while_it_downloads_end_as_one_favorite() {
    let t = Landing::new(true).await;
    let song = t.teardrop();
    let alice = arpeggi("alice");
    for _ in 0..4 {
        t.star(&alice, &song).await;
    }
    assert_eq!(t.state.star_on_arrival.held(), 1);
    assert_eq!(t.starred2_ids(&alice).await, std::slice::from_ref(&song));

    t.land(&song);
    t.until_starred(1).await;
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(t.calls_to("star").await.len(), 1);
    assert_eq!(t.starred2_ids(&alice).await, [LANDED]);
    assert!(t.song_starred(&alice, &song).await.is_some());
}
