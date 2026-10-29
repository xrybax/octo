//! Library health's version check: the songs whose file is not the version their tags say, the
//! imported songs that point at another version than they name, and what can be done about each:
//! retag the file as what it is, replace it with the tagged version, match the import again, put
//! the right file back from Recently removed, or keep it as it is. Nothing here runs by itself.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use indexmap::IndexMap;
use octo_core::common::SongIdentity;
use octo_core::common::dotnet::is_blank;
use octo_core::settings::LibraryAction;
use octo_core::tagging::version_audit::VersionProblem;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::info;

use super::health::{navidrome_user_of, not_signed_in};
use super::json_body::{FromBody, Kind, Member, Shape};
use super::plumbing::{accepted, ok};
use super::request_binding::{bind_body, error, split};
use crate::app::AppState;
use crate::http::routes::RouteSet;
use crate::services::common::download_picks::AnywaySong;
use crate::services::health::health_fixes::{FixStep, TagChange, fix_actions};
use crate::services::imports::ImportTrack;
use crate::services::imports::import_matcher::{LibraryIndex, version_agrees};
use crate::services::library::library_action_executor::LibraryActionRequest;
use crate::services::library::library_identity_sweep::VersionRow;
use crate::services::library::library_tag_edits::song_tag_fields as fields;
use crate::services::library::quality_upgrade_worker::LibrarySongRow;

/// The name the page and its fixes give the version check's finding.
pub const VERSIONS: &str = "versions";

pub const TITLE: &str = "Not the version it's tagged as";
pub const MEANING: &str = "The file is another take of its song than its tags say: a remix by someone else, the original where a remix is named, a sped-up or slowed copy, or a recording of another length.";
pub const ADVICE: &str = "Retag it as what it really is, replace it with the version its tags name, or keep it as it is. Nothing changes until you pick.";
pub const IMPORTS_TITLE: &str = "Wrong file for an imported song";
pub const IMPORTS_MEANING: &str =
    "An imported list's song points at a file of another version than the one it names.";
pub const RETAG_LABEL: &str = "Retag as what it is";
pub const REPLACE_LABEL: &str = "Replace with the right version";
pub const KEEP_LABEL: &str = "Keep";
pub const REMATCH_LABEL: &str = "Match the import again";
pub const PUT_BACK_LABEL: &str = "Put back the right file";
pub const CHECK_NOW_LABEL: &str = "Check now";
pub const COVER_LABEL: &str = "Cover belongs to another artist's release";
pub const REPLACE_COVER_LABEL: &str = "Replace cover";

pub fn routes() -> RouteSet {
    RouteSet::new()
        .route("/api/admin/health/versions/check", post(check_now))
        .route("/api/admin/health/versions/find", post(find))
        .route("/api/admin/health/versions/keep", post(keep))
        .route("/api/admin/health/versions/cover", post(replace_cover))
        .route("/api/admin/health/versions/keepcover", post(keep_cover))
        .route("/api/admin/health/versions/replace", post(replace))
        .route("/api/admin/health/versions/rematch", post(rematch))
        .route("/api/admin/health/versions/putback", post(put_back))
}

/// `{ "ids": [...], "id": "...", "owner": "..." }`: the songs a request is about, the one a
/// check or a put back is about, and whose imported lists are matched again.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct VersionRequest {
    ids: Option<Vec<String>>,
    id: Option<String>,
    owner: Option<String>,
}

impl FromBody for VersionRequest {
    const SHAPE: &'static Shape = &Shape {
        name: "Octo.Controllers.LibraryHealthController+VersionRequest",
        by_constructor: true,
        members: &[
            Member {
                name: "ids",
                kind: Kind::TextList,
            },
            Member {
                name: "id",
                kind: Kind::Text,
            },
            Member {
                name: "owner",
                kind: Kind::Text,
            },
        ],
    };
}

