//! A peer's file holds a field under more names than the one Octo writes, and the library server
//! takes the first it finds. Checked on real MP3, FLAC, MP4 and Opus files, written through the
//! download's tag body and read back.

use std::path::{Path, PathBuf};

use octo_core::models::domain::Song;

use super::*;
use crate::audio::test_support::{require_ffmpeg, run_ffmpeg};
use crate::tags::credit_names::StrayAlbumArtists;
use crate::tags::kept_identity::{ALBUM_ARTIST_KEYS, KeptIdentityTags};
use crate::tags::song_tags::{GenreWrite, write_song};
use crate::tags::tag_writer_extras::native_tags;
use crate::tags::test_support::{flac, mp3, write};

const FORMATS: [&str; 4] = ["flac", "mp3", "m4a", "opus"];

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
        "m4a" | "opus" => {
            let codec = if format == "m4a" {
                "aac -b:a 128k"
            } else {
                "libopus -b:a 64k"
            };
            run_ffmpeg(
                dir,
                &format!("-f lavfi -i sine=frequency=440:duration=1 -c:a {codec} peer.{format}"),
            );
            dir.join(format!("peer.{format}"))
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

/// Every named field the file carries, its name lowercase, with its values.
fn named(path: &Path) -> Vec<(String, Vec<String>)> {
    let file = open(path);
    let container = read_container(&file).expect("a tag");
    fields_in(&file, container)
        .into_iter()
        .map(|(name, values)| (name.to_lowercase(), values))
        .collect()
}

fn names(path: &Path) -> Vec<String> {
    named(path).into_iter().map(|(name, _)| name).collect()
}

/// The issue's peer: a copy of "Don't Start Now" from a compilation, track 3 of 40 on disc 1 of
/// 2 there, filed under Various Artists under every name taggers use.
fn compilation_peer(dir: &Path, format: &str) -> PathBuf {
    let path = audio(dir, format);
    let mut file = open(&path);
    file.set_title(Some("Don't Start Now"));
    file.set_performers(&strings(&["Dua Lipa"]));
    file.set_album(Some("Now That's What I Call Music! 105"));
    file.set_album_artists(&strings(&["Various Artists"]));
    for name in [
        "ALBUM ARTIST",
        "ALBUM_ARTIST",
        "ALBUM_ARTISTS",
        "ALBUMARTISTSORT",
        "ALBUMARTIST_CREDIT",
        "ALBUMARTISTS_CREDIT",
        "ALBUMARTISTS_SORT",
    ] {
        plant(&mut file, name, &["Various Artists"]);
    }
    plant(
        &mut file,
        "MUSICBRAINZ_ALBUMARTISTID",
        &[credit_names::VARIOUS_ARTISTS_ID],
    );
    for (name, value) in [
        ("TRACK", "3"),
        ("TRACKC", "40"),
        ("TOTALTRACKS", "40"),
        ("DISC", "1"),
        ("DISCC", "2"),
        ("TOTALDISCS", "2"),
    ] {
        plant(&mut file, name, &[value]);
    }
    extras::set_compilation(&mut file, true);
    file.save().expect("saved");
    path
}

fn future_nostalgia() -> Song {
    Song {
        title: "Don't Start Now".into(),
        artist: "Dua Lipa".into(),
        album: "Future Nostalgia".into(),
        album_artist: Some("Dua Lipa".into()),
        track: Some(2),
        total_tracks: Some(11),
        disc_number: Some(1),
        total_discs: Some(1),
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
fn a_compilation_copy_ends_with_only_octos_album_artist_numbers_and_no_flag() {
    require_ffmpeg!();
    for format in FORMATS {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = compilation_peer(dir.path(), format);

        tag(&path, &future_nostalgia());

        let fields = named(&path);
        for (name, values) in &fields {
            assert!(
                !values.iter().any(|value| credit_names::is_various_artists(value)
                    || value.eq_ignore_ascii_case(credit_names::VARIOUS_ARTISTS_ID)),
                "{format}: {name} kept {values:?}"
            );
            for gone in ["track", "trackc", "totaltracks", "disc", "discc", "totaldiscs"] {
                assert_ne!(name, gone, "{format}: {gone} stayed in {fields:?}");
            }
        }
        let file = open(&path);
        assert_eq!(
            (file.track(), file.track_count(), file.disc(), file.disc_count()),
            (2, 11, 1, 1),
            "{format}"
        );
        assert!(!extras::is_compilation(&file), "{format}");
        assert_eq!(
            KeptIdentityTags::navidrome_view(&file)
                .iter()
                .filter(|(key, _)| ALBUM_ARTIST_KEYS.contains(&key.as_str()))
                .flat_map(|(_, values)| values.clone())
                .collect::<Vec<_>>(),
            ["Dua Lipa"],
            "{format}"
        );
        if format == "flac" || format == "opus" {
            let xiph = file.xiph.as_ref().expect("a Vorbis comment");
            assert_eq!(xiph.field("TRACKNUMBER"), ["02"], "{format}");
            assert_eq!(xiph.field("TRACKTOTAL"), ["11"], "{format}");
            assert_eq!(xiph.field("DISCNUMBER"), ["1"], "{format}");
        }
    }
}

#[test]
fn a_song_of_a_compilation_keeps_its_flag_and_one_by_various_artists_is_left_as_it_is() {
    for (compilation, album_artist, flagged) in [
        (true, "Various Artists", true),
        (false, "Various Artists", true),
        (false, "Dua Lipa", false),
    ] {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = write(dir.path(), "song.flac", &flac());
        {
            let mut file = open(&path);
            extras::set_compilation(&mut file, true);
            file.save().expect("saved");
        }
        let song = Song {
            title: "Don't Start Now".into(),
            artist: "Dua Lipa".into(),
            album_artist: Some(album_artist.into()),
            is_compilation: compilation,
            ..Default::default()
        };
        tag(&path, &song);
        assert_eq!(
            extras::is_compilation(&open(&path)),
            flagged,
            "{compilation} {album_artist}"
        );
    }
}

#[test]
fn a_field_octo_did_not_write_keeps_its_other_names() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = write(dir.path(), "peer.flac", &flac());
    {
        let mut file = open(&path);
        plant(&mut file, "TRACK", &["3"]);
        plant(&mut file, "DISC", &["1"]);
        file.save().expect("saved");
    }
    let song = Song {
        title: "T".into(),
        ..Default::default()
    };
    tag(&path, &song);
    let fields = names(&path);
    assert!(fields.contains(&"track".to_string()), "{fields:?}");
    assert!(fields.contains(&"disc".to_string()), "{fields:?}");
}

#[test]
fn other_names_of_every_written_field_go_from_each_container() {
    require_ffmpeg!();
    for format in FORMATS {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = audio(dir.path(), format);
        {
            let mut file = open(&path);
            for (name, value) in [
                ("TITLE", "Another"),
                ("ALBUM", "Another"),
                ("TRACKNUMBER", "9"),
                ("DISCNUMBER", "4"),
                ("RECORDINGDATE", "1999"),
                ("WRITER", "Someone"),
                ("MUSICBRAINZ/TRACK ID", "11111111-1111-1111-1111-111111111111"),
                ("WM/ISRC", "GBAHT1900001"),
            ] {
                plant(&mut file, name, &[value]);
            }
            file.save().expect("saved");
        }
        let song = Song {
            title: "Don't Start Now".into(),
            artist: "Dua Lipa".into(),
            album: "Future Nostalgia".into(),
            track: Some(2),
            disc_number: Some(1),
            year: Some(2019),
            contributors: strings(&["Caroline Ailin"]),
            isrc: Some("GBAHT1901121".into()),
            music_brainz_recording_id: Some("22222222-2222-2222-2222-222222222222".into()),
            ..Default::default()
        };
        tag(&path, &song);

        let fields = names(&path);
        // Octo's own Vorbis names stay; every other name goes.
        let own: &[&str] = match format {
            "flac" | "opus" => &["title", "album", "tracknumber", "discnumber"],
            _ => &[],
        };
        for name in [
            "title",
            "album",
            "tracknumber",
            "discnumber",
            "recordingdate",
            "writer",
            "musicbrainz/track id",
            "wm/isrc",
        ] {
            assert_eq!(
                fields.contains(&name.to_string()),
                own.contains(&name),
                "{format}: {name} in {fields:?}"
            );
        }
        let file = open(&path);
        assert_eq!(file.title().as_deref(), Some("Don't Start Now"), "{format}");
        assert_eq!(file.track(), 2, "{format}");
    }
}

// ---- files already in the library ---------------------------------------------------------

fn flac_with(dir: &Path, fields: &[(&str, &str)]) -> PathBuf {
    let path = write(dir, "song.flac", &flac());
    let mut file = open(&path);
    for (name, value) in fields {
        plant(&mut file, name, &[value]);
    }
    file.save().expect("saved");
    path
}

#[test]
fn a_track_number_under_another_name_is_a_copy_only_when_it_is_the_same_number() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let same = flac_with(dir.path(), &[("TRACKNUMBER", "02"), ("TRACK", "2/11")]);
    let found = strays(&open(&same));
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].words, found[0].own.as_str(), found[0].copies),
        ("track number", "2", true)
    );
    assert_eq!(found[0].fields[0].name, "TRACK");

    let dir = tempfile::tempdir().expect("a temp dir");
    let other = flac_with(
        dir.path(),
        &[
            ("TRACKNUMBER", "02"),
            ("TRACK", "3"),
            ("DISC", "1"),
            ("DISCNUMBER", "1"),
        ],
    );
    let found = strays(&open(&other));
    let track = found
        .iter()
        .find(|stray| stray.words == "track number")
        .expect("the track");
    assert!(!track.copies);
    assert_eq!(track.fields[0].values, ["3"]);
    let disc = found
        .iter()
        .find(|stray| stray.words == "disc number")
        .expect("the disc");
    assert!(disc.copies);
}

