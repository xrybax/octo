//! setRating through the app, and the playlist names read from XML as well as JSON.
//!
//! Where a star counts as a command (#47). With Review on, a star is a command only on a track
//! Octo asked the person about; everywhere else it is just a rating, relayed and left alone.

use octo_core::settings::{
    AppSettings, LibraryAction, LibraryActionDefinition, LibraryActionSettings, LibraryRatingScope,
};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer};

use super::playlist_names::playlist_names;
use super::test_app::*;
use crate::app::AppState;
use crate::services::library::IgnoredRating;
use crate::services::library::library_action_test_support::asked_about;

/// Navidrome answering ok to everything.
async fn ok_navidrome() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(navidrome_json(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#,
        ))
        .mount(&server)
        .await;
    server
}

fn rating_settings(url: &str, scope: LibraryRatingScope) -> AppSettings {
    AppSettings {
        library_actions: LibraryActionSettings {
            enabled: true,
            ratings_enabled: true,
            review_enabled: true,
            ratings_scope: scope,
            allowed_users: vec!["alice".into()],
            actions: vec![LibraryActionDefinition {
                action: LibraryAction::Delete,
                enabled: true,
                rating: Some(1),
                ..Default::default()
            }],
            ..Default::default()
        },
        ..settings(url)
    }
}

fn rate(id: &str) -> String {
    format!("/rest/setRating.view?u=alice&t=token&s=salt&v=1.16.1&c=test&f=json&id={id}&rating=1")
}

#[tokio::test]
async fn set_rating_with_review_on_ignores_a_track_octo_did_not_ask_about() {
    for scope in [LibraryRatingScope::NoticeOnly, LibraryRatingScope::Auto] {
        let navidrome = ok_navidrome().await;
        let state = state_with(rating_settings(&navidrome.uri(), scope), |inner| {
            inner.notice_queue = asked_about("alice", "nd-asked");
        });
        let app = app(&state);

        assert!(
            get(&app, &rate("nd-elsewhere")).await.status.is_success(),
            "{scope:?}"
        );
        assert_eq!(state.library_action_rating_worker.pending(), 0, "{scope:?}");

        assert!(
            get(&app, &rate("nd-asked")).await.status.is_success(),
            "{scope:?}"
        );
        assert_eq!(state.library_action_rating_worker.pending(), 1, "{scope:?}");
    }
}

#[tokio::test]
async fn set_rating_global_counts_on_any_track() {
    let navidrome = ok_navidrome().await;
    let state = state_with(
        rating_settings(&navidrome.uri(), LibraryRatingScope::Global),
        |inner| inner.notice_queue = asked_about("alice", "nd-asked"),
    );
    let app = app(&state);

    assert!(get(&app, &rate("nd-elsewhere")).await.status.is_success());

    assert_eq!(state.library_action_rating_worker.pending(), 1);
}

