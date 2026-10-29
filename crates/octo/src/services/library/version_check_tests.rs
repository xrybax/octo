//! The version check's gathering and what it keeps, over the real case of G-Eazy's "Let's Get
//! Lost": the music database's recordings, the catalog's ISRC entries and AcoustID's answers as
//! they were on 2026-10-14, from sources that answer from memory.

use std::sync::Mutex;

use octo_core::fingerprint::acoust_id_client::{AcoustIdRecording, AcoustIdResult};
use octo_core::fingerprint::verification::VerificationResult;

use super::*;

const BEAR_FACE: &str = "3f839c1f-10a7-4640-a9b3-2d100acdae37";
const TEKLIFE: &str = "5a584600-24a3-47a0-8f9d-d15e58bfa694";
const ORIGINAL: &str = "20cb620c-1b37-4e93-9b4b-c64b0a2faddc";

#[derive(Default)]
struct Remembered {
    resampled: Option<AcoustIdLookup>,
    asked: Mutex<Vec<String>>,
}

#[async_trait]
impl VersionSources for Remembered {
    async fn song_recordings(&self, artist: &str, title: &str) -> Option<Vec<KnownRecording>> {
        self.asked
            .lock()
            .unwrap()
            .push(format!("song {artist} / {title}"));
        Some(vec![
            KnownRecording::new(ORIGINAL, "Let\u{2019}s Get Lost", Some(240_680)),
            KnownRecording::new(
                "d5981ba3-6655-40ab-ab96-651c47864f40",
                "Let\u{2019}s Get Lost",
                Some(242_640),
            ),
            KnownRecording::new(TEKLIFE, "Let's Get Lost (Teklife remix)", Some(269_653)),
            KnownRecording::new(BEAR_FACE, "Let's Get Lost (Bear//Face remix)", Some(242_202)),
        ])
    }

    async fn isrc_entry(&self, isrc: &str) -> Option<IsrcEntry> {
        self.asked.lock().unwrap().push(format!("isrc {isrc}"));
        (isrc == "USRC11501446").then(|| IsrcEntry {
            isrc: isrc.to_string(),
            title: "Let's Get Lost (feat. Devon Baldwin) (Bear//Face Remix)".into(),
            seconds: Some(242),
        })
    }

    async fn recording(&self, recording_id: &str) -> Option<KnownRecording> {
        self.asked
            .lock()
            .unwrap()
            .push(format!("recording {recording_id}"));
        (recording_id == TEKLIFE)
            .then(|| KnownRecording::new(TEKLIFE, "Let's Get Lost (Teklife remix)", Some(269_653)))
    }

    async fn tracks_of(&self, recording_id: &str) -> Option<Vec<String>> {
        self.asked.lock().unwrap().push(format!("tracks {recording_id}"));
        Some(vec!["2e106084-0a00-4843-a66d-3c67b961e18e".into()])
    }

    async fn resampled_lookup(&self, path: &str, speed: f64) -> Option<AcoustIdLookup> {
        self.asked
            .lock()
            .unwrap()
            .push(format!("resampled {path} {speed:.2}"));
        self.resampled.clone()
    }
}

/// AcoustID's answer: one result at `score`, under AcoustID id `track`, with these recordings.
fn answer(track: &str, score: f64, recordings: Vec<AcoustIdRecording>) -> VerificationResult {
    let mut result = AcoustIdResult::new(score, recordings);
    result.id = Some(track.to_string());
    VerificationResult {
        lookup: Some(AcoustIdLookup {
            is_ok: true,
            results: vec![result],
            ..AcoustIdLookup::default()
        }),
        threshold: 0.85,
        ..VerificationResult::default()
    }
}

fn superstar() -> AcoustIdRecording {
    AcoustIdRecording {
        duration_ms: Some(235_800),
        ..AcoustIdRecording::new(
            "76a71dbe-3b16-4bb3-a7db-cb860d3bbee4",
            "Let's Get Lost",
            ["Superstar"],
            None,
            None,
        )
    }
}

