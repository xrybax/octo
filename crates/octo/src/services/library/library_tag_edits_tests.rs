//! Tests of `LibraryTagEdits` and `TagEditJournal`, each function on its own. The endpoints that
//! use them are tested in `controllers::subsonic::library_edit_tests`.

use std::path::{Path, PathBuf};

use super::*;
use crate::services::test_support::{flac, mp3};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/rust-migration/fixtures/state/tag-edits.json"
);

fn changes(pairs: &[(&str, Option<&str>)]) -> IndexMap<String, Option<String>> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.map(str::to_string)))
        .collect()
}

/// An MP3 with a title, an artist and an album.
fn song(dir: &Path, name: &str, artist: &str, title: &str, album: &str) -> PathBuf {
    let path = dir.join(name);
    let bytes = if name.ends_with(".flac") { flac() } else { mp3() };
    std::fs::write(&path, bytes).expect("written");
    let mut file = TagFile::open(&path).expect("opens");
    file.set_title(Some(title));
    file.set_performers(&[artist.to_string()]);
    file.set_album(Some(album));
    file.save().expect("saved");
    path
}

fn value(values: &TagValues, field: &str) -> Option<String> {
    values.get(field).cloned().flatten()
}

#[test]
fn the_fixture_round_trips_byte_for_byte() {
    let text = std::fs::read_to_string(FIXTURE).expect("the fixture is in the repo");
    let entries: Vec<TagEditEntry> = serde_json::from_str(&text).expect("the fixture reads");
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].before.get("isrc"), Some(&None));
    let album = entries[1]
        .album_before
        .as_ref()
        .expect("an album join keeps the album");
    assert_eq!(album.album.as_deref(), Some("Jazz (Deluxe)"));
    assert_eq!(album.track, 12);
    assert_eq!(entries[2].run_id.as_deref(), Some("run-explicit-1"));
    assert_eq!(octo_core::json::to_string(&entries), text.trim_end_matches('\n'));
}

#[test]
fn load_and_save_go_through_the_file() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("tag-edits.json");
    assert!(load(&path).is_empty());
    let entries: Vec<TagEditEntry> =
        serde_json::from_str(&std::fs::read_to_string(FIXTURE).expect("the fixture")).expect("reads");
    assert!(save(&path, &entries));
    assert_eq!(load(&path), entries);
    std::fs::write(&path, "not json").expect("writes");
    assert!(load(&path).is_empty());
}

// ---- The explicit advisory -------------------------------------------------------------------

#[test]
fn set_advisory_round_trips_and_clears() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = song(dir.path(), "x.mp3", "A", "B", "C");

    let written = set_advisory(&path, Some(1));
    let again = set_advisory(&path, Some(1));
    let cleared = set_advisory(&path, None);

    assert!(written.changed);
    assert!(!again.changed);
    assert!(cleared.changed);
    assert_eq!(
        cleared.before.get(tag_edit_kinds::ADVISORY),
        Some(&Some("1".to_string()))
    );
    let file = TagFile::open(&path).expect("opens");
    assert_eq!(read_advisory(&file), None);
}

// ---- Each function on its own ----------------------------------------------------------------

#[test]
fn read_lists_every_field_in_order_and_none_for_what_is_missing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = song(dir.path(), "a.flac", "Logic", "America", "Everybody");
    let values = read(&path).expect("reads");
    assert_eq!(
        values.keys().map(String::as_str).collect::<Vec<_>>(),
        song_tag_fields::ALL
    );
    assert_eq!(value(&values, song_tag_fields::TITLE).as_deref(), Some("America"));
    assert_eq!(value(&values, song_tag_fields::ARTIST).as_deref(), Some("Logic"));
    assert_eq!(value(&values, song_tag_fields::YEAR), None);
    assert_eq!(value(&values, song_tag_fields::ISRC), None);
    assert!(read(&dir.path().join("missing.mp3")).is_err());
}