#[test]
fn a_field_with_no_value_of_its_own_has_no_strays() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = flac_with(dir.path(), &[("TRACK", "3"), ("TOTALTRACKS", "12")]);
    assert!(strays(&open(&path)).is_empty());
}

#[test]
fn strays_are_read_in_the_container_the_library_server_reads() {
    require_ffmpeg!();
    for format in FORMATS {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = audio(dir.path(), format);
        {
            let mut file = open(&path);
            file.set_track(2);
            plant(&mut file, "TRACK", &["3"]);
            file.save().expect("saved");
        }
        let found = strays(&open(&path));
        assert_eq!(found.len(), 1, "{format}: {found:?}");
        assert_eq!((found[0].own.as_str(), found[0].copies), ("2", false), "{format}");
    }
}

#[test]
fn only_other_names_are_put_back() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = flac_with(dir.path(), &[("TRACKNUMBER", "02"), ("TRACK", "3")]);
    let mut file = open(&path);
    let gone = drop_other_names(&mut file, &[&TRACK]);
    assert_eq!(gone.len(), 1);
    file.save().expect("saved");
    assert!(!names(&path).contains(&"track".to_string()));

    let mut file = open(&path);
    let mut back = gone.clone();
    for (name, value) in [("TRACKNUMBER", "7"), ("ARTIST", "Someone")] {
        back.push(NamedField {
            container: Container::Vorbis,
            name: name.into(),
            values: vec![value.into()],
        });
    }
    restore_fields(&mut file, &back);
    file.save().expect("saved");
    let file = open(&path);
    let xiph = file.xiph.as_ref().expect("a Vorbis comment");
    assert_eq!(xiph.field("TRACK"), ["3"]);
    assert_eq!(xiph.field("TRACKNUMBER"), ["02"]);
    assert!(xiph.field("ARTIST").is_empty());
}