fn problem_label(problem: Option<VersionProblem>) -> &'static str {
    match problem {
        Some(VersionProblem::OtherVersion) => "Another version",
        Some(VersionProblem::Speed) => "Sped up or slowed",
        Some(VersionProblem::LengthOff) => "Another length",
        None => "Another version",
    }
}

fn filter_of(problem: Option<VersionProblem>) -> &'static str {
    match problem {
        Some(VersionProblem::Speed) => "speed",
        Some(VersionProblem::LengthOff) => "length",
        _ => "other",
    }
}

fn count_text(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// What Retag as what it is writes for one song, each change in words.
fn retag_changes(row: &VersionRow) -> Vec<Value> {
    row.version
        .retag
        .iter()
        .map(|(tag, value)| {
            let now = match tag.as_str() {
                fields::TITLE => Some(row.title.as_str()),
                fields::ALBUM => Some(row.album.as_str()),
                _ => None,
            };
            let label = crate::services::health::health_fixes::tag_name(tag);
            let words = match value {
                Some(value) => TagChange::new(tag, now, value).words(),
                None => format!("{label}: taken off"),
            };
            json!({ "tag": tag, "label": label, "value": value, "words": words })
        })
        .collect()
}

/// One imported song pointing at a file of another version.
struct WrongImport {
    owner: String,
    list_id: String,
    list_name: String,
    track: ImportTrack,
    points_at: String,
    words: String,
    /// The library song of the version the row names, when the library has one.
    right_in_library: Option<(String, String, String)>,
    /// A song of that version in Recently removed, when the library has none.
    right_removed: Option<(String, String, String)>,
}

fn pseudo_row(id: &str, title: &str, artist: &str, album: &str) -> LibrarySongRow {
    LibrarySongRow {
        id: id.to_string(),
        title: title.to_string(),
        artist: artist.to_string(),
        album: Some(album.to_string()),
        ..LibrarySongRow::default()
    }
}

/// The imported songs that point at a file whose title or album names another version than the
/// song does, or whose audio the version check found is another version.
fn wrong_imports(
    state: &AppState,
    songs: &[Arc<LibrarySongRow>],
    versions: &[VersionRow],
) -> Vec<WrongImport> {
    let by_id: HashMap<&str, &LibrarySongRow> = songs
        .iter()
        .map(|song| (song.id.as_str(), song.as_ref()))
        .collect();
    let found: HashMap<&str, &VersionRow> = versions.iter().map(|row| (row.id.as_str(), row)).collect();
    let mut wrong: Vec<WrongImport> = Vec::new();
    for list in state.imports.all_lists() {
        for track in &list.tracks {
            let Some(id) = track.library_id.as_deref() else {
                continue;
            };
            let Some(file) = by_id.get(id) else {
                continue;
            };
            let words = if !version_agrees(track, file) {
                format!(
                    "'{}' points at '{}' on '{}', which names another version.",
                    track.title,
                    file.title,
                    file.album.as_deref().unwrap_or("")
                )
            } else if let Some(row) = found.get(id) {
                format!(
                    "'{}' points at '{}', whose audio is not that version: {}",
                    track.title, file.title, row.version.words
                )
            } else {
                continue;
            };
            wrong.push(WrongImport {
                owner: list.owner.clone(),
                list_id: list.id.clone(),
                list_name: list.name.clone(),
                track: track.clone(),
                points_at: id.to_string(),
                words,
                right_in_library: None,
                right_removed: None,
            });
        }
    }
    if wrong.is_empty() {
        return wrong;
    }
    let index = LibraryIndex::new(
        songs
            .iter()
            .filter(|song| !found.contains_key(song.id.as_str()))
            .map(|song| song.as_ref().clone()),
    );
    let removed = state.library_edits.trash();
    for item in &mut wrong {
        item.right_in_library = index
            .find(&item.track)
            .filter(|row| row.id != item.points_at)
            .map(|row| {
                (
                    row.id.clone(),
                    row.title.clone(),
                    row.album.clone().unwrap_or_default(),
                )
            });
        if item.right_in_library.is_none() {
            item.right_removed = removed
                .iter()
                .filter(|gone| gone.replaced.is_none())
                .find(|gone| {
                    let row = pseudo_row(&gone.id, &gone.title, &gone.artist, &gone.album);
                    version_agrees(&item.track, &row)
                        && SongIdentity::same_text(
                            &item.track.title,
                            &item.track.artist,
                            &gone.title,
                            &gone.artist,
                            None,
                        )
                        .is_same()
                })
                .map(|gone| (gone.id.clone(), gone.title.clone(), gone.album.clone()));
        }
    }
    wrong
}

/// The version check's finding for the report, shaped like the other checks; None when nothing
/// was found.
pub fn versions_check(state: &AppState, songs: &[Arc<LibrarySongRow>]) -> Option<Value> {
    let sweep = &state.library_identity_sweep;
    if !sweep.checks_versions() {
        return None;
    }
    let rows = sweep.version_findings();
    let covers = sweep.cover_findings();
    let imports = wrong_imports(state, songs, &rows);
    if rows.is_empty() && imports.is_empty() && covers.is_empty() {
        return None;
    }
    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            let problem = row.version.problem();
            json!({
                "id": row.id,
                "problem": filter_of(problem),
                "problemLabel": problem_label(problem),
                "heard": row.version.heard,
                "sureLabel": if row.version.heard { "Heard in the audio" } else { "Likely" },
                "words": row.version.words,
                "is": row.version.is_title,
                "retag": retag_changes(row),
                "canRetag": !row.version.retag.is_empty(),
                "expectedSeconds": row.version.expected_seconds,
            })
        })
        .collect();
    let items: Vec<Value> = items
        .into_iter()
        .chain(covers.iter().map(|row| {
            json!({
                "id": row.id,
                "problem": "cover",
                "problemLabel": COVER_LABEL,
                "heard": true,
                "sureLabel": "Seen in the cover",
                "words": row.cover.words,
                "is": Value::Null,
                "retag": [],
                "canRetag": false,
                "cover": true,
                "release": row.cover.release_title,
                "releaseArtist": row.cover.release_artist,
            })
        }))
        .collect();
    let import_items: Vec<Value> = imports
        .iter()
        .map(|item| {
            json!({
                "owner": item.owner,
                "list": item.list_id,
                "listName": item.list_name,
                "key": item.track.key,
                "title": item.track.title,
                "artist": item.track.artist,
                "album": item.track.album,
                "pointsAt": item.points_at,
                "words": item.words,
                "right": item.right_in_library.as_ref().map(|(id, title, album)| json!({ "id": id, "title": title, "album": album, "where": "library" }))
                    .or_else(|| item.right_removed.as_ref().map(|(id, title, album)| json!({ "id": id, "title": title, "album": album, "where": "removed" }))),
            })
        })
        .collect();
    let count = |problem: &str| items.iter().filter(|item| item["problem"] == problem).count();
    let filters = json!([
        { "id": "all", "label": "All", "count": items.len() + import_items.len() },
        { "id": "other", "label": "Another version", "count": count("other") },
        { "id": "speed", "label": "Sped up or slowed", "count": count("speed") },
        { "id": "length", "label": "Another length", "count": count("length") },
        { "id": "cover", "label": "Another artist's cover", "count": count("cover") },
        { "id": "imports", "label": "Imported songs", "count": import_items.len() },
    ]);
    let total = items.len() + import_items.len();
    Some(json!({
        "check": VERSIONS,
        "title": TITLE,
        "count": total,
        "countLabel": count_text(total, "song", "songs"),
        "meaning": MEANING,
        "advice": ADVICE,
        "importsTitle": IMPORTS_TITLE,
        "importsMeaning": IMPORTS_MEANING,
        "retagLabel": RETAG_LABEL,
        "replaceLabel": REPLACE_LABEL,
        "keepLabel": KEEP_LABEL,
        "rematchLabel": REMATCH_LABEL,
        "putBackLabel": PUT_BACK_LABEL,
        "checkNowLabel": CHECK_NOW_LABEL,
        "filters": filters,
        "songs": rows.iter().map(|row| row.id.clone()).chain(covers.iter().map(|row| row.id.clone())).collect::<Vec<_>>(),
        "replaceCoverLabel": REPLACE_COVER_LABEL,
        "items": items,
        "imports": import_items,
    }))
}

