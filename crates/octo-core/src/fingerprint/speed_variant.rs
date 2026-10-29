//! A copy played at another speed: "sped up", nightcore, "slowed". Chromaprint hears a
//! resampled copy as different audio, so its fingerprint names nothing; the copy's length
//! against the recording's says by how much it was resampled, and the copy resampled back by
//! that much fingerprints as the recording again.

use super::recording_identity::RecordingKind;

/// The fastest speed change looked for: a copy at 1.33 times the speed runs 0.75 of the length.
pub const SHORTEST_RATIO: f64 = 0.75;

/// The slowest: a copy at 0.75 times the speed runs 1.33 of the length.
pub const LONGEST_RATIO: f64 = 1.34;

/// How far off a copy's length must be, as a share of the recording's, before it counts as
/// played at another speed. Masterings and trimmed silence move a length by a second or two;
/// sped-up and slowed uploads move it by 7 to 25 percent.
pub const LEAST_SHARE: f64 = 0.04;

/// And in milliseconds, so a short song's few seconds of trimmed silence never count.
pub const LEAST_MS: i64 = 6_000;

/// The file's length over the recording's when that is a speed change worth checking: off by
/// at least [`LEAST_SHARE`] and [`LEAST_MS`], and within [`SHORTEST_RATIO`] and
/// [`LONGEST_RATIO`]. None for lengths that agree, are unknown, or differ by too much to be the
/// same song at another speed.
pub fn speed_ratio(file_ms: i64, recording_ms: i64) -> Option<f64> {
    if file_ms <= 0 || recording_ms <= 0 {
        return None;
    }
    let ratio = file_ms as f64 / recording_ms as f64;
    let apart = (file_ms - recording_ms).abs();
    let share = (ratio - 1.0).abs();
    (share >= LEAST_SHARE && apart >= LEAST_MS && (SHORTEST_RATIO..=LONGEST_RATIO).contains(&ratio))
        .then_some(ratio)
}

/// The kind of copy a ratio from [`speed_ratio`] is: shorter is faster.
pub fn kind_for(ratio: f64) -> RecordingKind {
    if ratio < 1.0 {
        RecordingKind::Sped
    } else {
        RecordingKind::Slowed
    }
}

/// How much faster or slower, in words: "1.25 times as fast", "0.80 times as fast".
pub fn speed_words(ratio: f64) -> String {
    format!("{:.2} times as fast", 1.0 / ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_a_quarter_faster_is_found_and_called_sped_up() {
        // The original of the case: 240.68 s; the same audio at 1.25x runs 192.54 s.
        let ratio = speed_ratio(192_544, 240_680).expect("a speed change");
        assert!((ratio - 0.8).abs() < 0.001);
        assert_eq!(kind_for(ratio), RecordingKind::Sped);
        assert_eq!(speed_words(ratio), "1.25 times as fast");
    }

    #[test]
    fn a_slowed_copy_is_found_and_called_slowed() {
        let ratio = speed_ratio(300_850, 240_680).expect("a speed change");
        assert_eq!(kind_for(ratio), RecordingKind::Slowed);
        assert_eq!(speed_words(ratio), "0.80 times as fast");
    }

    #[test]
    fn lengths_that_agree_or_are_far_apart_are_no_speed_change() {
        // The original and the Bear//Face remix differ by 1.5 s: masterings, not speed.
        assert_eq!(speed_ratio(242_202, 240_680), None);
        // The Teklife remix against the Bear//Face remix: 11 percent, but another recording;
        // the resampled fingerprint is what tells the two apart, so the ratio is still offered.
        assert!(speed_ratio(269_653, 242_202).is_some());
        // Half the length is a cut, not a speed change.
        assert_eq!(speed_ratio(120_000, 240_680), None);
        assert_eq!(speed_ratio(0, 240_680), None);
        assert_eq!(speed_ratio(240_680, 0), None);
        // A short song's trimmed silence: 5 percent but under six seconds.
        assert_eq!(speed_ratio(95_000, 100_000), None);
    }
}
