//! A library file's tags held against the one recording its audio was found to be.
//!
//! The same rules the write-time guard runs on a download, run on a file already in the
//! library: stray spaces, an album that names another take, a take the title hides, and ids
//! that belong to another recording than the audio. Each finding says in words what is wrong,
//! and the audit names the tags that would put it right. Nothing here writes; the library
//! sweep and Library health decide what is written, and when.

use std::collections::{BTreeSet, HashMap};

use indexmap::IndexMap;

use crate::common::dotnet::{self, is_blank};
use crate::common::song_identity::SongIdentity;
use crate::fingerprint::recording_identity::{Identity, RecordingKind};
use crate::models::domain::song::Song;
use crate::tagging::tag_guard::{self, GuardFinding};

/// The names the apps and Library health use for the tags an audit may write.
pub mod audit_fields {
    pub const TITLE: &str = "title";
    pub const ALBUM: &str = "album";
    pub const RECORDING_ID: &str = "recordingId";
    pub const RELEASE_TRACK_ID: &str = "releaseTrackId";
    pub const RELEASE_ID: &str = "releaseId";
    pub const RELEASE_GROUP_ID: &str = "releaseGroupId";
    /// The album artist's other names: only ever taken off.
    pub const ALBUM_ARTIST_NAMES: &str = "albumArtistNames";
    /// Other names of the title, album, track and disc numbers and ids: only ever taken off.
    pub const OTHER_NAMES: &str = "otherNames";
}

use audit_fields as fields;

/// What a library file's tags say now, as written: spaces and all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileTags {
    pub title: String,
    pub album: String,
    pub recording_id: Option<String>,
    pub release_track_id: Option<String>,
    pub release_id: Option<String>,
    pub release_group_id: Option<String>,
    /// The album artist written again under other names the library server reads beside it.
    pub other_album_artists: Option<OtherAlbumArtists>,
    /// Other fields written again under other names the library server reads beside them.
    pub other_names: Vec<OtherNames>,
}

/// One field a file holds under other names beside its own value, which the library server
/// reads as the same field: TRACK beside TRACKNUMBER.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OtherNames {
    /// The field, in words: "track number".
    pub field: String,
    /// The value in the field's own place.
    pub own: String,
    /// The other fields' names, as the file spells them.
    pub names: Vec<String>,
    pub values: Vec<String>,
    /// Every value is the field's own value again.
    pub copies: bool,
}

/// Fields beside a file's own album artist that the library server reads as more album artists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OtherAlbumArtists {
    /// The album artist in the file's own field.
    pub album_artist: String,
    /// The other fields' names, as the file spells them.
    pub names: Vec<String>,
    pub values: Vec<String>,
    /// Every value names the album artist again, in other capitals or spacing.
    pub copies: bool,
}

/// One kind of disagreement between a file's tags and its audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AuditRule {
    /// A title or an album with spaces at either end.
    Spaces,
    /// The album names a take the audio is not: studio audio on "(instrumentals)".
    Album,
    /// The audio is a take its title does not say: a live take titled plainly.
    Title,
    /// The recording id or the release track id belongs to another recording than the audio.
    Ids,
    /// Another copy of the same song in the library is a different take.
    Takes,
    /// The album artist is written again under other names the library server reads.
    AlbumArtistNames,
    /// Another field is written again under other names the library server reads.
    OtherNames,
}

impl AuditRule {
    pub const ALL: [AuditRule; 7] = [
        AuditRule::Spaces,
        AuditRule::Album,
        AuditRule::Title,
        AuditRule::Ids,
        AuditRule::Takes,
        AuditRule::AlbumArtistNames,
        AuditRule::OtherNames,
    ];

    /// The rule's name, as the state file and the dashboard keep it.
    pub fn name(self) -> &'static str {
        match self {
            AuditRule::Spaces => "spaces",
            AuditRule::Album => "album",
            AuditRule::Title => "title",
            AuditRule::Ids => "ids",
            AuditRule::Takes => "takes",
            AuditRule::AlbumArtistNames => "albumArtistNames",
            AuditRule::OtherNames => "otherNames",
        }
    }

    pub fn parse(name: &str) -> Option<AuditRule> {
        AuditRule::ALL.into_iter().find(|rule| rule.name() == name)
    }

    /// A few words for the rule, as a badge shows it.
    pub fn label(self) -> &'static str {
        match self {
            AuditRule::Spaces => "Stray spaces",
            AuditRule::Album => "Album names another take",
            AuditRule::Title => "Title hides the take",
            AuditRule::Ids => "Ids of another recording",
            AuditRule::Takes => "Another take beside it",
            AuditRule::AlbumArtistNames => "Album artist under other names",
            AuditRule::OtherNames => "Tags under other names",
        }
    }
}

