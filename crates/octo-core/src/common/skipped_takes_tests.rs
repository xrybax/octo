use super::*;

fn outside(title: &str, album: &str) -> Song {
    Song {
        id: format!("ext-deezer-song-{title}"),
        title: title.to_string(),
        artist: "Artist".to_string(),
        album: album.to_string(),
        is_local: false,
        ..Default::default()
    }
}

fn library(title: &str, album: &str) -> Song {
    Song {
        id: format!("lib-{title}"),
        is_local: true,
        ..outside(title, album)
    }
}

#[test]
fn titles_naming_a_take_are_read() {
    for title in [
        "Welcome to the Black Parade (Instrumental)",
        "So What - Instrumental",
        "So What [Instrumental Version]",
        "Heat Waves Instrumental",
        "Hello (Karaoke Version)",
        "Hello - Karaoke",
        "Hello (Originally Performed by Adele)",
        "Hello (In the Style of Adele)",
        "Hello (Backing Track)",
    ] {
        assert!(title_names_skipped(title), "{title}");
    }
}

#[test]
fn titles_naming_a_cover_are_read() {
    for title in [
        "Hello (Cover)",
        "Hello (Acoustic Cover)",
        "Hello - Cover Version",
        "Hello [Cover]",
        "Hello (Tribute to Adele)",
        "Hello - A Tribute to Adele",
    ] {
        assert!(title_names_skipped(title), "{title}");
        assert!(
            RecordingKind::marked_in(title).contains(&RecordingKind::Cover),
            "{title}"
        );
    }
}

#[test]
fn a_word_in_a_name_is_not_a_take() {
    for title in [
        "Cover Me",
        "Covered in Blood",
        "Under Cover of Darkness",
        "Undercover",
        "Tribute",
        "Karaoke Queen",
        "Karaoke",
        "Instrumental",
        "Instrumentality",
        "The Karaoke Song of Life",
        "Welcome to the Black Parade",
        "Hello (Live)",
        "Hello (Remix)",
        "Song (Live at the Cover Club)",
    ] {
        assert!(!title_names_skipped(title), "{title}");
    }
}

#[test]
fn albums_naming_a_take_are_read() {
    assert!(album_names_skipped("The Black Parade (instrumentals)"));
    assert!(album_names_skipped("A Muse In Her Feelings (Instrumentals)"));
    assert!(album_names_skipped("Instrumentals"));
    assert!(album_names_skipped("Karaoke Hits, Vol. 3"));
    assert!(album_names_skipped("25 (Covers)"));
    assert!(album_names_skipped("Hello - A Tribute to Adele"));
    // A name that only starts with a tribute is often the artist's own record: Miles Davis's
    // "A Tribute to Jack Johnson", Steel Pulse's "Tribute to the Martyrs".
    assert!(!album_names_skipped("A Tribute to Jack Johnson"));
    assert!(!album_names_skipped("Tribute to the Martyrs"));
    assert!(!album_names_skipped(
        "Hot Fuss (Deluxe Edition) [Instrumentals Bonus]"
    ));
    assert!(album_names_skipped("Cover Versions of Adele"));
    assert!(album_names_skipped("Under the Covers, Vol. 1"));
    assert!(!album_names_skipped("Cover Me"));
    assert!(!album_names_skipped("Under Cover of Darkness"));
    assert!(!album_names_skipped("The Black Parade"));
    assert!(!album_names_skipped("Tree City Sessions 2"));
    assert!(!album_names_skipped(""));
}

#[test]
fn a_song_on_an_instrumentals_album_is_one() {
    assert!(song_names_skipped(
        "Welcome to the Black Parade",
        "The Black Parade (Instrumentals)"
    ));
    assert!(!song_names_skipped(
        "Welcome to the Black Parade",
        "The Black Parade"
    ));
}

#[test]
fn a_single_named_after_its_song_is_judged_by_the_title() {
    assert!(!song_names_skipped("Karaoke Queen", "Karaoke Queen"));
    assert!(!song_names_skipped("Instrumental", "Instrumental"));
    assert!(song_names_skipped(
        "Hello (Karaoke Version)",
        "Hello (Karaoke Version)"
    ));
}

#[test]
fn only_outside_songs_are_hidden_and_only_while_on() {
    let take = outside("Hello (Karaoke Version)", "Hello");
    assert!(hides_song(&take, true));
    assert!(!hides_song(&take, false));
    assert!(hides_song(&outside("Hello (Acoustic Cover)", "Hello"), true));
    assert!(!hides_song(&library("Hello (Acoustic Cover)", "Hello"), true));
    assert!(!hides_song(&outside("Cover Me", "Born in the U.S.A."), true));
    assert!(!hides_song(&library("Hello (Karaoke Version)", "Hello"), true));
    assert!(!hides_song(&outside("Hello", "25"), true));
}

#[test]
fn listed_songs_keeps_the_order_of_what_is_left() {
    let songs = vec![
        outside("One", "A"),
        outside("Two (Instrumental)", "A"),
        library("Three (Instrumental)", "B"),
        outside("Four", "Four (Instrumentals)"),
        outside("Five", "C"),
    ];
    let titles = |songs: Vec<Song>| songs.into_iter().map(|song| song.title).collect::<Vec<_>>();
    assert_eq!(
        titles(listed_songs(songs.clone(), true)),
        ["One", "Three (Instrumental)", "Five"]
    );
    assert_eq!(titles(listed_songs(songs.clone(), false)).len(), songs.len());
}

#[test]
fn listed_albums_leaves_out_instrumental_albums_while_on() {
    let album = |title: &str| Album {
        id: format!("ext-deezer-album-{title}"),
        title: title.to_string(),
        ..Default::default()
    };
    let albums = vec![
        album("The Black Parade"),
        album("The Black Parade (Instrumentals)"),
    ];
    let kept = listed_albums(albums.clone(), true);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].title, "The Black Parade");
    assert_eq!(listed_albums(albums, false).len(), 2);
}
