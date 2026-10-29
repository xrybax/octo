//! The catch-all's faithful relay, through the whole pipeline, against a Navidrome stood in
//! for by wiremock.

use axum::http::{HeaderMap, Method};
use http_body_util::BodyExt;
use octo_core::settings::{AppSettings, SubsonicSettings};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::http::pipeline::{App, build_with};
use crate::http::routes::RouteSet;

fn state(url: &str) -> AppState {
    AppState::for_tests(AppSettings {
        subsonic: SubsonicSettings {
            url: Some(url.to_string()),
            ..Default::default()
        },
        ..Default::default()
    })
}

fn app(state: AppState) -> App {
    build_with(state, RouteSet::new())
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

async fn send(app: &App, method: Method, uri: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("infallible");
    let (parts, body) = res.into_parts();
    let body = body.collect().await.expect("body").to_bytes();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

#[test]
fn octo_owned_paths_are_matched_as_the_controller_matched_them() {
    for (endpoint, owned) in [
        ("admin", true),
        ("administrator", true),
        ("Admin/nope.js", true),
        ("api/admin/whatever", true),
        ("API/ADMINISTRATOR", true),
        ("assets/x.png", true),
        ("Assets", false),
        ("favicon.ico", true),
        ("FAVICON.ICO", true),
        ("favicon.ico/x", false),
        ("rest/ping", false),
        ("api/song", false),
    ] {
        assert_eq!(is_octo_owned(endpoint), owned, "{endpoint}");
    }
}

#[test]
fn the_endpoint_is_the_decoded_path_but_an_encoded_slash() {
    assert_eq!(endpoint_of("/rest/getFoo.view"), "rest/getFoo.view");
    assert_eq!(endpoint_of("/%61dmin/x"), "admin/x");
    assert_eq!(
        endpoint_of("/api/song/a%20b%2Fc%2fd%C3%A9"),
        "api/song/a b%2Fc%2fdé"
    );
    assert_eq!(endpoint_of("/"), "");
}

#[tokio::test]
async fn an_unknown_subsonic_call_is_relayed_with_its_status_body_and_allowlisted_headers() {
    let server = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getNothingAtAll"))
        .respond_with(
            ResponseTemplate::new(404)
                .insert_header("Vary", "Origin")
                .insert_header("X-Not-Allowlisted", "x")
                .set_body_raw("404 page not found\n", "text/plain; charset=utf-8"),
        )
        .mount(&server)
        .await;
    let app = app(state(&server.uri()));

    let r = send(
        &app,
        Method::GET,
        "/rest/getNothingAtAll?u=admin&id=A&id=B",
        &[],
        "",
    )
    .await;

    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.body, "404 page not found\n");
    assert_eq!(r.headers["content-type"], "text/plain; charset=utf-8");
    assert_eq!(r.headers["vary"], "Origin");
    assert!(!r.headers.contains_key("x-not-allowlisted"));
    // Written as a stream, so sent chunked rather than with a length.
    assert!(!r.headers.contains_key("content-length"));
    let sent = &server.received_requests().await.expect("recording")[0];
    // The repeated id goes upstream as the client sent it, not as "A,B".
    assert_eq!(sent.url.query(), Some("u=admin&id=A&id=B"));
}

#[tokio::test]
async fn a_method_the_routes_do_not_take_is_relayed_with_that_method() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/rest/getAlbum"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(r#"{"subsonic-response":{}}"#, "application/json"),
        )
        .mount(&server)
        .await;
    let app = app(state(&server.uri()));

    let r = send(&app, Method::DELETE, "/rest/getAlbum?f=json&id=x", &[], "").await;

    assert_eq!(
        (r.status, r.body.as_str()),
        (StatusCode::OK, r#"{"subsonic-response":{}}"#)
    );
}

