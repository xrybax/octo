//! The audit on the cases found in the library: a studio file on a promo of instrumentals with
//! that promo's release track, a vocal file under "(Instrumentals)", a live take titled plainly
//! beside its studio copy, and stray spaces.

use super::*;

fn identity(recording: &str, kind: RecordingKind, by_length: bool) -> Identity {
    Identity {
        recording_id: recording.into(),
        kind,
        by_length,
        kind_certain: true,
    }
}

fn heard(recording: &str, kind: RecordingKind, by_length: bool, sure: bool) -> HeardAs {
    HeardAs {
        identity: Identity {
            kind_certain: by_length || sure,
            ..identity(recording, kind, by_length)
        },
        sure,
        file_ms: Some(311_106),
        written: None,
        track_owner: None,
        on_release: None,
    }
}

/// A recording the fingerprint names, of this take.
fn listed(kind: RecordingKind) -> NamedRecording {
    NamedRecording {
        listed: true,
        kind: Some(kind),
        length_ms: None,
    }
}

/// A recording only the music database knows, of this take and length.
fn unlisted(kind: RecordingKind, length_ms: i64) -> NamedRecording {
    NamedRecording {
        listed: false,
        kind: Some(kind),
        length_ms: Some(length_ms),
    }
}

fn change<'a>(audit: &'a TagAudit, field: &str) -> Option<Option<&'a str>> {
    audit.changes.get(field).map(|value| value.as_deref())
}

fn rules(audit: &TagAudit) -> Vec<AuditRule> {
    audit.findings.iter().map(|finding| finding.rule).collect()
}

fn parade() -> FileTags {
    FileTags {
        title: "Welcome to the Black Parade".into(),
        album: "The Black Parade (instrumentals)".into(),
        recording_id: Some("rec-studio".into()),
        release_track_id: Some("track-on-promo".into()),
        release_id: Some("r-promo".into()),
        release_group_id: Some("g-parade".into()),
        other_album_artists: None,
        other_names: Vec::new(),
    }
}

#[test]
fn a_studio_file_on_a_promo_of_instrumentals_gets_the_plain_album_and_loses_the_promos_ids() {
    let mut studio = heard("rec-studio", RecordingKind::Studio, true, true);
    studio.track_owner = Some(("rec-instrumental".into(), listed(RecordingKind::Instrumental)));
    studio.on_release = Some(None);
    let found = audit(&parade(), Some(&studio));

    assert_eq!(rules(&found), [AuditRule::Album, AuditRule::Ids]);
    assert!(found.sure);
    assert_eq!(change(&found, "album"), Some(Some("The Black Parade")));
    assert_eq!(change(&found, "title"), None);
    // The recording id already names the audio.
    assert_eq!(change(&found, "recordingId"), None);
    assert_eq!(change(&found, "releaseTrackId"), Some(None));
    assert_eq!(change(&found, "releaseId"), Some(None));
    assert_eq!(change(&found, "releaseGroupId"), Some(None));
    let ids = &found.findings[1].words;
    assert!(
        ids.contains("track-on-promo") && ids.contains("an instrumental"),
        "{ids}"
    );
}

/// The Black Parade with its plain album, still carrying the promo's release track.
fn parade_on_its_album() -> FileTags {
    FileTags {
        album: "The Black Parade".into(),
        release_id: Some("r-parade".into()),
        ..parade()
    }
}

#[test]
fn a_release_that_lists_the_audios_recording_keeps_its_ids_and_names_the_right_track() {
    let mut studio = heard("rec-studio", RecordingKind::Studio, true, true);
    studio.track_owner = Some(("rec-instrumental".into(), listed(RecordingKind::Instrumental)));
    studio.on_release = Some(Some("track-studio".into()));
    let found = audit(&parade_on_its_album(), Some(&studio));
    assert_eq!(change(&found, "releaseTrackId"), Some(Some("track-studio")));
    assert_eq!(change(&found, "releaseId"), None);
    assert_eq!(change(&found, "releaseGroupId"), None);
}

#[test]
fn when_the_release_is_not_known_only_the_wrong_track_comes_off() {
    let mut studio = heard("rec-studio", RecordingKind::Studio, true, true);
    studio.track_owner = Some(("rec-instrumental".into(), listed(RecordingKind::Instrumental)));
    let found = audit(&parade_on_its_album(), Some(&studio));
    assert_eq!(change(&found, "releaseTrackId"), Some(None));
    assert_eq!(change(&found, "releaseId"), None);
    assert_eq!(change(&found, "releaseGroupId"), None);

    // A release the audio is not on keeps its release and group too: they file the song on its
    // album. Only the track that is another recording's comes off.
    studio.on_release = Some(None);
    let found = audit(&parade_on_its_album(), Some(&studio));
    assert_eq!(change(&found, "releaseTrackId"), Some(None));
    assert_eq!(change(&found, "releaseId"), None);
    assert_eq!(change(&found, "releaseGroupId"), None);
}