#[tokio::test]
async fn the_original_tagged_remix_is_found_from_the_lengths_and_acoust_ids_silence() {
    let sources = Remembered::default();
    let input = VersionInput {
        path: "/music/G-Eazy - Let's Get Lost.flac".into(),
        artist: "G-Eazy".into(),
        title: "Let's Get Lost".into(),
        album: "Let's Get Lost Remixes".into(),
        file_ms: Some(242_202),
        ..VersionInput::default()
    };
    let verdict = answer("5df93b57-71b4-4db1-a24e-1d483e9948f7", 0.998, vec![superstar()]);
    let facts = gather(&sources, &input, &verdict, 0.85).await;
    assert!(facts.heard.is_empty(), "Superstar's song is not G-Eazy's");
    assert!(facts.heard_anything);
    let found = octo_core::tagging::version_audit::judge(&facts).expect("a finding");
    let kept = kept_of(&found, &input);
    assert_eq!(kept.problem, "otherVersion");
    assert!(kept.heard);
    assert_eq!(kept.is_id.as_deref(), Some(BEAR_FACE));
    assert_eq!(
        kept.retag,
        IndexMap::from([
            (
                "title".to_string(),
                Some("Let's Get Lost (Bear//Face remix)".to_string())
            ),
            ("recordingId".to_string(), Some(BEAR_FACE.to_string())),
        ])
    );
    let asked = sources.asked.lock().unwrap().clone();
    assert!(asked.contains(&format!("tracks {ORIGINAL}")), "{asked:?}");
}

#[tokio::test]
async fn the_bear_face_tagged_teklife_remix_is_retagged_without_the_bear_face_isrc() {
    let sources = Remembered::default();
    let input = VersionInput {
        path: "/music/G-Eazy - Let's Get Lost (feat. Devon Baldwin) [Bear__Face Remix].flac".into(),
        artist: "G-Eazy".into(),
        title: "Let's Get Lost (feat. Devon Baldwin) [Bear//Face Remix]".into(),
        album: "Let's Get Lost Remixes (feat. Devon Baldwin) - EP".into(),
        recording_id: Some(TEKLIFE.into()),
        isrcs: vec!["USRC11501446".into()],
        file_ms: Some(269_653),
        ..VersionInput::default()
    };
    // AcoustID knows the sound but ties it to no recording.
    let verdict = answer("36f17cf0-2803-415d-b9f1-441e498360d2", 0.998, Vec::new());
    let facts = gather(&sources, &input, &verdict, 0.85).await;
    let found = octo_core::tagging::version_audit::judge(&facts).expect("a finding");
    let kept = kept_of(&found, &input);
    assert_eq!(kept.is_id.as_deref(), Some(TEKLIFE));
    assert_eq!(kept.expected_seconds, Some(242));
    assert_eq!(
        kept.retag,
        IndexMap::from([
            (
                "title".to_string(),
                Some("Let's Get Lost (Teklife remix)".to_string())
            ),
            ("recordingId".to_string(), Some(TEKLIFE.to_string())),
            ("isrc".to_string(), None),
        ])
    );
}

#[tokio::test]
async fn a_sped_up_copy_is_played_back_at_its_speed_and_heard_as_the_original() {
    let original = AcoustIdRecording {
        duration_ms: Some(240_680),
        ..AcoustIdRecording::new(
            ORIGINAL,
            "Let\u{2019}s Get Lost",
            ["G\u{2010}Eazy", "Devon Baldwin"],
            None,
            None,
        )
    };
    let mut resampled = AcoustIdResult::new(0.97, vec![original]);
    resampled.id = Some("2e106084-0a00-4843-a66d-3c67b961e18e".into());
    let sources = Remembered {
        resampled: Some(AcoustIdLookup {
            is_ok: true,
            results: vec![resampled],
            ..AcoustIdLookup::default()
        }),
        ..Remembered::default()
    };
    let input = VersionInput {
        path: "/music/G-Eazy - Let's Get Lost (feat. Devon Baldwin).flac".into(),
        artist: "G-Eazy".into(),
        title: "Let's Get Lost (feat. Devon Baldwin)".into(),
        album: "These Things Happen".into(),
        file_ms: Some(192_544),
        ..VersionInput::default()
    };
    let verdict = answer("unknown", 0.10, Vec::new());
    let facts = gather(&sources, &input, &verdict, 0.85).await;
    let found = octo_core::tagging::version_audit::judge(&facts).expect("a finding");
    let kept = kept_of(&found, &input);
    assert_eq!(kept.problem, "speed");
    assert_eq!(kept.kind.as_deref(), Some("Sped Up"));
    assert_eq!(
        kept.retag,
        IndexMap::from([(
            "title".to_string(),
            Some("Let's Get Lost (feat. Devon Baldwin) (Sped Up)".to_string())
        )])
    );
    let asked = sources.asked.lock().unwrap().clone();
    assert!(
        asked
            .iter()
            .any(|line| line.starts_with("resampled ") && line.ends_with(" 0.80")),
        "{asked:?}"
    );
}