#[test]
fn a_compilations_sort_name_credit_and_id_beside_another_album_artist_are_strays() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = write(dir.path(), "song.flac", &flac());
    {
        let mut file = open(&path);
        file.set_album_artists(&strings(&["Dua Lipa"]));
        plant(&mut file, "ALBUMARTISTSORT", &["Various Artists"]);
        plant(&mut file, "ALBUMARTIST_CREDIT", &["Various Artists"]);
        plant(
            &mut file,
            "MUSICBRAINZ_ALBUMARTISTID",
            &[credit_names::VARIOUS_ARTISTS_ID],
        );
        file.save().expect("saved");
    }
    let stray = StrayAlbumArtists::of(&open(&path)).expect("strays");
    assert!(!stray.copies);
    assert_eq!(
        stray.names(),
        [
            "ALBUMARTISTSORT",
            "ALBUMARTIST_CREDIT",
            "MUSICBRAINZ_ALBUMARTISTID"
        ]
    );

    // Taken off and put back, they are the same fields again.
    let mut file = open(&path);
    credit_names::remove_fields(&mut file, &stray.fields);
    file.save().expect("saved");
    assert!(StrayAlbumArtists::of(&open(&path)).is_none());
    let mut file = open(&path);
    credit_names::restore_fields(&mut file, &stray.fields);
    file.save().expect("saved");
    assert_eq!(StrayAlbumArtists::of(&open(&path)), Some(stray));
}

#[test]
fn the_album_artists_own_sort_name_is_no_stray() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = write(dir.path(), "song.flac", &flac());
    {
        let mut file = open(&path);
        file.set_album_artists(&strings(&["Dua Lipa"]));
        plant(&mut file, "ALBUMARTISTSORT", &["Lipa, Dua"]);
        plant(&mut file, "ALBUMARTIST_CREDIT", &["Dua Lipa"]);
        file.save().expect("saved");
    }
    assert!(StrayAlbumArtists::of(&open(&path)).is_none());
}
