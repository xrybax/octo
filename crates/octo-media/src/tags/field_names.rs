//! Every other name the library server reads a field under, for the fields Octo writes beside a
//! song's credits (`credit_names` has those), after Navidrome 0.64's resources/mappings.yaml. It
//! reads the track number from TRACK, TRCK, TRACKNUMBER, TRKN, WM/TrackNumber and ITRK alike and
//! takes the first it finds, so a peer's TRACK=3 beside Octo's TRACKNUMBER=2 shows track 3; a
//! second title, album or disc number goes the same way.
//!
//! Names are lowercase and matched against ID3 TXXX descriptions, Vorbis field names and MP4
//! freeform names alike. The frames and atoms Octo writes (TRCK, trkn) are not among them, and
//! neither are the names Octo writes in each container, listed per field.

use octo_core::common::dotnet::eq_ignore_case;

use super::credit_names::{
    self, Container, NamedField, clean, fields_in, read_container, remove_field, same_name, wanted,
};
use super::tag_file::TagFile;
use super::tag_writer_extras::{self as extras, APPLE_MEAN, MUSIC_BRAINZ_UFID_OWNER, TagFields};

/// How two values of a field are the same value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The same words in other capitals or spacing.
    Text,
    /// The same number, written "02", "2" or "2/12".
    Number,
    /// The same id in other capitals.
    Id,
}

/// One field Octo writes and the names the library server reads it under.
pub struct FieldName {
    /// The field, in words.
    pub words: &'static str,
    /// Every name the library server reads it under, and the names other taggers keep a copy
    /// under that players read.
    names: &'static [&'static str],
    /// The names Octo writes it under: a TXXX description, Vorbis fields, a freeform atom. A
    /// field with a frame or atom of its own has none in that container.
    id3: &'static [&'static str],
    vorbis: &'static [&'static str],
    mp4: &'static [&'static str],
    kind: Kind,
    /// The value in the field's own place, for holding the other names against it.
    own: fn(&TagFile) -> Option<String>,
}

fn number(value: u32) -> Option<String> {
    (value > 0).then(|| value.to_string())
}

fn recording_id(file: &TagFile) -> Option<String> {
    let id3 = file.id3v2.as_ref().and_then(|id3| {
        id3.ufid(MUSIC_BRAINZ_UFID_OWNER)
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    });
    id3.or_else(|| {
        file.xiph
            .as_ref()
            .and_then(|xiph| xiph.first_field("MUSICBRAINZ_TRACKID"))
    })
    .or_else(|| {
        file.apple
            .as_ref()
            .and_then(|apple| apple.dash_box(APPLE_MEAN, "MusicBrainz Track Id"))
    })
}

pub const TITLE: FieldName = FieldName {
    words: "title",
    names: &["title", "tit2", "\u{a9}nam", "inam"],
    id3: &[],
    vorbis: &["title"],
    mp4: &[],
    kind: Kind::Text,
    own: TagFile::title,
};

pub const ALBUM: FieldName = FieldName {
    words: "album",
    names: &["album", "talb", "\u{a9}alb", "wm/albumtitle", "iprd"],
    id3: &[],
    vorbis: &["album"],
    mp4: &[],
    kind: Kind::Text,
    own: TagFile::album,
};

pub const TRACK: FieldName = FieldName {
    words: "track number",
    names: &["track", "trck", "tracknumber", "trkn", "wm/tracknumber", "itrk"],
    id3: &[],
    vorbis: &["tracknumber"],
    mp4: &[],
    kind: Kind::Number,
    own: |file| number(file.track()),
};

/// TRACKC is how some taggers keep the total, and players read it.
pub const TRACK_TOTAL: FieldName = FieldName {
    words: "track total",
    names: &["tracktotal", "totaltracks", "trackc"],
    id3: &[],
    vorbis: &["tracktotal"],
    mp4: &[],
    kind: Kind::Number,
    own: |file| number(file.track_count()),
};

pub const DISC: FieldName = FieldName {
    words: "disc number",
    names: &["disc", "tpos", "discnumber", "disk", "wm/partofset"],
    id3: &[],
    vorbis: &["discnumber"],
    mp4: &[],
    kind: Kind::Number,
    own: |file| number(file.disc()),
};

/// DISCC is how some taggers keep the total, and players read it.
pub const DISC_TOTAL: FieldName = FieldName {
    words: "disc total",
    names: &["disctotal", "totaldiscs", "discc"],
    id3: &[],
    vorbis: &["disctotal"],
    mp4: &[],
    kind: Kind::Number,
    own: |file| number(file.disc_count()),
};

/// The year Octo writes is the recording date.
pub const YEAR: FieldName = FieldName {
    words: "year",
    names: &["date", "tdrc", "recordingdate", "icrd", "record date"],
    id3: &[],
    vorbis: &["date"],
    mp4: &[],
    kind: Kind::Number,
    own: |file| number(file.year()),
};

