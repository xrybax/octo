//! Whether a library file is the version of its song that its tags say: not a remix, another
//! remix, a sped-up or slowed copy, a live take or any other take than the one its title names.
//!
//! The audio decides when it can: the fingerprint's recordings, and the file's own recording id
//! when its length is the file's to the millisecond. When the fingerprint names nothing of the
//! song, the lengths of the song's recordings in the music database decide among its versions,
//! and the album and the fingerprint's own silence about the tagged version back that up. What
//! is found is only ever offered: nothing here changes a file.

use crate::common::SongIdentity;
use crate::common::version_gap::{self, Against};
use crate::common::version_variant;
use crate::fingerprint::recording_identity::RecordingKind;
use crate::fingerprint::speed_variant;

/// One recording of the song as the music database lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownRecording {
    pub id: String,
    pub title: String,
    pub length_ms: Option<i64>,
}

impl KnownRecording {
    pub fn new(id: &str, title: &str, length_ms: Option<i64>) -> Self {
        KnownRecording {
            id: id.to_string(),
            title: title.to_string(),
            length_ms,
        }
    }
}

/// What the catalog says the file's ISRC is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IsrcEntry {
    pub isrc: String,
    pub title: String,
    pub seconds: Option<i32>,
}

/// Everything the check gathered about one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionFacts {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The file's own length, to the millisecond.
    pub file_ms: Option<i64>,
    /// The recordings of the song the fingerprint named (any version, by this artist), from its
    /// best answer at or above the threshold.
    pub heard: Vec<KnownRecording>,
    /// The fingerprint was known to AcoustID at or above the threshold, whatever it names.
    pub heard_anything: bool,
    /// AcoustID's own ids for the file's sound.
    pub heard_tracks: Vec<String>,
    /// The recording the file's own recording id names, as the music database has it.
    pub tagged_recording: Option<KnownRecording>,
    /// What the catalog says the file's ISRC is.
    pub isrc: Option<IsrcEntry>,
    /// The song's recordings in the music database, every version of it by this artist.
    pub catalog: Vec<KnownRecording>,
    /// AcoustID's ids for the sound of the recordings of the tagged version, when they were
    /// asked for; empty when none has any.
    pub tagged_tracks: Option<Vec<String>>,
    /// The file resampled back to the speed of a recording of the song, and what the fingerprint
    /// of that copy named.
    pub resampled: Option<Resampled>,
}

/// The file played back at another speed, and the recording it then fingerprints as.
#[derive(Debug, Clone, PartialEq)]
pub struct Resampled {
    /// The file's length over the recording's: under 1 for a sped-up copy.
    pub ratio: f64,
    pub recording: KnownRecording,
}

impl Eq for Resampled {}

/// How sure the check is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sureness {
    /// The audio itself says so: its fingerprint, its resampled fingerprint, or its own recording
    /// id at its exact length.
    Heard,
    /// The lengths and names say so, and nothing heard says otherwise.
    Likely,
}

/// What kind of problem it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VersionProblem {
    /// Another version of the song than its tags name.
    OtherVersion,
    /// The tagged version played faster or slower.
    Speed,
    /// Not the length of the tagged version, and nothing says which version it is.
    LengthOff,
}

impl VersionProblem {
    pub fn name(self) -> &'static str {
        match self {
            VersionProblem::OtherVersion => "otherVersion",
            VersionProblem::Speed => "speed",
            VersionProblem::LengthOff => "lengthOff",
        }
    }

    pub fn parse(name: &str) -> Option<VersionProblem> {
        [
            VersionProblem::OtherVersion,
            VersionProblem::Speed,
            VersionProblem::LengthOff,
        ]
        .into_iter()
        .find(|problem| problem.name() == name)
    }
}

/// What the check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionFinding {
    pub problem: VersionProblem,
    pub sureness: Sureness,
    /// What the file really is, when one known recording: what Retag writes.
    pub is: Option<KnownRecording>,
    /// The take a sped-up or slowed copy is, for its title word.
    pub kind: Option<RecordingKind>,
    /// Why, in plain words with the numbers.
    pub words: String,
    /// The tagged version's length, when known: what a replacement is held to.
    pub expected_seconds: Option<i32>,
    /// The file's ISRC belongs to the tagged version, not to the audio: Retag takes it off.
    pub isrc_belongs_elsewhere: bool,
}

