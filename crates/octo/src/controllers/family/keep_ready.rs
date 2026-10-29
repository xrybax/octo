//! The Family page's playlists kept ready: a member chooses their own.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

use super::{Web, body_as, ok, refusal};
use crate::app::AppState;
use crate::services::prefetch::{KeepReadyChoice, UnknownList};

/// `GET /api/family/me/keep-ready`: the member's playlists that can hold outside songs, which are
/// kept ready, and how much of each is ready.
pub async fn my_keep_ready(State(state): State<AppState>, web: Web) -> Response {
    let (signed, _) = match web.member(&state).await {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    let view = state.keep_ready.view(signed.user()).await;
    ok(&serde_json::to_value(view).unwrap_or(Value::Null))
}

/// `PUT /api/family/me/keep-ready` with `{"lists", "mostPlayed"}`: saves what the member keeps
/// ready. Every list must be one of theirs; a `user` in the body is ignored.
pub async fn set_my_keep_ready(State(state): State<AppState>, web: Web, body: Bytes) -> Response {
    let (signed, _) = match web.member(&state).await {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    let choice: KeepReadyChoice = match body_as(&body) {
        Ok(choice) => choice,
        Err(refused) => return refused,
    };
    let user = signed.user().to_string();
    if let Err(UnknownList(id)) = state
        .keep_ready
        .choose(&user, choice.lists, choice.most_played)
        .await
    {
        return refusal(
            StatusCode::BAD_REQUEST,
            &format!("{id} is not one of your playlists."),
        );
    }
    let view = state.keep_ready.view(&user).await;
    ok(&json!(view))
}
