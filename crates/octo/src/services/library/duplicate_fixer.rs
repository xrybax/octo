//! Fix duplicates from any app: what the executor hands a Fix duplicates request to (a star or
//! the action playlist), and the words the Duplicates and Fix duplicates playlists carry so a
//! person who only uses another app knows what to do.

use futures::future::BoxFuture;
use octo_core::settings::{LibraryAction, LibraryActionSettings, NoticeKind};
use octo_subsonic::SubsonicCredential;

use super::library_action_journal::LibraryActionState;

/// How a Fix duplicates request went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateFixAnswer {
    pub state: LibraryActionState,
    pub detail: String,
    /// Every copy of the song, when the fix started or found nothing to remove.
    pub copies: Vec<String>,
    /// Every copy stays, because every copy is starred or rated.
    pub all_kept: bool,
}

impl DuplicateFixAnswer {
    pub fn said(state: LibraryActionState, detail: impl Into<String>) -> Self {
        DuplicateFixAnswer {
            state,
            detail: detail.into(),
            copies: Vec::new(),
            all_kept: false,
        }
    }
}

/// Fixes every copy of a song, as Library health's Fix these copies does.
pub trait DuplicateFixer: Send + Sync {
    /// Fixes every copy of the song `navidrome_id` is one copy of, as `user` asked. `credential`
    /// is the sign-in of the star rating that asked, when one did: that rating is taken off first,
    /// so it never counts as a mark that keeps the copy.
    fn fix_copies_of<'a>(
        &'a self,
        navidrome_id: &'a str,
        user: &'a str,
        credential: Option<&'a SubsonicCredential>,
    ) -> BoxFuture<'a, DuplicateFixAnswer>;
}

/// The star rating that asks for `action` while ratings are on, or None.
fn star_for(settings: &LibraryActionSettings, action: LibraryAction) -> Option<i32> {
    if !settings.ratings_enabled {
        return None;
    }
    settings
        .effective_actions()
        .into_iter()
        .find(|entry| entry.action == action && entry.enabled)
        .and_then(|entry| entry.rating)
        .filter(|rating| *rating > 0)
}

/// The action playlist's title for `action` while it is on and playlists are on, or None.
fn playlist_for(settings: &LibraryActionSettings, action: LibraryAction) -> Option<String> {
    if !settings.playlists_enabled {
        return None;
    }
    settings
        .effective_actions()
        .into_iter()
        .find(|entry| entry.action == action && entry.enabled)
        .map(|entry| settings.playlist_title(&entry))
}

/// "add it to Fix duplicates, or give it 3 stars", or None when neither asks for `action`.
fn how_to_ask(settings: &LibraryActionSettings, action: LibraryAction, what: &str) -> Option<String> {
    let playlist = playlist_for(settings, action);
    let star = star_for(settings, action);
    let stars = |n: i32| {
        if n == 1 {
            "1 star".to_string()
        } else {
            format!("{n} stars")
        }
    };
    match (playlist, star) {
        (Some(playlist), Some(star)) => Some(format!(
            "add {what} to \"{}\", or give it {}",
            playlist.trim(),
            stars(star)
        )),
        (Some(playlist), None) => Some(format!("add {what} to \"{}\"", playlist.trim())),
        (None, Some(star)) => Some(format!("give {what} {}", stars(star))),
        (None, None) => None,
    }
}

/// What fixing a song does, in one sentence, as the Keep every rated copy switch is set.
fn fix_rule(settings: &LibraryActionSettings) -> &'static str {
    if settings.keep_rated_copies {
        "every copy anyone gave a heart or a star rating stays, the copies nobody marked go to the server's trash, and the best sound is kept in a copy that stays"
    } else {
        "one copy stays, with the best sound, in the copy people played, hearted or rated most, and the other copies go to the server's trash"
    }
}

