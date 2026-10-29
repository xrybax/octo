//! Every name the library server reads a song's credits under, after Navidrome 0.64's
//! resources/mappings.yaml. It reads the album artist from TPE2, ALBUMARTIST, "ALBUM ARTIST",
//! ALBUM_ARTIST, aART and WM/AlbumArtist alike, and the album artists from ALBUMARTISTS and
//! "ALBUM ARTISTS" in its place; the artist and the artists the same way, and the sort names of
//! each. A value under a second name is a second album artist to it, so one track of an album
//! that carries it beside another that does not is two albums.
//!
//! Names are lowercase and matched against ID3 TXXX descriptions, Vorbis field names and MP4
//! freeform names alike. Each container's own frame or atom (TPE2, aART) is not among them: it
//! is where Octo writes the value.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use octo_core::common::dotnet::{eq_ignore_case, is_null_or_white_space};

use super::tag_file::{Format, TagFile};
use super::tag_writer_extras::APPLE_MEAN;

/// The album artist's names (`albumartist`).
pub const ALBUM_ARTIST: &[&str] = &[
    "albumartist",
    "album artist",
    "album_artist",
    "wm/albumartist",
    "tpe2",
    "aart",
];
/// The album artists as a list (`albumartists`), with the spelling ALBUM_ARTISTS taggers also
/// write.
pub const ALBUM_ARTISTS: &[&str] = &["albumartists", "album artists", "album_artists"];
pub const ALBUM_ARTIST_SORT: &[&str] = &["albumartistsort", "wm/albumartistsortorder", "tso2", "soaa"];
/// The album artists' sort names, with the spelling ALBUMARTISTS_SORT taggers also write.
pub const ALBUM_ARTISTS_SORT: &[&str] = &["albumartistssort", "albumartists_sort"];
/// The album artist as the release credits it, which taggers keep beside the album artist.
pub const ALBUM_ARTIST_CREDIT: &[&str] = &[
    "albumartist_credit",
    "albumartistcredit",
    "albumartists_credit",
    "albumartistscredit",
];
/// The album artist's MusicBrainz ids (`musicbrainz_albumartistid`).
pub const ALBUM_ARTIST_ID: &[&str] = &[
    "musicbrainz album artist id",
    "musicbrainz_albumartistid",
    "musicbrainz/album artist id",
];
/// The artist's names (`artist`).
pub const ARTIST: &[&str] = &["artist", "author", "iart", "tpe1", "\u{a9}art"];
/// The artists as a list (`artists`).
pub const ARTISTS: &[&str] = &["artists", "wm/artists"];
pub const ARTIST_SORT: &[&str] = &["artistsort", "wm/artistsortorder", "tsop", "soar"];
pub const ARTISTS_SORT: &[&str] = &["artistssort", "artists_sort"];
pub const ARTIST_CREDIT: &[&str] = &["artist_credit", "artistcredit", "artists_credit", "artistscredit"];
/// The artist's MusicBrainz ids (`musicbrainz_artistid`).
pub const ARTIST_ID: &[&str] = &[
    "musicbrainz artist id",
    "musicbrainz_artistid",
    "musicbrainz/artist id",
];

/// MusicBrainz's id for Various Artists.
pub const VARIOUS_ARTISTS_ID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";

/// Whether a name is the one a compilation goes under.
pub fn is_various_artists(name: &str) -> bool {
    let name = name.trim();
    ["Various Artists", "Various", "VA"]
        .iter()
        .any(|various| eq_ignore_case(name, various))
}

/// One credit and its names: the one value, the list, and the sort names of each, with the frame,
/// field and atom each container holds the value and its sort name in.
struct Role {
    names: &'static [&'static str],
    lists: &'static [&'static str],
    sorts: &'static [&'static str],
    list_sorts: &'static [&'static str],
    /// The credit as credited, and its MusicBrainz ids: they describe the credit, so they go
    /// with the sort names when it changes.
    credits: &'static [&'static str],
    ids: &'static [&'static str],
    frame: &'static str,
    vorbis: &'static str,
    atom: [u8; 4],
    sort_frame: &'static str,
    sort_vorbis: &'static str,
    sort_atom: [u8; 4],
    /// The name the list goes under in every container when Octo writes one.
    list_name: &'static str,
    /// Whether a list that is the value split into its names stays beside it.
    keeps_split: bool,
}

