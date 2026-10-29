//! The real case of G-Eazy's "Let's Get Lost": three files, their tags, lengths, ISRCs and
//! recording ids as the library and the music database have them.

use super::*;

const ORIGINAL: &str = "20cb620c-1b37-4e93-9b4b-c64b0a2faddc";
const COMPILATION: &str = "d5981ba3-6655-40ab-ab96-651c47864f40";
const BEAR_FACE: &str = "3f839c1f-10a7-4640-a9b3-2d100acdae37";
const TEKLIFE: &str = "5a584600-24a3-47a0-8f9d-d15e58bfa694";

/// MusicBrainz's recordings of the song by G-Eazy.
fn catalog() -> Vec<KnownRecording> {
    vec![
        KnownRecording::new(ORIGINAL, "Let\u{2019}s Get Lost", Some(240_680)),
        KnownRecording::new(COMPILATION, "Let\u{2019}s Get Lost", Some(242_640)),
        KnownRecording::new(TEKLIFE, "Let's Get Lost (Teklife remix)", Some(269_653)),
        KnownRecording::new(
            "bdacd536-70fb-4264-ac91-18b46e952397",
            "Let's Get Lost (Devon Baldwin rework)",
            Some(255_834),
        ),
        KnownRecording::new(
            "98796223-5de7-4e27-93e9-54cf9c3267fc",
            "Let\u{2019}s Get Lost (Jarreau Vandal remix)",
            Some(225_000),
        ),
        KnownRecording::new(
            "91942aad-9c88-4bbc-9bc0-f07aca2dceea",
            "Let's Get Lost (Funkin Matt remix)",
            Some(273_840),
        ),
        KnownRecording::new(
            "8690bd95-5da1-4835-acfa-d7fd41a6e77f",
            "Let's Get Lost (Louis Futon remix)",
            Some(267_427),
        ),
        KnownRecording::new(BEAR_FACE, "Let's Get Lost (Bear//Face remix)", Some(242_202)),
    ]
}

/// AcoustID's ids for the original's sound (the original and its compilation copy share them).
fn original_tracks() -> Vec<String> {
    [
        "0af35246-5e74-469a-8f90-8c366329ceb9",
        "6f940625-2f4a-4625-8a02-1edc69a63fb9",
        "e3eccfac-eeb5-4379-a4e1-f3445fcbb349",
        "2e106084-0a00-4843-a66d-3c67b961e18e",
    ]
    .map(String::from)
    .to_vec()
}

/// `5usZcgFijfma1blnIYNjXB`: tagged as the original on the remix EP, 242.2024 s, no ids. Its
/// fingerprint is known (98% sure) but tied only to Superstar's "Let's Get Lost".
fn five_us_z() -> VersionFacts {
    VersionFacts {
        title: "Let's Get Lost".into(),
        artist: "G-Eazy".into(),
        album: "Let's Get Lost Remixes".into(),
        file_ms: Some(242_202),
        heard: Vec::new(),
        heard_anything: true,
        heard_tracks: vec!["5df93b57-71b4-4db1-a24e-1d483e9948f7".into()],
        tagged_recording: None,
        isrc: None,
        catalog: catalog(),
        tagged_tracks: Some(original_tracks()),
        resampled: None,
    }
}

/// `1l7NVUVMjStbjNcPflszjx`: tagged as the Bear//Face remix with its ISRC, 269.6533 s, carrying
/// the Teklife remix's recording id. Its fingerprint is known but tied to no recording.
fn one_l7_n() -> VersionFacts {
    VersionFacts {
        title: "Let's Get Lost (feat. Devon Baldwin) [Bear//Face Remix]".into(),
        artist: "G-Eazy".into(),
        album: "Let's Get Lost Remixes (feat. Devon Baldwin) - EP".into(),
        file_ms: Some(269_653),
        heard: Vec::new(),
        heard_anything: true,
        heard_tracks: vec!["36f17cf0-2803-415d-b9f1-441e498360d2".into()],
        tagged_recording: Some(KnownRecording::new(
            TEKLIFE,
            "Let's Get Lost (Teklife remix)",
            Some(269_653),
        )),
        isrc: Some(IsrcEntry {
            isrc: "USRC11501446".into(),
            title: "Let's Get Lost (feat. Devon Baldwin) (Bear//Face Remix)".into(),
            seconds: Some(242),
        }),
        catalog: catalog(),
        tagged_tracks: Some(Vec::new()),
        resampled: None,
    }
}

