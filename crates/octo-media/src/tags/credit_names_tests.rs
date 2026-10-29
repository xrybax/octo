//! A peer's file names its credits under more names than the one Octo writes, and the library
//! server reads every one. Checked on real MP3, FLAC, MP4 and Opus files, written through the
//! download's tag body and read back the way the library server reads them.

use std::path::{Path, PathBuf};

use octo_core::models::domain::Song;

use super::*;
use crate::audio::test_support::{require_ffmpeg, run_ffmpeg};
use crate::tags::kept_identity::{ALBUM_ARTIST_KEYS, ALBUM_ARTISTS_KEYS, KeptIdentityTags};
use crate::tags::song_tags::{GenreWrite, write_song};
use crate::tags::tag_writer_extras::native_tags;
use crate::tags::test_support::{flac, mp3, write};

fn open(path: &Path) -> TagFile {
    TagFile::open(path).expect("opens")
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

/// A file of the format, as ffmpeg or the synthetic writers make it.
fn audio(dir: &Path, format: &str) -> PathBuf {
    match format {
        "mp3" => write(dir, "peer.mp3", &mp3()),
        "flac" => write(dir, "peer.flac", &flac()),
        "m4a" => {
            run_ffmpeg(
                dir,
                "-f lavfi -i sine=frequency=440:duration=1 -c:a aac -b:a 128k peer.m4a",
            );
            dir.join("peer.m4a")
        }
        "opus" => {
            run_ffmpeg(
                dir,
                "-f lavfi -i sine=frequency=440:duration=1 -c:a libopus -b:a 64k peer.opus",
            );
            dir.join("peer.opus")
        }
        other => panic!("no {other}"),
    }
}

/// One named field in the container the format carries: TXXX, Vorbis field or freeform atom.
fn plant(file: &mut TagFile, name: &str, values: &[&str]) {
    let values = strings(values);
    let tags = native_tags(file);
    if let Some(id3) = tags.id3 {
        id3.set_user_text(name, &values, true);
    }
    if let Some(xiph) = tags.xiph {
        let strs: Vec<&str> = values.iter().map(String::as_str).collect();
        xiph.set_field(name, &strs);
    }
    if let Some(apple) = tags.apple {
        apple.set_dash_boxes(APPLE_MEAN, name, &values);
    }
}

/// The names of every named field the file carries, lowercase.
fn named_fields(path: &Path) -> Vec<String> {
    let file = open(path);
    let container = read_container(&file).expect("a tag");
    fields_in(&file, container)
        .into_iter()
        .map(|(name, _)| name.to_lowercase())
        .collect()
}

/// The values the library server reads under the keys.
fn read_as_server(path: &Path, keys: &[&str]) -> Vec<String> {
    let view = KeptIdentityTags::navidrome_view(&open(path));
    keys.iter()
        .flat_map(|key| view.get(*key).cloned().unwrap_or_default())
        .collect()
}

/// The issue's peer: the album artist under all five spellings, with a sort name of its own.
fn miki_peer(dir: &Path, format: &str) -> PathBuf {
    let path = audio(dir, format);
    let mut file = open(&path);
    file.set_title(Some("industry plant"));
    file.set_performers(&strings(&["Miki"]));
    file.set_album(Some("industry plant"));
    file.set_album_artists(&strings(&["miki"]));
    for name in ["ALBUM ARTIST", "ALBUM_ARTIST", "ALBUMARTISTS", "ALBUM_ARTISTS"] {
        plant(&mut file, name, &["Miki"]);
    }
    plant(&mut file, "ALBUMARTISTSORT", &["Miki"]);
    plant(&mut file, "ALBUMARTISTSSORT", &["Miki"]);
    file.save().expect("saved");
    path
}

fn octo_song(artist: &str, album_artist: &str) -> Song {
    Song {
        title: "industry plant".into(),
        artist: artist.into(),
        album: "industry plant".into(),
        album_artist: Some(album_artist.into()),
        ..Default::default()
    }
}

fn tag(path: &Path, song: &Song) {
    let mut file = open(path);
    write_song(&mut file, song, &GenreWrite::Keep, false);
    file.save().expect("saved");
}

// ---- the download's tag body --------------------------------------------------------------

#[test]
fn a_peer_file_with_all_five_spellings_ends_with_only_octos_album_artist() {
    require_ffmpeg!();
    for format in ["flac", "mp3", "m4a", "opus"] {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = miki_peer(dir.path(), format);
        // As it arrived, the library server reads two album artists and a list in their place.
        assert!(
            read_as_server(&path, &ALBUM_ARTIST_KEYS).len() > 1,
            "{format}: {:?}",
            named_fields(&path)
        );

        tag(&path, &octo_song("miki", "miki"));

        let fields = named_fields(&path);
        for gone in [
            "album artist",
            "album_artist",
            "albumartists",
            "album_artists",
            "albumartistssort",
            "albumartistsort",
        ] {
            assert!(
                !fields.contains(&gone.to_string()),
                "{format}: {gone} stayed in {fields:?}"
            );
        }
        assert_eq!(read_as_server(&path, &ALBUM_ARTIST_KEYS), ["miki"], "{format}");
        assert!(read_as_server(&path, &ALBUM_ARTISTS_KEYS).is_empty(), "{format}");
        let identity = KeptIdentityTags::read(&path, None).expect("reads");
        assert_eq!(
            (identity.album_artist, identity.album_artists),
            (strings(&["miki"]), Vec::new()),
            "{format}"
        );
        assert_eq!(open(&path).album_artists(), ["miki"], "{format}");
    }
}

#[test]
fn a_track_from_a_clean_peer_and_one_from_a_messy_peer_read_as_one_album() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let messy = miki_peer(dir.path(), "flac");
    let clean_dir = tempfile::tempdir().expect("a temp dir");
    let clean = write(clean_dir.path(), "clean.flac", &flac());
    for path in [&messy, &clean] {
        tag(path, &octo_song("miki", "miki"));
    }
    let messy = KeptIdentityTags::read(&messy, None).expect("reads");
    let clean = KeptIdentityTags::read(&clean, None).expect("reads");
    assert_eq!(
        KeptIdentityTags::pid_inputs(&messy),
        KeptIdentityTags::pid_inputs(&clean)
    );
}