const ALBUM_ARTIST_ROLE: Role = Role {
    names: ALBUM_ARTIST,
    lists: ALBUM_ARTISTS,
    sorts: ALBUM_ARTIST_SORT,
    list_sorts: ALBUM_ARTISTS_SORT,
    credits: ALBUM_ARTIST_CREDIT,
    ids: ALBUM_ARTIST_ID,
    frame: "TPE2",
    vorbis: "ALBUMARTIST",
    atom: *b"aART",
    sort_frame: "TSO2",
    sort_vorbis: "ALBUMARTISTSORT",
    sort_atom: *b"soaa",
    list_name: "ALBUMARTISTS",
    keeps_split: false,
};

const ARTIST_ROLE: Role = Role {
    names: ARTIST,
    lists: ARTISTS,
    sorts: ARTIST_SORT,
    list_sorts: ARTISTS_SORT,
    credits: ARTIST_CREDIT,
    ids: ARTIST_ID,
    frame: "TPE1",
    vorbis: "ARTIST",
    atom: *b"\xa9ART",
    sort_frame: "TSOP",
    sort_vorbis: "ARTISTSORT",
    sort_atom: *b"soar",
    list_name: "ARTISTS",
    keeps_split: true,
};

/// Where a field lives: an ID3 TXXX frame, a Vorbis field or an MP4 freeform atom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    Id3,
    Vorbis,
    Mp4,
}

const CONTAINERS: [Container; 3] = [Container::Id3, Container::Vorbis, Container::Mp4];

/// One field a file carries: where, under what name, and its values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedField {
    #[serde(rename = "in")]
    pub container: Container,
    pub name: String,
    pub values: Vec<String>,
}

/// The credits as the library server reads them, taken before Octo writes its own so the sort
/// names can be held against them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Credits {
    artist: Vec<String>,
    album_artist: Vec<String>,
}

impl Credits {
    pub fn read(file: &TagFile) -> Credits {
        Credits {
            artist: heard(file, &ARTIST_ROLE),
            album_artist: heard(file, &ALBUM_ARTIST_ROLE),
        }
    }
}

/// Which credits Octo wrote: the artist, the artists as a list, the album artist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Written {
    pub artist: bool,
    pub artists: bool,
    pub album_artist: bool,
}

/// After Octo wrote a song's credits: every other name of each goes from every container, so
/// the library server reads Octo's value alone. The album artists a file arrived with go too,
/// since the library server reads them in place of the album artist; the artists a file arrived
/// with stay only as the artist split into its names. A sort name stays only while the credit it
/// sorts is unchanged, and then under its one name; the credit as credited and its MusicBrainz
/// ids stay only while it is unchanged too. `before` is [`Credits::read`] of the file before Octo
/// wrote anything.
pub fn tidy_written(file: &mut TagFile, before: &Credits, written: Written) {
    if written.artist {
        tidy_role(file, &ARTIST_ROLE, &before.artist, written.artists);
    }
    if written.album_artist {
        tidy_role(file, &ALBUM_ARTIST_ROLE, &before.album_artist, false);
    }
}

fn tidy_role(file: &mut TagFile, role: &Role, before: &[String], list_written: bool) {
    remove_names(file, role.names, |container, name| {
        container == Container::Vorbis && eq_ignore_case(name, role.vorbis)
    });
    for container in CONTAINERS {
        let mut kept_list = false;
        let credit = own_values(file, container, role);
        for (name, values) in fields_in(file, container) {
            if !wanted(role.lists, &name) {
                continue;
            }
            let keep = if list_written {
                eq_ignore_case(&name, role.list_name)
            } else {
                role.keeps_split
                    && !kept_list
                    && credit.len() == 1
                    && is_split(&clean(&values), &credit[0], true)
            };
            if keep {
                kept_list = true;
            } else {
                remove_field(file, container, &name);
            }
        }
        if !kept_list {
            remove_in(file, container, role.list_sorts, |_, _| false);
        }
    }
    if heard(file, role) != before {
        remove_names(file, role.sorts, |_, _| false);
        remove_names(file, role.list_sorts, |_, _| false);
        remove_names(file, role.credits, |_, _| false);
        remove_names(file, role.ids, |_, _| false);
        if let Some(id3) = file.id3v2.as_mut() {
            id3.remove_frames(role.sort_frame);
        }
        if let Some(apple) = file.apple.as_mut() {
            apple.clear_data(role.sort_atom);
        }
        return;
    }
    for container in CONTAINERS {
        if has_own_sort(file, container, role) {
            remove_in(file, container, role.sorts, |container, name| {
                container == Container::Vorbis && eq_ignore_case(name, role.sort_vorbis)
            });
        }
    }
}

