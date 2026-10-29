//! Whether two titles of one song name the same version of it, read both ways.
//!
//! [`SongIdentity::added_versions`] reads one way only: a match that adds "(Live)" to the request
//! is another take, a request that says more than its match is given the benefit of the doubt.
//! That is right for the markers a music database keeps out of a recording's title (a live
//! take's venue and date sit in its disambiguation), and wrong for the ones it always writes
//! there: a recording titled plainly is not the remix, the sped-up upload or the instrumental a
//! request names. Two remixes credited to different people are two recordings however alike the
//! rest of the title reads.

use std::collections::BTreeSet;

use super::song_identity::{SongIdentity, SongTitle};

/// What the other title is held to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Against {
    /// A music database recording's own title: a marker the request names that the database
    /// writes elsewhere (live, acoustic, demo, a cover) may be missing from it.
    Recording,
    /// Another title written for people (a library file, a catalog entry, an import row): every
    /// marker counts both ways.
    Title,
}

/// Version markers a music database writes into a recording's title, never only into its
/// disambiguation: their absence from a recording's title says the recording is not that take.
const TITLED: [&str; 25] = [
    "remix",
    "mix",
    "dub",
    "vip",
    "rework",
    "bootleg",
    "extended",
    "sped up",
    "slowed",
    "reverb",
    "nightcore",
    "drumless",
    "8d",
    "piano",
    "orchestral",
    "lofi",
    "bass boosted",
    "mashup",
    "medley",
    "megamix",
    "blend",
    "instrumental",
    "acapella",
    "karaoke",
    "reprise",
];

/// Markers that say who made a take, whose credits must agree when both titles give one.
const CREDITED: [&str; 4] = ["remix", "mix", "dub", "edit"];

/// How two titles of one song differ in version.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VersionGap {
    /// Markers the other title carries that the asked one does not.
    pub added: BTreeSet<String>,
    /// Markers the asked title names that the other one lacks.
    pub missing: BTreeSet<String>,
    /// Both name who made the remix (or mix, dub, edit), and no name is shared.
    pub other_remixer: bool,
}

impl VersionGap {
    /// The difference in words: "a remix by someone else", "sped up, where the original was
    /// asked for", "the plain song, where the remix was asked for".
    pub fn words(&self) -> String {
        if self.other_remixer && self.added.is_empty() && self.missing.is_empty() {
            return "a remix by someone else".to_string();
        }
        let list = |set: &BTreeSet<String>| set.iter().map(String::as_str).collect::<Vec<_>>().join(" + ");
        match (self.added.is_empty(), self.missing.is_empty()) {
            (false, true) => format!("{}, where that version was not asked for", list(&self.added)),
            (true, false) => format!("not the {} asked for", list(&self.missing)),
            _ => format!(
                "{} where {} was asked for",
                list(&self.added),
                list(&self.missing)
            ),
        }
    }
}

/// The distinct markers of a title, with "unplugged" read as "live".
fn markers(title: &SongTitle) -> BTreeSet<String> {
    SongIdentity::distinct_versions(title, None)
        .into_iter()
        .map(|marker| {
            if marker == "unplugged" {
                "live".to_string()
            } else {
                marker
            }
        })
        .collect()
}

/// How `got` differs in version from `asked`, or None when it is the same take as far as the
/// two titles tell. Read only for two titles of one song: whether they are one song at all is
/// the caller's to decide first.
pub fn version_gap(asked: &str, got: &str, against: Against) -> Option<VersionGap> {
    let want = SongIdentity::parse_title(asked, None);
    let have = SongIdentity::parse_title(got, None);
    let (want_markers, have_markers) = (markers(&want), markers(&have));
    let added: BTreeSet<String> = have_markers.difference(&want_markers).cloned().collect();
    let missing: BTreeSet<String> = want_markers
        .difference(&have_markers)
        .filter(|marker| against == Against::Title || TITLED.contains(&marker.as_str()))
        .cloned()
        .collect();
    let credited = want_markers
        .iter()
        .chain(have_markers.iter())
        .any(|marker| CREDITED.contains(&marker.as_str()));
    let other_remixer = credited
        && !want.remixers.is_empty()
        && !have.remixers.is_empty()
        && !want
            .remixers
            .iter()
            .any(|remixer| have.remixers.contains(remixer));
    (!added.is_empty() || !missing.is_empty() || other_remixer).then_some(VersionGap {
        added,
        missing,
        other_remixer,
    })
}

/// Whether `got` is another version of the song `asked` names, as [`version_gap`] reads it.
pub fn other_version(asked: &str, got: &str, against: Against) -> bool {
    version_gap(asked, got, against).is_some()
}

#[cfg(test)]
#[path = "version_gap_tests.rs"]
mod tests;