/// One thing an audit found, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditFinding {
    pub rule: AuditRule,
    pub words: String,
}

/// What is known of a recording a file's tags name, other than the one the audio was found to
/// be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamedRecording {
    /// The fingerprint names it among the recordings of the audio.
    pub listed: bool,
    /// The kind of take it is, when the fingerprint or the music database says.
    pub kind: Option<RecordingKind>,
    /// How long it runs, in milliseconds, when known.
    pub length_ms: Option<i64>,
}

/// What the fingerprint said about a file's audio, beyond the one recording it decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeardAs {
    pub identity: Identity,
    /// The decision cannot be another recording: the exact length chose it, or it was the only
    /// recording that agreed with the song. Only a sure identity writes ids, and only sure
    /// findings are fixed all at once.
    pub sure: bool,
    /// How long the file runs, in milliseconds, when known.
    pub file_ms: Option<i64>,
    /// What is known of the file's own recording id. None when nothing is: a recording the
    /// fingerprint does not name and the music database was not asked about.
    pub written: Option<NamedRecording>,
    /// The recording the file's release track id belongs to, and what is known of it.
    pub track_owner: Option<(String, NamedRecording)>,
    /// Whether the file's release lists the audio's recording: `Some(Some(track))` with the
    /// track it is there, `Some(None)` when it does not list it, None when that is unknown.
    pub on_release: Option<Option<String>>,
}

/// How far a recording's length may be from the file's and still be the same audio: at least
/// this many milliseconds, and at least [`LENGTH_SLACK`] of the file's length. Masterings and
/// releases of one recording list lengths a few seconds apart.
pub const MIN_LENGTH_SLACK_MS: i64 = 10_000;
pub const LENGTH_SLACK: f64 = 0.07;

/// Whether a recording that runs `recording_ms` cannot be a file that runs `file_ms`.
pub fn length_contradicts(recording_ms: i64, file_ms: i64) -> bool {
    if recording_ms <= 0 || file_ms <= 0 {
        return false;
    }
    let slack = MIN_LENGTH_SLACK_MS.max((file_ms as f64 * LENGTH_SLACK).round() as i64);
    (recording_ms - file_ms).abs() > slack
}

/// Why a recording a file's tags name is not the audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Elsewhere {
    /// It is another kind of take than the audio, which is known to be its own kind.
    Take(RecordingKind),
    /// The file's exact length chose another recording of the same take.
    Length,
    /// It runs this many milliseconds, too far from the file's length.
    Runs(i64),
}

/// Why the recording `id` cannot be the audio, or None when it is the audio, may be, or nothing
/// says. A recording the fingerprint does not name is not wrong for that alone: an album and a
/// remaster list the same audio under recordings AcoustID often does not tie to the print. It is
/// only wrong when the music database says its take or its length is not the audio's.
fn elsewhere(heard: &HeardAs, id: &str, named: Option<&NamedRecording>) -> Option<Elsewhere> {
    let identity = &heard.identity;
    if dotnet::eq_ignore_case(id, &identity.recording_id) {
        return None;
    }
    let named = named?;
    if named.listed {
        // Among the takes the print is tied to, only a known take rules one out: when the take
        // was only the likeliest pick, the file's own may be as right.
        let kind = named.kind.unwrap_or(identity.kind);
        if !identity.kind_certain || identity.admits(id, kind) {
            return None;
        }
        return Some(if kind == identity.kind {
            Elsewhere::Length
        } else {
            Elsewhere::Take(kind)
        });
    }
    if identity.kind_certain
        && let Some(kind) = named.kind
        && kind != identity.kind
    {
        return Some(Elsewhere::Take(kind));
    }
    match (named.length_ms, heard.file_ms) {
        (Some(length), Some(file)) if length_contradicts(length, file) => Some(Elsewhere::Runs(length)),
        _ => None,
    }
}