/// The fields beside a file's own album artist that the library server reads as more album
/// artists, in the container it reads: the album artist under another name, and a list of album
/// artists that is not the album artist itself or split into its names. With them, the sort
/// names, credits and MusicBrainz ids of another album artist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StrayAlbumArtists {
    /// The album artist the file names in its own field.
    pub album_artist: Vec<String>,
    pub fields: Vec<NamedField>,
    /// Every value names the album artist again, in other capitals or spacing.
    pub copies: bool,
}

impl StrayAlbumArtists {
    /// The fields the library server reads beside the file's own album artist. None when there
    /// are none, or when the file names no album artist of its own: then they are its album
    /// artist.
    pub fn of(file: &TagFile) -> Option<StrayAlbumArtists> {
        let container = read_container(file)?;
        let role = &ALBUM_ARTIST_ROLE;
        let kept = clean(&own_values(file, container, role));
        if kept.is_empty() {
            return None;
        }
        let mut fields: Vec<NamedField> = Vec::new();
        let mut copies = true;
        let (mut list_kept, mut list_stray) = (false, false);
        for (name, values) in fields_in(file, container) {
            let values = clean(&values);
            if values.is_empty() {
                continue;
            }
            let field = NamedField {
                container,
                name: name.clone(),
                values: values.clone(),
            };
            if wanted(role.names, &name)
                && !(container == Container::Vorbis && eq_ignore_case(&name, role.vorbis))
            {
                copies &= values
                    .iter()
                    .all(|value| kept.iter().any(|own| same_name(own, value)));
                fields.push(field);
            } else if wanted(role.lists, &name) {
                if is_list_of(&values, &kept, true) {
                    list_kept = true;
                } else {
                    copies &= is_list_of(&values, &kept, false);
                    list_stray = true;
                    fields.push(field);
                }
            }
        }
        if list_stray && !list_kept {
            for (name, values) in fields_in(file, container) {
                let values = clean(&values);
                if wanted(role.list_sorts, &name) && !values.is_empty() {
                    fields.push(NamedField {
                        container,
                        name,
                        values,
                    });
                }
            }
        }
        // A sort name, credit or MusicBrainz id of a compilation, or of a name already found
        // beside the album artist, describes another album artist: a peer's copy from a
        // compilation keeps "Various Artists" in all of them.
        if !kept.iter().any(|own| is_various_artists(own)) {
            let others: Vec<String> = fields
                .iter()
                .flat_map(|field| &field.values)
                .filter(|value| !kept.iter().any(|own| same_name(own, value)))
                .cloned()
                .collect();
            let describes_another = |value: &String| {
                is_various_artists(value) || others.iter().any(|other| same_name(other, value))
            };
            for (name, values) in fields_in(file, container) {
                let values = clean(&values);
                let seen = fields.iter().any(|field| eq_ignore_case(&field.name, &name));
                let another = if wanted(role.ids, &name) {
                    values
                        .iter()
                        .all(|id| eq_ignore_case(id.trim(), VARIOUS_ARTISTS_ID))
                } else if wanted(role.sorts, &name)
                    || wanted(role.list_sorts, &name)
                    || wanted(role.credits, &name)
                {
                    values.iter().all(describes_another)
                } else {
                    false
                };
                if another && !seen && !values.is_empty() {
                    copies = false;
                    fields.push(NamedField {
                        container,
                        name,
                        values,
                    });
                }
            }
        }
        (!fields.is_empty()).then_some(StrayAlbumArtists {
            album_artist: kept,
            fields,
            copies,
        })
    }

    /// The fields' names, each once, as the file spells them.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for field in &self.fields {
            if !names.iter().any(|seen| eq_ignore_case(seen, &field.name)) {
                names.push(field.name.clone());
            }
        }
        names
    }

    /// Every value, each once.
    pub fn values(&self) -> Vec<String> {
        let mut values: Vec<String> = Vec::new();
        for value in self.fields.iter().flat_map(|field| &field.values) {
            if !values.contains(value) {
                values.push(value.clone());
            }
        }
        values
    }
}

/// Takes the fields off.
pub fn remove_fields(file: &mut TagFile, fields: &[NamedField]) {
    for field in fields {
        remove_field(file, field.container, &field.name);
    }
}

