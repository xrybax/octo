//! An outside song someone hearted reads as their favorite in every answer that lists it: one
//! waiting for its download that becomes a favorite when it lands, and one whose library copy a
//! heart made the favorite. Apps that draw the heart from a song's own `starred` field (an album
//! page, the song's page, a search) keep it through a refresh. Only the caller's own hearts are
//! marked, only for a sign-in Navidrome accepts, and a song that already says when it was
//! starred keeps that.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_LENGTH, ETAG};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, SecondsFormat, Utc};
use octo_subsonic::SubsonicCredential;
use octo_subsonic::xml::XElement;
use serde_json::{Map, Value};

use super::size_flag::{MAX_BODY, Read, answer_format, read_whole};
use crate::app::AppState;
use crate::controllers::subsonic::caller_checks::{MAX_REQUEST_BODY, SubsonicCall, signed_in_user};
use crate::services::common::StarOnArrival;
use crate::services::family::list_rules::{Edit, edit};
use crate::services::subsonic::CredentialVerdict;

/// Endpoints whose answers are never song lists, and the hearts' own, which say it themselves.
const UNMARKED: &[&str] = &[
    "stream",
    "download",
    "getcoverart",
    "getavatar",
    "hls",
    "star",
    "unstar",
    "getstarred",
    "getstarred2",
];

pub async fn heart_marks(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if request.method() == Method::HEAD || !marked_endpoint(&request) || !state.star_on_arrival.marks_anyone()
    {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let Ok(body) = axum::body::to_bytes(body, MAX_REQUEST_BODY).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let call = SubsonicCall::from_parts(&state, &parts, body.clone());
    let response = next.run(Request::from_parts(parts, Body::from(body))).await;
    if answer_format(response.status(), response.headers()).is_none() {
        return response;
    }
    let Some(credential) = SubsonicCredential::from(&call.parameters) else {
        return response;
    };
    let Ok(who) = signed_in_user(&state, &call).await else {
        return response;
    };
    let marks = state
        .star_on_arrival
        .marks(&StarOnArrival::holder_name(&credential, Some(&who)));
    if marks.is_empty() {
        return response;
    }
    let accepted = state
        .credential_check
        .check(Some(&credential), &call.proxy)
        .await
        .is_ok_and(|verdict| verdict == CredentialVerdict::Accepted);
    if !accepted {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = match read_whole(body, MAX_BODY).await {
        Read::Whole(bytes) => bytes,
        Read::Unread(body) => return Response::from_parts(parts, body),
    };
    let Some(marked) = mark(&bytes, &marks) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    parts
        .headers
        .insert(CONTENT_LENGTH, HeaderValue::from(marked.len()));
    parts.headers.remove(ETAG);
    Response::from_parts(parts, Body::from(marked))
}

/// A Subsonic call (under `/rest/`) whose answer can list songs.
fn marked_endpoint(request: &Request) -> bool {
    let path = request.uri().path().to_ascii_lowercase();
    let Some((_, endpoint)) = path.split_once("/rest/") else {
        return false;
    };
    let endpoint = endpoint.trim_end_matches('/');
    let endpoint = endpoint.strip_suffix(".view").unwrap_or(endpoint);
    !UNMARKED.contains(&endpoint)
}

/// The answer with each listed song in `marks` given its `starred` time, or `None` when nothing
/// changed or the answer is not a Subsonic answer.
pub fn mark(body: &[u8], marks: &[(String, DateTime<Utc>)]) -> Option<Vec<u8>> {
    let starred_of = |id: &str| {
        marks
            .iter()
            .find(|(marked, _)| marked == id)
            .map(|(_, at)| at.to_rfc3339_opts(SecondsFormat::Millis, true))
    };
    let edited = edit(
        body,
        |response| {
            if response.get("status").and_then(Value::as_str) != Some("ok") {
                return false;
            }
            let mut changed = false;
            for value in response.values_mut() {
                changed |= mark_json(value, &starred_of);
            }
            changed
        },
        |root| root.attribute("status") == Some("ok") && mark_xml(root, &starred_of),
    );
    match edited {
        Edit::Changed(bytes) => Some(bytes),
        Edit::Unchanged | Edit::Unreadable => None,
    }
}

fn mark_json(value: &mut Value, starred_of: &impl Fn(&str) -> Option<String>) -> bool {
    match value {
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| mark_json(item, starred_of) | changed),
        Value::Object(fields) => {
            let mut changed = mark_fields(fields, starred_of);
            for child in fields.values_mut() {
                changed |= mark_json(child, starred_of);
            }
            changed
        }
        _ => false,
    }
}

fn mark_fields(fields: &mut Map<String, Value>, starred_of: &impl Fn(&str) -> Option<String>) -> bool {
    if fields.contains_key("starred") || fields.get("isDir").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    let Some(starred) = fields.get("id").and_then(Value::as_str).and_then(starred_of) else {
        return false;
    };
    fields.insert("starred".into(), Value::String(starred));
    true
}

fn mark_xml(element: &mut XElement, starred_of: &impl Fn(&str) -> Option<String>) -> bool {
    let mut changed = false;
    if element.attribute("starred").is_none()
        && element.attribute("isDir") != Some("true")
        && let Some(starred) = element.attribute("id").and_then(starred_of)
    {
        element.set_attr("starred", starred);
        changed = true;
    }
    for child in element.elements_mut() {
        changed |= mark_xml(child, starred_of);
    }
    changed
}

#[cfg(test)]
#[path = "heart_marks_tests.rs"]
mod tests;