/// A length in words: "5:11".
fn minutes(ms: i64) -> String {
    let seconds = (ms + 500) / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// What the audio is, in words: its take when that is known, else only which recording.
fn audio_words(heard: &HeardAs) -> String {
    let identity = &heard.identity;
    if identity.kind_certain {
        format!("{} ({})", identity.kind.described(), identity.recording_id)
    } else {
        format!("the recording {}", identity.recording_id)
    }
}

/// What an audit found, and the tags that would put it right: a value to write, or None to
/// take a tag off the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagAudit {
    pub findings: Vec<AuditFinding>,
    pub changes: IndexMap<String, Option<String>>,
    pub sure: bool,
}

impl TagAudit {
    /// Whether the audit found anything.
    pub fn found(&self) -> bool {
        !self.findings.is_empty()
    }

    /// Whether every finding is only stray spaces, which are put right without asking.
    pub fn only_spaces(&self) -> bool {
        self.found() && self.findings.iter().all(|f| f.rule == AuditRule::Spaces)
    }

    /// The changes that take stray spaces off and nothing else.
    pub fn space_changes(&self, tags: &FileTags) -> IndexMap<String, Option<String>> {
        let mut changes = IndexMap::new();
        let title = tags.title.trim();
        if title.len() != tags.title.len() && !title.is_empty() {
            changes.insert(fields::TITLE.to_string(), Some(title.to_string()));
        }
        let album = tags.album.trim();
        if album.len() != tags.album.len() && !album.is_empty() {
            changes.insert(fields::ALBUM.to_string(), Some(album.to_string()));
        }
        changes
    }

    /// The changes put right without asking: stray spaces, and other names of the album artist
    /// or of another field that only repeat its value.
    pub fn automatic_changes(&self, tags: &FileTags) -> IndexMap<String, Option<String>> {
        let mut changes = self.space_changes(tags);
        if tags
            .other_album_artists
            .as_ref()
            .is_some_and(|other| other.copies)
        {
            changes.insert(fields::ALBUM_ARTIST_NAMES.to_string(), None);
        }
        if !tags.other_names.is_empty() && tags.other_names.iter().all(|other| other.copies) {
            changes.insert(fields::OTHER_NAMES.to_string(), None);
        }
        changes
    }
}

/// The other names of the album artist and of the other fields held alone, for a file whose
/// other answers are known: the findings and their changes, sure only when every one is a copy.
pub fn other_names_alone(tags: &FileTags) -> TagAudit {
    let mut found = TagAudit {
        sure: true,
        ..TagAudit::default()
    };
    if let Some(other) = &tags.other_album_artists {
        other_album_artists(other, &mut found);
    }
    other_names(&tags.other_names, &mut found);
    found
}

/// The findings for fields under other names, one a field, and their change: the other names
/// come off, each field's own value stays. Ones that say something else wait for a person.
fn other_names(others: &[OtherNames], found: &mut TagAudit) {
    for other in others {
        let names = listed(&other.names);
        let own = other.own.trim();
        let field = &other.field;
        let words = if other.copies {
            format!(
                "The {field} '{own}' is written again under {names}, which the library server reads as the {field} too."
            )
        } else {
            let values: Vec<String> = other.values.iter().map(|value| format!("'{value}'")).collect();
            let say = if other.names.len() == 1 { "says" } else { "say" };
            found.sure = false;
            format!(
                "Beside the {field} '{own}', {names} {say} {}. The library server reads them as the {field} too and can show that instead. Taking them off keeps '{own}'.",
                listed(&values)
            )
        };
        found.findings.push(AuditFinding {
            rule: AuditRule::OtherNames,
            words,
        });
    }
    if !others.is_empty() {
        found.changes.insert(fields::OTHER_NAMES.to_string(), None);
    }
}

/// Names in a sentence: "A", "A and B", "A, B and C".
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

/// The finding for an album artist under other names, and its change: they come off, the file's
/// own album artist stays. Ones that name someone else wait for a person.
fn other_album_artists(other: &OtherAlbumArtists, found: &mut TagAudit) {
    let names = listed(&other.names);
    let own = other.album_artist.trim();
    let words = if other.copies {
        format!(
            "The album artist '{own}' is written again under {names}. The library server reads each as another album artist, so this song can sit on an album of its own."
        )
    } else {
        let values: Vec<String> = other.values.iter().map(|value| format!("'{value}'")).collect();
        found.sure = false;
        format!(
            "Beside the album artist '{own}', {names} name {}. The library server reads them with the album artist, so this song can sit on an album of its own. Taking them off keeps '{own}'.",
            listed(&values)
        )
    };
    found.findings.push(AuditFinding {
        rule: AuditRule::AlbumArtistNames,
        words,
    });
    found.changes.insert(fields::ALBUM_ARTIST_NAMES.to_string(), None);
}

