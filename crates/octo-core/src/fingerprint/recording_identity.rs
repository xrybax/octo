//! Which one recording a file is, decided by its audio, and what kind of take that recording
//! is.
//!
//! One fingerprint is often tied to several recordings of the same song: the studio take, a
//! live take, an instrumental, a karaoke backing. Their titles read alike and their lengths sit
//! within seconds, so the title, the artist and a whole-second length cannot tell them apart.
//! Two things can: the file's length to the millisecond against each recording's, and what the
//! music database says each recording is (its title's markers, its disambiguation, the releases
//! it is on).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::acoust_id_client::{AcoustIdRecording, AcoustIdRelease};
use crate::common::song_identity::SongIdentity;
use crate::common::{dotnet, live_version};

/// What kind of take a recording is. Everything that is not one of the others is `Studio`,
/// including a remaster or an edit, which are the same performance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum RecordingKind {
    Studio,
    Live,
    Instrumental,
    Karaoke,
    Cover,
    Acapella,
    Remix,
    Demo,
    /// A copy played faster than the recording: "sped up", nightcore.
    Sped,
    /// A copy played slower than the recording: "slowed", "slowed + reverb".
    Slowed,
}

impl RecordingKind {
    /// The word a title carries for this kind, as Octo writes it: "(Live)", "(Instrumental)".
    /// None for a studio take.
    pub fn title_word(self) -> Option<&'static str> {
        match self {
            RecordingKind::Studio => None,
            RecordingKind::Live => Some("Live"),
            RecordingKind::Instrumental => Some("Instrumental"),
            RecordingKind::Karaoke => Some("Karaoke"),
            RecordingKind::Cover => Some("Cover"),
            RecordingKind::Acapella => Some("Acapella"),
            RecordingKind::Remix => Some("Remix"),
            RecordingKind::Demo => Some("Demo"),
            RecordingKind::Sped => Some("Sped Up"),
            RecordingKind::Slowed => Some("Slowed"),
        }
    }

    /// The kind's name in a sentence: "a live recording", "an instrumental".
    pub fn described(self) -> &'static str {
        match self {
            RecordingKind::Studio => "the studio recording",
            RecordingKind::Live => "a live recording",
            RecordingKind::Instrumental => "an instrumental",
            RecordingKind::Karaoke => "a karaoke backing track",
            RecordingKind::Cover => "a cover",
            RecordingKind::Acapella => "an acapella",
            RecordingKind::Remix => "a remix",
            RecordingKind::Demo => "a demo",
            RecordingKind::Sped => "a sped-up copy",
            RecordingKind::Slowed => "a slowed copy",
        }
    }

    /// The kind a version marker names, as SongIdentity and the folder reading name them.
    pub fn of_marker(marker: &str) -> Option<RecordingKind> {
        Some(match marker {
            "live" | "unplugged" => RecordingKind::Live,
            "instrumental" => RecordingKind::Instrumental,
            "karaoke" => RecordingKind::Karaoke,
            "cover" => RecordingKind::Cover,
            "acapella" => RecordingKind::Acapella,
            "remix" | "dj mix" | "vip" | "rework" | "dub" => RecordingKind::Remix,
            "demo" => RecordingKind::Demo,
            "sped up" | "nightcore" => RecordingKind::Sped,
            "slowed" => RecordingKind::Slowed,
            _ => return None,
        })
    }

    /// The kinds a text's version markers name: "(Instrumental)", "- Karaoke Version", "(Acoustic
    /// Cover)", "(Tribute to Adele)". Words that are only part of a name ("Cover Me", "Karaoke
    /// Queen") are not markers.
    pub fn marked_in(text: &str) -> BTreeSet<RecordingKind> {
        if dotnet::is_blank(text) {
            return BTreeSet::new();
        }
        let mut found: BTreeSet<RecordingKind> = SongIdentity::parse_title(text, None)
            .versions
            .iter()
            .filter_map(|marker| RecordingKind::of_marker(marker))
            .collect();
        // "cover" inside a longer bracket ("(Live at the Cover Club)") names a place, not a take:
        // a cover's bracket or dash tail ends with the word.
        if !COVER_MARK.is_match(text) {
            found.remove(&RecordingKind::Cover);
        }
        if MARKED_TRIBUTE.is_match(text) {
            found.insert(RecordingKind::Cover);
        }
        found
    }

    /// The kinds an album's or a release's name gives: its markers, a bracket or dash tail that
    /// is the take itself ("(Instrumentals)", "[Karaoke Version]", "- Acoustic Covers"), a whole
    /// name that is ("Instrumentals"), and "karaoke", "covers" or "cover versions" anywhere. A
    /// bracket that only mentions instrumentals ("[Instrumentals Bonus]") or a name that only
    /// contains a word ("Dark Lane Demo Tapes", "A Tribute to Jack Johnson") gives none.
    pub fn named_in(text: &str) -> BTreeSet<RecordingKind> {
        if dotnet::is_blank(text) {
            return BTreeSet::new();
        }
        let mut found = RecordingKind::marked_in(text);
        let whole = text.trim();
        if let Some(kind) = WHOLE_TAKE.captures(whole).and_then(|c| take_word(&c[1])) {
            found.insert(kind);
        }
        for part in take_parts(text) {
            if let Some(kind) = PART_TAKE.captures(part.trim()).and_then(|c| take_word(&c[1])) {
                found.insert(kind);
            }
        }
        // Words no album of songs is called by unless it holds those takes: "Karaoke Hits, Vol.
        // 3", "Under the Covers", "Cover Versions of Adele".
        if ALBUM_KARAOKE.is_match(text) {
            found.insert(RecordingKind::Karaoke);
        }
        if ALBUM_COVERS.is_match(text) {
            found.insert(RecordingKind::Cover);
        }
        found
    }

    /// The kinds a request asks for, read from its title's markers alone: "Song (Live)" asks for
    /// a live take, "Karaoke Queen" asks for nothing. An album is never a wish: a library copy's
    /// album may itself be the wrong tag that is being replaced.
    pub fn requested_by(title: &str) -> BTreeSet<RecordingKind> {
        RecordingKind::marked_in(title)
    }
}

