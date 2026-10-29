//! The last check before Octo writes a song's tags: every tag must agree with the one recording
//! the audio is.
//!
//! Each rule either fixes the song in place or names what it could not settle. Nothing here
//! guesses: a take is written into the title only when the audio was found to be that take, and
//! an album is only cut back to its plain name when the audio was found to be another take than
//! the one the album names.

use std::collections::BTreeSet;

use crate::common::dotnet::{self, is_blank};
use crate::fingerprint::recording_identity::{Identity, RecordingKind};
use crate::models::domain::song::Song;
use crate::tagging::tag_evidence::ReleaseCandidate;

/// What a rule changed, or found and left, in words for the log and "How it was tagged".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardFinding {
    pub rule: &'static str,
    pub detail: String,
    /// The rule changed the song; false when it only found something a person should look at.
    pub fixed: bool,
}

impl GuardFinding {
    fn fixed(rule: &'static str, detail: String) -> Self {
        Self {
            rule,
            detail,
            fixed: true,
        }
    }

    fn found(rule: &'static str, detail: String) -> Self {
        Self {
            rule,
            detail,
            fixed: false,
        }
    }
}

/// Runs every rule on the song about to be written. `identity` is the recording the audio was
/// found to be; `chosen` is the release the album came from.
pub fn guard(
    song: &mut Song,
    identity: Option<&Identity>,
    chosen: Option<&ReleaseCandidate>,
) -> Vec<GuardFinding> {
    let mut findings = Vec::new();
    trim_edges(song, &mut findings);
    if let Some(identity) = identity {
        one_recording(song, identity, chosen, &mut findings);
        album_agrees(song, identity, &mut findings);
        take_in_title(song, identity, &mut findings);
    } else {
        unconfirmed_album(song, &mut findings);
    }
    findings
}

/// Titles and albums carry no stray spaces at either end.
fn trim_edges(song: &mut Song, findings: &mut Vec<GuardFinding>) {
    let title = song.title.trim();
    if title.len() != song.title.len() && !title.is_empty() {
        findings.push(GuardFinding::fixed(
            "spaces",
            format!("took the spaces off the title '{title}'"),
        ));
        song.title = title.to_string();
    }
    let album = song.album.trim();
    if album.len() != song.album.len() && !album.is_empty() {
        findings.push(GuardFinding::fixed(
            "spaces",
            format!("took the spaces off the album '{album}'"),
        ));
        song.album = album.to_string();
    }
}

/// The recording id, and the release ids beside it, all belong to the recording the audio is:
/// a file never names one take as its recording and another take's release track.
fn one_recording(
    song: &mut Song,
    identity: &Identity,
    chosen: Option<&ReleaseCandidate>,
    findings: &mut Vec<GuardFinding>,
) {
    if let Some(written) = song
        .music_brainz_recording_id
        .as_deref()
        .filter(|id| !is_blank(id))
        && !dotnet::eq_ignore_case(written, &identity.recording_id)
    {
        // Another recording of the same kind may stand for the audio (an album and a
        // compilation list it twice); one the exact length ruled out may not.
        if identity.by_length {
            findings.push(GuardFinding::fixed(
                "one recording",
                format!(
                    "the recording id {written} is another take than the audio ({}); written as {}",
                    identity.kind.described(),
                    identity.recording_id
                ),
            ));
            song.music_brainz_recording_id = Some(identity.recording_id.clone());
            clear_release_ids(song);
            return;
        }
    }
    let recording = song
        .music_brainz_recording_id
        .as_deref()
        .filter(|id| !is_blank(id))
        .map(str::to_string);
    let release_from_another = match (
        recording.as_deref(),
        chosen.and_then(|c| c.recording_id.as_deref()),
    ) {
        (Some(recording), Some(chosen)) => !dotnet::eq_ignore_case(recording, chosen),
        _ => false,
    };
    if release_from_another && has_release_ids(song) {
        findings.push(GuardFinding::fixed(
            "one recording",
            "the release ids belonged to another recording than the one written; left off".to_string(),
        ));
        clear_release_ids(song);
    }
}

fn has_release_ids(song: &Song) -> bool {
    [
        &song.music_brainz_release_track_id,
        &song.music_brainz_release_id,
        &song.music_brainz_release_group_id,
    ]
    .iter()
    .any(|id| id.as_deref().is_some_and(|id| !is_blank(id)))
}

fn clear_release_ids(song: &mut Song) {
    song.music_brainz_release_track_id = None;
    song.music_brainz_release_id = None;
    song.music_brainz_release_group_id = None;
}