#[test]
fn a_vocal_song_under_instrumentals_gets_its_plain_album_and_its_recording_id() {
    let tags = FileTags {
        title: "So What (feat. Popcaan)".into(),
        album: "A Muse In Her Feelings (Instrumentals)".into(),
        ..Default::default()
    };
    let found = audit(
        &tags,
        Some(&heard("rec-so-what", RecordingKind::Studio, false, true)),
    );
    assert_eq!(rules(&found), [AuditRule::Album]);
    assert_eq!(change(&found, "album"), Some(Some("A Muse In Her Feelings")));
    assert_eq!(change(&found, "recordingId"), Some(Some("rec-so-what")));
    assert_eq!(
        found.findings[0].words,
        "The album 'A Muse In Her Feelings (Instrumentals)' names an instrumental, but the audio is the studio recording."
    );
}

#[test]
fn a_live_take_titled_plainly_says_so_in_its_title() {
    let tags = FileTags {
        title: "Uneasy Hearts Weigh The Most".into(),
        album: "Tree City Sessions 2".into(),
        ..Default::default()
    };
    let found = audit(&tags, Some(&heard("rec-live", RecordingKind::Live, true, true)));
    assert_eq!(rules(&found), [AuditRule::Title]);
    assert_eq!(
        found.findings[0].words,
        "The audio is a live recording, and the title does not say so."
    );
    assert_eq!(
        change(&found, "title"),
        Some(Some("Uneasy Hearts Weigh The Most (Live)"))
    );
    assert_eq!(change(&found, "recordingId"), Some(Some("rec-live")));
}

#[test]
fn an_identity_that_is_not_sure_neither_writes_nor_rules_out_the_files_own_take() {
    let mut guess = heard("rec-studio", RecordingKind::Studio, false, false);
    guess.track_owner = Some(("rec-live".into(), listed(RecordingKind::Live)));
    guess.written = Some(listed(RecordingKind::Live));
    let tags = FileTags {
        recording_id: Some("rec-live".into()),
        ..parade()
    };
    let found = audit(&tags, Some(&guess));
    assert!(!found.sure);
    // The studio take was only the likelier pick among the takes the print is tied to, so the
    // live ids the file carries may be as right: nothing is found and the album stays.
    assert!(!found.found(), "{:?}", found.findings);
    assert!(found.changes.is_empty(), "{:?}", found.changes);
}

#[test]
fn an_identity_that_is_not_sure_never_names_the_audios_take() {
    let mut guess = heard("rec-studio", RecordingKind::Studio, false, false);
    // A recording a minute longer than the file is not the audio, whatever take it is.
    guess.written = Some(unlisted(RecordingKind::Studio, 371_000));
    let tags = FileTags {
        recording_id: Some("rec-extended".into()),
        ..parade_on_its_album()
    };
    let found = audit(&tags, Some(&guess));
    assert_eq!(rules(&found), [AuditRule::Ids]);
    assert_eq!(
        found.findings[0].words,
        "The recording id rec-extended is a recording of 6:11, but the audio runs 5:11."
    );
    assert!(!found.findings[0].words.contains("studio"));
    assert!(found.changes.is_empty());
}

#[test]
fn another_takes_recording_id_is_replaced_when_the_length_chose_the_audio() {
    let tags = FileTags {
        title: "Welcome to the Black Parade".into(),
        album: "The Black Parade".into(),
        recording_id: Some("rec-instrumental".into()),
        ..Default::default()
    };
    let mut studio = heard("rec-studio", RecordingKind::Studio, true, true);
    studio.written = Some(listed(RecordingKind::Instrumental));
    let found = audit(&tags, Some(&studio));
    assert_eq!(rules(&found), [AuditRule::Ids]);
    assert!(found.findings[0].words.contains("is an instrumental"));
    assert_eq!(change(&found, "recordingId"), Some(Some("rec-studio")));
}

#[test]
fn another_recording_of_the_same_take_is_left_as_it_is() {
    // An album and a compilation often list the same audio under two recordings.
    let tags = FileTags {
        title: "Teardrop".into(),
        album: "Mezzanine".into(),
        recording_id: Some("rec-compilation".into()),
        ..Default::default()
    };
    let mut studio = heard("rec-album", RecordingKind::Studio, false, true);
    studio.written = Some(listed(RecordingKind::Studio));
    let found = audit(&tags, Some(&studio));
    assert!(!found.found());
    assert!(found.changes.is_empty());
}

