//! How well a search result's artist name fits the one asked for, for ranking cover art hits.

use super::SongIdentity;
use super::fuzzy::token_sort_ratio;
use super::romanize::{romanized_key, romanized_words, words};

/// How well `got` names the artist `want`, from 0 to 100, for pushing another artist's hits to
/// the bottom of a cover search:
///
/// - 100: the same name by [`SongIdentity::same_artist_name`] (case, accents, a leading "The",
///   stylized characters and aliases ignored), or the same name spelled in another script;
/// - 80: the same words in any order, allowing a typo ("Fonsi, Luis" for "Luis Fonsi");
/// - 60: one name holds the other as whole words ("Bob Marley" in "Bob Marley & The Wailers",
///   never "Air" in "Airbourne");
/// - otherwise 10 for each word they share, at most 50.
///
/// Either name empty scores 0.
pub fn cover_name_score(want: &str, got: &str) -> u8 {
    if SongIdentity::key(want).is_empty() || SongIdentity::key(got).is_empty() {
        return 0;
    }
    if SongIdentity::same_artist_name(want, got) {
        return 100;
    }
    let (want_roman, got_roman) = (romanized_key(want), romanized_key(got));
    if !want_roman.is_empty() && want_roman == got_roman {
        return 100;
    }

    let (want_words, got_words) = (words(want), words(got));
    let sorted = token_sort_ratio(&want_words, &got_words)
        .max(token_sort_ratio(&romanized_words(want), &romanized_words(got)));
    if sorted >= 0.92 {
        return 80;
    }

    let want_list: Vec<&str> = want_words.split(' ').filter(|word| !word.is_empty()).collect();
    let got_list: Vec<&str> = got_words.split(' ').filter(|word| !word.is_empty()).collect();
    let (short, long) = if want_list.len() <= got_list.len() {
        (&want_list, &got_list)
    } else {
        (&got_list, &want_list)
    };
    if !short.is_empty() && long.windows(short.len()).any(|run| run == short.as_slice()) {
        return 60;
    }

    let mut shared: Vec<&str> = want_list
        .iter()
        .copied()
        .filter(|word| got_list.contains(word))
        .collect();
    shared.sort_unstable();
    shared.dedup();
    (shared.len().min(5) * 10) as u8
}

/// A release's name as a key, its store's single or EP word taken off: "I'm On One - Single" is
/// "imonone".
fn release_key(name: &str) -> String {
    let trimmed = RELEASE_TAIL.replace(name.trim(), "");
    SongIdentity::key(trimmed.trim())
}

/// " - Single", " - EP", " (Single)", " [EP]" at the end of a store's release name.
static RELEASE_TAIL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)(?:\s+[-\x{2013}]\s+(?:single|ep)|\s*[\(\[](?:single|ep)[\)\]])\s*$")
        .expect("a fixed pattern compiles")
});

/// A credit's lead artists: its names before any guest ("feat.", "ft.", "featuring", "with"),
/// so "DJ Khaled feat. Drake, Rick Ross & Lil Wayne" leads with DJ Khaled alone and "Drake &
/// Tyga" with both.
fn lead_names(credit: &str) -> Vec<String> {
    let lead = GUEST.split(credit).next().unwrap_or(credit);
    let parsed = SongIdentity::parse_artists(lead);
    let mut names: Vec<String> = parsed.names.clone();
    if names.is_empty() && !SongIdentity::key(&parsed.display).is_empty() {
        names.push(parsed.display.clone());
    }
    names
}

static GUEST: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\s*[\(\[]?\b(?:feat\.?|ft\.?|featuring|with)\s")
        .expect("a fixed pattern compiles")
});

/// Whether a release's artist is one of the artists a cover is for: a lead artist of the release
/// is a lead artist of one of `want` (the album artist, and for a loose track the track artist),
/// by [`SongIdentity::same_artist_name`]. A guest is never enough: "DJ Khaled feat. Drake" is not
/// Drake's release.
pub fn release_artist_fits(want: &[&str], got: &str) -> bool {
    let got_names = lead_names(got);
    if got_names.is_empty() {
        return false;
    }
    want.iter()
        .filter(|w| !SongIdentity::key(w).is_empty())
        .any(|want| {
            SongIdentity::same_artist_name(want, got)
                || cover_name_score(want, got) >= 80
                || lead_names(want).iter().any(|name| {
                    got_names
                        .iter()
                        .any(|other| SongIdentity::same_artist_name(name, other))
                })
        })
}

/// Whether a release names the album a cover is for: the same name by its key, a store's
/// " - Single" or " - EP" aside, never one name holding the other ("I'm On One" is not "I'm On
/// One 3").
pub fn release_title_fits(want: &str, got: &str) -> bool {
    let (want_key, got_key) = (release_key(want), release_key(got));
    let roman = |name: &str| romanized_key(&RELEASE_TAIL.replace(name.trim(), ""));
    !want_key.is_empty()
        && (want_key == got_key
            || (!roman(want).is_empty() && roman(want) == roman(got))
            || SongIdentity::loose_key(&RELEASE_TAIL.replace(want.trim(), ""))
                == SongIdentity::loose_key(&RELEASE_TAIL.replace(got.trim(), "")))
}