/// A "tribute to" opening a bracket or a dash tail, which names a cover: "Hello (Tribute to
/// Adele)", "Hello - A Tribute to Adele".
static MARKED_TRIBUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:[\(\[\{]|\s[-\x{2013}]\s)\s*(?:a\s+)?tribute\s+to\b")
        .expect("a fixed pattern compiles")
});

/// A bracket or dash tail that ends with "cover" or "cover version": "(Cover)", "(Acoustic
/// Cover)", "- Cover Version".
static COVER_MARK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[\(\[\{][^\)\]\}]*\bcover(?:\s+version)?\s*[\)\]\}]|\s[-\x{2013}]\s[^\(\[\{]*\bcover(?:\s+version)?\s*$",
    )
    .expect("a fixed pattern compiles")
});

/// "Karaoke" anywhere in an album's name.
static ALBUM_KARAOKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bkaraoke\b").expect("a fixed pattern compiles"));

/// "Covers" or "cover versions" anywhere in an album's name. A lone "cover" is not one ("Cover
/// Me", "Under Cover of Darkness"), and neither is "tribute to" outside a bracket ("A Tribute to
/// Jack Johnson" is Miles Davis's own album).
static ALBUM_COVERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bcovers\b|\bcover\s+versions\b").expect("a fixed pattern compiles"));

/// The words that name a take in an album's name.
const TAKE_WORDS: &str = r"(instrumentals?|karaoke|a\s*cappellas?|acapellas?|covers?|cover\s+versions?)";

/// An album named only for its take: "Instrumentals", "Karaoke", "Cover Versions".
static WHOLE_TAKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^(?:the\s+)?{TAKE_WORDS}(?:\s+(?:versions?|editions?))?$"
    ))
    .expect("a fixed pattern compiles")
});

/// A bracket or dash tail that is the take, with up to three words before it: "(Instrumentals)",
/// "(The Instrumentals)", "[Acoustic Covers]", "- Karaoke Version".
static PART_TAKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^(?:(?:the|a|an)\s+)?(?:[\p{{L}}\p{{N}}'&]+\s+){{0,3}}?{TAKE_WORDS}(?:\s+(?:versions?|editions?|mix(?:es)?|only))?$"
    ))
    .expect("a fixed pattern compiles")
});

/// The kind a take word names.
fn take_word(word: &str) -> Option<RecordingKind> {
    let word = word.to_ascii_lowercase();
    if word.starts_with("instrumental") {
        Some(RecordingKind::Instrumental)
    } else if word.starts_with("karaoke") {
        Some(RecordingKind::Karaoke)
    } else if word.starts_with("cover") {
        Some(RecordingKind::Cover)
    } else if word.contains("cappella") || word.starts_with("acapella") {
        Some(RecordingKind::Acapella)
    } else {
        None
    }
}