/// `6Ab7filWyAu4VzstkaDG3a`: the real original on "These Things Happen", 240.68 s, which the
/// fingerprint names.
fn six_ab7() -> VersionFacts {
    VersionFacts {
        title: "Let's Get Lost (feat. Devon Baldwin)".into(),
        artist: "G-Eazy".into(),
        album: "These Things Happen".into(),
        file_ms: Some(240_680),
        heard: vec![
            KnownRecording::new(ORIGINAL, "Let\u{2019}s Get Lost", Some(240_680)),
            KnownRecording::new(COMPILATION, "Let\u{2019}s Get Lost", Some(242_640)),
        ],
        heard_anything: true,
        heard_tracks: vec!["2e106084-0a00-4843-a66d-3c67b961e18e".into()],
        tagged_recording: None,
        isrc: None,
        catalog: catalog(),
        tagged_tracks: Some(original_tracks()),
        resampled: None,
    }
}

#[test]
fn the_original_tagged_file_on_the_remix_ep_is_the_bear_face_remix() {
    let found = judge(&five_us_z()).expect("a finding");
    assert_eq!(found.problem, VersionProblem::OtherVersion);
    assert_eq!(found.sureness, Sureness::Heard);
    assert_eq!(found.is.as_ref().map(|r| r.id.as_str()), Some(BEAR_FACE));
    assert_eq!(
        found.words,
        "Tagged as 'Let's Get Lost', but the file runs 4:02.20, the length of 'Let's Get Lost \
         (Bear//Face remix)' (4:02.20) to the millisecond. AcoustID does not hear the tagged \
         version in this file. The album 'Let's Get Lost Remixes' says remix. The tagged version \
         runs 4:01.66."
    );
    assert!(!found.isrc_belongs_elsewhere);
}

#[test]
fn the_bear_face_tagged_file_is_the_teklife_remix_by_its_own_recording_id() {
    let found = judge(&one_l7_n()).expect("a finding");
    assert_eq!(found.problem, VersionProblem::OtherVersion);
    assert_eq!(found.sureness, Sureness::Heard);
    assert_eq!(found.is.as_ref().map(|r| r.id.as_str()), Some(TEKLIFE));
    assert!(found.isrc_belongs_elsewhere, "the ISRC is the Bear//Face remix's");
    assert_eq!(found.expected_seconds, Some(242));
    assert!(
        found
            .words
            .contains("its own recording id names 'Let's Get Lost (Teklife remix)' (4:29.65)")
    );
    assert!(found.words.contains("that recording's length to the millisecond"));
    assert!(found.words.contains("Its ISRC USRC11501446"));
}

#[test]
fn the_real_original_is_left_alone() {
    assert_eq!(judge(&six_ab7()), None);
}

#[test]
fn without_the_fingerprint_the_album_and_exact_length_still_say_likely() {
    let facts = VersionFacts {
        tagged_tracks: None,
        heard_anything: false,
        heard_tracks: Vec::new(),
        ..five_us_z()
    };
    let found = judge(&facts).expect("a finding");
    assert_eq!(found.sureness, Sureness::Likely);
    assert_eq!(found.is.as_ref().map(|r| r.id.as_str()), Some(BEAR_FACE));
}