/// What Retag as what it is would write for the songs in `keys`. Writes nothing.
pub fn preview(state: &AppState, user: &str, keys: Option<&[String]>) -> Response {
    let can = state.library_health.abilities(user);
    let rows = state.library_identity_sweep.version_findings();
    let picked: Vec<&VersionRow> = match keys.filter(|keys| !keys.is_empty()) {
        Some(keys) => rows
            .iter()
            .filter(|row| keys.contains(&row.id) && !row.version.retag.is_empty())
            .collect(),
        None => Vec::new(),
    };
    if picked.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "Nothing says what these files really are, so there is nothing to retag them as. Replace them or keep them.",
        );
    }
    let one = picked.len() == 1;
    let lines: Vec<Value> = picked
        .iter()
        .map(|row| {
            let changes: Vec<String> = retag_changes(row)
                .iter()
                .filter_map(|change| change["words"].as_str().map(str::to_string))
                .collect();
            json!({ "id": row.id, "title": row.title, "words": changes.join("; ") })
        })
        .collect();
    let steps: Vec<Value> = picked
        .iter()
        .map(|row| {
            let mut with: IndexMap<String, String> = row
                .version
                .retag
                .iter()
                .map(|(tag, value)| (tag.clone(), value.clone().unwrap_or_default()))
                .collect();
            with.insert(fix_actions::FILE_STAMP.to_string(), row.stamp.clone());
            super::health::step_view(&FixStep::with(fix_actions::RETAG, &row.id, &row.title, with))
        })
        .collect();
    ok(&json!({
        "title": if one { RETAG_LABEL.to_string() } else { format!("{RETAG_LABEL}: {}", count_text(picked.len(), "song", "songs")) },
        "meaning": "Each file gets the title and ids of the recording it really is. Undo puts the old tags back.",
        "go": if one { "Retag it".to_string() } else { format!("Retag {}", count_text(picked.len(), "song", "songs")) },
        "label": "Retagging as what they are",
        "canFix": can.edit,
        "why": if can.edit { None } else { can.why.clone() },
        "lines": lines,
        "steps": steps,
    }))
}

