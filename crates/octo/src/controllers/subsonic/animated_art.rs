//! `octoAnimatedArt` version 1: Apple Music's animated album art for the Octo apps (see
//! [`crate::services::cover_art::animated_art`]).
//!
//! - `getAnimatedArt?id=<album id>` (a library album or an outside one), or `?artist=&album=`:
//!   `animatedArt` with `square` and `tall`, each `{video, poster, color, width, height}`, either
//!   one left out when Apple has no such loop; no `animatedArt` when the album has none. Waits
//!   up to [`ASK_WAIT`] for an album not looked up before.
//! - `getAlbum` in JSON, asked by an Octo app, carries the same `animatedArt` on the album when
//!   it is already known. It never waits: an unknown album is looked up in the background, and
//!   the next getAlbum (or getAnimatedArt) has it.
//! - `animated/{token}/{file}`: the relay, while `Metadata:AnimatedArt` is `relay`. No sign-in, as
//!   a player follows playlist links without one; the token is random and only leads to its
//!   album's loops.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures::TryStreamExt;
use octo_core::common::dotnet::is_blank;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use super::caller_checks::{SubsonicCall, as_json, refuse_unless_signed_in};
use super::family::octo_reply;
use crate::app::AppState;
use crate::http::error::AppResult;
use crate::http::routes::RouteSet;
use crate::services::common::star_on_arrival::StarOnArrival;
use crate::services::cover_art::ArtLookup;
use crate::services::cover_art::animated_art::{FoundArtwork, RELAY_ROOT, RelayFile};
use crate::services::i_music_metadata_service::IMusicMetadataService;

/// How long getAnimatedArt waits for an album Apple has not been asked about.
pub const ASK_WAIT: Duration = Duration::from_secs(10);

/// How long a player may keep a relayed file: the loops at an address never change.
const RELAYED_FOR: &str = "public, max-age=604800, immutable";

pub fn routes() -> RouteSet {
    RouteSet::new()
        .subsonic("getAnimatedArt", get(get_animated_art).post(get_animated_art))
        .route_with_head(&format!("/{RELAY_ROOT}/{{token}}/{{file}}"), get(relay))
}

/// The artwork kept for an album, for an Octo app's getAlbum: `None` for any other app, for XML,
/// and while nothing is known yet (a lookup then starts in the background).
pub(crate) fn album_field(
    state: &AppState,
    call: &SubsonicCall,
    format: &str,
    artist: &str,
    album: &str,
    barcode: Option<&str>,
) -> Option<Value> {
    if format != "json" || !StarOnArrival::is_octo_app(call.param_opt("c")) {
        return None;
    }
    let cover = state.animated_art.peek(artist, album, barcode)?;
    Some(state.animated_art.field(&cover))
}

/// `field` added to a JSON getAlbum answer's album, in place: the album object's first key goes
/// after it, so nothing is parsed. The answer as it was when it has no album object.
pub fn with_album_field(mut body: Vec<u8>, field: &Value) -> Vec<u8> {
    const ALBUM: &[u8] = b"\"album\":{";
    let Some(at) = body.windows(ALBUM.len()).position(|w| w == ALBUM) else {
        return body;
    };
    let open = at + ALBUM.len();
    let mut insert = format!("\"animatedArt\":{field}").into_bytes();
    if body.get(open) != Some(&b'}') {
        insert.push(b',');
    }
    body.splice(open..open, insert);
    body
}

/// [`with_album_field`] on a finished response.
pub async fn response_with_album_field(response: Response, field: &Value) -> Response {
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let body = with_album_field(bytes.to_vec(), field);
    parts
        .headers
        .insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    Response::from_parts(parts, Body::from(body))
}