pub const GENRE: FieldName = FieldName {
    words: "genre",
    names: &["genre", "tcon", "\u{a9}gen", "wm/genre", "ignr"],
    id3: &[],
    vorbis: &["genre"],
    mp4: &[],
    kind: Kind::Text,
    own: |file| file.genres().into_iter().next(),
};

pub const BPM: FieldName = FieldName {
    words: "BPM",
    names: &["bpm", "tbpm", "tmpo", "wm/beatsperminute"],
    id3: &[],
    vorbis: &["bpm"],
    mp4: &[],
    kind: Kind::Number,
    own: |_| None,
};

/// The library server reads a writer as a composer.
pub const COMPOSER: FieldName = FieldName {
    words: "composer",
    names: &[
        "composer",
        "tcom",
        "\u{a9}wrt",
        "wm/composer",
        "imus",
        "writer",
        "iwri",
    ],
    id3: &[],
    vorbis: &["composer"],
    mp4: &[],
    kind: Kind::Text,
    own: |file| file.composers().into_iter().next(),
};

pub const COPYRIGHT: FieldName = FieldName {
    words: "copyright",
    names: &["copyright", "tcop", "cprt", "icop"],
    id3: &[],
    vorbis: &["copyright"],
    mp4: &[],
    kind: Kind::Text,
    own: TagFile::copyright,
};

pub const COMPILATION: FieldName = FieldName {
    words: "compilation flag",
    names: &["compilation", "tcmp", "cpil", "wm/iscompilation"],
    id3: &[],
    vorbis: &["compilation"],
    mp4: &[],
    kind: Kind::Number,
    own: |_| None,
};

pub const ISRC: FieldName = FieldName {
    words: "ISRC",
    names: &["isrc", "tsrc", "wm/isrc"],
    id3: &[],
    vorbis: &["isrc"],
    mp4: &["isrc"],
    kind: Kind::Id,
    own: |file| extras::read_text(file, TagFields::ISRC),
};

pub const CATALOG_NUMBER: FieldName = FieldName {
    words: "catalog number",
    names: &["catalognumber", "wm/catalogno"],
    id3: &["catalognumber"],
    vorbis: &["catalognumber"],
    mp4: &["catalognumber"],
    kind: Kind::Text,
    own: |_| None,
};

pub const ORIGINAL_DATE: FieldName = FieldName {
    words: "original date",
    names: &[
        "originaldate",
        "tdor",
        "tory",
        "originalyear",
        "origyear",
        "wm/originalreleasetime",
        "wm/originalreleaseyear",
    ],
    id3: &[],
    vorbis: &["originaldate", "originalyear"],
    mp4: &["originaldate"],
    kind: Kind::Text,
    own: |_| None,
};

/// The recording id: a UFID frame on ID3.
pub const RECORDING_ID: FieldName = FieldName {
    words: "recording id",
    names: &[
        "musicbrainz_trackid",
        "musicbrainz track id",
        "musicbrainz/track id",
    ],
    id3: &[],
    vorbis: &["musicbrainz_trackid"],
    mp4: &["musicbrainz track id"],
    kind: Kind::Id,
    own: recording_id,
};

pub const RELEASE_TRACK_ID: FieldName = FieldName {
    words: "release track id",
    names: &[
        "musicbrainz_releasetrackid",
        "musicbrainz release track id",
        "musicbrainz/release track id",
    ],
    id3: &["musicbrainz release track id"],
    vorbis: &["musicbrainz_releasetrackid"],
    mp4: &["musicbrainz release track id"],
    kind: Kind::Id,
    own: |file| extras::read_text(file, TagFields::RELEASE_TRACK_ID),
};

pub const RELEASE_ID: FieldName = FieldName {
    words: "release id",
    names: &[
        "musicbrainz_albumid",
        "musicbrainz album id",
        "musicbrainz/album id",
    ],
    id3: &["musicbrainz album id"],
    vorbis: &["musicbrainz_albumid"],
    mp4: &["musicbrainz album id"],
    kind: Kind::Id,
    own: |file| extras::read_text(file, TagFields::ALBUM_ID),
};

pub const RELEASE_GROUP_ID: FieldName = FieldName {
    words: "release group id",
    names: &[
        "musicbrainz_releasegroupid",
        "musicbrainz release group id",
        "musicbrainz/release group id",
    ],
    id3: &["musicbrainz release group id"],
    vorbis: &["musicbrainz_releasegroupid"],
    mp4: &["musicbrainz release group id"],
    kind: Kind::Id,
    own: |file| extras::read_text(file, TagFields::RELEASE_GROUP_ID),
};

pub const ARTIST_ID: FieldName = FieldName {
    words: "artist id",
    names: credit_names::ARTIST_ID,
    id3: &["musicbrainz artist id"],
    vorbis: &["musicbrainz_artistid"],
    mp4: &["musicbrainz artist id"],
    kind: Kind::Id,
    own: |_| None,
};