async fn bound(request: Request) -> Result<(axum::http::request::Parts, VersionRequest), Box<Response>> {
    let (parts, body) = split(request).await;
    let bound: VersionRequest = bind_body(&parts, &body)?;
    Ok((parts, bound))
}

/// Checks one song's tags and version now.
async fn check_now(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = match bound(request).await {
        Ok(bound) => bound,
        Err(answer) => return *answer,
    };
    if navidrome_user_of(&state, &parts).is_none() {
        return not_signed_in();
    }
    let Some(id) = body.id.filter(|id| !is_blank(id)) else {
        return error(StatusCode::BAD_REQUEST, "Say which song to check.");
    };
    match state.library_identity_sweep.check_now(&id).await {
        Ok(song) => ok(&json!({
            "id": id,
            "title": song.title,
            "version": song.version.as_ref().filter(|v| !v.kept).map(|v| json!({
                "problem": filter_of(v.problem()),
                "problemLabel": problem_label(v.problem()),
                "heard": v.heard,
                "words": v.words,
                "is": v.is_title,
            })),
            "words": match song.version.as_ref().filter(|v| !v.kept) {
                Some(version) => version.words.clone(),
                None => "It is the version its tags say, as far as its audio and the music database tell.".to_string(),
            },
        })),
        Err(why) => error(StatusCode::CONFLICT, &why),
    }
}

