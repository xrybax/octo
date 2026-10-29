//! getStationSongs (octoStations): a radio station's songs a page at a time. The station's own
//! songs come first; after them the station goes on with new songs of its kind for as long as a
//! client keeps asking, never one the client already queued.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use octo_core::common::dotnet;
use octo_core::models::domain::Song;
use octo_core::models::radio::{LastFmRadioStation, LastFmRadioStationKind, LastFmRadioTrack};
use octo_core::soulseek::RoutingKind;
use octo_subsonic::Parameters;
use tokio::time::Instant;

use super::action_helpers::{
    RequestAborted, SubsonicCall, int_param, materialize_picks, playlist_stations, queue_refresh_if_stale,
    serve_station_songs, suggested_sources,
};
use super::playlists::signed_in;
use crate::app::AppState;
use crate::http::routes::RouteSet;
use crate::services::last_fm::last_fm_radio_recommendation_service::served_song_track;

/// Songs answered when the call does not say how many.
const DEFAULT_COUNT: i32 = 50;

/// The most songs one call answers.
const MOST_SONGS: i32 = 100;

/// How many of the queued ids are read, the newest kept.
const AFTER_READ: usize = 200;

/// How many times the station is asked within one call when what it gave resolves to too few
/// songs (outside songs not found, explicit songs filtered, back-to-back artists dropped).
const ROUNDS: usize = 3;

/// How long a later page waits, over all its rounds, for the station to find new songs. The
/// station goes on looking after the page is answered.
const PAGE_WALK_WAIT: Duration = Duration::from_secs(4);

/// The parameters that sign a call in.
const SIGN_IN_KEYS: [&str; 8] = ["u", "p", "t", "s", "apiKey", "jwt", "v", "c"];

/// The answer for a station that is not the caller's, not there, or not served.
const GONE: &str = "That station isn't on this server any more.";

pub fn routes() -> RouteSet {
    RouteSet::new().subsonic("getStationSongs", get(get_station_songs).post(get_station_songs))
}

/// Whether octoStations is listed: while radio is on and its stations are served as playlists,
/// the ids getStationSongs takes.
pub fn offered(state: &AppState) -> bool {
    let settings = state.settings.current();
    settings.last_fm.enable_radio && settings.last_fm.expose_radio_as_playlists
}

/// `count` songs (1 to 100, 50 when not sent) of the caller's station `id`, none of them one of
/// the `after` ids (repeated, the songs already queued, oldest first), shaped as getPlaylist
/// lists a station's songs. Error 70 for a station that is not the caller's or not there.
pub async fn get_station_songs(State(state): State<AppState>, req: Request) -> Response {
    let aborted = RequestAborted::new();
    let response = station_songs(&state, req, &aborted).await;
    aborted.answered();
    response
}

async fn station_songs(state: &AppState, req: Request, aborted: &RequestAborted) -> Response {
    let call = match SubsonicCall::read(state, req).await {
        Ok(call) => call,
        Err(response) => return response,
    };
    let format = call.format.as_str();
    let builder = &state.subsonic_response_builder;
    let id = call.param_or("id", "").trim().to_string();
    if id.is_empty() {
        return builder
            .create_error(format, 10, "Required parameter is missing: id")
            .into_response();
    }
    if !signed_in(state, &call, format).await {
        return builder
            .create_error(format, 40, "Wrong username or password")
            .into_response();
    }
    // A listener's stations are theirs alone: the list is the caller's own, as for getPlaylist.
    let username = call.param_or("u", "").to_string();
    let Some(station) = playlist_stations(state, &username)
        .into_iter()
        .find(|station| station.id == id)
    else {
        return builder.create_error(format, 70, GONE).into_response();
    };
    let count = int_param(&call.parameters, "count", DEFAULT_COUNT).clamp(1, MOST_SONGS) as usize;
    let after = queued_ids(call.param("after"));
    let songs = next_songs(state, &call, &station, &username, &after, count, aborted).await;
    queue_refresh_if_stale(state, &username);
    builder
        .create_station_songs_response(format, &station, &songs)
        .into_response()
}