#[tokio::test]
async fn a_json_body_is_forwarded_as_sent_with_the_parameters_in_the_query() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/rest/getUser"))
        .and(query_param("username", "admin"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let app = app(state(&server.uri()));
    let body = r#"{"u": "admin", "p": "pw", "f": "json", "username": "admin"}"#;

    let r = send(
        &app,
        Method::POST,
        "/rest/getUser",
        &[("Content-Type", "application/json")],
        body,
    )
    .await;

    // Nothing upstream sets a content type, so the answer is labeled by f.
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers["content-type"], "application/json");
    let sent = &server.received_requests().await.expect("recording")[0];
    assert_eq!(String::from_utf8_lossy(&sent.body), body);
    assert_eq!(sent.headers["content-type"], "application/json");
    assert_eq!(sent.url.query(), Some("u=admin&p=pw&f=json&username=admin"));
}

#[tokio::test]
async fn a_native_admin_login_passing_through_is_captured() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "token": "jwt", "isAdmin": true, "username": "admin", "subsonicToken": "t", "subsonicSalt": "s",
        })))
        .mount(&server)
        .await;
    let state = state(&server.uri());
    let app = app(state.clone());

    let r = send(
        &app,
        Method::POST,
        "/auth/login",
        &[("Content-Type", "application/json")],
        r#"{"username":"admin","password":"pw"}"#,
    )
    .await;

    assert_eq!(r.status, StatusCode::OK);
    assert!(state.navidrome_identity.has_admin_identity());
    assert_eq!(
        state
            .navidrome_identity
            .username_for_native_token(Some("jwt"))
            .as_deref(),
        Some("admin")
    );
}

#[tokio::test]
async fn an_unreachable_navidrome_is_the_error_envelope() {
    let address = crate::services::test_support::refused_address();
    let app = app(state(&format!("http://{address}")));

    let r = send(&app, Method::GET, "/rest/getArtists?f=json", &[], "").await;

    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.body,
        format!(
            r#"{{"subsonic-response":{{"status":"failed","version":"1.16.1","error":{{"code":0,"message":"Error connecting to Subsonic server: Could not connect to {address}: the connection was refused."}}}}}}"#
        )
    );
}

#[tokio::test]
async fn octo_owned_paths_and_the_root_are_never_relayed() {
    let server = MockServer::builder().start().await;
    let app = app(state(&server.uri()));

    assert_eq!(
        send(&app, Method::GET, "/favicon.ico", &[], "").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, Method::POST, "/%61dmin/x", &[], "").await.status,
        StatusCode::NOT_FOUND
    );
    for method in [Method::GET, Method::HEAD] {
        assert_eq!(
            send(&app, method.clone(), "/", &[], "").await.status,
            StatusCode::FOUND,
            "{method} /"
        );
    }
    assert!(server.received_requests().await.expect("recording").is_empty());
}

/// #80: the bare address with another method binds an empty endpoint and is relayed to
/// Navidrome's own root; only GET and HEAD are redirected.
#[tokio::test]
async fn a_post_to_the_bare_address_is_relayed() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(405).set_body_raw("nope", "text/plain"))
        .mount(&server)
        .await;
    let app = app(state(&server.uri()));

    let r = send(&app, Method::POST, "/", &[], "").await;

    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.body, "nope");
    assert_eq!(server.received_requests().await.expect("recording").len(), 1);
}

// ---- savePlayQueue, read for the songs fetched ahead ------------------------------------------

/// A state with the songs fetched ahead on or off, and one outside song in its registry.
fn prefetch_state(url: &str, on: bool) -> (AppState, String) {
    let state = AppState::for_tests(AppSettings {
        subsonic: SubsonicSettings {
            url: Some(url.to_string()),
            ..Default::default()
        },
        prefetch: octo_core::settings::PrefetchSettings {
            enabled: on,
            ..Default::default()
        },
        ..Default::default()
    });
    let outside = state.external_id_registry.register(
        octo_core::soulseek::soulseek_metadata_service::SoulseekRouting {
            kind: octo_core::soulseek::soulseek_metadata_service::RoutingKind::Song,
            artist: Some("Artist".into()),
            title: Some("Outside".into()),
            ..Default::default()
        },
    );
    (state, outside)
}

