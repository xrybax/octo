//! `Octo.Models.Settings.ImportSettings` (the `Imports` section).

use serde::{Deserialize, Serialize};

/// Importing songs from somewhere else: a public Spotify link, a file another service exported,
/// or a pasted list. Octo shows what the library already has, keeps a Navidrome playlist of it,
/// and fetches the rest a few songs an hour through the same chain a heart uses. Keys this
/// section does not name, such as ones an older settings.json holds, are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ImportSettings {
    /// How many missing songs the trickle starts in an hour, at most. It also waits while
    /// anyone's own downloads are running, so it never holds up a heart. 0 stops it starting any.
    /// Environment variable: IMPORTS__SONGSPERHOUR
    pub songs_per_hour: i32,

    /// How often a list from a public link that is kept as a playlist or fetched in full is read
    /// again, so new songs there reach the playlist and the trickle. 0 reads them only when asked.
    /// Environment variable: IMPORTS__REFRESHHOURS
    pub refresh_hours: i32,
}

impl ImportSettings {
    pub fn effective_songs_per_hour(&self) -> i32 {
        self.songs_per_hour.clamp(0, 600)
    }
}

impl Default for ImportSettings {
    fn default() -> Self {
        Self {
            songs_per_hour: 20,
            refresh_hours: 6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pace_is_clamped() {
        let s = ImportSettings {
            songs_per_hour: 10_000,
            ..Default::default()
        };
        assert_eq!(s.effective_songs_per_hour(), 600);
        let typed = ImportSettings {
            songs_per_hour: -1,
            ..Default::default()
        };
        assert_eq!(typed.effective_songs_per_hour(), 0);
        assert_eq!(ImportSettings::default().refresh_hours, 6);
    }

    #[test]
    fn keys_the_section_does_not_name_are_ignored() {
        let read: ImportSettings = serde_json::from_str(
            r#"{"SpotifyClientId":"abc","SpotifyRedirectUri":"http://127.0.0.1/callback","SongsPerHour":5}"#,
        )
        .expect("read");
        assert_eq!(
            read,
            ImportSettings {
                songs_per_hour: 5,
                ..Default::default()
            }
        );
    }
}
