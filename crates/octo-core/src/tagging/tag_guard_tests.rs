//! The guard on the cases that reached the library: a studio file filed under a promo of
//! instrumentals with a release track of that promo, a vocal file under "(Instrumentals)", and
//! stray spaces.

use super::*;

fn identity(recording: &str, kind: RecordingKind, by_length: bool) -> Identity {
    Identity {
        recording_id: recording.into(),
        kind,
        by_length,
        kind_certain: true,
    }
}

#[test]
fn a_take_that_is_only_likely_changes_nothing() {
    let mut song = parade_as_written();
    let unsure = Identity {
        kind_certain: false,
        ..identity("rec-studio", RecordingKind::Studio, false)
    };
    guard(&mut song, Some(&unsure), None);
    assert_eq!(song.album, "The Black Parade (instrumentals)");

    let mut live = Song {
        title: "Live Forever".into(),
        album: "Definitely Maybe".into(),
        ..Default::default()
    };
    let unsure_live = Identity {
        kind_certain: false,
        ..identity("rec", RecordingKind::Live, false)
    };
    guard(&mut live, Some(&unsure_live), None);
    assert_eq!(live.title, "Live Forever");
}

#[test]
fn only_the_bracket_naming_the_take_is_cut() {
    assert_eq!(
        plain_album(
            "(What's the Story) Morning Glory? (Instrumentals)",
            RecordingKind::Instrumental
        )
        .as_deref(),
        Some("(What's the Story) Morning Glory?")
    );
    assert_eq!(
        plain_album("Songs (For Lovers) [Karaoke]", RecordingKind::Karaoke).as_deref(),
        Some("Songs (For Lovers)")
    );
    assert_eq!(
        plain_album("The Black Parade - Instrumentals", RecordingKind::Instrumental).as_deref(),
        Some("The Black Parade")
    );
    assert_eq!(plain_album("Instrumentals", RecordingKind::Instrumental), None);
    assert_eq!(plain_album("Mezzanine", RecordingKind::Instrumental), None);
}

#[test]
fn a_remix_titled_as_a_mix_is_not_titled_again() {
    let mut song = Song {
        title: "Song (Skrillex Mix)".into(),
        ..Default::default()
    };
    guard(
        &mut song,
        Some(&identity("rec", RecordingKind::Remix, true)),
        None,
    );
    assert_eq!(song.title, "Song (Skrillex Mix)");
}

fn parade_as_written() -> Song {
    Song {
        title: "Welcome to the Black Parade".into(),
        artist: "My Chemical Romance".into(),
        album: "The Black Parade (instrumentals)".into(),
        music_brainz_recording_id: Some("rec-studio".into()),
        music_brainz_release_track_id: Some("track-on-promo".into()),
        music_brainz_release_id: Some("r-promo".into()),
        ..Default::default()
    }
}

fn promo_candidate() -> ReleaseCandidate {
    ReleaseCandidate {
        recording_id: Some("rec-instrumental".into()),
        release_id: Some("r-promo".into()),
        ..Default::default()
    }
}

#[test]
fn a_studio_file_never_keeps_another_takes_album_or_release_track() {
    let mut song = parade_as_written();
    let found = guard(
        &mut song,
        Some(&identity("rec-studio", RecordingKind::Studio, true)),
        Some(&promo_candidate()),
    );
    assert_eq!(song.album, "The Black Parade");
    assert_eq!(song.title, "Welcome to the Black Parade");
    assert_eq!(song.music_brainz_recording_id.as_deref(), Some("rec-studio"));
    assert_eq!(song.music_brainz_release_track_id, None);
    assert_eq!(song.music_brainz_release_id, None);
    assert!(found.iter().all(|f| f.fixed));
    assert!(found.iter().any(|f| f.rule == "one recording"));
    assert!(found.iter().any(|f| f.rule == "album agrees"));
}

#[test]
fn a_vocal_file_under_instrumentals_gets_the_plain_album() {
    let mut song = Song {
        title: "So What (feat. Popcaan)".into(),
        album: "A Muse In Her Feelings (Instrumentals)".into(),
        ..Default::default()
    };
    guard(
        &mut song,
        Some(&identity("rec", RecordingKind::Studio, false)),
        None,
    );
    assert_eq!(song.album, "A Muse In Her Feelings");
}

#[test]
fn a_live_take_says_so_in_its_title_and_keeps_its_live_album() {
    let mut song = Song {
        title: "Welcome to the Black Parade".into(),
        album: "The Black Parade Is Dead!".into(),
        music_brainz_recording_id: Some("rec-live".into()),
        ..Default::default()
    };
    guard(
        &mut song,
        Some(&identity("rec-live", RecordingKind::Live, true)),
        None,
    );
    assert_eq!(song.title, "Welcome to the Black Parade (Live)");
    assert_eq!(song.album, "The Black Parade Is Dead!");
}

#[test]
fn an_instrumental_says_so_once() {
    let mut song = Song {
        title: "So What (Instrumental)".into(),
        album: "A Muse In Her Feelings (Instrumentals)".into(),
        ..Default::default()
    };
    let found = guard(
        &mut song,
        Some(&identity("rec", RecordingKind::Instrumental, true)),
        None,
    );
    assert_eq!(song.title, "So What (Instrumental)");
    assert_eq!(song.album, "A Muse In Her Feelings (Instrumentals)");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn another_recording_of_the_same_take_may_stand_for_the_audio() {
    let mut song = Song {
        title: "Teardrop".into(),
        album: "Mezzanine".into(),
        music_brainz_recording_id: Some("rec-compilation".into()),
        music_brainz_release_id: Some("r-mezz".into()),
        ..Default::default()
    };
    let chosen = ReleaseCandidate {
        recording_id: Some("rec-compilation".into()),
        ..Default::default()
    };
    let found = guard(
        &mut song,
        Some(&identity("rec-album", RecordingKind::Studio, false)),
        Some(&chosen),
    );
    assert_eq!(song.music_brainz_recording_id.as_deref(), Some("rec-compilation"));
    assert_eq!(song.music_brainz_release_id.as_deref(), Some("r-mezz"));
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn stray_spaces_go() {
    let mut song = Song {
        title: "Uneasy Hearts Weigh The Most ".into(),
        album: "Psychonaut ".into(),
        ..Default::default()
    };
    guard(&mut song, None, None);
    assert_eq!(song.title, "Uneasy Hearts Weigh The Most");
    assert_eq!(song.album, "Psychonaut");
}

#[test]
fn with_nothing_confirmed_an_album_naming_a_take_is_only_reported() {
    let mut song = Song {
        title: "Bad Things".into(),
        album: "Instrumental Playback Hits - Karaoke Remix Playlist 2017.1".into(),
        ..Default::default()
    };
    let found = guard(&mut song, None, None);
    assert_eq!(
        song.album,
        "Instrumental Playback Hits - Karaoke Remix Playlist 2017.1"
    );
    assert!(
        found.iter().any(|f| !f.fixed && f.rule == "album agrees"),
        "{found:?}"
    );
}

#[test]
fn a_title_that_asks_for_the_take_keeps_its_album() {
    let mut song = Song {
        title: "Smile in Your Sleep (Live)".into(),
        album: "Decade (live at the El Mocambo)".into(),
        ..Default::default()
    };
    let found = guard(&mut song, Some(&identity("rec", RecordingKind::Live, true)), None);
    assert_eq!(song.album, "Decade (live at the El Mocambo)");
    assert!(found.is_empty(), "{found:?}");
}