/// The library songs whose title or artist holds every word of the search, for Check one song now.
async fn find(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = match bound(request).await {
        Ok(bound) => bound,
        Err(answer) => return *answer,
    };
    if navidrome_user_of(&state, &parts).is_none() {
        return not_signed_in();
    }
    let query = body.id.unwrap_or_default();
    if is_blank(&query) {
        return error(StatusCode::BAD_REQUEST, "Say what song to look for.");
    }
    let Some(found) = state.library_identity_sweep.find_songs(&query, FIND_LIMIT).await else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Octo could not read your library from Navidrome.",
        );
    };
    let songs: Vec<Value> = found
        .iter()
        .map(|song| json!({ "id": song.id, "title": song.title, "artist": song.artist, "album": song.album }))
        .collect();
    ok(&json!({ "songs": songs }))
}

/// How many songs Check one song now offers for a search.
const FIND_LIMIT: usize = 12;

/// Keeps these songs' covers as they are: the findings go until the files change.
async fn keep_cover(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = match bound(request).await {
        Ok(bound) => bound,
        Err(answer) => return *answer,
    };
    if navidrome_user_of(&state, &parts).is_none() {
        return not_signed_in();
    }
    let ids = body.ids.unwrap_or_default();
    if ids.is_empty() {
        return error(StatusCode::BAD_REQUEST, "Say which songs to keep.");
    }
    let kept = state.library_identity_sweep.keep_covers(&ids);
    ok(&json!({ "ok": true, "kept": kept }))
}

/// Replaces each song's album cover, which is another artist's release's: the cover is rejected
/// and another found by the album's own artist, or Octo's drawn cover goes in when none is.
/// Undo puts the old picture back, as for a cover run.
async fn replace_cover(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = match bound(request).await {
        Ok(bound) => bound,
        Err(answer) => return *answer,
    };
    let Some(user) = navidrome_user_of(&state, &parts) else {
        return not_signed_in();
    };
    let can = state.library_health.abilities(&user);
    if !can.edit {
        return error(
            StatusCode::FORBIDDEN,
            can.why.as_deref().unwrap_or("Octo may not change files for you."),
        );
    }
    let ids = body.ids.unwrap_or_default();
    let rows: Vec<_> = state
        .library_identity_sweep
        .cover_findings()
        .into_iter()
        .filter(|row| ids.contains(&row.id))
        .collect();
    if rows.is_empty() {
        return error(StatusCode::BAD_REQUEST, "Say which songs' covers to replace.");
    }
    let mut lines: Vec<String> = Vec::new();
    for row in &rows {
        let covers = state.cover_art_service.clone();
        let outcome = state
            .library_action_executor
            .replace_cover_from_check(&row.id, move |album, artist| {
                covers.get_named_cover(
                    album,
                    Some(artist),
                    Some(1200),
                    octo_media::cover::cover_art_service::list_kinds::MIX,
                )
            })
            .await;
        info!(
            "Version check: replaced the cover of '{} - {}' for {user}: {}",
            row.artist,
            row.title,
            outcome.detail.as_deref().unwrap_or("")
        );
        lines.push(format!("{}: {}", row.title, outcome.detail.unwrap_or_default()));
    }
    ok(&json!({ "ok": true, "words": lines.join(" ") }))
}

/// Keeps these files as they are: their findings go until the files change.
async fn keep(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = match bound(request).await {
        Ok(bound) => bound,
        Err(answer) => return *answer,
    };
    if navidrome_user_of(&state, &parts).is_none() {
        return not_signed_in();
    }
    let ids = body.ids.unwrap_or_default();
    if ids.is_empty() {
        return error(StatusCode::BAD_REQUEST, "Say which songs to keep.");
    }
    let kept = state.library_identity_sweep.keep_versions(&ids);
    ok(&json!({ "ok": true, "kept": kept }))
}