/// The bracketed parts of a name and its tail after the last " - ".
fn take_parts(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find(['(', '[', '{']) {
        let close_char = match &rest[open..open + 1] {
            "(" => ')',
            "[" => ']',
            _ => '}',
        };
        let Some(close) = rest[open + 1..].find(close_char).map(|at| open + 1 + at) else {
            break;
        };
        parts.push(&rest[open + 1..close]);
        rest = &rest[close + 1..];
    }
    for dash in [" - ", " \u{2013} "] {
        if let Some(at) = text.rfind(dash) {
            parts.push(&text[at + dash.len()..]);
        }
    }
    parts
}

/// What the music database adds about one recording beyond what AcoustID sends: its
/// disambiguation, and every release it is on with that release's status and its group's
/// secondary types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordingFacts {
    pub recording_id: String,
    pub title: String,
    pub disambiguation: Option<String>,
    pub length_ms: Option<i64>,
    pub releases: Vec<FactRelease>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FactRelease {
    pub title: String,
    pub status: Option<String>,
    pub group_title: Option<String>,
    pub group_secondary_types: Vec<String>,
}

/// The kind of take a recording is: its title's markers first, then the database's
/// disambiguation, then the releases it is on (every one of them live, or every one of them
/// named as instrumentals, karaoke, covers, remixes or demos).
pub fn kind_of(recording: &AcoustIdRecording, facts: Option<&RecordingFacts>) -> RecordingKind {
    if let Some(kind) = first_kind(&RecordingKind::marked_in(&recording.title)) {
        return kind;
    }
    if let Some(facts) = facts {
        if let Some(kind) = facts.disambiguation.as_deref().and_then(note_kind) {
            return kind;
        }
        if let Some(kind) = kind_from_fact_releases(&facts.releases) {
            return kind;
        }
    }
    kind_from_releases(&recording.releases).unwrap_or(RecordingKind::Studio)
}

/// The kind a database note names: "instrumental", "karaoke", "cover", or a live take's
/// "live, 2007-04-08: Palacio de los Deportes".
fn note_kind(note: &str) -> Option<RecordingKind> {
    let mut kinds = RecordingKind::named_in(note);
    if let Some(kind) = PART_TAKE.captures(note.trim()).and_then(|c| take_word(&c[1])) {
        kinds.insert(kind);
    }
    if live_version::mentions(Some(note)) {
        kinds.insert(RecordingKind::Live);
    }
    if NOTE_COVER.is_match(note) {
        kinds.insert(RecordingKind::Cover);
    }
    first_kind(&kinds)
}

/// A database note that calls the recording a cover: "cover", "Adele cover".
static NOTE_COVER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bcover(?:\s+version)?\b").expect("a fixed pattern compiles"));

/// The most telling kind of several: one that changes what is heard most comes first.
fn first_kind(kinds: &BTreeSet<RecordingKind>) -> Option<RecordingKind> {
    [
        RecordingKind::Karaoke,
        RecordingKind::Cover,
        RecordingKind::Instrumental,
        RecordingKind::Acapella,
        RecordingKind::Sped,
        RecordingKind::Slowed,
        RecordingKind::Live,
        RecordingKind::Demo,
        RecordingKind::Remix,
    ]
    .into_iter()
    .find(|kind| kinds.contains(kind))
}

/// A kind every release names, by its own title or by being typed so.
fn kind_from_releases(releases: &[AcoustIdRelease]) -> Option<RecordingKind> {
    if releases.is_empty() {
        return None;
    }
    let per_release: Vec<BTreeSet<RecordingKind>> = releases
        .iter()
        .map(|release| {
            let mut kinds = release_secondary_kinds(&release.secondary_types);
            if let Some(title) = release.title.as_deref() {
                kinds.extend(release_title_kinds(title));
            }
            kinds
        })
        .collect();
    shared_kind(&per_release)
}

fn kind_from_fact_releases(releases: &[FactRelease]) -> Option<RecordingKind> {
    if releases.is_empty() {
        return None;
    }
    let per_release: Vec<BTreeSet<RecordingKind>> = releases
        .iter()
        .map(|release| {
            let mut kinds = release_secondary_kinds(&release.group_secondary_types);
            kinds.extend(release_title_kinds(&release.title));
            kinds
        })
        .collect();
    shared_kind(&per_release)
}

/// What a release's type says its recordings are. Only Live: a remix release or a demo
/// collection often carries the original mix too, so only a recording's own title or note
/// makes it a remix or a demo.
fn release_secondary_kinds(types: &[String]) -> BTreeSet<RecordingKind> {
    types
        .iter()
        .filter(|kind| kind.eq_ignore_ascii_case("live"))
        .map(|_| RecordingKind::Live)
        .collect()
}