#[test]
fn invalid_says_why_a_change_cannot_be_written() {
    let long = "x".repeat(501);
    let cases: Vec<(Vec<(&str, Option<&str>)>, Option<&str>)> = vec![
        (vec![], Some("No tags were sent to change.")),
        (
            vec![("mood", Some("happy"))],
            Some("\"mood\" is not a tag this server changes."),
        ),
        (
            vec![("Title", Some(long.as_str()))],
            Some("The title is too long."),
        ),
        (
            vec![("albumartist", Some("a\u{7}b"))],
            Some("The album artist has characters a tag cannot hold."),
        ),
        (
            vec![("title", Some("  "))],
            Some("A song needs its title, so it cannot be left empty."),
        ),
        (
            vec![("album", None)],
            Some("A song needs its album, so it cannot be left empty."),
        ),
        (vec![("genre", None), ("isrc", Some(""))], None),
        (vec![("year", Some("999"))], Some("\"999\" is not a year.")),
        (vec![("year", Some("+2001"))], Some("\"+2001\" is not a year.")),
        (vec![("year", Some(" 2001 "))], None),
        (vec![("track", Some("0"))], Some("\"0\" is not a track number.")),
        (
            vec![("disc", Some("1000"))],
            Some("\"1000\" is not a disc number."),
        ),
        (vec![("track", Some("12")), ("disc", Some("2"))], None),
        (vec![("isrc", Some("nope"))], Some("\"nope\" is not an ISRC.")),
        (vec![("ISRC", Some("us-rc1-17-00001"))], None),
    ];
    for (pairs, expected) in cases {
        assert_eq!(invalid(&changes(&pairs)).as_deref(), expected, "{pairs:?}");
    }
}

#[test]
fn write_changes_only_what_differs_and_says_before_and_after() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = song(dir.path(), "a.mp3", "Logic", "America", "Everybody");

    let result = write(
        &path,
        &changes(&[
            ("TITLE", Some(" America ")),
            ("year", Some("2017")),
            ("genre", Some("Hip-Hop; Rap")),
            ("isrc", Some("us-rc1-17-00001")),
        ]),
    );

    assert!(result.changed, "{result:?}");
    assert_eq!(result.error, None);
    assert_eq!(value(&result.before, song_tag_fields::YEAR), None);
    assert_eq!(
        value(&result.after, song_tag_fields::YEAR).as_deref(),
        Some("2017")
    );
    assert_eq!(
        value(&result.after, song_tag_fields::ISRC).as_deref(),
        Some("USRC11700001")
    );
    let now = read(&path).expect("reads");
    assert_eq!(
        value(&now, song_tag_fields::GENRE).as_deref(),
        Some("Hip-Hop; Rap")
    );
    assert_eq!(
        value(&now, song_tag_fields::ISRC).as_deref(),
        Some("USRC11700001")
    );
    assert_eq!(value(&now, song_tag_fields::TITLE).as_deref(), Some("America"));

    let same = write(&path, &changes(&[("year", Some("2017"))]));
    assert!(!same.changed);
    assert_eq!(same.before, same.after);
}

/// Clearing an ISRC takes it off the file, so the undo of an ISRC retag leaves no code behind.
#[test]
fn clearing_an_isrc_takes_it_off_the_file() {
    let dir = tempfile::tempdir().expect("a temp dir");
    for name in ["a.mp3", "a.flac"] {
        let path = song(dir.path(), name, "Logic", "America", "Everybody");
        assert!(write(&path, &changes(&[("isrc", Some("USRC11700001"))])).changed);

        let cleared = write(&path, &changes(&[("isrc", None)]));

        assert!(cleared.changed, "{name}");
        assert_eq!(value(&cleared.after, song_tag_fields::ISRC), None, "{name}");
        let file = TagFile::open(&path).expect("opens");
        assert_eq!(extras::read_text(&file, TagFields::ISRC), None, "{name}");
        assert_eq!(file.isrc(), None, "{name}");
    }
}

const RECORDING: &str = "28982738-0000-4000-8000-000000000001";
const TRACK: &str = "7a7a7a7a-0000-4000-8000-000000000005";
const RELEASE: &str = "89e29cc2-0000-4000-8000-000000000004";