#[test]
fn playlist_names_reads_xml_as_well_as_json() {
    let xml = "<subsonic-response xmlns=\"http://subsonic.org/restapi\" status=\"ok\"><playlists>\
        <playlist id=\"1\" name=\"\u{1F6E0} Delete\"/><playlist id=\"2\" name=\"\u{25B8} Review\"/>\
        </playlists></subsonic-response>";
    let json = "{\"subsonic-response\":{\"status\":\"ok\",\"playlists\":{\"playlist\":[{\"id\":\"1\",\"name\":\"\u{1F6E0} Delete\"}]}}}";

    assert_eq!(
        playlist_names(Some(xml.as_bytes()), "xml"),
        vec!["\u{1F6E0} Delete".to_string(), "\u{25B8} Review".to_string()]
    );
    assert_eq!(
        playlist_names(Some(json.as_bytes()), "json"),
        vec!["\u{1F6E0} Delete".to_string()]
    );
    assert!(playlist_names(Some(b"<not xml"), "xml").is_empty());
    // No body, and a JSON name that is not a string, give nothing.
    assert!(playlist_names(None, "json").is_empty());
    assert!(
        playlist_names(
            Some(br#"{"subsonic-response":{"playlists":{"playlist":[{"name":"a"},{"name":3}]}}}"#),
            "JSON"
        )
        .is_empty()
    );
}

// ---------------------------------------------------------------------------------------
// Every sign-in, and the ratings Octo did not act on
// ---------------------------------------------------------------------------------------

/// Navidrome for the sign-in tests: it accepts the token "token", the password "secret", and
/// the API key "alice-key" (whose owner is alice), names nobody for any other key, describes
/// nd-1 as Teardrop, and records every rating put back to 0 with the sign-in it carried.
#[derive(Clone, Default)]
struct SignInNavidrome {
    cleared: std::sync::Arc<parking_lot::Mutex<Vec<Vec<(String, String)>>>>,
    /// How long getSong takes to answer.
    song_delay: std::time::Duration,
}

impl wiremock::Respond for SignInNavidrome {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        let path = request.url.path();
        let pairs: Vec<(String, String)> = request.url.query_pairs().into_owned().collect();
        let value = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        if path.ends_with("/rest/tokenInfo") {
            return if value("apiKey") == Some("alice-key") {
                navidrome_json(
                    r#"{"subsonic-response":{"status":"ok","version":"1.16.1","tokenInfo":{"username":"alice"}}}"#,
                )
            } else {
                navidrome_json(NAVIDROME_WRONG_PASSWORD)
            };
        }
        let signed_in = value("t") == Some("token")
            || value("p") == Some("secret")
            || value("p") == Some("enc:736563726574")
            || value("apiKey").is_some_and(|key| key.ends_with("-key"))
            || value("t").is_some_and(|t| {
                value("s").is_some_and(|s| {
                    use md5::{Digest, Md5};
                    hex::encode(Md5::digest(format!("secret{s}").as_bytes())) == t
                })
            });
        if !signed_in {
            return navidrome_json(NAVIDROME_WRONG_PASSWORD);
        }
        if path.ends_with("/rest/setRating") && value("rating") == Some("0") {
            self.cleared.lock().push(
                pairs
                    .iter()
                    .filter(|(k, _)| matches!(k.as_str(), "u" | "t" | "s" | "p" | "apiKey"))
                    .cloned()
                    .collect(),
            );
        }
        if path.ends_with("/rest/getSong") {
            return navidrome_json(
                r#"{"subsonic-response":{"status":"ok","version":"1.16.1","song":{"id":"nd-1","title":"Teardrop","artist":"Massive Attack","album":"Mezzanine"}}}"#,
            )
            .set_delay(self.song_delay);
        }
        navidrome_json(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#)
    }
}

async fn sign_in_navidrome() -> (MockServer, SignInNavidrome) {
    sign_in_navidrome_with(SignInNavidrome::default()).await
}

async fn sign_in_navidrome_with(responder: SignInNavidrome) -> (MockServer, SignInNavidrome) {
    let server = MockServer::builder().start().await;
    Mock::given(any())
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    (server, responder)
}

fn value<'a>(sign_in: &'a [(String, String)], key: &str) -> Option<&'a str> {
    sign_in.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// A rating queues its action whichever way the app signs in, and the rating is put back to 0
/// signed in the same way: u with t and s as sent, a password turned into a token made from it,
/// or the API key.
#[tokio::test]
async fn every_sign_in_queues_the_action_and_clears_with_its_own_sign_in() {
    let forms = [
        "u=alice&t=token&s=salt",
        "u=alice&p=secret",
        "u=alice&p=enc:736563726574",
        "apiKey=alice-key",
    ];
    for form in forms {
        let (navidrome, responder) = sign_in_navidrome().await;
        let state = state_with(
            rating_settings(&navidrome.uri(), LibraryRatingScope::Global),
            |_| {},
        );
        let app = app(&state);

        let reply = get(
            &app,
            &format!("/rest/setRating.view?{form}&v=1.16.1&c=test&f=json&id=nd-1&rating=1"),
        )
        .await;
        assert_eq!(reply.envelope()["status"], "ok", "{form}: {}", reply.text());
        assert_eq!(state.library_action_rating_worker.pending(), 1, "{form}");

        // Delete switched off before the worker runs: the action is Skipped, which consumes the
        // rating, so it is put back to 0.
        let mut off = (*state.settings.current()).clone();
        off.library_actions.actions[0].enabled = false;
        state.settings.set(off);
        let stopping = tokio_util::sync::CancellationToken::new();
        let running = tokio::spawn(state.library_action_rating_worker.clone().run(stopping.clone()));
        crate::services::common::test_fakes::until(|| !responder.cleared.lock().is_empty()).await;
        stopping.cancel();
        let _ = running.await;

        let cleared = responder.cleared.lock()[0].clone();
        assert_eq!(
            value(&cleared, "p"),
            None,
            "{form}: a password is never sent again"
        );
        if form.starts_with("apiKey") {
            assert_eq!(value(&cleared, "apiKey"), Some("alice-key"), "{form}");
            assert_eq!(value(&cleared, "u"), None, "{form}");
        } else {
            assert_eq!(value(&cleared, "u"), Some("alice"), "{form}");
        }
        if form.contains("t=token") {
            assert_eq!(
                (value(&cleared, "t"), value(&cleared, "s")),
                (Some("token"), Some("salt"))
            );
        }
        if form.contains("p=") {
            let salt = value(&cleared, "s").expect("a salt");
            use md5::{Digest, Md5};
            let token = hex::encode(Md5::digest(format!("secret{salt}").as_bytes()));
            assert_eq!(value(&cleared, "t"), Some(token.as_str()), "{form}");
        }
    }
}