#[test]
fn a_recording_id_the_fingerprint_does_not_name_is_unknown_not_wrong() {
    // Studio audio whose exact length chose the AcoustID recording, in a file tagged with the
    // album's own recording of the same audio, which AcoustID does not tie to the print.
    let tags = FileTags {
        title: "Welcome to the Black Parade".into(),
        album: "The Black Parade".into(),
        recording_id: Some("rec-album".into()),
        release_track_id: Some("track-album".into()),
        release_id: Some("r-parade".into()),
        release_group_id: Some("g-parade".into()),
        other_album_artists: None,
        other_names: Vec::new(),
    };
    let mut studio = heard("rec-studio", RecordingKind::Studio, true, true);
    studio.on_release = Some(None);
    let found = audit(&tags, Some(&studio));
    assert!(!found.found(), "{:?}", found.findings);
    assert!(found.changes.is_empty(), "{:?}", found.changes);

    // The music database says it is the same take, at the same length: still nothing.
    studio.written = Some(unlisted(RecordingKind::Studio, 311_000));
    let found = audit(&tags, Some(&studio));
    assert!(!found.found(), "{:?}", found.findings);
    assert!(found.changes.is_empty(), "{:?}", found.changes);
}

#[test]
fn a_recording_id_the_music_database_says_is_another_take_is_replaced_and_the_album_stays_whole() {
    let tags = FileTags {
        title: "Welcome to the Black Parade".into(),
        album: "The Black Parade".into(),
        recording_id: Some("rec-live-elsewhere".into()),
        release_track_id: Some("track-album".into()),
        release_id: Some("r-parade".into()),
        release_group_id: Some("g-parade".into()),
        other_album_artists: None,
        other_names: Vec::new(),
    };
    let mut studio = heard("rec-studio", RecordingKind::Studio, true, true);
    studio.written = Some(unlisted(RecordingKind::Live, 305_000));
    studio.on_release = Some(None);
    let found = audit(&tags, Some(&studio));
    assert_eq!(rules(&found), [AuditRule::Ids]);
    assert_eq!(
        found.findings[0].words,
        "The recording id rec-live-elsewhere is a live recording, but the audio is the studio recording (rec-studio)."
    );
    assert_eq!(change(&found, "recordingId"), Some(Some("rec-studio")));
    // Nothing proves whose the track is, and the release and its group file the song on its
    // album: none of them is touched.
    for field in ["releaseTrackId", "releaseId", "releaseGroupId"] {
        assert_eq!(change(&found, field), None, "{field}");
    }
}

#[test]
fn a_renamed_album_takes_the_other_releases_ids_off() {
    // dvsn "So What", vocal studio audio, filed under the instrumentals release by its ids.
    let tags = FileTags {
        title: "So What".into(),
        album: "A Muse In Her Feelings (Instrumentals)".into(),
        recording_id: None,
        release_track_id: Some("track-on-instrumentals".into()),
        release_id: Some("r-instrumentals".into()),
        release_group_id: Some("g-instrumentals".into()),
        other_album_artists: None,
        other_names: Vec::new(),
    };
    let found = audit(
        &tags,
        Some(&heard("rec-so-what", RecordingKind::Studio, false, true)),
    );
    assert_eq!(rules(&found), [AuditRule::Album]);
    assert_eq!(change(&found, "album"), Some(Some("A Muse In Her Feelings")));
    assert_eq!(change(&found, "releaseId"), Some(None));
    assert_eq!(change(&found, "releaseGroupId"), Some(None));
    assert_eq!(change(&found, "releaseTrackId"), Some(None));
    assert_eq!(change(&found, "recordingId"), Some(Some("rec-so-what")));

    // An album left as it is keeps its ids.
    let fine = FileTags {
        album: "A Muse In Her Feelings".into(),
        ..tags
    };
    let found = audit(
        &fine,
        Some(&heard("rec-so-what", RecordingKind::Studio, false, true)),
    );
    assert_eq!(change(&found, "releaseId"), None);
    assert_eq!(change(&found, "releaseGroupId"), None);
    assert_eq!(change(&found, "releaseTrackId"), None);
}

#[test]
fn a_length_far_from_the_files_is_another_recording_and_a_near_one_is_not() {
    assert!(!length_contradicts(311_000, 311_106));
    assert!(!length_contradicts(305_426, 311_106));
    assert!(length_contradicts(371_000, 311_106));
    // Seven per cent of a long song is more than ten seconds.
    assert!(!length_contradicts(640_000, 600_000));
    assert!(length_contradicts(650_000, 600_000));
    assert!(!length_contradicts(0, 311_106));
}