/// What a release's own name says its recordings are: only takes with no singing or no band
/// ("(instrumentals)", "Karaoke Hits", "Acapellas"). A release named for its remixes holds the
/// original mix as often as not.
fn release_title_kinds(title: &str) -> BTreeSet<RecordingKind> {
    RecordingKind::named_in(title)
        .into_iter()
        .filter(|kind| {
            matches!(
                kind,
                RecordingKind::Instrumental | RecordingKind::Karaoke | RecordingKind::Acapella
            )
        })
        .collect()
}

/// The kind every release carries, the most telling first.
fn shared_kind(per_release: &[BTreeSet<RecordingKind>]) -> Option<RecordingKind> {
    let first = per_release.first()?;
    let shared: BTreeSet<RecordingKind> = first
        .iter()
        .copied()
        .filter(|kind| per_release.iter().all(|kinds| kinds.contains(kind)))
        .collect();
    first_kind(&shared)
}

/// How far a recording's listed length may be from the file's and still be the length that
/// decides between takes.
pub const DECISIVE_WITHIN_MS: i64 = 1_000;

/// How much further off every other take must be for that length to decide: at least this
/// much, and at least [`DECISIVE_RATIO`] times the best one's distance. Masterings of one
/// recording drift by a second or two, so less would let a remaster pick another take.
pub const DECISIVE_MARGIN_MS: i64 = 2_500;
pub const DECISIVE_RATIO: i64 = 4;

/// The one recording among several that agree on title and artist whose length alone says it
/// is the file: within [`DECISIVE_WITHIN_MS`] of the file, and every other agreeing recording
/// at least [`DECISIVE_MARGIN_MS`] further off and [`DECISIVE_RATIO`] times as far. None when
/// the file's length or any agreeing recording's length is unknown (that one could be the
/// file), or the lengths do not decide. A recording listed twice (by id) counts once.
pub fn decided_by_length<'a>(
    agreeing: &[&'a AcoustIdRecording],
    file_ms: Option<i64>,
) -> Option<&'a AcoustIdRecording> {
    let file_ms = file_ms.filter(|ms| *ms > 0)?;
    let mut distances: Vec<(i64, &'a AcoustIdRecording)> = Vec::new();
    for recording in agreeing {
        let length = recording.duration_ms.filter(|ms| *ms > 0)?;
        if distances
            .iter()
            .any(|(_, seen)| dotnet::eq_ignore_case(&seen.recording_id, &recording.recording_id))
        {
            continue;
        }
        distances.push(((length - file_ms).abs(), recording));
    }
    distances.sort_by_key(|(distance, _)| *distance);
    let (best, chosen) = *distances.first()?;
    if best > DECISIVE_WITHIN_MS {
        return None;
    }
    // A lone recording with a length is not a choice between takes.
    if distances.len() < 2 {
        return None;
    }
    let decisive = distances[1..]
        .iter()
        .all(|(distance, _)| *distance >= best + DECISIVE_MARGIN_MS && *distance >= best * DECISIVE_RATIO);
    decisive.then_some(chosen)
}

/// The recording [`choose`] took, and how sure it is of the take.
#[derive(Debug, Clone, Copy)]
pub struct Chosen<'a> {
    pub recording: &'a AcoustIdRecording,
    pub kind: RecordingKind,
    /// The file's exact length decided it among takes that share the fingerprint.
    pub by_length: bool,
    /// The take is known: the length decided it, or every agreeing recording is that take.
    pub kind_certain: bool,
}

/// The recording a file is, among the ones that agree with the request on title and artist:
/// the one its exact length decides; else the first of a kind the title asks for ("Song
/// (Live)"); else the first studio take; else the first that `refuses` lets through; else the
/// first. A kind the album or a word in the title merely allows ("Live Forever") never wins over
/// the studio take; it only stops a refusal. `kind` says what each recording is.
pub fn choose<'a>(
    agreeing: &[&'a AcoustIdRecording],
    file_ms: Option<i64>,
    asked: &BTreeSet<RecordingKind>,
    refuses: impl Fn(RecordingKind) -> bool,
    kind: impl Fn(&AcoustIdRecording) -> RecordingKind,
) -> Option<Chosen<'a>> {
    let kinds: Vec<RecordingKind> = agreeing.iter().map(|recording| kind(recording)).collect();
    let pick = |index: usize, by_length: bool| {
        let chosen_kind = kinds[index];
        Chosen {
            recording: agreeing[index],
            kind: chosen_kind,
            by_length,
            kind_certain: by_length || kinds.iter().all(|other| *other == chosen_kind),
        }
    };
    if let Some(decided) = decided_by_length(agreeing, file_ms) {
        let index = agreeing.iter().position(|r| std::ptr::eq(*r, decided))?;
        return Some(pick(index, true));
    }
    let index = kinds
        .iter()
        .position(|found| asked.contains(found))
        .or_else(|| kinds.iter().position(|found| *found == RecordingKind::Studio))
        .or_else(|| kinds.iter().position(|found| !refuses(*found)))
        .or((!agreeing.is_empty()).then_some(0))?;
    Some(pick(index, false))
}