#[test]
fn the_musicbrainz_ids_are_written_and_cleared_and_put_back_on_both_kinds_of_file() {
    let dir = tempfile::tempdir().expect("a temp dir");
    for name in ["x.mp3", "x.flac"] {
        let path = song(
            dir.path(),
            name,
            "My Chemical Romance",
            "Welcome to the Black Parade",
            "The Black Parade",
        );
        {
            let mut file = TagFile::open(&path).expect("opens");
            extras::set_release_track_id(&mut file, Some(TRACK));
            extras::set_text(&mut file, TagFields::ALBUM_ID, Some(RELEASE));
            extras::set_text(&mut file, TagFields::RELEASE_GROUP_ID, Some(RELEASE));
            file.save().expect("saved");
        }

        let written = write(
            &path,
            &changes(&[
                ("recordingId", Some(RECORDING)),
                ("releaseTrackId", Some("")),
                ("releaseId", None),
                ("releaseGroupId", None),
            ]),
        );
        assert!(written.changed, "{name}");
        // The ids an edit names are read before it, so its undo puts them back.
        assert_eq!(value(&written.before, "recordingId"), None, "{name}");
        assert_eq!(
            value(&written.before, "releaseTrackId").as_deref(),
            Some(TRACK),
            "{name}"
        );
        assert_eq!(
            value(&written.after, "recordingId").as_deref(),
            Some(RECORDING),
            "{name}"
        );
        let mut file = TagFile::open(&path).expect("opens");
        assert_eq!(
            extras::read_recording_id(&mut file).as_deref(),
            Some(RECORDING),
            "{name}"
        );
        for field in ["releaseTrackId", "releaseId", "releaseGroupId"] {
            assert_eq!(read_identity(&mut file, field), None, "{name} {field}");
        }
        // The tags the apps list are the same nine.
        assert!(!read(&path).expect("reads").contains_key("recordingId"));

        let back: IndexMap<String, Option<String>> = written.before.clone();
        assert!(write(&path, &back).changed, "{name}");
        let mut file = TagFile::open(&path).expect("opens");
        assert_eq!(extras::read_recording_id(&mut file), None, "{name}");
        assert_eq!(
            read_identity(&mut file, "releaseTrackId").as_deref(),
            Some(TRACK),
            "{name}"
        );
        assert_eq!(
            read_identity(&mut file, "releaseId").as_deref(),
            Some(RELEASE),
            "{name}"
        );
        assert_eq!(
            read_identity(&mut file, "releaseGroupId").as_deref(),
            Some(RELEASE),
            "{name}"
        );
    }
}

#[test]
fn an_id_that_is_not_a_musicbrainz_id_is_refused_and_a_blank_one_clears() {
    assert_eq!(
        invalid(&changes(&[("recordingId", Some("rec-studio"))])).as_deref(),
        Some("\"rec-studio\" is not a MusicBrainz id.")
    );
    assert_eq!(invalid(&changes(&[("RELEASEID", Some(RELEASE))])), None);
    assert_eq!(invalid(&changes(&[("releaseTrackId", Some(""))])), None);
    assert_eq!(song_tag_fields::canonical("RecordingID"), Some("recordingId"));
    assert_eq!(words("releaseGroupId"), "release group id");
}

#[test]
fn a_file_that_cannot_be_read_fails_in_plain_words() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "hello").expect("written");
    assert_eq!(
        write(&text, &changes(&[("year", Some("2001"))])).error.as_deref(),
        Some("this kind of file cannot hold tags the server can write")
    );
    let missing = write(
        &dir.path().join("missing.mp3"),
        &changes(&[("year", Some("2001"))]),
    );
    assert!(
        missing
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("the file could not be written (")),
        "{missing:?}"
    );
}

#[test]
fn join_album_takes_the_leads_album_and_year_and_restore_album_puts_it_back() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let lead = song(dir.path(), "lead.mp3", "Queen", "Mustapha", "Jazz");
    let target = song(dir.path(), "target.mp3", "Queen", "Bicycle Race", "Jazz (Disc 2)");
    {
        let mut file = TagFile::open(&lead).expect("opens");
        file.set_year(1978);
        file.set_album_artists(&["Queen".to_string()]);
        file.save().expect("saved");
    }
    let before = KeptIdentityTags::read(&target, None).expect("reads");

    let joined = join_album(&target, &lead, None, None);

    assert!(joined.changed, "{joined:?}");
    assert_eq!(
        value(&joined.before, song_tag_fields::ALBUM).as_deref(),
        Some("Jazz (Disc 2)")
    );
    assert_eq!(
        value(&joined.after, song_tag_fields::ALBUM).as_deref(),
        Some("Jazz")
    );
    assert_eq!(
        value(&joined.after, song_tag_fields::YEAR).as_deref(),
        Some("1978")
    );
    assert_eq!(
        value(&joined.after, song_tag_fields::TITLE).as_deref(),
        Some("Bicycle Race")
    );
    assert!(!join_album(&target, &lead, None, None).changed);

    let restored = restore_album(&target, &before, None);

    assert!(restored.changed, "{restored:?}");
    let now = read(&target).expect("reads");
    assert_eq!(
        value(&now, song_tag_fields::ALBUM).as_deref(),
        Some("Jazz (Disc 2)")
    );
    assert_eq!(value(&now, song_tag_fields::YEAR), None);
    assert_eq!(
        restore_album(&target, &before, Some("19x8")).error.as_deref(),
        Some("The input string '19x8' was not in a correct format.")
    );
}