/// The ids the client already queued, oldest first: `after` repeated or comma-separated, the
/// last [`AFTER_READ`] of them.
fn queued_ids(after: Option<&str>) -> Vec<String> {
    let ids: Vec<String> = after
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    let older = ids.len().saturating_sub(AFTER_READ);
    ids.into_iter().skip(older).collect()
}

/// The station's next `count` songs after `after`, resolved as getPlaylist resolves a station
/// and served the same way. A first page (no `after`) answers with what the station holds now;
/// a later one waits at most [`PAGE_WALK_WAIT`] for the station to find new songs. Either way
/// the station goes on finding them in the background for the next page.
async fn next_songs(
    state: &AppState,
    call: &SubsonicCall,
    station: &LastFmRadioStation,
    username: &str,
    after: &[String],
    count: usize,
    aborted: &RequestAborted,
) -> Vec<Song> {
    let auth = sign_in(&call.parameters);
    let first_page = after.is_empty();
    let wait_until = Instant::now()
        + if first_page {
            Duration::ZERO
        } else {
            PAGE_WALK_WAIT
        };
    let mut skip: HashSet<String> = after.iter().cloned().collect();
    let mut left_out: Vec<LastFmRadioTrack> = after
        .iter()
        .filter_map(|id| queued_track(state, username, station, id))
        .collect();
    let mut previous: Option<(String, String)> = after
        .last()
        .and_then(|id| queued_track(state, username, station, id))
        .map(|track| (track.artist, track.album.unwrap_or_default()));
    let mut picked: Vec<(Song, Option<LastFmRadioTrack>)> = Vec::new();
    for _ in 0..ROUNDS {
        let wanted = count - picked.len();
        // Discovery Radio asks for half again as many, to stand in for library songs past its
        // share.
        let asked = if station.kind == LastFmRadioStationKind::Discovery {
            wanted + wanted.div_ceil(2)
        } else {
            wanted
        };
        // Songs served before come round again only when this call has found none at all.
        let picks = state
            .last_fm_radio_recommendations
            .next_for_station_within(
                username,
                &station.id,
                &left_out,
                asked,
                picked.is_empty(),
                wait_until.saturating_duration_since(Instant::now()),
                &aborted.token,
            )
            .await;
        if picks.is_empty() {
            break;
        }
        let (tracks, reserve) = picks.split_at(wanted.min(picks.len()));
        let resolved = materialize_picks(
            state,
            &call.proxy,
            station,
            tracks,
            reserve,
            &auth,
            &skip,
            previous
                .as_ref()
                .map(|(artist, album)| (artist.as_str(), album.as_str())),
        )
        .await;
        left_out.extend(picks.iter().cloned());
        for (song, track) in resolved {
            if picked.len() >= count {
                break;
            }
            if skip.insert(song.id.clone()) {
                picked.push((song, Some(track)));
            }
        }
        if let Some((last, _)) = picked.last() {
            previous = Some((last.artist.clone(), last.album.clone()));
        }
        if picked.len() >= count {
            break;
        }
    }
    if first_page && station.kind == LastFmRadioStationKind::Discovery {
        picked = blend_snapshot(state, username, station, picked, &auth, count).await;
    }
    let songs: Vec<Song> = picked.iter().map(|(song, _)| song.clone()).collect();
    let suggested = suggested_sources(&songs);
    let songs = serve_station_songs(state, username, songs, &suggested);
    // A song is remembered as the station track it was picked as, so it stays out by the
    // station's own spelling even when the library's copy is spelled otherwise.
    let tracks: HashMap<String, LastFmRadioTrack> = picked
        .into_iter()
        .filter_map(|(song, track)| Some((song.id, track?)))
        .collect();
    let served: Vec<(Song, Option<LastFmRadioTrack>)> = songs
        .iter()
        .map(|song| (song.clone(), tracks.get(&song.id).cloned()))
        .collect();
    let recommendations = &state.last_fm_radio_recommendations;
    recommendations.record_station_served_as(
        username,
        &station.id,
        served.iter().map(|(song, track)| match track {
            Some(track) => (track.clone(), Some(song.id.clone())),
            None => (served_song_track(song), None),
        }),
    );
    recommendations.remember_served_tracks(
        username,
        served
            .iter()
            .filter_map(|(song, track)| Some((song.id.clone(), track.clone()?))),
    );
    songs
}