#[test]
fn an_exact_length_alone_on_a_plain_album_says_nothing() {
    let facts = VersionFacts {
        album: "These Things Happen".into(),
        tagged_tracks: None,
        heard_anything: false,
        heard_tracks: Vec::new(),
        ..five_us_z()
    };
    assert_eq!(judge(&facts), None);
}

#[test]
fn the_bear_face_tagged_file_without_its_recording_id_is_still_the_wrong_length() {
    let facts = VersionFacts {
        tagged_recording: None,
        ..one_l7_n()
    };
    let found = judge(&facts).expect("a finding");
    assert_eq!(found.sureness, Sureness::Likely);
    assert_eq!(found.problem, VersionProblem::OtherVersion);
    assert_eq!(found.is.as_ref().map(|r| r.id.as_str()), Some(TEKLIFE));
    assert!(
        found
            .words
            .starts_with("The file runs 4:29.65, but its ISRC USRC11501446 ('Let's Get Lost (feat. Devon Baldwin) (Bear//Face Remix)') runs 4:02.00."),
        "{}",
        found.words
    );
}

#[test]
fn a_sped_up_copy_that_fingerprints_as_the_original_when_slowed_back() {
    let facts = VersionFacts {
        title: "Let's Get Lost (feat. Devon Baldwin)".into(),
        album: "These Things Happen".into(),
        file_ms: Some(192_544),
        heard: Vec::new(),
        heard_anything: false,
        heard_tracks: Vec::new(),
        tagged_tracks: None,
        resampled: Some(Resampled {
            ratio: 192_544.0 / 240_680.0,
            recording: KnownRecording::new(ORIGINAL, "Let\u{2019}s Get Lost", Some(240_680)),
        }),
        ..five_us_z()
    };
    assert!(
        speed_ratio_to_try(&VersionFacts {
            resampled: None,
            ..facts.clone()
        })
        .is_some()
    );
    let found = judge(&facts).expect("a finding");
    assert_eq!(found.problem, VersionProblem::Speed);
    assert_eq!(found.kind, Some(RecordingKind::Sped));
    assert_eq!(
        found.words,
        "The audio is a sped-up copy of 'Let\u{2019}s Get Lost' (4:00.68): played back at 0.80 \
         times the speed, it fingerprints as that recording."
    );
}

#[test]
fn a_copy_of_another_length_that_does_not_fingerprint_back_is_only_the_wrong_length() {
    let facts = VersionFacts {
        title: "Let's Get Lost (feat. Devon Baldwin)".into(),
        album: "These Things Happen".into(),
        file_ms: Some(300_850),
        heard: Vec::new(),
        heard_anything: false,
        heard_tracks: Vec::new(),
        tagged_tracks: None,
        ..five_us_z()
    };
    let found = judge(&facts).expect("a finding");
    assert_eq!(found.problem, VersionProblem::LengthOff);
    assert!(
        found.words.contains("0.80 times the tagged version's speed"),
        "{}",
        found.words
    );
}

#[test]
fn a_heard_remix_of_a_file_tagged_as_the_original_is_named() {
    let facts = VersionFacts {
        title: "Let's Get Lost".into(),
        album: "These Things Happen".into(),
        file_ms: Some(267_427),
        heard: vec![KnownRecording::new(
            "8690bd95-5da1-4835-acfa-d7fd41a6e77f",
            "Let's Get Lost (Louis Futon remix)",
            Some(267_427),
        )],
        ..five_us_z()
    };
    let found = judge(&facts).expect("a finding");
    assert_eq!(found.sureness, Sureness::Heard);
    assert_eq!(
        found.words,
        "Tagged as 'Let's Get Lost', but the fingerprint is 'Let's Get Lost (Louis Futon remix)' (4:27.43)."
    );
}

#[test]
fn lengths_read_with_hundredths() {
    assert_eq!(length_words(242_202), "4:02.20");
    assert_eq!(length_words(269_653), "4:29.65");
    assert_eq!(length_words(59_999), "1:00.00");
}