#[test]
fn add_cover_fills_a_gap_only_and_remove_cover_takes_it_out() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = song(dir.path(), "a.flac", "Logic", "America", "Everybody");
    let mut jpeg = Vec::new();
    image::DynamicImage::new_rgb8(8, 8)
        .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
        .expect("encodes");
    assert!(!has_picture(&path));

    assert!(add_cover(&path, &jpeg).changed);
    assert!(has_picture(&path));
    let pictures = TagFile::open(&path).expect("opens").pictures();
    assert_eq!(pictures[0].mime_type, "image/jpeg");
    assert_eq!(pictures[0].description, "Cover");
    assert!(!add_cover(&path, &jpeg).changed);

    assert!(remove_cover(&path).changed);
    assert!(!has_picture(&path));
    assert!(!remove_cover(&path).changed);
    assert!(!has_picture(&dir.path().join("missing.flac")));
}

fn entry(id: &str, navidrome_id: &str, run_id: Option<&str>) -> TagEditEntry {
    TagEditEntry {
        id: id.into(),
        navidrome_id: navidrome_id.into(),
        kind: tag_edit_kinds::RETAG.into(),
        run_id: run_id.map(str::to_string),
        ..TagEditEntry::default()
    }
}

#[test]
fn the_journal_keeps_edits_newest_last_and_marks_them_undone() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("config").join("tag-edits.json");
    let journal = TagEditJournal::new(Some(path.clone()));

    assert!(journal.record(entry("1", "song-a", None), true));
    assert!(journal.record(entry("2", "song-a", Some("run")), true));
    assert!(journal.record(entry("3", "song-b", Some("run")), true));

    assert_eq!(journal.last("song-a").map(|e| e.id), Some("2".into()));
    assert_eq!(
        journal.recent(2).into_iter().map(|e| e.id).collect::<Vec<_>>(),
        ["3", "2"]
    );
    assert_eq!(
        journal
            .of_run("run")
            .into_iter()
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        ["2", "3"]
    );
    assert!(journal.mark_undone("2", true));
    assert!(!journal.mark_undone("nope", true));
    assert_eq!(journal.last("song-a").map(|e| e.id), Some("1".into()));
    assert_eq!(
        journal
            .of_run("run")
            .into_iter()
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        ["3"]
    );

    let again = TagEditJournal::new(Some(path));
    assert_eq!(again.recent(10).len(), 3);
    assert_eq!(again.last("song-a").map(|e| e.id), Some("1".into()));
}

#[test]
fn a_run_records_without_saving_until_it_flushes() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("tag-edits.json");
    let journal = TagEditJournal::new(Some(path.clone()));

    assert!(journal.record(entry("1", "song-a", Some("run")), false));
    assert!(!path.exists());
    assert!(journal.mark_undone("1", false));
    assert!(!path.exists());
    assert!(journal.flush());
    let kept = load(&path);
    assert_eq!(kept.len(), 1);
    assert!(kept[0].undone);
}

#[test]
fn the_journal_is_bounded_and_an_in_memory_one_always_keeps() {
    let journal = TagEditJournal::new(None);
    for at in 0..MAX_ENTRIES + 5 {
        assert!(journal.record(entry(&at.to_string(), "song", None), false));
    }
    let recent = journal.recent(usize::MAX);
    assert_eq!(recent.len(), MAX_ENTRIES);
    assert_eq!(recent.last().map(|e| e.id.as_str()), Some("5"));
    assert!(journal.flush());
}

/// A FLAC whose album artist is `own`, written again under the other names given.
fn spelled(dir: &Path, own: &str, others: &[(&str, &str)]) -> PathBuf {
    let path = song(dir, "a.flac", "Miki", "industry plant", "industry plant");
    let mut file = TagFile::open(&path).expect("opens");
    file.set_album_artists(&[own.to_string()]);
    for (name, value) in others {
        extras::set_multi_value(&mut file, name, &[value.to_string()]);
    }
    file.save().expect("saved");
    path
}

/// The album artist fields as the library server reads them.
fn album_artists_read(path: &Path) -> Vec<(String, Vec<String>)> {
    let view = KeptIdentityTags::navidrome_view(&TagFile::open(path).expect("opens"));
    ["albumartist", "album artist", "album_artist", "albumartists"]
        .into_iter()
        .filter_map(|key| Some((key.to_string(), view.get(key)?.clone())))
        .collect()
}

fn read_as(pairs: &[(&str, &str)]) -> Vec<(String, Vec<String>)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), vec![value.to_string()]))
        .collect()
}