/// The page's songs with the station's own (the leading ones, up to the last of the station's
/// tracks) blended as getPlaylist blends Discovery Radio, the rest after them, at most `count`.
async fn blend_snapshot(
    state: &AppState,
    username: &str,
    station: &LastFmRadioStation,
    picked: Vec<(Song, Option<LastFmRadioTrack>)>,
    auth: &Parameters,
    count: usize,
) -> Vec<(Song, Option<LastFmRadioTrack>)> {
    let own: HashSet<String> = station
        .tracks
        .iter()
        .map(|track| dotnet::ordinal_ignore_case_key(&track_key(track)))
        .collect();
    let Some(last_own) = picked.iter().rposition(|(_, track)| {
        track
            .as_ref()
            .is_some_and(|track| own.contains(&dotnet::ordinal_ignore_case_key(&track_key(track))))
    }) else {
        return picked;
    };
    let mut picked = picked;
    let rest = picked.split_off(last_own + 1);
    let mut tracks: HashMap<String, LastFmRadioTrack> = HashMap::new();
    let mut snapshot = Vec::with_capacity(picked.len());
    for (song, track) in picked {
        if let Some(track) = track {
            tracks.insert(song.id.clone(), track);
        }
        snapshot.push(song);
    }
    let blended = state
        .generated_playlists
        .blend_into_discovery(username, station, snapshot, auth)
        .await;
    let mut seen = HashSet::new();
    blended
        .into_iter()
        .map(|song| {
            let track = tracks.remove(&song.id);
            (song, track)
        })
        .chain(rest)
        .filter(|(song, _)| seen.insert(song.id.clone()))
        .take(count)
        .collect()
}

/// The sign-in among a call's parameters, the only ones that go on to Navidrome with the songs
/// looked up for it.
fn sign_in(parameters: &Parameters) -> Parameters {
    parameters
        .iter()
        .filter(|(key, _)| SIGN_IN_KEYS.iter().any(|name| name.eq_ignore_ascii_case(key)))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// A track's key, as the stations match songs.
fn track_key(track: &LastFmRadioTrack) -> String {
    octo_core::last_fm::last_fm_radio_seed_normalizer::track_key(&track.artist, &track.title)
}

/// What a queued id stands for: a song a station served this listener, one of this station's
/// tracks, or an outside song's route. None for an id none of them knows; it is still never
/// answered.
fn queued_track(
    state: &AppState,
    username: &str,
    station: &LastFmRadioStation,
    id: &str,
) -> Option<LastFmRadioTrack> {
    if let Some(track) = state.last_fm_radio_recommendations.served_track(username, id) {
        return Some(track);
    }
    if let Some(track) = station
        .tracks
        .iter()
        .chain(&station.reserve)
        .find(|track| track.resolved_id.as_deref() == Some(id))
    {
        return Some(track.clone());
    }
    let route = state.external_id_registry.lookup(id)?.snapshot();
    let (Some(artist), Some(title)) = (route.artist, route.title) else {
        return None;
    };
    (route.kind == RoutingKind::Song && !artist.is_empty() && !title.is_empty()).then(|| LastFmRadioTrack {
        artist,
        title,
        album: route.album,
        duration: route.duration,
        resolved_id: Some(id.to_string()),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_ids_read_repeats_and_commas_and_keep_the_newest_two_hundred() {
        assert_eq!(queued_ids(Some("a, b,,c")), ["a", "b", "c"]);
        assert!(queued_ids(None).is_empty());
        let many: Vec<String> = (0..250).map(|i| format!("id{i}")).collect();
        let read = queued_ids(Some(&many.join(",")));
        assert_eq!(read.len(), AFTER_READ);
        assert_eq!(read.first().map(String::as_str), Some("id50"));
        assert_eq!(read.last().map(String::as_str), Some("id249"));
    }
}