/// Replaces each file with the version its tags name, through the download checks: held to its
/// tagged title and length, so another take is refused. The old file goes to Recently removed.
async fn replace(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = match bound(request).await {
        Ok(bound) => bound,
        Err(answer) => return *answer,
    };
    let Some(user) = navidrome_user_of(&state, &parts) else {
        return not_signed_in();
    };
    let can = state.library_health.abilities(&user);
    if !can.edit {
        return error(
            StatusCode::FORBIDDEN,
            can.why.as_deref().unwrap_or("Octo may not change files for you."),
        );
    }
    let actions = state.settings.current().library_actions.clone();
    let refusal = if !actions.enabled {
        Some("Library actions are off. Turn them on to replace songs.".to_string())
    } else if !actions.is_allowed(Some(&user)) {
        Some(format!("{user} is not on the library actions allowlist."))
    } else if !actions
        .effective_actions()
        .into_iter()
        .any(|entry| entry.action == LibraryAction::WrongVersion && entry.enabled)
    {
        Some(
            "The Wrong version action is off under Library actions. Turn it on to replace songs.".to_string(),
        )
    } else {
        None
    };
    if let Some(refusal) = refusal {
        return error(StatusCode::BAD_REQUEST, &refusal);
    }
    let ids = body.ids.unwrap_or_default();
    let rows: Vec<VersionRow> = state
        .library_identity_sweep
        .version_findings()
        .into_iter()
        .filter(|row| ids.contains(&row.id))
        .collect();
    if rows.is_empty() {
        return error(StatusCode::BAD_REQUEST, "Say which songs to replace.");
    }
    for row in &rows {
        let mut request = LibraryActionRequest::new(LibraryAction::WrongVersion, &row.id, &user);
        request.anyway = Some(AnywaySong {
            title: row.title.clone(),
            album: Some(row.album.clone()).filter(|album| !is_blank(album)),
            length: row.version.expected_seconds,
            retitle: false,
        });
        let executor = state.library_action_executor.clone();
        let (artist, title) = (row.artist.clone(), row.title.clone());
        tokio::spawn(async move {
            match executor.apply(request).await {
                Ok(outcome) => info!(
                    "Version check: replacing '{artist} - {title}': {}",
                    outcome.detail.as_deref().unwrap_or("")
                ),
                Err(e) => info!("Version check: replacing '{artist} - {title}' failed: {e}"),
            }
        });
    }
    accepted(&json!({
        "ok": true,
        "queued": rows.len(),
        "words": format!(
            "Looking for the right version of {}. Each one shows in Downloads; the old file goes to Recently removed.",
            count_text(rows.len(), "song", "songs")
        ),
    }))
}

/// Matches a person's imported lists again, so each song points at the version it names.
async fn rematch(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = match bound(request).await {
        Ok(bound) => bound,
        Err(answer) => return *answer,
    };
    let Some(user) = navidrome_user_of(&state, &parts) else {
        return not_signed_in();
    };
    let owner = body.owner.filter(|owner| !is_blank(owner)).unwrap_or(user);
    state.imports.rematch(&owner).await;
    ok(&json!({ "ok": true, "words": "Matched the lists again." }))
}

/// Puts a removed song back from Recently removed, then matches the lists again.
async fn put_back(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = match bound(request).await {
        Ok(bound) => bound,
        Err(answer) => return *answer,
    };
    let Some(user) = navidrome_user_of(&state, &parts) else {
        return not_signed_in();
    };
    let Some(id) = body.id.filter(|id| !is_blank(id)) else {
        return error(StatusCode::BAD_REQUEST, "Say which song to put back.");
    };
    let outcome = state.library_edits.restore(&id, &user).await;
    if outcome.state != "applied" {
        return error(
            StatusCode::CONFLICT,
            outcome.detail.as_deref().unwrap_or("It could not be put back."),
        );
    }
    let owner = body.owner.filter(|owner| !is_blank(owner)).unwrap_or(user);
    state.imports.rematch(&owner).await;
    ok(&json!({
        "ok": true,
        "words": "Put back. The list points at it once Navidrome has scanned it; match again then if it does not.",
    }))
}