/// An album that names another take than the audio is cut back to its plain name: "The Black
/// Parade (instrumentals)" for the studio take is "The Black Parade".
fn album_agrees(song: &mut Song, identity: &Identity, findings: &mut Vec<GuardFinding>) {
    if is_blank(&song.album) {
        return;
    }
    let asked = RecordingKind::requested_by(&song.title);
    let Some(named) = identity.contradicted_by(&song.album, &asked) else {
        return;
    };
    let plain = plain_album(&song.album, named);
    match plain {
        Some(plain) => {
            findings.push(GuardFinding::fixed(
                "album agrees",
                format!(
                    "the album '{}' names {}, but the audio is {}; written as '{plain}'",
                    song.album,
                    named.described(),
                    identity.kind.described()
                ),
            ));
            song.album = plain;
            song.music_brainz_album_title = None;
            // The release ids beside the old name are that release's, which the album no longer
            // is; left on, the library would file the song under the other release.
            clear_release_ids(song);
        }
        None => findings.push(GuardFinding::found(
            "album agrees",
            format!(
                "the album '{}' names {}, but the audio is {}; nothing plain was left of the name",
                song.album,
                named.described(),
                identity.kind.described()
            ),
        )),
    }
}

/// The album's name without the one bracket, or the " - ..." tail, that names `kind`; every other
/// bracket stays ("(What's the Story) Morning Glory? (Instrumentals)" is "(What's the Story)
/// Morning Glory?"). None when no such part is found or nothing plain would be left.
pub fn plain_album(album: &str, kind: RecordingKind) -> Option<String> {
    let names_kind = |text: &str| RecordingKind::named_in(text).contains(&kind);
    let mut kept = String::with_capacity(album.len());
    let mut removed = false;
    let mut rest = album;
    while let Some(open) = rest.find(['(', '[']) {
        let close_char = if rest[open..].starts_with('(') { ')' } else { ']' };
        let Some(close) = rest[open + 1..].find(close_char).map(|at| open + 1 + at) else {
            break;
        };
        let inner = &rest[open + 1..close];
        kept.push_str(&rest[..open]);
        if !removed && names_kind(inner) {
            removed = true;
        } else {
            kept.push_str(&rest[open..=close]);
        }
        rest = &rest[close + 1..];
    }
    kept.push_str(rest);
    if !removed
        && let Some(dash) = kept.rfind(" - ")
        && names_kind(&kept[dash + 3..])
    {
        kept.truncate(dash);
        removed = true;
    }
    let plain = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    (removed && !plain.is_empty() && !names_kind(&plain)).then_some(plain)
}

/// Words a remix's title carries in place of "Remix": "(Skrillex Mix)", "(Club Rework)".
static MIX_WORD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:re-?mix(?:ed)?|mix|rmx|rework(?:ed)?|dub|vip|bootleg|flip|edit)\b")
        .expect("a fixed pattern compiles")
});

/// A take that is not the studio one says so in its title: "Welcome to the Black Parade
/// (Live)". Only when the audio is known to be that take.
fn take_in_title(song: &mut Song, identity: &Identity, findings: &mut Vec<GuardFinding>) {
    let Some(word) = identity.kind.title_word() else {
        return;
    };
    if !identity.kind_certain || RecordingKind::marked_in(&song.title).contains(&identity.kind) {
        return;
    }
    if identity.kind == RecordingKind::Remix && MIX_WORD.is_match(&song.title) {
        return;
    }
    let titled = format!("{} ({word})", song.title.trim());
    findings.push(GuardFinding::fixed(
        "take in title",
        format!("the audio is {}; titled '{titled}'", identity.kind.described()),
    ));
    song.title = titled;
}

/// With no recording found for the audio, an album that names a take the title does not is
/// only reported: it may be right (an instrumental on its own album) or a wrong tag.
fn unconfirmed_album(song: &Song, findings: &mut Vec<GuardFinding>) {
    if is_blank(&song.album) {
        return;
    }
    let in_title: BTreeSet<RecordingKind> = RecordingKind::marked_in(&song.title);
    if let Some(named) = RecordingKind::named_in(&song.album)
        .into_iter()
        .find(|kind| !in_title.contains(kind))
    {
        findings.push(GuardFinding::found(
            "album agrees",
            format!(
                "the album '{}' names {} and nothing confirmed what the audio is",
                song.album,
                named.described()
            ),
        ));
    }
}

#[cfg(test)]
#[path = "tag_guard_tests.rs"]
mod tests;