fn ok_json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_string(), "application/json")
}

#[tokio::test]
async fn a_saved_play_queue_is_relayed_as_it_came_and_read_for_the_songs_fetched_ahead() {
    let server = MockServer::builder().start().await;
    Mock::given(path("/rest/savePlayQueue"))
        .respond_with(ok_json(r#"{"subsonic-response":{"status":"ok","saved":1}}"#))
        .mount(&server)
        .await;
    Mock::given(path("/rest/ping"))
        .respond_with(ok_json(r#"{"subsonic-response":{"status":"ok"}}"#))
        .mount(&server)
        .await;
    let uri = "/rest/savePlayQueue?id=lib-1&id=lib-2&id=lib-3&current=lib-2&u=alice&t=tok&s=salt&v=1.16.1&c=Octo&f=json";

    let (off, _) = prefetch_state(&server.uri(), false);
    let before = send(&app(off), Method::GET, uri, &[], "").await;
    let (on, _) = prefetch_state(&server.uri(), true);
    let after = send(&app(on.clone()), Method::GET, uri, &[], "").await;

    assert_eq!(after.status, before.status);
    assert_eq!(after.body, before.body);
    assert_eq!(
        after.headers.get("content-type"),
        before.headers.get("content-type")
    );
    crate::services::common::test_fakes::until(|| on.prefetch.saved_queue_of("alice").is_some()).await;
    let saved = on.prefetch.saved_queue_of("alice").expect("saved");
    assert_eq!(saved.ids, ["lib-1", "lib-2", "lib-3"]);
    assert_eq!(saved.current.as_deref(), Some("lib-2"));
}

#[tokio::test]
async fn a_saved_queue_that_starts_with_an_outside_song_is_answered_as_before_and_read() {
    let server = MockServer::builder().start().await;
    Mock::given(path("/rest/ping"))
        .respond_with(ok_json(r#"{"subsonic-response":{"status":"ok"}}"#))
        .mount(&server)
        .await;
    Mock::given(path("/rest/savePlayQueueByIndex"))
        .respond_with(ok_json(r#"{"subsonic-response":{"status":"ok"}}"#))
        .mount(&server)
        .await;

    let (off, outside) = prefetch_state(&server.uri(), false);
    let (on, outside_on) = prefetch_state(&server.uri(), true);
    assert_eq!(outside, outside_on, "the registry's ids are the routing's");
    let uri = format!(
        "/rest/savePlayQueueByIndex?id={outside}&currentIndex=0&u=alice&t=tok&s=salt&v=1.16.1&c=Octo&f=json"
    );
    let before = send(&app(off), Method::GET, &uri, &[], "").await;
    let after = send(&app(on.clone()), Method::GET, &uri, &[], "").await;
    assert_eq!(after.status, before.status);
    assert_eq!(after.body, before.body);
    crate::services::common::test_fakes::until(|| on.prefetch.saved_queue_of("alice").is_some()).await;
    let saved = on.prefetch.saved_queue_of("alice").expect("saved");
    assert_eq!(saved.ids, std::slice::from_ref(&outside));
    assert_eq!(saved.current.as_deref(), Some(outside.as_str()));
    assert_eq!(saved.current_index, Some(0));
}

#[tokio::test]
async fn a_saved_queue_with_a_refused_sign_in_is_not_read() {
    let server = MockServer::builder().start().await;
    Mock::given(path("/rest/ping"))
        .respond_with(ok_json(
            r#"{"subsonic-response":{"status":"failed","error":{"code":40,"message":"Wrong username or password"}}}"#,
        ))
        .mount(&server)
        .await;
    let (on, outside) = prefetch_state(&server.uri(), true);
    let uri = format!("/rest/savePlayQueue?id={outside}&u=mallory&t=bad&s=salt&v=1.16.1&c=x&f=json");
    send(&app(on.clone()), Method::GET, &uri, &[], "").await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(on.prefetch.saved_queue_of("mallory").is_none());
}