#[test]
fn a_sort_name_stays_under_one_name_while_the_album_artist_is_the_same() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = write(dir.path(), "peer.flac", &flac());
    {
        let mut file = open(&path);
        file.set_album_artists(&strings(&["The Beatles"]));
        plant(&mut file, "ALBUMARTISTSORT", &["Beatles, The"]);
        plant(&mut file, "WM/ALBUMARTISTSORTORDER", &["Beatles"]);
        file.save().expect("saved");
    }
    tag(&path, &octo_song("The Beatles", "The Beatles"));
    let file = open(&path);
    let xiph = file.xiph.as_ref().expect("a Vorbis comment");
    assert_eq!(xiph.field("ALBUMARTISTSORT"), ["Beatles, The"]);
    assert!(xiph.field("WM/ALBUMARTISTSORTORDER").is_empty());
}

#[test]
fn a_sort_name_of_another_album_artist_goes_with_it() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = write(dir.path(), "peer.mp3", &mp3());
    {
        let mut file = open(&path);
        file.set_album_artists(&strings(&["Kanye West"]));
        file.set_performers(&strings(&["Kanye West"]));
        let id3 = file.id3v2.as_mut().expect("an ID3 tag");
        id3.set_text_frame("TSO2", &["West, Kanye"]);
        id3.set_text_frame("TSOP", &["West, Kanye"]);
        id3.set_user_text("ALBUMARTISTSORT", &strings(&["West, Kanye"]), true);
        file.save().expect("saved");
    }
    tag(&path, &octo_song("Ye", "Ye"));
    let file = open(&path);
    let id3 = file.id3v2.as_ref().expect("an ID3 tag");
    assert!(id3.text_values("TSO2").is_empty());
    assert!(id3.text_values("TSOP").is_empty());
    assert!(
        id3.user_texts()
            .iter()
            .all(|(description, _)| !description.eq_ignore_ascii_case("ALBUMARTISTSORT")),
        "{:?}",
        id3.user_texts()
    );
    assert_eq!(id3.text_values("TPE2"), ["Ye"]);
}