/// Whether a release's cover is the album's: its title names the album and its artist is the
/// album's (or, for a loose track, the track's).
pub fn release_fits(want_title: &str, want_artists: &[&str], got_title: &str, got_artist: &str) -> bool {
    release_title_fits(want_title, got_title) && release_artist_fits(want_artists, got_artist)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Still Got It" by Tyga feat. Drake, on the "I'm On One 3" mixtape with album artist
    /// Drake: DJ Khaled's "I'm On One" (on "We the Best Forever") is another artist's release
    /// of another name, and Tyga's "Careless World" is another release.
    #[test]
    fn another_artists_release_of_a_near_name_is_never_the_albums() {
        let want = ["Drake", "Tyga feat. Drake"];
        assert!(!release_fits("I'm On One 3", &want, "I'm On One", "DJ Khaled"));
        assert!(!release_fits(
            "I'm On One 3",
            &want,
            "We the Best Forever",
            "DJ Khaled feat. Drake"
        ));
        assert!(!release_fits(
            "I'm On One 3",
            &want,
            "I'm On One 3",
            "DJ Khaled feat. Drake, Rick Ross & Lil Wayne"
        ));
        assert!(!release_fits(
            "I'm On One 3",
            &want,
            "Careless World: Rise Of The Last King",
            "Tyga"
        ));
        assert!(!release_title_fits("I'm On One 3", "I'm On One"));
        assert!(release_fits("I'm On One 3", &want, "I'm On One 3", "Drake"));
        assert!(release_fits(
            "Still Got It",
            &want,
            "Still Got It - Single",
            "Drake & Tyga"
        ));
        assert!(release_fits(
            "Still Got It",
            &want,
            "Still Got It (Single)",
            "Tyga"
        ));
    }

    #[test]
    fn a_release_written_otherwise_still_fits() {
        assert!(release_fits(
            "Mezzanine",
            &["Massive Attack"],
            "MEZZANINE",
            "Massive Attack"
        ));
        assert!(release_fits(
            "$ome $exy $ongs 4 U",
            &["PARTYNEXTDOOR"],
            "Some Sexy Songs 4 U",
            "PARTYNEXTDOOR & Drake"
        ));
        assert!(release_fits("Blond", &["The Weeknd"], "Blond - EP", "Weeknd"));
        assert!(!release_artist_fits(&["Drake"], ""));
        assert!(release_artist_fits(&["Luis Fonsi"], "Fonsi, Luis"));
        assert!(!release_artist_fits(&["Luis Fonsi"], "Luis Fonsiman"));
        assert!(release_fits("Gruppa krovi", &["Kino"], "Группа крови", "Кино"));
        assert!(!release_title_fits("", "Anything"));
    }

    #[test]
    fn the_same_artist_however_written_is_a_full_score() {
        assert_eq!(cover_name_score("Beyoncé", "Beyonce"), 100);
        assert_eq!(cover_name_score("The Weeknd", "weeknd"), 100);
        assert_eq!(cover_name_score("$uicideboy$", "Suicideboys"), 100);
        assert_eq!(cover_name_score("Kanye West", "Ye"), 100);
        assert_eq!(cover_name_score("Кино", "Kino"), 100);
    }

    #[test]
    fn the_same_words_in_another_order_score_eighty() {
        assert_eq!(cover_name_score("Luis Fonsi", "Fonsi, Luis"), 80);
        assert_eq!(cover_name_score("Christina Aguilera", "Christina Aguillera"), 80);
    }

    #[test]
    fn whole_words_inside_the_other_name_score_sixty() {
        assert_eq!(cover_name_score("Bob Marley", "Bob Marley & The Wailers"), 60);
        assert_eq!(cover_name_score("Daft Punk feat. Pharrell", "Daft Punk"), 60);
    }

    #[test]
    fn letters_inside_a_word_are_not_the_name() {
        // A substring used to score 60 here.
        assert_eq!(cover_name_score("Air", "Airbourne"), 0);
        assert_eq!(cover_name_score("Muse", "Museum Choir"), 0);
    }

    #[test]
    fn shared_words_count_ten_each() {
        assert_eq!(cover_name_score("Red Hot Chili Peppers", "Red Hot Band"), 20);
        assert_eq!(cover_name_score("Radiohead", "Coldplay"), 0);
    }

    #[test]
    fn an_empty_name_scores_nothing() {
        assert_eq!(cover_name_score("Radiohead", ""), 0);
        assert_eq!(cover_name_score("", "Radiohead"), 0);
    }
}