/// Puts back fields [`remove_fields`] took off. Only the album artist's other names come back,
/// with the sort names, credits and ids that went with them: a field under any other name, or
/// the album artist's own field, is left out.
pub fn restore_fields(file: &mut TagFile, fields: &[NamedField]) {
    let role = &ALBUM_ARTIST_ROLE;
    restore_where(file, fields, |container, name| {
        let other_name = wanted(role.names, name)
            && !(container == Container::Vorbis && eq_ignore_case(name, role.vorbis));
        other_name
            || [role.lists, role.list_sorts, role.sorts, role.credits, role.ids]
                .iter()
                .any(|names| wanted(names, name))
    });
}

/// Puts back the fields `allowed` names, each in its container.
pub(crate) fn restore_where(
    file: &mut TagFile,
    fields: &[NamedField],
    allowed: impl Fn(Container, &str) -> bool,
) {
    for field in fields {
        let name = field.name.as_str();
        if !allowed(field.container, name) {
            continue;
        }
        let values: Vec<String> = clean(&field.values);
        if values.is_empty() {
            continue;
        }
        match field.container {
            Container::Id3 => {
                if let Some(id3) = file.id3v2.as_mut() {
                    id3.set_user_text(name, &values, true);
                }
            }
            Container::Vorbis => {
                if let Some(xiph) = file.xiph.as_mut() {
                    let strs: Vec<&str> = values.iter().map(String::as_str).collect();
                    xiph.set_field(name, &strs);
                }
            }
            Container::Mp4 => {
                if let Some(apple) = file.apple.as_mut() {
                    apple.set_dash_boxes(APPLE_MEAN, name, &values);
                }
            }
        }
    }
}

/// The fields as text, for a journal to keep.
pub fn to_text(fields: &[NamedField]) -> String {
    serde_json::to_string(fields).unwrap_or_default()
}

/// Fields [`to_text`] wrote; None for text it did not write.
pub fn from_text(text: &str) -> Option<Vec<NamedField>> {
    serde_json::from_str(text).ok()
}

// ---- reading ----------------------------------------------------------------------------------

/// The container the library server reads: the Vorbis comment on FLAC and Ogg, the iTunes atoms
/// on MP4, ID3 on the rest.
pub(super) fn read_container(file: &TagFile) -> Option<Container> {
    match file.format() {
        Format::Flac | Format::Ogg => file.xiph.is_some().then_some(Container::Vorbis),
        Format::Mp4 => file.apple.is_some().then_some(Container::Mp4),
        _ => file.id3v2.is_some().then_some(Container::Id3),
    }
}

/// A credit as the library server reads it: the list when the file has one, else every value
/// under any of the credit's names.
fn heard(file: &TagFile, role: &Role) -> Vec<String> {
    let Some(container) = read_container(file) else {
        return Vec::new();
    };
    let fields = fields_in(file, container);
    let list = collect(&fields, role.lists);
    if !list.is_empty() {
        return list;
    }
    let mut values = match container {
        Container::Vorbis => Vec::new(),
        _ => clean(&own_values(file, container, role)),
    };
    values.extend(collect(&fields, role.names));
    values
}

/// The credit in the container's own place for it: the frame, the Vorbis field, the atom.
fn own_values(file: &TagFile, container: Container, role: &Role) -> Vec<String> {
    match container {
        Container::Id3 => file
            .id3v2
            .as_ref()
            .map(|id3| id3.text_values(role.frame))
            .unwrap_or_default(),
        Container::Vorbis => file
            .xiph
            .as_ref()
            .map(|xiph| xiph.field(role.vorbis))
            .unwrap_or_default(),
        Container::Mp4 => file
            .apple
            .as_ref()
            .map(|apple| apple.text(role.atom))
            .unwrap_or_default(),
    }
}

fn has_own_sort(file: &TagFile, container: Container, role: &Role) -> bool {
    let values = match container {
        Container::Id3 => file
            .id3v2
            .as_ref()
            .map(|id3| id3.text_values(role.sort_frame))
            .unwrap_or_default(),
        Container::Vorbis => file
            .xiph
            .as_ref()
            .map(|xiph| xiph.field(role.sort_vorbis))
            .unwrap_or_default(),
        Container::Mp4 => file
            .apple
            .as_ref()
            .map(|apple| apple.text(role.sort_atom))
            .unwrap_or_default(),
    };
    !clean(&values).is_empty()
}

