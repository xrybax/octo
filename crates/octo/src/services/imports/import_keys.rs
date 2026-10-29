//! The key that names a song across every imported list, and the JSON readers the link and file
//! imports share.

use octo_core::common::dotnet::{self, is_blank};
use octo_core::common::song_identity::SongIdentity;
use serde_json::Value;

/// Keys for imported songs: Spotify's track id when the source names one, else the song's words.
pub struct ImportKeys;

impl ImportKeys {
    /// The song by its words, the same for "Drake feat. Rihanna" and "Drake, Rihanna". A remix
    /// credited to someone keeps their name, so two remixes of one song are two songs.
    pub fn for_words(artist: Option<&str>, title: Option<&str>) -> String {
        let (artist, title) = (artist.unwrap_or(""), title.unwrap_or(""));
        let key = format!("song:{}", SongIdentity::match_key(artist, title));
        let mut remixers = SongIdentity::parse_title(title, Some(artist)).remixers;
        remixers.retain(|remixer| !remixer.is_empty());
        if remixers.is_empty() {
            return key;
        }
        remixers.sort();
        format!("{key}|by:{}", remixers.join("+"))
    }

    /// A key saved before remixes kept their remixer's name, as [`Self::for_words`] writes it
    /// now; any other key as it is.
    pub fn current(key: &str, artist: &str, title: &str) -> String {
        if !key.starts_with("song:") {
            return key.to_string();
        }
        let now = Self::for_words(Some(artist), Some(title));
        if now.len() > key.len() && now.starts_with(key) && now[key.len()..].starts_with("|by:") {
            now
        } else {
            key.to_string()
        }
    }

    /// Spotify's key for a track id or a spotify:track: URI, or None.
    pub fn for_spotify(id_or_uri: Option<&str>) -> Option<String> {
        let value = id_or_uri.filter(|v| !is_blank(v))?.trim();
        const PREFIX: &str = "spotify:track:";
        let value = if dotnet::starts_with_ignore_case(value, PREFIX) {
            &value[PREFIX.len()..]
        } else if find_ascii_ignore_case(value, "open.spotify.com/track/").is_some() {
            let at = find_ascii_ignore_case(value, "/track/").unwrap_or(0);
            value[at + 7..].split(['?', '/']).next().unwrap_or("")
        } else {
            value
        };
        let length = dotnet::utf16_len(value);
        (length > 10
            && length < 40
            && value
                .chars()
                .all(|c| dotnet::is_letter_utf16(c) || dotnet::is_digit_utf16(c)))
        .then(|| format!("spotify:{value}"))
    }
}

fn find_ascii_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    (0..=h.len().checked_sub(n.len())?).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// A length in milliseconds as whole seconds: a whole number above 0, rounded half to even.
pub(crate) fn seconds_of(value: Option<&Value>) -> Option<i32> {
    let ms = value.and_then(Value::as_i64).filter(|ms| *ms > 0)?;
    Some(dotnet::round(ms as f64 / 1000.0, 0) as i32)
}

/// A member that is a string, or None.
pub(crate) fn str_of(e: &Value, name: &str) -> Option<String> {
    match e.as_object()?.get(name)? {
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_come_from_spotify_ids_or_the_songs_words() {
        assert_eq!(
            ImportKeys::for_spotify(Some("spotify:track:4uLU6hMCjMI75M1A2tKUQC")).as_deref(),
            Some("spotify:4uLU6hMCjMI75M1A2tKUQC")
        );
        assert_eq!(
            ImportKeys::for_spotify(Some("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=x"))
                .as_deref(),
            Some("spotify:4uLU6hMCjMI75M1A2tKUQC")
        );
        assert_eq!(ImportKeys::for_spotify(Some("short")), None);
        assert_eq!(ImportKeys::for_spotify(None), None);
        assert_eq!(
            ImportKeys::for_words(Some("Drake, Rihanna"), Some("Too Good")),
            ImportKeys::for_words(Some("Drake feat. Rihanna"), Some("Too Good"))
        );
    }

    #[test]
    fn lengths_are_whole_seconds_and_only_strings_are_read() {
        let row = serde_json::json!({"ms": 2500, "zero": 0, "name": "Teardrop", "n": 3});
        assert_eq!(seconds_of(row.get("ms")), Some(2));
        assert_eq!(seconds_of(row.get("zero")), None);
        assert_eq!(seconds_of(row.get("missing")), None);
        assert_eq!(str_of(&row, "name").as_deref(), Some("Teardrop"));
        assert_eq!(str_of(&row, "n"), None);
    }
}