/// A recording's length within this of the file's is the file's own: digital releases list their
/// length to the sample, so two takes rarely sit this close.
pub const EXACT_MS: i64 = 50;

/// Within this, a length fits a recording as masterings of one take do.
pub const FITS_MS: i64 = 2_000;

/// A length further off than this share and this many milliseconds is another recording's.
pub const OFF_SHARE: f64 = 0.03;
pub const OFF_MS: i64 = 4_000;

/// "4:02.20".
pub fn length_words(ms: i64) -> String {
    let tenths = (ms + 5) / 10;
    format!("{}:{:02}.{:02}", tenths / 6000, (tenths / 100) % 60, tenths % 100)
}

fn quoted(recording: &KnownRecording) -> String {
    match recording.length_ms {
        Some(ms) => format!("'{}' ({})", recording.title, length_words(ms)),
        None => format!("'{}'", recording.title),
    }
}

/// Whether a recording's title is the tagged version of the song.
fn tagged_version(title: &str, recording: &KnownRecording) -> bool {
    same_song(title, &recording.title)
        && !version_gap::other_version(title, &recording.title, Against::Recording)
}

/// Whether a recording's title is another version of the tagged song.
fn other_version(title: &str, recording: &KnownRecording) -> bool {
    same_song(title, &recording.title)
        && version_gap::other_version(title, &recording.title, Against::Recording)
}

fn same_song(a: &str, b: &str) -> bool {
    SongIdentity::same_title(a, b, None).verdict != crate::common::SongVerdict::Different
}

fn far_apart(a: i64, b: i64) -> bool {
    let apart = (a - b).abs();
    apart > OFF_MS && apart as f64 > b.max(1) as f64 * OFF_SHARE
}

/// The versions the album names that the title does not: "remix" for "Let's Get Lost" on "Let's
/// Get Lost Remixes".
pub fn album_versions_beyond_title(title: &str, album: &str, artist: &str) -> Vec<String> {
    if album.trim().is_empty() {
        return Vec::new();
    }
    let in_title = version_variant::requested(Some(title), None);
    version_variant::in_folder(album, Some(artist))
        .into_iter()
        .filter(|version| !in_title.contains(version))
        .collect()
}