pub const ALBUM_ARTIST_ID: FieldName = FieldName {
    words: "album artist id",
    names: credit_names::ALBUM_ARTIST_ID,
    id3: &["musicbrainz album artist id"],
    vorbis: &["musicbrainz_albumartistid"],
    mp4: &["musicbrainz album artist id"],
    kind: Kind::Id,
    own: |_| None,
};

/// The fields the library tag check holds a file's other names against: the ones whose second
/// value shows as another song, album or place on it.
pub const CHECKED: [&FieldName; 8] = [
    &TITLE,
    &ALBUM,
    &TRACK,
    &TRACK_TOTAL,
    &DISC,
    &DISC_TOTAL,
    &ISRC,
    &RECORDING_ID,
];

/// Every field here, for putting back what an undo names.
const ALL: [&FieldName; 21] = [
    &TITLE,
    &ALBUM,
    &TRACK,
    &TRACK_TOTAL,
    &DISC,
    &DISC_TOTAL,
    &YEAR,
    &GENRE,
    &BPM,
    &COMPOSER,
    &COPYRIGHT,
    &COMPILATION,
    &ISRC,
    &CATALOG_NUMBER,
    &ORIGINAL_DATE,
    &RECORDING_ID,
    &RELEASE_TRACK_ID,
    &RELEASE_ID,
    &RELEASE_GROUP_ID,
    &ARTIST_ID,
    &ALBUM_ARTIST_ID,
];

impl FieldName {
    /// Whether a named field in the container is this field under another name than Octo's.
    fn other_name(&self, container: Container, name: &str) -> bool {
        let own = match container {
            Container::Id3 => self.id3,
            Container::Vorbis => self.vorbis,
            Container::Mp4 => self.mp4,
        };
        wanted(self.names, name) && !wanted(own, name)
    }

    fn same(&self, a: &str, b: &str) -> bool {
        match self.kind {
            Kind::Text => same_name(a, b),
            Kind::Id => eq_ignore_case(a.trim(), b.trim()),
            Kind::Number => leading_number(a).is_some_and(|n| Some(n) == leading_number(b)),
        }
    }
}

/// The number a value starts with: 2 for "02" and "2/12".
fn leading_number(value: &str) -> Option<u64> {
    let digits: String = value.trim().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

const CONTAINERS: [Container; 3] = [Container::Id3, Container::Vorbis, Container::Mp4];

/// After Octo wrote the fields: every other name each goes by, in every container, so the
/// library server reads Octo's value alone. Returns what went, values and all.
pub fn drop_other_names(file: &mut TagFile, written: &[&FieldName]) -> Vec<NamedField> {
    let mut gone: Vec<NamedField> = Vec::new();
    for container in CONTAINERS {
        for (name, values) in fields_in(file, container) {
            if written.iter().any(|field| field.other_name(container, &name)) {
                remove_field(file, container, &name);
                gone.push(NamedField {
                    container,
                    name,
                    values,
                });
            }
        }
    }
    gone
}

/// Whether the file holds the field under another name than Octo's, in any container.
pub fn has_other_names(file: &TagFile, field: &FieldName) -> bool {
    CONTAINERS.into_iter().any(|container| {
        fields_in(file, container)
            .iter()
            .any(|(name, _)| field.other_name(container, name))
    })
}

/// One field a file holds under other names beside its own value, in the container the library
/// server reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrayField {
    /// The field, in words.
    pub words: &'static str,
    /// The value in the field's own place.
    pub own: String,
    pub fields: Vec<NamedField>,
    /// Every value is the field's own value again.
    pub copies: bool,
}

/// The fields of [`CHECKED`] a file holds under other names beside their own value. A field
/// with no value of its own is left alone: then the other name is its value.
pub fn strays(file: &TagFile) -> Vec<StrayField> {
    let Some(container) = read_container(file) else {
        return Vec::new();
    };
    let named = fields_in(file, container);
    let mut found: Vec<StrayField> = Vec::new();
    for field in CHECKED {
        let Some(own) = (field.own)(file)
            .map(|own| own.trim().to_string())
            .filter(|own| !own.is_empty())
        else {
            continue;
        };
        let mut stray = StrayField {
            words: field.words,
            own: own.clone(),
            fields: Vec::new(),
            copies: true,
        };
        for (name, values) in &named {
            let values = clean(values);
            if values.is_empty() || !field.other_name(container, name) {
                continue;
            }
            stray.copies &= values.iter().all(|value| field.same(&own, value));
            stray.fields.push(NamedField {
                container,
                name: name.clone(),
                values,
            });
        }
        if !stray.fields.is_empty() {
            found.push(stray);
        }
    }
    found
}

/// Puts back fields that went. Only other names of the fields here come back: a field's own
/// name, or one that is no field here, is left out.
pub fn restore_fields(file: &mut TagFile, fields: &[NamedField]) {
    credit_names::restore_where(file, fields, |container, name| {
        ALL.iter().any(|field| field.other_name(container, name))
    });
}

#[cfg(test)]
#[path = "field_names_tests.rs"]
mod tests;