#[test]
fn stray_spaces_are_found_with_or_without_the_audio() {
    let tags = FileTags {
        title: "Teardrop ".into(),
        album: " Mezzanine".into(),
        ..Default::default()
    };
    let alone = audit(&tags, None);
    assert!(alone.only_spaces());
    assert_eq!(alone.findings.len(), 2);
    assert_eq!(
        alone.findings[0].words,
        "The title 'Teardrop' has stray spaces at its ends."
    );
    assert_eq!(
        alone.findings[1].words,
        "The album 'Mezzanine' has stray spaces at its ends."
    );
    assert_eq!(change(&alone, "title"), Some(Some("Teardrop")));
    assert_eq!(change(&alone, "album"), Some(Some("Mezzanine")));
    assert_eq!(alone.space_changes(&tags), alone.changes);

    let with_audio = audit(&tags, Some(&heard("rec", RecordingKind::Studio, true, true)));
    // The sweep writes only the spaces by itself; the recording id waits for a fix.
    assert!(with_audio.only_spaces());
    assert_eq!(change(&with_audio, "recordingId"), Some(Some("rec")));
    assert_eq!(with_audio.space_changes(&tags).len(), 2);
}

#[test]
fn without_the_audio_an_album_naming_a_take_is_not_a_finding() {
    let tags = FileTags {
        title: "So What".into(),
        album: "A Muse In Her Feelings (Instrumentals)".into(),
        ..Default::default()
    };
    let found = audit(&tags, None);
    assert!(!found.found());
    assert!(found.changes.is_empty());
}

#[test]
fn an_album_with_nothing_plain_left_is_found_but_not_changed() {
    let tags = FileTags {
        title: "So What".into(),
        album: "Instrumentals".into(),
        ..Default::default()
    };
    let found = audit(&tags, Some(&heard("rec", RecordingKind::Studio, true, true)));
    assert_eq!(rules(&found), [AuditRule::Album]);
    assert_eq!(change(&found, "album"), None);
    assert!(found.findings[0].words.ends_with("so it waits for you."));
}

#[test]
fn a_live_copy_beside_the_studio_copy_is_named_and_the_studio_copy_is_not() {
    let copy = |id: &str, title: &str, album: &str, kind| TakeCopy {
        id: id.into(),
        artist: "Dance Gavin Dance".into(),
        title: title.into(),
        album: album.into(),
        kind,
    };
    let found = takes_beside(&[
        copy(
            "studio",
            "Uneasy Hearts Weigh The Most",
            "Dance Gavin Dance",
            RecordingKind::Studio,
        ),
        copy(
            "live",
            "Uneasy Hearts Weigh The Most",
            "Tree City Sessions 2",
            RecordingKind::Live,
        ),
        copy("other", "Lemon Meringue Tie", "Mothership", RecordingKind::Studio),
    ]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, "live");
    assert!(found[0].1.contains("a live recording"), "{}", found[0].1);
    assert!(found[0].1.contains("'Dance Gavin Dance'"), "{}", found[0].1);

    // A live copy whose title says so is a different song, not a hidden one.
    let named = takes_beside(&[
        copy(
            "studio",
            "Uneasy Hearts Weigh The Most",
            "Dance Gavin Dance",
            RecordingKind::Studio,
        ),
        copy(
            "live",
            "Uneasy Hearts Weigh The Most (Live)",
            "Tree City Sessions 2",
            RecordingKind::Live,
        ),
    ]);
    assert!(named.is_empty());
}

#[test]
fn rule_names_read_back() {
    for rule in AuditRule::ALL {
        assert_eq!(AuditRule::parse(rule.name()), Some(rule));
        assert!(!rule.label().is_empty());
    }
    assert_eq!(AuditRule::parse("nope"), None);
}

fn other_names(copies: bool) -> OtherAlbumArtists {
    OtherAlbumArtists {
        album_artist: "miki".into(),
        names: vec![
            "ALBUM ARTIST".into(),
            "ALBUM_ARTIST".into(),
            "ALBUMARTISTS".into(),
        ],
        values: vec![if copies { "Miki" } else { "Kanye West" }.into()],
        copies,
    }
}

