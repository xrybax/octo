//! How sure a download's AcoustID check was, read back from the words its log line kept
//! ([`super::verification::VerificationResult::acoust_id_text`]), so a list of library songs
//! can tell the sure matches from the unsure ones long after the download.

/// What one AcoustID check line says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckedMatch {
    /// AcoustID named the same recording, at or above the score the check needed.
    pub sure: bool,
    /// The score it gave, as a whole percent, when the line names one.
    pub score: Option<i32>,
}

/// The match a check line describes, or `None` for words that are not an AcoustID check.
pub fn checked_match(words: &str) -> Option<CheckedMatch> {
    let words = words.trim();
    if !words.starts_with("AcoustID") {
        return None;
    }
    Some(CheckedMatch {
        sure: words.starts_with("AcoustID: the same recording"),
        score: percent_before(words, "% sure").or_else(|| percent_after(words, "(best ")),
    })
}

/// The whole number just before the first `marker`.
fn percent_before(words: &str, marker: &str) -> Option<i32> {
    let end = words.find(marker)?;
    let digits: String = words[..end]
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

/// The whole number just after the first `marker`.
fn percent_after(words: &str, marker: &str) -> Option<i32> {
    let start = words.find(marker)? + marker.len();
    let digits: String = words[start..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_of_check_line_reads_as_sure_or_not_with_its_score() {
        let cases = [
            (
                "AcoustID: the same recording, 92% sure (needs 85%)",
                Some((true, Some(92))),
            ),
            (
                "AcoustID: the same recording, 88% sure (needs 85%), and the ISRC matches",
                Some((true, Some(88))),
            ),
            (
                "AcoustID says this is 'Air - Playground Love', 97% sure (needs 85%)",
                Some((false, Some(97))),
            ),
            (
                "AcoustID: the same song, 90% sure (needs 85%), but that recording runs 4:12",
                Some((false, Some(90))),
            ),
            ("AcoustID: no match above 85% (best 61%)", Some((false, Some(61)))),
            (
                "AcoustID knows this sound, 91% sure, but no recording is linked to it",
                Some((false, Some(91))),
            ),
            ("AcoustID has no entry for this sound", Some((false, None))),
            ("AcoustID did not answer", Some((false, None))),
            ("Checked the recording", None),
            ("", None),
        ];
        for (words, expected) in cases {
            let read = checked_match(words).map(|m| (m.sure, m.score));
            assert_eq!(read, expected, "{words}");
        }
    }
}
