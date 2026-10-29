//! The recording a file is and the kind of take it is, on real answers: "Welcome to the Black
//! Parade" ties one fingerprint to the studio take (311.106 s), a live take from "The Black
//! Parade Is Dead!" (305.426 s) and the instrumental from a promo CD of instrumentals
//! (309.684 s).

use super::*;

fn release(title: Option<&str>, group: &str, secondary: &[&str]) -> AcoustIdRelease {
    AcoustIdRelease {
        title: title.map(str::to_string),
        group_title: Some(group.to_string()),
        secondary_types: secondary.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn recording(id: &str, title: &str, ms: i64, releases: Vec<AcoustIdRelease>) -> AcoustIdRecording {
    AcoustIdRecording {
        duration_seconds: Some(((ms as f64) / 1000.0).round() as i32),
        duration_ms: Some(ms),
        releases,
        ..AcoustIdRecording::new(id, title, ["My Chemical Romance"], None, None)
    }
}

fn studio() -> AcoustIdRecording {
    recording(
        "28982738",
        "Welcome to the Black Parade",
        311_106,
        vec![
            release(None, "The Black Parade", &[]),
            release(None, "100 Greatest Rock Anthems", &["Compilation"]),
        ],
    )
}

fn live() -> AcoustIdRecording {
    recording(
        "eca90222",
        "Welcome to the Black Parade",
        305_426,
        vec![release(None, "The Black Parade Is Dead!", &["Live"])],
    )
}

fn instrumental() -> AcoustIdRecording {
    recording(
        "3efde39a",
        "Welcome to the Black Parade",
        309_684,
        vec![release(
            Some("The Black Parade (instrumentals)"),
            "The Black Parade",
            &[],
        )],
    )
}

fn plain() -> BTreeSet<RecordingKind> {
    BTreeSet::new()
}

#[test]
fn kinds_come_from_the_title_the_releases_and_the_database() {
    assert_eq!(kind_of(&studio(), None), RecordingKind::Studio);
    assert_eq!(kind_of(&live(), None), RecordingKind::Live);
    // The promo sits in the normal release group; only its own title says instrumentals.
    assert_eq!(kind_of(&instrumental(), None), RecordingKind::Instrumental);
    let titled = recording("59452e6a", "So What (instrumental)", 194_969, vec![]);
    assert_eq!(kind_of(&titled, None), RecordingKind::Instrumental);

    // AcoustID knows nothing, the database's disambiguation does.
    let bare = recording("x", "Welcome to the Black Parade", 309_684, vec![]);
    assert_eq!(kind_of(&bare, None), RecordingKind::Studio);
    let facts = RecordingFacts {
        disambiguation: Some("instrumental".to_string()),
        ..Default::default()
    };
    assert_eq!(kind_of(&bare, Some(&facts)), RecordingKind::Instrumental);
    let live_facts = RecordingFacts {
        disambiguation: Some("live, 2007-04-08: Palacio de los Deportes, Mexico City".to_string()),
        ..Default::default()
    };
    assert_eq!(kind_of(&bare, Some(&live_facts)), RecordingKind::Live);
}

#[test]
fn a_studio_take_on_a_live_album_too_is_still_studio() {
    let mixed = recording(
        "s",
        "Song",
        200_000,
        vec![
            release(None, "Album", &[]),
            release(None, "Live at Wembley", &["Live"]),
        ],
    );
    assert_eq!(kind_of(&mixed, None), RecordingKind::Studio);
}

#[test]
fn the_exact_length_decides_between_takes() {
    let (s, l, i) = (studio(), live(), instrumental());
    let all = [&s, &l, &i];
    // The copy Octo first kept: 305.43 s is the live take, not the instrumental it was tagged as.
    assert_eq!(
        decided_by_length(&all, Some(305_427)).map(|r| r.recording_id.as_str()),
        Some("eca90222")
    );
    // 311.11 s is 1.4 s from the instrumental: a remaster drifts that much, so the length
    // does not decide, and the kind does (see choose).
    assert_eq!(decided_by_length(&all, Some(311_107)), None);
    // Nor does 309.7 s, 1.4 s from the studio take.
    assert_eq!(decided_by_length(&all, Some(309_700)), None);
    assert_eq!(decided_by_length(&all, Some(309_000)), None);
    // Nothing near.
    assert_eq!(decided_by_length(&all, Some(250_000)), None);
    assert_eq!(decided_by_length(&all, None), None);
}

#[test]
fn one_recording_with_a_length_is_no_choice() {
    let s = studio();
    assert_eq!(decided_by_length(&[&s], Some(311_106)), None);
}

#[test]
fn the_same_recording_listed_twice_counts_once() {
    let (a, b, l) = (studio(), studio(), live());
    assert_eq!(
        decided_by_length(&[&a, &b, &l], Some(311_100)).map(|r| r.recording_id.as_str()),
        Some("28982738")
    );
}

#[test]
fn choose_takes_the_length_then_the_kind_asked_for() {
    let (s, l, i) = (studio(), live(), instrumental());
    let all = [&i, &l, &s];
    let kind = |r: &AcoustIdRecording| kind_of(r, None);

    let never = |_: RecordingKind| false;

    let chosen = choose(&all, Some(305_426), &plain(), never, kind).unwrap();
    assert_eq!(chosen.recording.recording_id, "eca90222");
    assert!(chosen.by_length && chosen.kind_certain);

    // No decisive length: a plain request takes the studio take, wherever it is listed, and
    // the take is not certain, since the others could be the file.
    let chosen = choose(&all, Some(311_107), &plain(), never, kind).unwrap();
    assert_eq!(chosen.recording.recording_id, "28982738");
    assert!(!chosen.by_length && !chosen.kind_certain);

    // A title that asks for the instrumental takes the instrumental.
    let asked = RecordingKind::requested_by("Welcome to the Black Parade (Instrumental)");
    let chosen = choose(&all, None, &asked, never, kind).unwrap();
    assert_eq!(chosen.recording.recording_id, "3efde39a");
}

#[test]
fn a_kind_only_allowed_never_wins_over_the_studio_take() {
    // "Live Forever" on "Definitely Maybe": the word lets a live take through, but the studio
    // take is still the one chosen when nothing else decides.
    let mut studio_a = studio();
    studio_a.recording_id = "studio-a".into();
    let mut studio_b = studio();
    studio_b.recording_id = "studio-b".into();
    studio_b.duration_ms = Some(311_600);
    let l = live();
    let all = [&l, &studio_a, &studio_b];
    let kind = |r: &AcoustIdRecording| kind_of(r, None);
    let chosen = choose(&all, Some(311_300), &plain(), |_| false, kind).unwrap();
    assert_eq!(chosen.kind, RecordingKind::Studio);
}

#[test]
fn with_no_studio_take_the_first_one_not_refused_is_chosen() {
    let karaoke = recording("kara", "Song (Karaoke Version)", 200_000, vec![]);
    let l = live();
    let all = [&karaoke, &l];
    let kind = |r: &AcoustIdRecording| kind_of(r, None);
    let refuses = |k: RecordingKind| k == RecordingKind::Karaoke;
    let chosen = choose(&all, None, &plain(), refuses, kind).unwrap();
    assert_eq!(chosen.kind, RecordingKind::Live);
}

#[test]
fn a_recording_with_no_length_stops_the_length_deciding() {
    let mut s = studio();
    s.duration_ms = None;
    let (i, l) = (instrumental(), live());
    assert_eq!(decided_by_length(&[&s, &i, &l], Some(309_700)), None);
}

#[test]
fn a_remix_release_does_not_make_its_original_mix_a_remix() {
    let original = recording(
        "orig",
        "Track (Original Mix)",
        400_000,
        vec![release(Some("Track (Remixes)"), "Track (Remixes)", &["Remix"])],
    );
    assert_eq!(kind_of(&original, None), RecordingKind::Studio);
    // A demo collection does not make its songs demos either.
    let demo_tape = recording(
        "d",
        "Song",
        200_000,
        vec![release(
            Some("Dark Lane Demo Tapes"),
            "Dark Lane Demo Tapes",
            &["Mixtape/Street", "Demo"],
        )],
    );
    assert_eq!(kind_of(&demo_tape, None), RecordingKind::Studio);
}

#[test]
fn an_album_is_never_a_wish() {
    assert!(RecordingKind::requested_by("Welcome to the Black Parade").is_empty());
    assert_eq!(
        RecordingKind::requested_by("Smile in Your Sleep (Live)"),
        BTreeSet::from([RecordingKind::Live])
    );
    assert_eq!(
        RecordingKind::requested_by("Bad Things (Karaoke Version)"),
        BTreeSet::from([RecordingKind::Karaoke])
    );
}

#[test]
fn unwanted_kinds() {
    assert!(!unwanted(RecordingKind::Studio, &plain()));
    assert!(unwanted(RecordingKind::Instrumental, &plain()));
    assert!(unwanted(RecordingKind::Live, &plain()));
    assert!(!unwanted(
        RecordingKind::Live,
        &BTreeSet::from([RecordingKind::Live])
    ));
}

#[test]
fn album_names_read_as_kinds() {
    assert_eq!(
        RecordingKind::named_in("The Black Parade (instrumentals)"),
        BTreeSet::from([RecordingKind::Instrumental])
    );
    assert_eq!(
        RecordingKind::named_in("A Muse In Her Feelings (Instrumentals)"),
        BTreeSet::from([RecordingKind::Instrumental])
    );
    assert!(RecordingKind::named_in("Tree City Sessions 2").is_empty());
    // An album that is only called so is not a demo: the kind comes from the recording.
    assert!(RecordingKind::named_in("Dark Lane Demo Tapes").is_empty());
}

#[test]
fn covers_are_a_take_of_their_own() {
    assert_eq!(RecordingKind::of_marker("cover"), Some(RecordingKind::Cover));
    assert_eq!(RecordingKind::Cover.title_word(), Some("Cover"));
    assert_eq!(RecordingKind::Cover.described(), "a cover");
    for title in [
        "Song (Cover)",
        "Song (Acoustic Cover)",
        "Song - Cover Version",
        "Song (Tribute to Adele)",
    ] {
        assert_eq!(
            RecordingKind::requested_by(title),
            BTreeSet::from([RecordingKind::Cover]),
            "{title}"
        );
    }
    for title in [
        "Cover Me",
        "Covered in Blood",
        "Under Cover of Darkness",
        "Undercover",
    ] {
        assert!(RecordingKind::named_in(title).is_empty(), "{title}");
    }
    assert_eq!(
        RecordingKind::named_in("Under the Covers"),
        BTreeSet::from([RecordingKind::Cover])
    );

    let titled = recording(
        "c",
        "Welcome to the Black Parade (Acoustic Cover)",
        300_000,
        vec![],
    );
    assert_eq!(kind_of(&titled, None), RecordingKind::Cover);
    let bare = recording("x", "Welcome to the Black Parade", 300_000, vec![]);
    let facts = RecordingFacts {
        disambiguation: Some("Adele cover".to_string()),
        ..Default::default()
    };
    assert_eq!(kind_of(&bare, Some(&facts)), RecordingKind::Cover);
    // A karaoke take is a backing track first, whatever else it is.
    let both = recording("k", "Hello (Karaoke Cover Version)", 300_000, vec![]);
    assert_eq!(kind_of(&both, None), RecordingKind::Karaoke);
    assert!(unwanted(RecordingKind::Cover, &plain()));
}

#[test]
fn a_song_called_karaoke_or_instrumental_is_still_a_studio_take() {
    // Catfish and the Bottlemen "Karaoke Queen", Drake "Karaoke": a word in the name is not a
    // take, so a download of them is never refused for being one.
    for title in ["Karaoke Queen", "Karaoke", "Instrumental", "Cover Me"] {
        let song = recording("r", title, 200_000, vec![]);
        assert_eq!(kind_of(&song, None), RecordingKind::Studio, "{title}");
        let never = BTreeSet::from([
            RecordingKind::Instrumental,
            RecordingKind::Karaoke,
            RecordingKind::Cover,
        ]);
        assert!(!Takes::for_request(Some(title), None, &never).refuses(RecordingKind::Studio));
        assert!(RecordingKind::requested_by(title).is_empty(), "{title}");
    }
}