/// The container's named fields: TXXX frames, Vorbis fields or freeform atoms, in file order.
pub(super) fn fields_in(file: &TagFile, container: Container) -> Vec<(String, Vec<String>)> {
    match container {
        Container::Id3 => file
            .id3v2
            .as_ref()
            .map(|id3| id3.user_texts())
            .unwrap_or_default(),
        Container::Vorbis => file
            .xiph
            .as_ref()
            .map(|xiph| {
                xiph.fields
                    .iter()
                    .map(|(name, values)| (name.clone(), values.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        Container::Mp4 => file
            .apple
            .as_ref()
            .map(|apple| apple.freeform(APPLE_MEAN))
            .unwrap_or_default(),
    }
}

/// Every value under the names, in field order, each once.
fn collect(fields: &[(String, Vec<String>)], names: &[&str]) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    for (name, own) in fields {
        if !wanted(names, name) {
            continue;
        }
        for value in clean(own) {
            if !values.contains(&value) {
                values.push(value);
            }
        }
    }
    values
}

pub(super) fn clean(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| !is_null_or_white_space(Some(value)))
        .map(|value| value.trim().to_string())
        .collect()
}

pub(super) fn wanted(names: &[&str], name: &str) -> bool {
    names.iter().any(|wanted| eq_ignore_case(wanted, name))
}

// ---- writing ----------------------------------------------------------------------------------

/// Every field under the names, from every container, except the ones `keep` picks.
fn remove_names(file: &mut TagFile, names: &[&str], keep: impl Fn(Container, &str) -> bool) {
    for container in CONTAINERS {
        remove_in(file, container, names, &keep);
    }
}

fn remove_in(
    file: &mut TagFile,
    container: Container,
    names: &[&str],
    keep: impl Fn(Container, &str) -> bool,
) {
    for (name, _) in fields_in(file, container) {
        if wanted(names, &name) && !keep(container, &name) {
            remove_field(file, container, &name);
        }
    }
}

pub(super) fn remove_field(file: &mut TagFile, container: Container, name: &str) {
    match container {
        Container::Id3 => {
            if let Some(id3) = file.id3v2.as_mut() {
                id3.retain_user_texts(|description| !eq_ignore_case(description, name));
            }
        }
        Container::Vorbis => {
            if let Some(xiph) = file.xiph.as_mut() {
                xiph.remove_field(name);
            }
        }
        Container::Mp4 => {
            if let Some(apple) = file.apple.as_mut() {
                while apple.dash_boxes(APPLE_MEAN, name).is_some() {
                    apple.remove_dash_box(APPLE_MEAN, name);
                }
            }
        }
    }
}

// ---- comparing names --------------------------------------------------------------------------

/// What may stand between two names of a credit split into its names: "A & B", "A, B and C",
/// "A feat. B".
static JOIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:,|,?\s*(?:&|\+|/|;|x|×|and|with|feat\.?|ft\.?|featuring|vs\.?|versus))$")
        .expect("a valid pattern")
});

/// Whether `values` are the names `credit` is made of, in its order, with only joining words
/// between them. `exact` holds capitals; otherwise they are ignored.
pub fn is_split(values: &[String], credit: &str, exact: bool) -> bool {
    if values.len() < 2 {
        return false;
    }
    let fold = |text: &str| {
        if exact {
            text.trim().to_string()
        } else {
            text.trim().to_lowercase()
        }
    };
    let credit = fold(credit);
    let mut at = 0;
    for (index, value) in values.iter().enumerate() {
        let value = fold(value);
        if value.is_empty() {
            return false;
        }
        let Some(found) = credit[at..].find(&value) else {
            return false;
        };
        let gap = credit[at..at + found].trim();
        if (index == 0 && !gap.is_empty()) || (index > 0 && !JOIN.is_match(gap)) {
            return false;
        }
        at += found + value.len();
    }
    credit[at..].trim().is_empty()
}

/// Whether a list of names is the credit: the same names, or the one name split into its names.
fn is_list_of(values: &[String], credit: &[String], exact: bool) -> bool {
    let same = values.len() == credit.len()
        && values.iter().zip(credit).all(|(value, own)| {
            if exact {
                value == own
            } else {
                same_name(value, own)
            }
        });
    same || (credit.len() == 1 && is_split(values, &credit[0], exact))
}

/// One name in other capitals or spacing.
pub(super) fn same_name(a: &str, b: &str) -> bool {
    let fold = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    fold(a) == fold(b)
}

#[cfg(test)]
#[path = "credit_names_tests.rs"]
mod tests;