#[test]
fn the_artists_stay_only_as_the_artist_split_into_its_names() {
    let dir = tempfile::tempdir().expect("a temp dir");
    // A Picard file: the credit, and its names as a list.
    let picard = write(dir.path(), "picard.flac", &flac());
    {
        let mut file = open(&picard);
        plant(&mut file, "ARTISTS", &["Massive Attack", "Elizabeth Fraser"]);
        plant(&mut file, "AUTHOR", &["Robert Del Naja"]);
        file.save().expect("saved");
    }
    tag(
        &picard,
        &octo_song("Massive Attack feat. Elizabeth Fraser", "Massive Attack"),
    );
    let file = open(&picard);
    let xiph = file.xiph.as_ref().expect("a Vorbis comment");
    assert_eq!(xiph.field("ARTISTS"), ["Massive Attack", "Elizabeth Fraser"]);
    assert!(
        xiph.field("AUTHOR").is_empty(),
        "AUTHOR is read as a second artist"
    );

    // A list naming someone else is read in place of Octo's artist, so it goes.
    let other = write(dir.path(), "other.flac", &flac());
    {
        let mut file = open(&other);
        plant(&mut file, "ARTISTS", &["Miki"]);
        plant(&mut file, "WM/ARTISTS", &["Miki"]);
        plant(&mut file, "ARTISTSSORT", &["Miki"]);
        file.save().expect("saved");
    }
    tag(&other, &octo_song("miki", "miki"));
    assert_eq!(
        read_as_server(&other, &["artist", "artists", "wm/artists"]),
        ["miki"]
    );
    assert!(!named_fields(&other).contains(&"artistssort".to_string()));
}

#[test]
fn a_list_octo_writes_is_the_only_one() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = write(dir.path(), "peer.flac", &flac());
    {
        let mut file = open(&path);
        plant(&mut file, "WM/ARTISTS", &["Somebody"]);
        file.save().expect("saved");
    }
    let song = Song {
        artists: strings(&["Bizarrap", "Rauw Alejandro"]),
        ..octo_song("Bizarrap & Rauw Alejandro", "Bizarrap")
    };
    tag(&path, &song);
    let file = open(&path);
    let xiph = file.xiph.as_ref().expect("a Vorbis comment");
    assert_eq!(xiph.field("ARTISTS"), ["Bizarrap", "Rauw Alejandro"]);
    assert!(xiph.field("WM/ARTISTS").is_empty());
}

// ---- album artists already in the library ---------------------------------------------------

#[test]
fn stray_album_artists_that_only_repeat_the_album_artist_are_copies() {
    require_ffmpeg!();
    for format in ["flac", "mp3", "m4a", "opus"] {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = miki_peer(dir.path(), format);
        let file = open(&path);
        let stray = StrayAlbumArtists::of(&file).expect("strays");
        assert!(stray.copies, "{format}");
        assert_eq!(stray.album_artist, ["miki"], "{format}");
        let names: Vec<String> = stray.names().iter().map(|name| name.to_lowercase()).collect();
        assert_eq!(
            names,
            [
                "album artist",
                "album_artist",
                "albumartists",
                "album_artists",
                "albumartistssort"
            ],
            "{format}"
        );
        assert_eq!(stray.values(), ["Miki"], "{format}");

        // Taken off, the file reads as the album artist alone; put back, as it arrived.
        let mut file = open(&path);
        remove_fields(&mut file, &stray.fields);
        file.save().expect("saved");
        assert_eq!(read_as_server(&path, &ALBUM_ARTIST_KEYS), ["miki"], "{format}");
        assert_eq!(StrayAlbumArtists::of(&open(&path)), None, "{format}");
        let kept = from_text(&to_text(&stray.fields)).expect("reads back");
        let mut file = open(&path);
        restore_fields(&mut file, &kept);
        file.save().expect("saved");
        let back = StrayAlbumArtists::of(&open(&path)).expect("strays again");
        let sorted = |mut names: Vec<String>| {
            names.sort();
            names
        };
        assert_eq!(sorted(back.names()), sorted(stray.names()), "{format}");
        assert_eq!(back.values(), stray.values(), "{format}");
        assert!(back.copies, "{format}");
    }
}