pub async fn get_animated_art(State(state): State<AppState>, req: Request) -> AppResult {
    let call = match SubsonicCall::read(&state, req).await {
        Ok(call) => call,
        Err(response) => return Ok(response),
    };
    let format = match call.format().to_ascii_lowercase().as_str() {
        "json" | "jsonp" => "json".to_string(),
        _ => "xml".to_string(),
    };
    let builder = &state.subsonic_response_builder;
    if let Some(refused) = refuse_unless_signed_in(&state, &call, &format).await? {
        return Ok(refused);
    }
    if !state.animated_art.offered() {
        return Ok(builder
            .create_error(&format, 0, "Animated art is off on this server")
            .into_response());
    }
    let id = call.param("id").trim().to_string();
    let (artist, album, barcode) = if is_blank(&id) {
        (
            call.param("artist").trim().to_string(),
            call.param("album").trim().to_string(),
            None,
        )
    } else {
        match names_of(&state, &call, &id).await {
            Some(names) => names,
            None => {
                return Ok(builder
                    .create_error(&format, 70, "Album not found")
                    .into_response());
            }
        }
    };
    if is_blank(&artist) || is_blank(&album) {
        return Ok(builder
            .create_error(
                &format,
                10,
                "Required parameter is missing: id, or artist and album",
            )
            .into_response());
    }
    match state
        .animated_art
        .find(&artist, &album, barcode.as_deref(), ASK_WAIT)
        .await
    {
        ArtLookup::Found(cover) => Ok(reply_with(&state, &format, &cover)),
        ArtLookup::None => Ok(octo_reply(&state, &format, None)),
        ArtLookup::Unavailable => Ok(builder
            .create_error(
                &format,
                0,
                "Apple Music could not be asked just now; try again later",
            )
            .into_response()),
    }
}

fn reply_with(state: &AppState, format: &str, cover: &FoundArtwork) -> Response {
    let field = state.animated_art.field(cover);
    octo_reply(state, format, Some(("animatedArt", &field)))
}

/// An album's artist, title and (for an outside album) barcode: from the catalog for an outside
/// album, from Navidrome as the caller for a library one.
async fn names_of(
    state: &AppState,
    call: &SubsonicCall,
    id: &str,
) -> Option<(String, String, Option<String>)> {
    let (outside, provider, external_id) = state.local_library.parse_song_id(id);
    if outside {
        let album = state
            .music_metadata
            .get_album(
                provider.as_deref().unwrap_or_default(),
                external_id.as_deref().unwrap_or_default(),
            )
            .await?;
        let barcode = album.songs.iter().find_map(|song| song.barcode.clone());
        return Some((album.artist, album.title, barcode));
    }
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "subsonic-response")]
        response: Inner,
    }
    #[derive(Deserialize)]
    struct Inner {
        album: Option<Named>,
    }
    #[derive(Deserialize)]
    struct Named {
        artist: Option<String>,
        name: Option<String>,
    }
    let mut parameters = as_json(&call.parameters);
    parameters.insert("id".into(), id.to_string());
    let answer = call.proxy.relay_safe("rest/getAlbum", &parameters).await?;
    let album = serde_json::from_slice::<Envelope>(&answer.body)
        .ok()?
        .response
        .album?;
    Some((album.artist?, album.name?, None))
}

async fn relay(
    State(state): State<AppState>,
    Path((token, file)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let Some(target) = state.animated_art.relay_file(&token, &file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match target {
        RelayFile::Playlist(url) => match state.animated_art.relayed_playlist(&url).await {
            Some(text) => {
                let mut response = Response::new(Body::from(text));
                let out = response.headers_mut();
                out.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/vnd.apple.mpegurl"),
                );
                out.insert(header::CACHE_CONTROL, HeaderValue::from_static(RELAYED_FOR));
                response
            }
            None => StatusCode::BAD_GATEWAY.into_response(),
        },
        RelayFile::Media(url) => {
            let mut ask = state.animated_art.relay_client().get(url);
            for name in [header::RANGE, header::IF_RANGE, header::IF_NONE_MATCH] {
                if let Some(value) = headers.get(&name) {
                    ask = ask.header(name, value.clone());
                }
            }
            let upstream = match ask.send().await {
                Ok(upstream) => upstream,
                Err(e) => {
                    debug!("Animated art relay could not reach Apple: {e}");
                    return StatusCode::BAD_GATEWAY.into_response();
                }
            };
            let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut out = HeaderMap::new();
            for name in [
                header::CONTENT_TYPE,
                header::CONTENT_LENGTH,
                header::CONTENT_RANGE,
                header::ACCEPT_RANGES,
                header::ETAG,
                header::LAST_MODIFIED,
            ] {
                if let Some(value) = upstream.headers().get(&name) {
                    out.insert(name, value.clone());
                }
            }
            if status.is_success() {
                out.insert(header::CACHE_CONTROL, HeaderValue::from_static(RELAYED_FOR));
            }
            let mut response = Response::new(Body::from_stream(
                upstream.bytes_stream().map_err(std::io::Error::other),
            ));
            *response.status_mut() = status;
            *response.headers_mut() = out;
            response
        }
    }
}

#[cfg(test)]
#[path = "animated_art_tests.rs"]
mod tests;