/// Holds a file's tags against its audio. Without `heard` only the spaces are checked: with no
/// recording found for the audio there is nothing to hold the rest against.
pub fn audit(tags: &FileTags, heard: Option<&HeardAs>) -> TagAudit {
    // The song the guard sees carries the file's release ids, so an album it cuts back to its
    // plain name takes them off with it. No recording id: a library file's recording id is
    // judged below, against the release it came from, which the guard of a download never has.
    let mut song = Song {
        title: tags.title.clone(),
        album: tags.album.clone(),
        music_brainz_release_track_id: tags.release_track_id.clone(),
        music_brainz_release_id: tags.release_id.clone(),
        music_brainz_release_group_id: tags.release_group_id.clone(),
        ..Default::default()
    };
    let guarded = tag_guard::guard(&mut song, heard.map(|h| &h.identity), None);
    let mut found = TagAudit {
        sure: heard.is_some_and(|h| h.sure),
        ..TagAudit::default()
    };
    // The guard decides; the words are said here, about the file as it is now.
    for GuardFinding { rule, detail, fixed } in guarded {
        let (rule, words) = match (rule, heard) {
            ("spaces", _) => {
                let (what, value) = if detail.contains("the album") {
                    ("album", tags.album.trim())
                } else {
                    ("title", tags.title.trim())
                };
                (
                    AuditRule::Spaces,
                    format!("The {what} '{value}' has stray spaces at its ends."),
                )
            }
            ("album agrees", Some(heard)) => {
                let asked = RecordingKind::requested_by(&tags.title);
                let named = heard
                    .identity
                    .contradicted_by(&tags.album, &asked)
                    .map_or("another take", RecordingKind::described);
                let left = if fixed {
                    ""
                } else {
                    " Nothing plain is left of the name, so it waits for you."
                };
                (
                    AuditRule::Album,
                    format!(
                        "The album '{}' names {named}, but the audio is {}.{left}",
                        tags.album.trim(),
                        heard.identity.kind.described()
                    ),
                )
            }
            ("take in title", Some(heard)) => (
                AuditRule::Title,
                format!(
                    "The audio is {}, and the title does not say so.",
                    heard.identity.kind.described()
                ),
            ),
            _ => continue,
        };
        found.findings.push(AuditFinding { rule, words });
    }
    if song.title != tags.title {
        found
            .changes
            .insert(fields::TITLE.to_string(), Some(song.title.clone()));
    }
    if song.album != tags.album {
        found
            .changes
            .insert(fields::ALBUM.to_string(), Some(song.album.clone()));
    }
    // An album renamed is no longer the release its ids name: left on, the library would keep
    // the song on that release's album, beside the one it is renamed to.
    let mut release_cleared = false;
    for (field, before, after) in [
        (
            fields::RELEASE_TRACK_ID,
            &tags.release_track_id,
            &song.music_brainz_release_track_id,
        ),
        (
            fields::RELEASE_ID,
            &tags.release_id,
            &song.music_brainz_release_id,
        ),
        (
            fields::RELEASE_GROUP_ID,
            &tags.release_group_id,
            &song.music_brainz_release_group_id,
        ),
    ] {
        if before.as_deref().is_some_and(|id| !is_blank(id)) && after.as_deref().is_none_or(is_blank) {
            found.changes.insert(field.to_string(), None);
            release_cleared = true;
        }
    }
    if let Some(heard) = heard {
        ids(tags, heard, release_cleared, &mut found);
    }
    if let Some(other) = &tags.other_album_artists {
        other_album_artists(other, &mut found);
    }
    other_names(&tags.other_names, &mut found);
    found
}