#[test]
fn an_album_artist_under_other_names_is_found_with_or_without_the_audio() {
    let tags = FileTags {
        title: "industry plant".into(),
        album: "industry plant".into(),
        other_album_artists: Some(other_names(true)),
        ..Default::default()
    };
    let alone = audit(&tags, None);
    assert_eq!(rules(&alone), [AuditRule::AlbumArtistNames]);
    assert_eq!(
        alone.findings[0].words,
        "The album artist 'miki' is written again under ALBUM ARTIST, ALBUM_ARTIST and ALBUMARTISTS. The library server reads each as another album artist, so this song can sit on an album of its own."
    );
    assert_eq!(change(&alone, "albumArtistNames"), Some(None));
    // Only copies of the album artist: put right without asking, and the audio's sureness holds.
    assert_eq!(alone.automatic_changes(&tags), alone.changes);
    let with_audio = audit(&tags, Some(&heard("rec", RecordingKind::Studio, true, true)));
    assert!(with_audio.sure);
    assert_eq!(with_audio.automatic_changes(&tags).len(), 1);
}

#[test]
fn another_album_artist_under_another_name_waits_for_a_person() {
    let tags = FileTags {
        title: "Stronger".into(),
        album: "Graduation".into(),
        other_album_artists: Some(other_names(false)),
        ..Default::default()
    };
    let found = audit(&tags, Some(&heard("rec", RecordingKind::Studio, true, true)));
    assert_eq!(rules(&found), [AuditRule::AlbumArtistNames]);
    assert!(
        found.findings[0].words.contains("name 'Kanye West'"),
        "{}",
        found.findings[0].words
    );
    assert!(found.findings[0].words.ends_with("Taking them off keeps 'miki'."));
    assert_eq!(change(&found, "albumArtistNames"), Some(None));
    assert!(found.automatic_changes(&tags).is_empty());
    assert!(!found.sure, "never in a fix of the sure ones");
    assert_eq!(
        AuditRule::parse("albumArtistNames"),
        Some(AuditRule::AlbumArtistNames)
    );
}

fn track_names(copies: bool) -> OtherNames {
    OtherNames {
        field: "track number".into(),
        own: "2".into(),
        names: vec!["TRACK".into()],
        values: vec![if copies { "2" } else { "3" }.into()],
        copies,
    }
}

#[test]
fn a_track_number_written_again_under_another_name_is_put_right_without_asking() {
    let tags = FileTags {
        title: "Don't Start Now".into(),
        album: "Future Nostalgia".into(),
        other_names: vec![track_names(true)],
        ..Default::default()
    };
    let alone = audit(&tags, None);
    assert_eq!(rules(&alone), [AuditRule::OtherNames]);
    assert_eq!(
        alone.findings[0].words,
        "The track number '2' is written again under TRACK, which the library server reads as the track number too."
    );
    assert_eq!(change(&alone, "otherNames"), Some(None));
    assert_eq!(alone.automatic_changes(&tags), alone.changes);
    assert_eq!(other_names_alone(&tags).changes, alone.changes);
    assert!(other_names_alone(&tags).sure);
}

#[test]
fn another_track_number_under_another_name_waits_for_a_person() {
    let tags = FileTags {
        title: "Don't Start Now".into(),
        album: "Future Nostalgia".into(),
        other_names: vec![
            track_names(false),
            OtherNames {
                field: "disc number".into(),
                own: "1".into(),
                names: vec!["DISC".into()],
                values: vec!["1".into()],
                copies: true,
            },
        ],
        ..Default::default()
    };
    let found = audit(&tags, Some(&heard("rec", RecordingKind::Studio, true, true)));
    assert_eq!(rules(&found), [AuditRule::OtherNames, AuditRule::OtherNames]);
    assert_eq!(
        found.findings[0].words,
        "Beside the track number '2', TRACK says '3'. The library server reads them as the track number too and can show that instead. Taking them off keeps '2'."
    );
    assert_eq!(change(&found, "otherNames"), Some(None));
    // One field that says something else holds the copies back with it.
    assert!(found.automatic_changes(&tags).is_empty());
    assert!(!found.sure, "never in a fix of the sure ones");
    assert!(!other_names_alone(&tags).sure);
    assert_eq!(AuditRule::parse("otherNames"), Some(AuditRule::OtherNames));
}

#[test]
fn the_album_artist_and_other_fields_held_alone_go_together() {
    let tags = FileTags {
        other_album_artists: Some(other_names(true)),
        other_names: vec![track_names(true)],
        ..Default::default()
    };
    let alone = other_names_alone(&tags);
    assert_eq!(
        rules(&alone),
        [AuditRule::AlbumArtistNames, AuditRule::OtherNames]
    );
    assert!(alone.sure);
    assert_eq!(alone.automatic_changes(&tags).len(), 2);
}