#[test]
fn an_album_artist_edit_takes_its_other_names_off_and_the_undo_puts_them_back() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = spelled(
        dir.path(),
        "Miki",
        &[("ALBUM ARTIST", "Miki"), ("ALBUMARTISTS", "Miki")],
    );

    let result = write(&path, &changes(&[("albumArtist", Some("miki"))]));
    assert!(result.changed, "{:?}", result.error);
    assert!(value(&result.before, "albumArtistNames").is_some());
    assert_eq!(result.after.get("albumArtistNames"), Some(&None));
    assert_eq!(album_artists_read(&path), read_as(&[("albumartist", "miki")]));

    // The undo writes back what the edit changed, as the edit service does.
    let back: IndexMap<String, Option<String>> = result
        .before
        .iter()
        .filter(|(key, value)| result.after.get(*key).cloned().flatten() != **value)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert!(write(&path, &back).changed);
    assert_eq!(
        album_artists_read(&path),
        read_as(&[
            ("albumartist", "Miki"),
            ("album artist", "Miki"),
            ("albumartists", "Miki")
        ])
    );
}

#[test]
fn the_album_artists_other_names_alone_come_off_and_a_file_without_them_is_left_alone() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = spelled(dir.path(), "Ye", &[("ALBUM ARTIST", "Kanye West")]);
    let take_off = changes(&[("albumArtistNames", None)]);

    let result = write(&path, &take_off);
    assert!(result.changed, "{:?}", result.error);
    assert_eq!(album_artists_read(&path), read_as(&[("albumartist", "Ye")]));

    let again = write(&path, &take_off);
    assert!(!again.changed);
    assert!(!again.before.contains_key("albumArtistNames"));
    // A plain edit of a file without them says nothing of them either.
    let edit = write(&path, &changes(&[("title", Some("Stronger"))]));
    assert!(edit.changed);
    assert!(!edit.before.contains_key("albumArtistNames"));
}

#[test]
fn the_album_artists_other_names_can_only_be_taken_off() {
    assert!(invalid(&changes(&[("albumArtistNames", Some("x"))])).is_some());
    assert_eq!(invalid(&changes(&[("albumArtistNames", None)])), None);
    assert_eq!(words("albumArtistNames"), "album artist's other names");
}

/// A FLAC on track `own`, with the track number written again under the other names given.
fn numbered(dir: &Path, own: u32, others: &[(&str, &str)]) -> PathBuf {
    let path = song(dir, "a.flac", "Dua Lipa", "Don't Start Now", "Future Nostalgia");
    let mut file = TagFile::open(&path).expect("opens");
    file.set_track(own);
    for (name, value) in others {
        extras::set_multi_value(&mut file, name, &[value.to_string()]);
    }
    file.save().expect("saved");
    path
}

/// The track number fields as the library server reads them.
fn track_numbers_read(path: &Path) -> Vec<(String, Vec<String>)> {
    let view = KeptIdentityTags::navidrome_view(&TagFile::open(path).expect("opens"));
    ["track", "tracknumber"]
        .into_iter()
        .filter_map(|key| Some((key.to_string(), view.get(key)?.clone())))
        .collect()
}

#[test]
fn a_track_edit_takes_its_other_names_off_and_the_undo_puts_them_back() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = numbered(dir.path(), 2, &[("TRACK", "3")]);

    let result = write(&path, &changes(&[("track", Some("4"))]));
    assert!(result.changed, "{:?}", result.error);
    assert!(value(&result.before, "otherNames").is_some());
    assert_eq!(result.after.get("otherNames"), Some(&None));
    assert_eq!(track_numbers_read(&path), read_as(&[("tracknumber", "04")]));

    let back: IndexMap<String, Option<String>> = result
        .before
        .iter()
        .filter(|(key, value)| result.after.get(*key).cloned().flatten() != **value)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert!(write(&path, &back).changed);
    assert_eq!(
        track_numbers_read(&path),
        read_as(&[("track", "3"), ("tracknumber", "02")])
    );
}

#[test]
fn other_names_alone_come_off_and_a_file_without_them_is_left_alone() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = numbered(dir.path(), 2, &[("TRACK", "3")]);
    let take_off = changes(&[("otherNames", None)]);

    let result = write(&path, &take_off);
    assert!(result.changed, "{:?}", result.error);
    assert_eq!(track_numbers_read(&path), read_as(&[("tracknumber", "02")]));

    let again = write(&path, &take_off);
    assert!(!again.changed);
    assert!(!again.before.contains_key("otherNames"));
    let edit = write(&path, &changes(&[("title", Some("Levitating"))]));
    assert!(edit.changed);
    assert!(!edit.before.contains_key("otherNames"));
}

#[test]
fn other_names_can_only_be_taken_off() {
    assert!(invalid(&changes(&[("otherNames", Some("x"))])).is_some());
    assert_eq!(invalid(&changes(&[("otherNames", None)])), None);
    assert_eq!(words("otherNames"), "tags' other names");
}