#[test]
fn another_name_under_a_second_spelling_is_no_copy() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = write(dir.path(), "peer.flac", &flac());
    let mut file = open(&path);
    file.set_album_artists(&strings(&["Ye"]));
    plant(&mut file, "ALBUM ARTIST", &["Kanye West"]);
    let stray = StrayAlbumArtists::of(&file).expect("strays");
    assert!(!stray.copies);
    assert_eq!(stray.values(), ["Kanye West"]);
}

#[test]
fn a_list_that_splits_the_album_artist_or_a_file_without_one_has_no_strays() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = write(dir.path(), "picard.flac", &flac());
    let mut file = open(&path);
    file.set_album_artists(&strings(&["Drake & Future"]));
    plant(&mut file, "ALBUMARTISTS", &["Drake", "Future"]);
    plant(&mut file, "ALBUMARTISTSSORT", &["Drake", "Future"]);
    assert_eq!(StrayAlbumArtists::of(&file), None);

    // With no album artist of its own, the other spelling is the album artist.
    let mut file = open(&write(dir.path(), "only.flac", &flac()));
    plant(&mut file, "ALBUM ARTIST", &["Miki"]);
    assert_eq!(StrayAlbumArtists::of(&file), None);
}

#[test]
fn only_the_album_artists_other_names_are_put_back() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = write(dir.path(), "song.flac", &flac());
    let mut file = open(&path);
    file.set_album_artists(&strings(&["miki"]));
    let sent = vec![
        NamedField {
            container: Container::Vorbis,
            name: "ALBUMARTIST".into(),
            values: strings(&["someone"]),
        },
        NamedField {
            container: Container::Vorbis,
            name: "TITLE".into(),
            values: strings(&["changed"]),
        },
        NamedField {
            container: Container::Vorbis,
            name: "ALBUM ARTIST".into(),
            values: strings(&["Miki"]),
        },
    ];
    restore_fields(&mut file, &sent);
    let xiph = file.xiph.as_ref().expect("a Vorbis comment");
    assert_eq!(xiph.field("ALBUMARTIST"), ["miki"]);
    assert!(xiph.field("TITLE").is_empty());
    assert_eq!(xiph.field("ALBUM ARTIST"), ["Miki"]);
    assert_eq!(from_text("not fields"), None);
}

// ---- names ----------------------------------------------------------------------------------

#[test]
fn a_split_is_the_credits_names_in_order_with_joining_words_between() {
    let split = |values: &[&str], credit: &str, exact: bool| is_split(&strings(values), credit, exact);
    assert!(split(&["Drake", "Future"], "Drake & Future", true));
    assert!(split(
        &["Massive Attack", "Elizabeth Fraser"],
        "Massive Attack feat. Elizabeth Fraser",
        true
    ));
    assert!(split(&["A", "B", "C"], "A, B and C", true));
    assert!(split(&["drake", "future"], "Drake & Future", false));
    assert!(!split(&["drake", "future"], "Drake & Future", true));
    assert!(!split(&["Drake"], "Drake & Future", true));
    assert!(!split(&["Drake", "Future"], "Drake & Future & Young Thug", true));
    assert!(!split(&["Future", "Drake"], "Drake & Future", true));
    assert!(!split(&["Simon", "Garfunkel"], "Simon Garfunkel", true));
}