/// Rates nd-1 with one star as `sign_in` and answers what Octo kept about it, if anything.
async fn ignored_after(settings: AppSettings, sign_in: &str) -> (AppState, Option<IgnoredRating>) {
    let state = state_with(settings, |inner| {
        inner.notice_queue = asked_about("alice", "nd-asked");
    });
    let app = app(&state);
    let reply = get(
        &app,
        &format!("/rest/setRating.view?{sign_in}&v=1.16.1&c=test&f=json&id=nd-1&rating=1"),
    )
    .await;
    assert_eq!(reply.envelope()["status"], "ok", "{}", reply.text());
    let kept = state.library_action_rating_worker.ignored().first().cloned();
    (state, kept)
}

/// Each reason a rating that asks for an action gets none is kept with its words, the song and
/// who rated it.
#[tokio::test]
async fn each_reason_a_rating_was_not_acted_on_is_kept_with_its_words() {
    let (navidrome, _) = sign_in_navidrome().await;
    let base = || rating_settings(&navidrome.uri(), LibraryRatingScope::Global);
    let token = "u=alice&t=token&s=salt";
    let mut cases: Vec<(AppSettings, &str, &str)> = Vec::new();

    let mut off = base();
    off.library_actions.enabled = false;
    cases.push((
        off,
        token,
        "Library actions are off. Turn on Enable library actions under Library actions.",
    ));
    let mut ratings_off = base();
    ratings_off.library_actions.ratings_enabled = false;
    cases.push((
        ratings_off,
        token,
        "Star ratings are off. Turn on Star ratings as commands under Library actions.",
    ));
    let mut someone_else = base();
    someone_else.library_actions.allowed_users = vec!["bob".into()];
    cases.push((
        someone_else,
        token,
        "alice is not on the allowed list. Add them to Who may trigger actions under Library actions.",
    ));
    let mut review_only = base();
    review_only.library_actions.ratings_scope = LibraryRatingScope::NoticeOnly;
    cases.push((
        review_only,
        token,
        "The song is not in Review or Duplicates, and only answers there count. Set Where a star counts to Any song to count a star on every song.",
    ));
    cases.push((
        base(),
        "apiKey=nobody-key",
        "Octo could not tell who rated the song, so it did nothing.",
    ));

    for (settings, sign_in, words) in cases {
        let (state, kept) = ignored_after(settings, sign_in).await;
        let kept = kept.unwrap_or_else(|| panic!("nothing kept for: {words}"));
        assert_eq!(kept.reason, words);
        assert_eq!(kept.song_id, "nd-1");
        assert_eq!(kept.rating, 1);
        assert_eq!(kept.action, LibraryAction::Delete);
        assert_eq!(state.library_action_rating_worker.pending(), 0, "{words}");
        if sign_in == token {
            assert_eq!(kept.username, "alice");
            let named = || {
                state
                    .library_action_rating_worker
                    .ignored()
                    .first()
                    .is_some_and(|r| r.title.is_some())
            };
            crate::services::common::test_fakes::until(named).await;
            let kept = state.library_action_rating_worker.ignored()[0].clone();
            assert_eq!(kept.title.as_deref(), Some("Teardrop"));
            assert_eq!(kept.artist.as_deref(), Some("Massive Attack"));
        }
    }
}