/// The Duplicates playlist's description: what it shows and how to settle it from any app.
pub fn duplicates_comment(settings: &LibraryActionSettings) -> String {
    let mut parts = vec![
        "Songs you have more than once: each song's copies together, the best-sounding copy first."
            .to_string(),
    ];
    match how_to_ask(settings, LibraryAction::FixDuplicates, "any one copy of it") {
        Some(ask) => {
            parts.push(format!("To fix a song, {ask}: {}.", fix_rule(settings)));
            if playlist_for(settings, LibraryAction::FixDuplicates).is_some() {
                parts.push(
                    "Add this whole playlist there to fix every song in it; more songs come in as these are fixed."
                        .to_string(),
                );
            }
        }
        None => parts.push(format!(
            "Library health on Octo's dashboard fixes them: {}.",
            fix_rule(settings)
        )),
    }
    if settings.keep_rated_copies {
        parts.push("To make sure a copy stays, give it a heart.".to_string());
    }
    if let Some(ask) = how_to_ask(settings, LibraryAction::Keep, "a copy") {
        parts.push(format!("To keep every copy of a song, {ask}."));
    }
    parts.push("Taking a song out of this playlist also says its copies are on purpose.".to_string());
    if settings.ratings_enabled && !settings.mapped_ratings().is_empty() {
        parts.push("Star ratings on these songs ask Octo to do something, so a heart is the way to mark a copy you want.".to_string());
    }
    parts.join(" ")
}

/// The Fix duplicates playlist's description: what adding a song to it does.
pub fn fix_playlist_comment(settings: &LibraryActionSettings) -> String {
    let from = if settings.duplicates_enabled {
        format!(
            ", for example from \"{}\"",
            settings.notice_title(NoticeKind::Duplicates).trim()
        )
    } else {
        String::new()
    };
    let days = settings.effective_quarantine_retention_days();
    let trash = if days > 0 {
        format!(
            "Copies that go wait in the server's trash for {days} days, where Recently removed on Octo's dashboard can put them back."
        )
    } else {
        "Copies that go wait in the server's trash, where Recently removed on Octo's dashboard can put them back."
            .to_string()
    };
    format!(
        "Add any copy of a song you have more than once{from} to fix that song: {}. Octo takes the song out of here once the fix has started. {trash}",
        fix_rule(settings)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use octo_core::settings::LibraryActionDefinition;

    fn on(keep_rated: bool, star: Option<i32>) -> LibraryActionSettings {
        let mut actions = vec![LibraryActionDefinition {
            action: LibraryAction::FixDuplicates,
            name: String::new(),
            enabled: true,
            rating: star,
        }];
        if star == Some(3) {
            actions.push(LibraryActionDefinition {
                action: LibraryAction::WrongVersion,
                name: String::new(),
                enabled: false,
                rating: Some(0),
            });
        }
        LibraryActionSettings {
            enabled: true,
            ratings_enabled: star.is_some(),
            duplicates_enabled: true,
            keep_rated_copies: keep_rated,
            actions,
            ..Default::default()
        }
    }

    #[test]
    fn the_duplicates_playlist_says_how_to_fix_and_protect_from_any_app() {
        let text = duplicates_comment(&on(true, None));
        assert!(
            text.contains("add any one copy of it to \"\u{1F6E0} Fix duplicates\""),
            "{text}"
        );
        assert!(
            text.contains("anyone gave a heart or a star rating stays"),
            "{text}"
        );
        assert!(text.contains("give it a heart"), "{text}");
        assert!(text.contains("Add this whole playlist there"), "{text}");
        let starred = duplicates_comment(&on(false, Some(3)));
        assert!(starred.contains("or give it 3 stars"), "{starred}");
        assert!(starred.contains("one copy stays"), "{starred}");
        assert!(starred.contains("a heart is the way"), "{starred}");
    }

    #[test]
    fn without_the_action_the_duplicates_playlist_points_at_the_dashboard() {
        let settings = LibraryActionSettings {
            enabled: true,
            duplicates_enabled: true,
            ..Default::default()
        };
        let text = duplicates_comment(&settings);
        assert!(text.contains("Library health on Octo's dashboard"), "{text}");
        assert!(!text.contains("Fix duplicates\""), "{text}");
    }

    #[test]
    fn the_fix_playlist_says_what_adding_a_song_does() {
        let text = fix_playlist_comment(&on(true, None));
        assert!(
            text.contains("for example from \"\u{25B8} Duplicates\""),
            "{text}"
        );
        assert!(
            text.contains("anyone gave a heart or a star rating stays"),
            "{text}"
        );
        assert!(text.contains("30 days"), "{text}");
    }

    #[test]
    fn the_playlist_words_use_no_dash() {
        for text in [
            duplicates_comment(&on(true, Some(3))),
            duplicates_comment(&on(false, None)),
            fix_playlist_comment(&on(true, None)),
            fix_playlist_comment(&on(false, None)),
        ] {
            assert!(
                !(text.contains('\u{2014}') || text.contains('\u{2013}') || text.contains(" - ")),
                "{text}"
            );
            assert!(!text.to_lowercase().contains("lossless"), "{text}");
        }
    }
}