/// Whether a recording's kind was never asked for.
pub fn unwanted(kind: RecordingKind, wanted: &BTreeSet<RecordingKind>) -> bool {
    kind != RecordingKind::Studio && !wanted.contains(&kind)
}

/// Which takes of a song a file may be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Takes {
    /// Any take: a song already in the library, which may be live or an instrumental on
    /// purpose.
    #[default]
    Any,
    /// The studio take and these kinds, nothing else.
    Only(BTreeSet<RecordingKind>),
}

impl Takes {
    /// The takes a download asked for by its title and its album: a live album lets a live
    /// take through ("Decade (live at the El Mocambo)"), and so does a title such as "Live
    /// Forever", generous on purpose. Kinds in `never` are refused even when asked for.
    pub fn for_request(title: Option<&str>, album: Option<&str>, never: &BTreeSet<RecordingKind>) -> Takes {
        let mut wanted: BTreeSet<RecordingKind> = title.map(RecordingKind::marked_in).unwrap_or_default();
        wanted.extend(album.map(RecordingKind::named_in).unwrap_or_default());
        if live_version::requested(title, album) {
            wanted.insert(RecordingKind::Live);
        }
        wanted.retain(|kind| !never.contains(kind));
        Takes::Only(wanted)
    }

    /// The kinds this allows besides the studio take; empty for [`Takes::Any`], whose choice
    /// still prefers the studio take.
    pub fn wanted(&self) -> BTreeSet<RecordingKind> {
        match self {
            Takes::Any => BTreeSet::new(),
            Takes::Only(kinds) => kinds.clone(),
        }
    }

    /// Whether a recording of this kind is refused.
    pub fn refuses(&self, kind: RecordingKind) -> bool {
        match self {
            Takes::Any => false,
            Takes::Only(kinds) => unwanted(kind, kinds),
        }
    }
}

/// The one recording a file's audio was found to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub recording_id: String,
    pub kind: RecordingKind,
    /// The file's exact length chose it among takes that share the fingerprint, so no other
    /// recording of the fingerprint may name the file's album.
    pub by_length: bool,
    /// The take is known: the length decided it, or every recording that agreed with the song
    /// is that take. Only then may a tag be changed to say so.
    pub kind_certain: bool,
}

impl Identity {
    /// The identity a confirmed verification decided, None otherwise.
    pub fn of(verification: Option<&super::verification::VerificationResult>) -> Option<Identity> {
        let verification = verification?;
        if verification.verdict != super::verification::VerificationVerdict::Confirmed {
            return None;
        }
        let recording_id = verification
            .recording_id
            .clone()
            .filter(|id| !dotnet::is_blank(id))?;
        Some(Identity {
            recording_id,
            kind: verification.kind.unwrap_or(RecordingKind::Studio),
            by_length: verification.by_length,
            kind_certain: verification.kind_certain,
        })
    }

    /// Whether another recording of the same fingerprint may still name the file's album: never
    /// when the exact length chose this one, and every recording of the same kind otherwise (an
    /// album and a compilation often list the same audio under two recordings).
    pub fn admits(&self, recording_id: &str, kind: RecordingKind) -> bool {
        dotnet::eq_ignore_case(recording_id, &self.recording_id) || (!self.by_length && kind == self.kind)
    }

    /// Whether an album or release title names a take this recording is not: "The Black Parade
    /// (instrumentals)" for the studio take. `asked` are the kinds the request's title named.
    /// None unless the take is certain: a guess never overrules what a tag says.
    pub fn contradicted_by(&self, album: &str, asked: &BTreeSet<RecordingKind>) -> Option<RecordingKind> {
        if !self.kind_certain {
            return None;
        }
        RecordingKind::named_in(album)
            .into_iter()
            .find(|named| *named != self.kind && !asked.contains(named))
    }
}

#[cfg(test)]
#[path = "recording_identity_tests.rs"]
mod tests;