/// The check on one file; None when it is the version its tags say, or nothing tells.
pub fn judge(facts: &VersionFacts) -> Option<VersionFinding> {
    let title = facts.title.trim();
    if title.is_empty() {
        return None;
    }
    let file_ms = facts.file_ms.filter(|ms| *ms > 0);
    let tagged: Vec<&KnownRecording> = facts
        .catalog
        .iter()
        .filter(|r| tagged_version(title, r))
        .collect();
    let expected_ms = expected_length(facts, &tagged);
    let expected_seconds = expected_ms.map(|ms| ((ms + 500) / 1000) as i32);
    let isrc_elsewhere = |is: &KnownRecording| {
        facts.isrc.as_ref().is_some_and(|entry| {
            same_song(title, &entry.title)
                && version_gap::version_gap(&is.title, &entry.title, Against::Title).is_some()
        })
    };
    let album_says = album_versions_beyond_title(title, &facts.album, &facts.artist);
    let album_words = if album_says.is_empty() {
        String::new()
    } else {
        format!(
            " The album '{}' says {}.",
            facts.album.trim(),
            album_says.join(" + ")
        )
    };

    // 1. Played at another speed: the copy resampled back fingerprints as a recording of the
    // song.
    if let Some(resampled) = &facts.resampled {
        let kind = speed_variant::kind_for(resampled.ratio);
        return Some(VersionFinding {
            problem: VersionProblem::Speed,
            sureness: Sureness::Heard,
            is: None,
            kind: Some(kind),
            words: format!(
                "The audio is {} of {}: played back at {}, it fingerprints as that recording.",
                kind.described(),
                quoted(&resampled.recording),
                speed_variant::speed_words(1.0 / resampled.ratio).replace("times as fast", "times the speed")
            ),
            expected_seconds,
            isrc_belongs_elsewhere: false,
        });
    }

    // 2. The fingerprint names another version of the song, and none of the tagged one.
    if !facts.heard.iter().any(|r| tagged_version(title, r))
        && let Some(is) = facts.heard.iter().find(|r| other_version(title, r))
    {
        return Some(VersionFinding {
            problem: VersionProblem::OtherVersion,
            sureness: Sureness::Heard,
            is: Some(is.clone()),
            kind: None,
            words: format!(
                "Tagged as '{title}', but the fingerprint is {}.{album_words}",
                quoted(is)
            ),
            expected_seconds,
            isrc_belongs_elsewhere: isrc_elsewhere(is),
        });
    }

    // 3. The file's own recording id names another version, at the file's exact length.
    if let (Some(is), Some(file_ms)) = (&facts.tagged_recording, file_ms)
        && other_version(title, is)
        && is.length_ms.is_some_and(|ms| (ms - file_ms).abs() <= FITS_MS)
        && !facts.heard.iter().any(|r| tagged_version(title, r))
    {
        let exact = is.length_ms.is_some_and(|ms| (ms - file_ms).abs() <= EXACT_MS);
        let mut words = format!(
            "Tagged as '{title}', but its own recording id names {}, and the file runs {}{}.",
            quoted(is),
            length_words(file_ms),
            if exact {
                ", that recording's length to the millisecond"
            } else {
                ""
            }
        );
        if let Some(entry) = &facts.isrc
            && let Some(seconds) = entry.seconds
        {
            words.push_str(&format!(
                " Its ISRC {} is '{}', which runs {}.",
                entry.isrc,
                entry.title,
                length_words(i64::from(seconds) * 1000)
            ));
        }
        return Some(VersionFinding {
            problem: VersionProblem::OtherVersion,
            sureness: if exact { Sureness::Heard } else { Sureness::Likely },
            is: Some(is.clone()),
            kind: None,
            words,
            expected_seconds,
            isrc_belongs_elsewhere: isrc_elsewhere(is),
        });
    }

    let file_ms = file_ms?;
    // Another version of the song whose length is the file's to the millisecond, while every
    // recording of the tagged version is further off.
    let exact_other = facts
        .catalog
        .iter()
        .filter(|r| other_version(title, r))
        .filter(|r| r.length_ms.is_some_and(|ms| (ms - file_ms).abs() <= EXACT_MS))
        .min_by_key(|r| r.length_ms.map(|ms| (ms - file_ms).abs()));
    let tagged_exact = tagged
        .iter()
        .any(|r| r.length_ms.is_some_and(|ms| (ms - file_ms).abs() <= EXACT_MS * 4));
    let not_heard_as_tagged = matches!(&facts.tagged_tracks, Some(tracks) if !tracks.is_empty())
        && facts.heard_anything
        && !facts.heard_tracks.is_empty()
        && facts.tagged_tracks.as_ref().is_some_and(|tracks| {
            !tracks
                .iter()
                .any(|t| facts.heard_tracks.iter().any(|h| h.eq_ignore_ascii_case(t)))
        })
        && !facts.heard.iter().any(|r| tagged_version(title, r));

    // 4. AcoustID hears none of the tagged version in the file, or the album names another
    // version, and another version runs the file's exact length.
    if let Some(is) = exact_other
        && !tagged_exact
        && (not_heard_as_tagged || !album_says.is_empty())
    {
        let mut words = format!(
            "Tagged as '{title}', but the file runs {}, the length of {} to the millisecond.",
            length_words(file_ms),
            quoted(is)
        );
        if not_heard_as_tagged {
            words.push_str(" AcoustID does not hear the tagged version in this file.");
        }
        words.push_str(&album_words);
        if let Some(ms) = expected_ms {
            words.push_str(&format!(" The tagged version runs {}.", length_words(ms)));
        }
        return Some(VersionFinding {
            problem: VersionProblem::OtherVersion,
            sureness: if not_heard_as_tagged {
                Sureness::Heard
            } else {
                Sureness::Likely
            },
            is: Some(is.clone()),
            kind: None,
            words,
            expected_seconds,
            isrc_belongs_elsewhere: isrc_elsewhere(is),
        });
    }

    // 5. Not the tagged version's length, whatever else it is.
    let expected = expected_ms?;
    if !far_apart(file_ms, expected) {
        return None;
    }
    let nearest_other = facts
        .catalog
        .iter()
        .filter(|r| other_version(title, r))
        .filter(|r| r.length_ms.is_some_and(|ms| (ms - file_ms).abs() <= FITS_MS))
        .min_by_key(|r| r.length_ms.map(|ms| (ms - file_ms).abs()));
    let source = if facts
        .isrc
        .as_ref()
        .is_some_and(|entry| entry.seconds.is_some_and(|s| i64::from(s) * 1000 == expected))
    {
        facts
            .isrc
            .as_ref()
            .map(|entry| format!("its ISRC {} ('{}')", entry.isrc, entry.title))
            .unwrap_or_default()
    } else {
        "the tagged version".to_string()
    };
    let mut words = format!(
        "The file runs {}, but {source} runs {}.",
        length_words(file_ms),
        length_words(expected)
    );
    if let Some(is) = nearest_other {
        words.push_str(&format!(" {} runs the file's length.", quoted(is)));
    }
    if speed_variant::speed_ratio(file_ms, expected).is_some() {
        words.push_str(&format!(
            " That is {} the tagged version's speed, but played back at it the file does not fingerprint as it.",
            speed_variant::speed_words(file_ms as f64 / expected as f64).replace("times as fast", "times")
        ));
    }
    words.push_str(&album_words);
    Some(VersionFinding {
        problem: if nearest_other.is_some() {
            VersionProblem::OtherVersion
        } else {
            VersionProblem::LengthOff
        },
        sureness: Sureness::Likely,
        is: nearest_other.cloned(),
        kind: None,
        words,
        expected_seconds,
        isrc_belongs_elsewhere: nearest_other.is_some_and(isrc_elsewhere),
    })
}