/// In rehearsal mode the action is still queued, so the rehearsal is written down, and the
/// rating is kept as one that changed nothing.
#[tokio::test]
async fn a_rehearsed_rating_is_queued_and_kept_as_changing_nothing() {
    let (navidrome, _) = sign_in_navidrome().await;
    let mut settings = rating_settings(&navidrome.uri(), LibraryRatingScope::Global);
    settings.library_actions.dry_run = true;

    let (state, kept) = ignored_after(settings, "u=alice&t=token&s=salt").await;

    assert_eq!(state.library_action_rating_worker.pending(), 1);
    assert_eq!(
        kept.expect("kept").reason,
        "Rehearsal mode is on, so nothing was changed. Turn off Rehearsal mode under Library actions."
    );
}

/// A rating that acts, a rating no action is mapped to, and a rating Navidrome refused are not
/// kept.
#[tokio::test]
async fn ratings_that_act_or_ask_for_nothing_are_not_kept() {
    let (navidrome, _) = sign_in_navidrome().await;
    let mut acting = rating_settings(&navidrome.uri(), LibraryRatingScope::Global);
    acting.library_actions.dry_run = false;
    let (state, kept) = ignored_after(acting, "u=alice&t=token&s=salt").await;
    assert_eq!(state.library_action_rating_worker.pending(), 1);
    assert_eq!(kept, None);

    let mut off = rating_settings(&navidrome.uri(), LibraryRatingScope::Global);
    off.library_actions.ratings_enabled = false;
    let state = state_with(off, |_| {});
    let app = app(&state);
    for query in [
        "u=alice&t=token&s=salt&f=json&id=nd-1&rating=3",
        "u=alice&t=bad&s=salt&f=json&id=nd-1&rating=1",
    ] {
        get(&app, &format!("/rest/setRating.view?{query}&v=1.16.1&c=test")).await;
    }
    assert!(state.library_action_rating_worker.ignored().is_empty());
}

/// A five star rating Keep did nothing with is most often a plain "I love this song": it is
/// neither kept for the dashboard nor counted, wherever the song is and whoever rated it.
#[tokio::test]
async fn a_keep_rating_that_does_nothing_is_never_kept() {
    let (navidrome, _) = sign_in_navidrome().await;
    let review_only = rating_settings(&navidrome.uri(), LibraryRatingScope::NoticeOnly);
    let mut not_allowed = rating_settings(&navidrome.uri(), LibraryRatingScope::Global);
    not_allowed.library_actions.allowed_users = vec!["bob".into()];
    assert_eq!(
        review_only.library_actions.action_for_rating(5).map(|a| a.action),
        Some(LibraryAction::Keep)
    );

    for settings in [review_only, not_allowed] {
        let state = state_with(settings, |inner| {
            inner.notice_queue = asked_about("alice", "nd-asked");
        });
        let app = app(&state);
        for _ in 0..3 {
            let reply = get(
                &app,
                "/rest/setRating.view?u=alice&t=token&s=salt&v=1.16.1&c=test&f=json&id=nd-1&rating=5",
            )
            .await;
            assert_eq!(reply.envelope()["status"], "ok", "{}", reply.text());
        }
        assert!(state.library_action_rating_worker.ignored().is_empty());
        assert_eq!(state.library_action_rating_worker.pending(), 0);
    }
}

/// A rating that is not acted on is kept the moment it is answered and named once the song's
/// artist and title come back: the reply never waits on that lookup.
#[tokio::test]
async fn a_kept_rating_is_named_after_the_reply() {
    let (navidrome, _) = sign_in_navidrome_with(SignInNavidrome {
        song_delay: std::time::Duration::from_millis(500),
        ..Default::default()
    })
    .await;
    let settings = rating_settings(&navidrome.uri(), LibraryRatingScope::NoticeOnly);

    let (state, kept) = ignored_after(settings, "u=alice&t=token&s=salt").await;

    let kept = kept.expect("kept at once");
    assert_eq!(kept.username, "alice");
    assert_eq!(kept.title, None, "named before the lookup could answer");
    let named = || {
        state
            .library_action_rating_worker
            .ignored()
            .first()
            .is_some_and(|r| r.title.is_some())
    };
    crate::services::common::test_fakes::until(named).await;
    let kept = state.library_action_rating_worker.ignored()[0].clone();
    assert_eq!(kept.title.as_deref(), Some("Teardrop"));
    assert_eq!(kept.artist.as_deref(), Some("Massive Attack"));
    assert_eq!(state.library_action_rating_worker.ignored().len(), 1);
}
