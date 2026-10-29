//! The "Get upcoming songs ahead" card: what is ready, what is being fetched, clearing the folder,
//! and each person's playlists kept ready. Behind the same guard as the rest of `/api/admin`.

use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use serde_json::{Value, json};
use tracing::info;

use super::plumbing::{error_json, query_value};
use super::request_binding::ok;
use crate::app::AppState;
use crate::http::routes::RouteSet;
use crate::middleware::admin_sign_in_gate::{Caller, caller_of};
use crate::services::admin::{AdminCaller, AdminCallerKind};
use crate::services::prefetch::{KeepReadyChoice, UnknownList};

/// The words a caller who may not choose playlists kept ready is answered with.
pub const SIGN_IN_FOR_KEEP_READY: &str =
    "Sign in with your Navidrome admin account to choose playlists kept ready.";
/// The words the dashboard is answered with while its sign-in is off.
pub const SIGN_IN_OFF_HIDES_KEEP_READY: &str = "Turn on dashboard sign-in to choose playlists kept ready for each person, since the lists show what each person plays most.";

pub fn routes() -> RouteSet {
    RouteSet::new()
        .route("/api/admin/prefetch", get(get_prefetch))
        .route("/api/admin/prefetch/clear", post(clear_prefetch))
        .route(
            "/api/admin/prefetch/keep-ready",
            get(get_keep_ready).put(put_keep_ready),
        )
}

/// `GET /api/admin/prefetch`: on or off, the songs ready and their size against the cap, the free
/// space on the folder's disk, the songs being fetched and next, the last failures, and why
/// nothing new starts when nothing does.
async fn get_prefetch(State(state): State<AppState>) -> Response {
    ok(&state.prefetch.status().await)
}

/// `POST /api/admin/prefetch/clear`: deletes every song downloaded ahead but one playing now.
async fn clear_prefetch(State(state): State<AppState>) -> Response {
    let cleared = state.prefetch.clear();
    info!("Deleted {cleared} songs downloaded ahead by admin request");
    ok(&json!({ "cleared": cleared }))
}

/// Whose playlists kept ready a caller may see and choose: the dashboard and a Navidrome admin's
/// app anyone's (`asked`, else the first person), a listener's app only its own. Nobody else, and
/// the words they are refused with.
fn whose(
    people: &[String],
    caller: Option<&AdminCaller>,
    asked: Option<String>,
) -> Result<String, &'static str> {
    let Some(caller) = caller else {
        return Err(SIGN_IN_FOR_KEEP_READY);
    };
    match caller.kind {
        AdminCallerKind::Dashboard | AdminCallerKind::NavidromeAdmin => Ok(asked
            .filter(|user| !user.trim().is_empty())
            .or_else(|| people.first().cloned())
            .unwrap_or_default()),
        AdminCallerKind::Listener => caller
            .user
            .clone()
            .filter(|user| !user.trim().is_empty())
            .ok_or(SIGN_IN_FOR_KEEP_READY),
        AdminCallerKind::SignInOff => Err(SIGN_IN_OFF_HIDES_KEEP_READY),
        AdminCallerKind::Recovery | AdminCallerKind::FamilyMember | AdminCallerKind::FamilyManager => {
            Err(SIGN_IN_FOR_KEEP_READY)
        }
    }
}

/// The people whose lists can be chosen: everyone Octo has recorded plays of, and everyone who
/// chose lists. Reading the plays is blocking file work.
async fn people(state: &AppState) -> Vec<String> {
    let listening = state.listening.clone();
    let mut names: Vec<String> =
        tokio::task::spawn_blocking(move || listening.people().into_iter().map(|(_, name)| name).collect())
            .await
            .unwrap_or_default();
    for user in state.keep_ready.chosen_users() {
        if !names.iter().any(|name| name.eq_ignore_ascii_case(&user)) {
            names.push(user);
        }
    }
    names
}

/// One person's playlists kept ready, with the people a dashboard may choose from.
async fn answer(state: &AppState, caller: Option<&AdminCaller>, user: &str, people: Vec<String>) -> Response {
    let mut view = serde_json::to_value(state.keep_ready.view(user).await).unwrap_or(Value::Null);
    let everyone = caller.is_some_and(|c| {
        matches!(
            c.kind,
            AdminCallerKind::Dashboard | AdminCallerKind::NavidromeAdmin
        )
    });
    view["people"] = json!(if everyone { people } else { vec![user.to_string()] });
    ok(&view)
}

/// `GET /api/admin/prefetch/keep-ready[?user=]`: one person's playlists that can hold outside
/// songs, which are picked or kept for being played from most, and how much of each is ready.
async fn get_keep_ready(
    State(state): State<AppState>,
    caller: Caller,
    RawQuery(query): RawQuery,
) -> Response {
    let caller = caller_of(&caller);
    let people = people(&state).await;
    let user = match whose(&people, caller, query_value(query.as_deref(), "user")) {
        Ok(user) => user,
        Err(words) => return error_json(StatusCode::FORBIDDEN, words),
    };
    answer(&state, caller, &user, people).await
}

/// `PUT /api/admin/prefetch/keep-ready` with `{"user", "lists", "mostPlayed"}`: saves what that
/// person keeps ready; every list must be one of theirs. Answers as the GET does.
async fn put_keep_ready(State(state): State<AppState>, caller: Caller, body: Bytes) -> Response {
    let caller = caller_of(&caller);
    let choice: KeepReadyChoice = match serde_json::from_slice(&body) {
        Ok(choice) => choice,
        Err(_) => return error_json(StatusCode::BAD_REQUEST, "Send the lists to keep ready."),
    };
    let people = people(&state).await;
    let user = match whose(&people, caller, choice.user.clone()) {
        Ok(user) if !user.trim().is_empty() => user,
        Ok(_) => return error_json(StatusCode::BAD_REQUEST, "Name the person whose lists these are."),
        Err(words) => return error_json(StatusCode::FORBIDDEN, words),
    };
    if let Err(UnknownList(id)) = state
        .keep_ready
        .choose(&user, choice.lists, choice.most_played)
        .await
    {
        return error_json(
            StatusCode::BAD_REQUEST,
            &format!("{id} is not one of {user}'s playlists."),
        );
    }
    info!("Playlists kept ready for {user} changed");
    answer(&state, caller, &user, people).await
}