#[test]
fn a_search_reads_only_the_artists_recordings() {
    let doc = serde_json::json!({
        "recordings": [
            {"id": "a", "title": "Let's Get Lost", "length": 240680,
             "artist-credit": [{"name": "G\u{2010}Eazy"}, {"name": "Devon Baldwin"}]},
            {"id": "b", "title": "Let's Get Lost", "length": 235800,
             "artist-credit": [{"name": "Superstar"}]},
            {"id": "c", "title": "Let's Get Lost (Bear//Face remix)",
             "artist-credit": [{"name": "G-Eazy"}]}
        ]
    });
    let found = recordings_by(&doc, "G-Eazy");
    assert_eq!(
        found,
        vec![
            KnownRecording::new("a", "Let's Get Lost", Some(240_680)),
            KnownRecording::new("c", "Let's Get Lost (Bear//Face remix)", None),
        ]
    );
}

// ---- covers -----------------------------------------------------------------------------------

/// A picture split down the middle or across, so the two look nothing alike.
fn picture(split_down: bool) -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{Rgb, RgbImage};
    let image = RgbImage::from_fn(300, 300, |x, y| {
        let dark = if split_down { x < 150 } else { y < 150 };
        if dark {
            Rgb([10, 10, 10])
        } else {
            Rgb([230, 40, 40])
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&image)
        .expect("encodes");
    out.into_inner()
}

/// The catalog as it answered on 2026-10-14 for the "I'm On One 3" mixtape: DJ Khaled's track
/// "I'm On One" on "We The Best Forever", and Tyga's "Careless World".
struct LooseCovers {
    khaled: Vec<u8>,
    tyga: Vec<u8>,
}

#[async_trait]
impl VersionSources for LooseCovers {
    async fn song_recordings(&self, _: &str, _: &str) -> Option<Vec<KnownRecording>> {
        None
    }
    async fn isrc_entry(&self, _: &str) -> Option<IsrcEntry> {
        None
    }
    async fn recording(&self, _: &str) -> Option<KnownRecording> {
        None
    }
    async fn tracks_of(&self, _: &str) -> Option<Vec<String>> {
        None
    }
    async fn resampled_lookup(&self, _: &str, _: f64) -> Option<AcoustIdLookup> {
        None
    }
    async fn cover_candidates(&self, _: &str, _: &str) -> Vec<(String, String, String)> {
        vec![
            ("We The Best Forever".into(), "DJ Khaled".into(), "khaled".into()),
            (
                "Careless World: Rise Of The Last King".into(),
                "Tyga".into(),
                "tyga".into(),
            ),
        ]
    }
    async fn image(&self, url: &str) -> Option<Vec<u8>> {
        Some(if url == "khaled" {
            self.khaled.clone()
        } else {
            self.tyga.clone()
        })
    }
}

fn still_got_it(cover: Vec<u8>) -> VersionInput {
    VersionInput {
        path: "/music/Tyga feat. Drake - Still Got It.flac".into(),
        artist: "Tyga feat. Drake".into(),
        album_artist: Some("Drake".into()),
        cover: Some(cover),
        title: "Still Got It".into(),
        album: "I'm On One 3".into(),
        recording_id: Some("cfdc336c-2806-468f-a645-375143cb9611".into()),
        file_ms: Some(179_720),
        ..VersionInput::default()
    }
}

#[tokio::test]
async fn a_cover_that_is_another_artists_release_is_found() {
    let sources = LooseCovers {
        khaled: picture(true),
        tyga: picture(false),
    };
    let found = check_cover(&sources, &still_got_it(picture(true)))
        .await
        .expect("another artist's cover");
    assert_eq!(found.release_title, "We The Best Forever");
    assert_eq!(found.release_artist, "DJ Khaled");
    assert_eq!(
        found.words,
        "The cover is the one of 'We The Best Forever' by DJ Khaled, another artist's release, not of 'I'm On One 3' by Drake."
    );
}

#[tokio::test]
async fn a_cover_of_none_of_the_other_releases_is_left_alone() {
    let sources = LooseCovers {
        khaled: picture(true),
        tyga: picture(true),
    };
    assert_eq!(check_cover(&sources, &still_got_it(picture(false))).await, None);
    assert_eq!(
        check_cover(
            &sources,
            &VersionInput {
                cover: None,
                ..still_got_it(picture(true))
            }
        )
        .await,
        None
    );
}