/// The tagged version's length: its ISRC's catalog entry when that names the tagged version, else
/// the recording its id names when that is the tagged version, else the song's tagged-version
/// recordings in the music database when they agree within masterings.
pub fn expected_length(facts: &VersionFacts, tagged: &[&KnownRecording]) -> Option<i64> {
    let title = facts.title.trim();
    if let Some(entry) = &facts.isrc
        && same_song(title, &entry.title)
        && !version_gap::other_version(title, &entry.title, Against::Title)
        && let Some(seconds) = entry.seconds.filter(|s| *s > 0)
    {
        return Some(i64::from(seconds) * 1000);
    }
    if let Some(own) = &facts.tagged_recording
        && tagged_version(title, own)
        && let Some(ms) = own.length_ms.filter(|ms| *ms > 0)
    {
        return Some(ms);
    }
    let lengths: Vec<i64> = tagged
        .iter()
        .filter_map(|r| r.length_ms.filter(|ms| *ms > 0))
        .collect();
    let (min, max) = (lengths.iter().min()?, lengths.iter().max()?);
    (max - min <= FITS_MS * 2).then_some((min + max) / 2)
}

/// The speed ratio worth a resampled fingerprint: the file's length over the tagged version's,
/// when that is a speed change ([`speed_variant::speed_ratio`]).
pub fn speed_ratio_to_try(facts: &VersionFacts) -> Option<f64> {
    let file_ms = facts.file_ms.filter(|ms| *ms > 0)?;
    let title = facts.title.trim();
    let tagged: Vec<&KnownRecording> = facts
        .catalog
        .iter()
        .filter(|r| tagged_version(title, r))
        .collect();
    speed_variant::speed_ratio(file_ms, expected_length(facts, &tagged)?)
}

/// Whether a recording is the tagged version of the song a title names.
pub fn is_tagged_version(title: &str, recording: &KnownRecording) -> bool {
    tagged_version(title.trim(), recording)
}

#[cfg(test)]
#[path = "version_audit_tests.rs"]
mod tests;
