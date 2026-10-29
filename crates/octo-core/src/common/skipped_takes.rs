//! The Skip covers and instrumentals setting applied to outside songs: while it is on (the
//! default), a song from outside the library whose title or album names a cover, a karaoke take
//! or an instrumental is left out of every list Octo makes of outside songs (search, radio and
//! mixes, charts, Made for you, top songs). Library songs are always listed: they are the
//! listener's own.
//!
//! A title is read by its version markers only ("(Instrumental)", "- Karaoke Version", "(Acoustic
//! Cover)", "(Originally Performed by X)"), so a song called "Karaoke Queen", "Cover Me" or
//! "Instrumental" keeps its name. An album is read as `RecordingKind::named_in` reads one, loose
//! words too ("Instrumentals", "Karaoke Hits", "A Tribute to Adele"), except an album named after
//! the song itself.

use crate::common::song_identity::SongIdentity;
use crate::fingerprint::recording_identity::RecordingKind;
use crate::models::domain::{Album, Song};

/// The takes Skip covers and instrumentals leaves out.
pub const SKIPPED: [RecordingKind; 3] = [
    RecordingKind::Instrumental,
    RecordingKind::Karaoke,
    RecordingKind::Cover,
];

/// Whether a song title's version markers name a cover, a karaoke take or an instrumental.
pub fn title_names_skipped(title: &str) -> bool {
    let marked = RecordingKind::marked_in(title);
    SKIPPED.iter().any(|kind| marked.contains(kind))
}

/// Whether an album title names a cover, a karaoke take or an instrumental: "The Black Parade
/// (Instrumentals)", "Karaoke Hits", "Instrumentals", "A Tribute to Adele".
pub fn album_names_skipped(album: &str) -> bool {
    let named = RecordingKind::named_in(album);
    SKIPPED.iter().any(|kind| named.contains(kind))
}

/// Whether a song is a cover, a karaoke take or an instrumental by its title, or by its album. A single
/// whose album is the song's own title is judged by the title alone.
pub fn song_names_skipped(title: &str, album: &str) -> bool {
    if title_names_skipped(title) {
        return true;
    }
    let album_is_title = SongIdentity::key(album) == SongIdentity::key(title);
    !album_is_title && album_names_skipped(album)
}

/// Whether a song is left out of a list while Skip covers and instrumentals is `skip`: an outside
/// song whose title or album names a cover, a karaoke take or an instrumental. Never a library
/// song.
pub fn hides_song(song: &Song, skip: bool) -> bool {
    skip && !song.is_local && song_names_skipped(&song.title, &song.album)
}

/// Whether an outside album is left out of a list while Skip covers and instrumentals is `skip`.
pub fn hides_album(album: &Album, skip: bool) -> bool {
    skip && album_names_skipped(&album.title)
}

/// The songs a list keeps while Skip covers and instrumentals is `skip`, in their order.
pub fn listed_songs(songs: Vec<Song>, skip: bool) -> Vec<Song> {
    if !skip {
        return songs;
    }
    songs.into_iter().filter(|song| !hides_song(song, skip)).collect()
}

/// The outside albums a list keeps while Skip covers and instrumentals is `skip`, in their order.
pub fn listed_albums(albums: Vec<Album>, skip: bool) -> Vec<Album> {
    if !skip {
        return albums;
    }
    albums
        .into_iter()
        .filter(|album| !hides_album(album, skip))
        .collect()
}

#[cfg(test)]
#[path = "skipped_takes_tests.rs"]
mod tests;