/// The recording id and the release track id beside it belong to the recording the audio is.
/// `release_cleared` says the album rule already took the release ids off.
fn ids(tags: &FileTags, heard: &HeardAs, release_cleared: bool, found: &mut TagAudit) {
    let identity = &heard.identity;
    let written = tags.recording_id.as_deref().filter(|id| !is_blank(id));
    let mut wrong_recording = false;
    if let Some(written) = written
        && let Some(why) = elsewhere(heard, written, heard.written.as_ref())
    {
        found.findings.push(AuditFinding {
            rule: AuditRule::Ids,
            words: ids_words(&format!("The recording id {written} is"), "", why, heard),
        });
        wrong_recording = true;
    }
    let track = tags.release_track_id.as_deref().filter(|id| !is_blank(id));
    let mut wrong_track = false;
    if let (Some(track), Some((owner, named))) = (track, heard.track_owner.as_ref())
        && let Some(why) = elsewhere(heard, owner, Some(named))
    {
        found.findings.push(AuditFinding {
            rule: AuditRule::Ids,
            words: ids_words(
                &format!("The release track id {track} is a track of"),
                &format!(" ({owner})"),
                why,
                heard,
            ),
        });
        wrong_track = true;
    }
    if !heard.sure {
        return;
    }
    // A file with no recording id gets the audio's; one with another recording's id gets it in
    // its place. One whose recording nothing rules out keeps what it has.
    if written.is_none() || wrong_recording {
        found.changes.insert(
            fields::RECORDING_ID.to_string(),
            Some(identity.recording_id.clone()),
        );
    }
    if release_cleared || !(wrong_recording || wrong_track) {
        return;
    }
    // The release and its group stay: they name the album the song is filed on, and taking them
    // off would split it from the rest of that album. Only the track can be put right.
    match &heard.on_release {
        // The file's release has the audio's recording: its track there is the one to name.
        Some(Some(own)) => {
            if track.is_none_or(|track| !dotnet::eq_ignore_case(track, own)) {
                found
                    .changes
                    .insert(fields::RELEASE_TRACK_ID.to_string(), Some(own.clone()));
            }
        }
        // A track known to be another recording's comes off.
        _ if wrong_track => {
            found.changes.insert(fields::RELEASE_TRACK_ID.to_string(), None);
        }
        _ => {}
    }
}

/// An Ids finding in words: `subject` names the tag ("The recording id X is"), `owner` the
/// recording it belongs to when that is not the tag itself.
fn ids_words(subject: &str, owner: &str, why: Elsewhere, heard: &HeardAs) -> String {
    match why {
        Elsewhere::Take(kind) => format!(
            "{subject} {}{owner}, but the audio is {}.",
            kind.described(),
            audio_words(heard)
        ),
        Elsewhere::Length => format!(
            "{subject} another recording{owner}, but the audio's exact length is that of {}.",
            heard.identity.recording_id
        ),
        Elsewhere::Runs(length) => format!(
            "{subject} a recording of {}{owner}, but the audio runs {}.",
            minutes(length),
            heard.file_ms.map_or_else(String::new, minutes)
        ),
    }
}

/// One copy of a song, as the takes check reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeCopy {
    pub id: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub kind: RecordingKind,
}

/// Copies of one song in the library where one is another take than the other, and the title
/// does not say so: a live take beside the studio copy, both titled plainly. Answers each such
/// copy's id with what it is beside.
pub fn takes_beside(copies: &[TakeCopy]) -> Vec<(String, String)> {
    let mut by_song: HashMap<String, Vec<&TakeCopy>> = HashMap::new();
    for copy in copies {
        if is_blank(&copy.title) {
            continue;
        }
        by_song
            .entry(SongIdentity::match_key(&copy.artist, &copy.title))
            .or_default()
            .push(copy);
    }
    let mut found: Vec<(String, String)> = Vec::new();
    for group in by_song.values().filter(|group| group.len() > 1) {
        let kinds: BTreeSet<RecordingKind> = group.iter().map(|copy| copy.kind).collect();
        if kinds.len() < 2 {
            continue;
        }
        for copy in group {
            if copy.kind == RecordingKind::Studio
                || RecordingKind::marked_in(&copy.title).contains(&copy.kind)
            {
                continue;
            }
            let Some(other) = group
                .iter()
                .find(|other| other.kind != copy.kind && other.id != copy.id)
            else {
                continue;
            };
            let place = if is_blank(&other.album) {
                String::new()
            } else {
                format!(" on '{}'", other.album.trim())
            };
            found.push((
                copy.id.clone(),
                format!(
                    "This copy is {}, and the library also has {}{place}. Both are titled '{}'.",
                    copy.kind.described(),
                    other.kind.described(),
                    copy.title.trim()
                ),
            ));
        }
    }
    found.sort();
    found
}

#[cfg(test)]
#[path = "tag_audit_tests.rs"]
mod tests;
