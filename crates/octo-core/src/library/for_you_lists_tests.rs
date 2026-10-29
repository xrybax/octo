//! Made for you's picking is what a listener hears, so it is tested directly.

use std::collections::HashSet;

use chrono::{DateTime, Months, NaiveDate, SecondsFormat, TimeZone, Utc};
use serde_json::json;

use super::*;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0)
        .single()
        .expect("a time")
}

#[derive(Default)]
struct S<'a> {
    plays: i32,
    played: Option<String>,
    starred: bool,
    rating: i32,
    title: Option<&'a str>,
}

fn song(id: &str, artist: &str, s: S) -> Node {
    let value = json!({
        "id": id,
        "title": s.title.map(str::to_string).unwrap_or_else(|| format!("Song {id}")),
        "artist": artist,
        "artistId": format!("ar-{artist}"),
        "playCount": s.plays,
        "played": s.played,
        "starred": if s.starred { Some("2025-01-01T00:00:00Z") } else { None },
        "userRating": s.rating,
        "duration": 200,
    });
    Node::parse(&value.to_string()).expect("JSON")
}

fn plays(n: i32) -> S<'static> {
    S {
        plays: n,
        ..Default::default()
    }
}

fn names(artists: &[TopArtist]) -> Vec<&str> {
    artists.iter().map(|artist| artist.name.as_str()).collect()
}

fn ids(songs: &[Node]) -> HashSet<String> {
    songs
        .iter()
        .map(|song| str_of(song, "id").unwrap_or_default().to_string())
        .collect()
}

fn iso(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

// ---- top artists ------------------------------------------------------------------------

#[test]
fn top_artists_are_plays_summed_only_ones_played_or_hearted_and_various_artists_is_nobody() {
    // Found live: with room for 50, a listener who played two artists got every artist in the
    // library as "theirs". An artist never played nor hearted is never one.
    let songs = [
        song("1", "Big", plays(10)),
        song("2", "Big", plays(10)),
        song("3", "Small", plays(5)),
        song("4", "Various Artists", plays(100)),
        song("5", "Quiet", S::default()),
        song(
            "6",
            "Hearted",
            S {
                starred: true,
                ..Default::default()
            },
        ),
    ];

    let top = top_artists(&songs, 50);

    assert_eq!(names(&top), ["Big", "Small", "Hearted"]);
    assert_eq!(top[0].plays, 20);
}

#[test]
fn top_artists_with_no_plays_are_the_hearted_and_with_nothing_at_all_the_biggest() {
    let hearted = [
        song("1", "Kept", S::default()),
        song("2", "Kept", S::default()),
        song("3", "Kept", S::default()),
        song(
            "4",
            "Loved",
            S {
                starred: true,
                ..Default::default()
            },
        ),
        song("5", "Neither", S::default()),
    ];
    assert_eq!(names(&top_artists(&hearted, 10)), ["Loved"]);

    // A new listener, with no plays or hearts yet, still gets lists from the library's biggest.
    let fresh = [
        song("1", "Kept", S::default()),
        song("2", "Kept", S::default()),
        song("3", "Neither", S::default()),
    ];
    assert_eq!(names(&top_artists(&fresh, 10)), ["Kept", "Neither"]);
}

// ---- Rediscover -------------------------------------------------------------------------

#[test]
fn rediscover_is_loved_songs_not_played_for_months_never_ones_with_no_date() {
    let old = iso(now().checked_sub_months(Months::new(8)).expect("a date"));
    let recent = iso(now().checked_sub_months(Months::new(1)).expect("a date"));
    let songs = [
        song(
            "often",
            "A",
            S {
                plays: 5,
                played: Some(old.clone()),
                ..Default::default()
            },
        ),
        song(
            "hearted",
            "B",
            S {
                plays: 1,
                played: Some(old.clone()),
                starred: true,
                ..Default::default()
            },
        ),
        song(
            "rated",
            "C",
            S {
                plays: 1,
                played: Some(old.clone()),
                rating: 4,
                ..Default::default()
            },
        ),
        song(
            "liked-but-recent",
            "D",
            S {
                plays: 9,
                played: Some(recent),
                ..Default::default()
            },
        ),
        song(
            "not-loved",
            "E",
            S {
                plays: 1,
                played: Some(old),
                ..Default::default()
            },
        ),
        song("no-date", "F", plays(9)),
    ];

    let picked = ids(&rediscover(&songs, now(), 6, 1));

    assert_eq!(
        picked,
        ["often", "hearted", "rated"]
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>()
    );
}

#[test]
fn rediscover_holds_still_for_a_seed_and_never_more_than_three_by_one_artist() {
    let old = iso(now().checked_sub_months(Months::new(12)).expect("a date"));
    let songs: Vec<Node> = (0..40)
        .map(|i| {
            let artist = if i < 20 {
                "Same".to_string()
            } else {
                format!("Artist {i}")
            };
            song(
                &format!("s{i}"),
                &artist,
                S {
                    plays: 5,
                    played: Some(old.clone()),
                    ..Default::default()
                },
            )
        })
        .collect();

    let first = rediscover(&songs, now(), 6, 7);
    let again = rediscover(&songs, now(), 6, 7);

    let text = |songs: &[Node]| {
        songs
            .iter()
            .map(|song| song.to_json_string(false))
            .collect::<Vec<_>>()
    };
    assert_eq!(text(&first), text(&again));
    assert!(
        first
            .iter()
            .filter(|song| str_of(song, "artist") == Some("Same"))
            .count() as i32
            <= REDISCOVER_PER_ARTIST
    );
    assert_eq!(first.len(), 23);
}

// ---- Deep Cuts --------------------------------------------------------------------------

#[test]
fn deep_cuts_are_played_at_most_once_from_the_top_artists_and_not_in_rediscover() {
    let songs = [
        song("never", "Top", S::default()),
        song("once", "Top", plays(1)),
        song("twice", "Top", plays(2)),
        song("other", "Elsewhere", S::default()),
        song("taken", "Top", S::default()),
    ];

    let picked = ids(&deep_cuts(
        &songs,
        &["ar-Top".to_string()],
        &["taken".to_string()].into_iter().collect(),
        3,
    ));

    assert_eq!(
        picked,
        ["never", "once"]
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>()
    );
}

// ---- New Releases picking ---------------------------------------------------------------

fn release(id: &str, kind: &str, date: Option<&str>) -> AlbumHit {
    AlbumHit {
        deezer_id: id.into(),
        title: "Title".into(),
        artist: "Artist".into(),
        cover_url: Some(format!("https://cdn.test/{id}.jpg")),
        year: None,
        track_count: 10,
        record_type: Some(kind.into()),
        release_date: date.map(str::to_string),
        explicit_lyrics: None,
    }
}

#[test]
fn recent_releases_are_the_last_weeks_newest_first_no_compilations_no_future_no_undated() {
    let today = NaiveDate::from_ymd_opt(2026, 10, 4).expect("a date");
    let releases = [
        release("old", "album", Some("2026-07-01")),
        release("album", "album", Some("2026-09-20")),
        release("single", "single", Some("2026-10-02")),
        release("ep", "ep", Some("2026-08-15")),
        release("compile", "compile", Some("2026-09-30")),
        release("future", "album", Some("2026-11-01")),
        release("undated", "album", None),
    ];

    let recent = recent_releases(&releases, today, 8);

    assert_eq!(
        recent
            .iter()
            .map(|release| release.deezer_id.as_str())
            .collect::<Vec<_>>(),
        ["single", "album", "ep"]
    );
}

fn track(position: i32, rank: Option<i32>) -> AlbumTrack {
    AlbumTrack {
        title: format!("Track {position}"),
        artist: "Artist".into(),
        duration: Some(200),
        track_position: Some(position),
        disc_number: Some(1),
        isrc: None,
        rank,
        explicit_content: None,
    }
}

#[test]
fn pick_tracks_an_album_gives_its_best_by_rank_in_album_order_a_single_its_first() {
    let album = [
        track(1, Some(10)),
        track(2, Some(900)),
        track(3, Some(50)),
        track(4, Some(800)),
        track(5, Some(700)),
    ];
    let positions = |tracks: Vec<AlbumTrack>| {
        tracks
            .iter()
            .map(|track| track.track_position.unwrap_or(0))
            .collect::<Vec<_>>()
    };

    assert_eq!(positions(pick_tracks(Some("album"), &album)), [2, 4, 5]);
    assert_eq!(positions(pick_tracks(Some("single"), &album)), [1, 2, 3]);
    let unranked: Vec<AlbumTrack> = album
        .iter()
        .map(|track| AlbumTrack {
            rank: None,
            ..track.clone()
        })
        .collect();
    assert_eq!(positions(pick_tracks(Some("album"), &unranked)), [1, 2, 3]);
}

// ---- names, filter and dates -------------------------------------------------------------

/// The names and the filter rules, and `played` read in the shapes Navidrome writes.
#[test]
fn names_filter_and_played() {
    assert_eq!(
        KINDS.iter().map(|kind| name(kind)).collect::<Vec<_>>(),
        ["New Releases", "Rediscover", "Deep Cuts"]
    );
    assert!(is_for_you("deepCuts") && !is_for_you("genre"));
    assert!(!explicit_allowed(Some(1), ExplicitFilter::CleanOnly));
    assert!(explicit_allowed(Some(3), ExplicitFilter::CleanOnly));
    assert!(!explicit_allowed(Some(3), ExplicitFilter::ExplicitOnly));
    assert!(explicit_allowed(None, ExplicitFilter::ExplicitOnly));
    assert!(explicit_allowed(Some(1), ExplicitFilter::All));

    let at = |text: &str| played(&Node::parse(&json!({ "played": text }).to_string()).expect("JSON"));
    assert_eq!(
        at("2025-10-04T12:00:00Z"),
        Some(Utc.with_ymd_and_hms(2025, 10, 4, 12, 0, 0).unwrap())
    );
    assert_eq!(
        at("2025-10-04T14:00:00.123+02:00").map(|t| t.timestamp()),
        Some(Utc.with_ymd_and_hms(2025, 10, 4, 12, 0, 0).unwrap().timestamp())
    );
    assert_eq!(
        at("2025-10-04T12:00:00"),
        Some(Utc.with_ymd_and_hms(2025, 10, 4, 12, 0, 0).unwrap())
    );
    assert_eq!(at("not a date"), None);
}

// ---- artist mixes -----------------------------------------------------------------------

fn named(id: &str, artist: &str, artists: &[&str]) -> Node {
    let value = json!({
        "id": id,
        "artist": artist,
        "artists": artists.iter().map(|name| json!({"id": format!("ar-{name}"), "name": name})).collect::<Vec<_>>(),
        "playCount": 0,
    });
    Node::parse(&value.to_string()).expect("JSON")
}

#[test]
fn songs_by_artists_match_each_artist_value_loosely() {
    let library = [
        named("a", "Suicideboys", &[]),
        named("b", "Drake feat. Rihanna", &[]),
        named("c", "Someone Else", &["Someone Else", "Rihanna"]),
        named("d", "Nobody", &[]),
    ];
    let found = |names: &[&str]| -> Vec<String> {
        let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
        songs_by_artists(&library, &names)
            .into_iter()
            .map(|song| str_of(song, "id").unwrap_or_default().to_string())
            .collect()
    };
    assert_eq!(found(&["$uicideboy$"]), ["a"]);
    assert_eq!(found(&["drake"]), ["b"]);
    assert_eq!(found(&["Rihanna"]), ["c"]);
    assert_eq!(found(&["Drake", "Nobody"]), ["b", "d"]);
    assert!(found(&[]).is_empty());
    assert!(found(&["", "  "]).is_empty());
}

fn by(artist: &str, count: usize) -> Vec<Node> {
    (0..count)
        .map(|i| song(&format!("{artist}-{i}"), artist, plays(1)))
        .collect()
}

fn artists_of(songs: &[Node]) -> Vec<String> {
    songs
        .iter()
        .map(|song| str_of(song, "artist").unwrap_or_default().to_string())
        .collect()
}

#[test]
fn an_artist_mix_is_half_the_artist_and_half_the_similar_artists_in_the_library() {
    let library: Vec<Node> = [
        by("Radiohead", 30),
        by("Portishead", 10),
        by("Massive Attack", 10),
        by("Other", 10),
    ]
    .concat();
    let similar = vec![
        "Portishead".to_string(),
        "Massive Attack".to_string(),
        "Not Here".to_string(),
    ];

    let mix = artist_mix(&library, "ar-Radiohead", &similar, 10, 3, 42);

    let artists = artists_of(&mix);
    assert_eq!(mix.len(), 10);
    assert_eq!(artists.iter().filter(|a| *a == "Radiohead").count(), 5);
    assert!(artists.iter().filter(|a| *a == "Portishead").count() <= 3);
    assert!(artists.iter().filter(|a| *a == "Massive Attack").count() <= 3);
    assert!(!artists.iter().any(|a| a == "Other"));
    // The halves alternate, the artist first.
    assert!(artists.iter().step_by(2).all(|a| a == "Radiohead"), "{artists:?}");
    // Every song once, and the same draw for the same seed.
    assert_eq!(ids(&mix).len(), 10);
    assert_eq!(mix, artist_mix(&library, "ar-Radiohead", &similar, 10, 3, 42));
    assert_ne!(
        ids(&mix),
        ids(&artist_mix(&library, "ar-Radiohead", &similar, 10, 3, 7))
    );
}

#[test]
fn a_short_half_is_filled_by_the_other_and_the_cap_never_holds_the_artist_back() {
    let library: Vec<Node> = [by("Radiohead", 30), by("Portishead", 10)].concat();
    let similar = vec!["Portishead".to_string()];

    // Portishead gives at most 3, so Radiohead fills the other 7.
    let mix = artist_mix(&library, "ar-Radiohead", &similar, 10, 3, 1);
    let artists = artists_of(&mix);
    assert_eq!(artists.iter().filter(|a| *a == "Radiohead").count(), 7);
    assert_eq!(artists.iter().filter(|a| *a == "Portishead").count(), 3);

    // An artist with two songs keeps both, and the similar artists fill the rest.
    let few: Vec<Node> = [by("Radiohead", 2), by("Portishead", 20)].concat();
    let mix = artist_mix(&few, "ar-Radiohead", &similar, 10, 50, 1);
    let artists = artists_of(&mix);
    assert_eq!(mix.len(), 10);
    assert_eq!(artists.iter().filter(|a| *a == "Radiohead").count(), 2);
}

#[test]
fn with_no_similar_artist_in_the_library_an_artist_mix_is_their_own_songs() {
    let library: Vec<Node> = [by("Radiohead", 30), by("Other", 10)].concat();
    for similar in [Vec::new(), vec!["Not Here".to_string()]] {
        let mix = artist_mix(&library, "ar-Radiohead", &similar, 10, 3, 9);
        assert_eq!(artists_of(&mix), vec!["Radiohead"; 10], "{similar:?}");
    }
    // A short artist is all of their songs.
    let mix = artist_mix(&by("Radiohead", 4), "ar-Radiohead", &[], 10, 3, 9);
    assert_eq!(mix.len(), 4);
}

/// The songs by `names` found by reading every song's artist values, as a plain reference for
/// the index.
fn songs_by_artists_read_through<'a>(songs: &'a [Node], names: &[String]) -> Vec<&'a Node> {
    let wanted: HashSet<String> = names
        .iter()
        .map(|name| SongIdentity::loose_key(name))
        .filter(|key| !key.is_empty())
        .collect();
    songs
        .iter()
        .filter(|song| {
            let mut values: Vec<String> = Vec::new();
            if let Some(artist) = str_of(song, "artist").filter(|artist| !artist.is_empty()) {
                values.push(artist.to_string());
                values.push(SongIdentity::primary_artist(artist));
            }
            if let Some(Node::Array(artists)) = song.get("artists") {
                values.extend(
                    artists
                        .iter()
                        .filter_map(|artist| str_of(artist, "name"))
                        .map(str::to_string),
                );
            }
            values
                .iter()
                .any(|value| wanted.contains(&SongIdentity::loose_key(value)))
        })
        .collect()
}

#[test]
fn one_index_finds_the_same_songs_as_reading_every_song() {
    let library: Vec<Node> = [
        by("Radiohead", 12),
        vec![
            named("a", "Suicideboys", &[]),
            named("b", "Drake feat. Rihanna", &[]),
            named("c", "Someone Else", &["Someone Else", "Rihanna"]),
            named("d", "Rihanna", &["Rihanna", "Drake"]),
            named("e", "", &[]),
        ],
        by("Portishead", 7),
        by("Massive Attack", 5),
        by("Drake", 4),
    ]
    .concat();
    let index = ArtistIndex::new(&library);
    assert_eq!(index.len(), library.len());
    let asks: [&[&str]; 7] = [
        &["$uicideboy$"],
        &["drake"],
        &["Rihanna", "Portishead"],
        &["Massive Attack", "Not Here", "radiohead"],
        &["Drake", "Rihanna", "Drake"],
        &[],
        &["", "  "],
    ];
    for ask in asks {
        let names: Vec<String> = ask.iter().map(|name| name.to_string()).collect();
        let read: Vec<&str> = songs_by_artists_read_through(&library, &names)
            .into_iter()
            .map(|song| str_of(song, "id").unwrap_or_default())
            .collect();
        let indexed: Vec<&str> = songs_by_artists_in(&library, &index, &names)
            .into_iter()
            .map(|song| str_of(song, "id").unwrap_or_default())
            .collect();
        assert_eq!(indexed, read, "{ask:?}");
    }

    // The mix drawn from the shared index is the one drawn by reading every song.
    let similar = vec![
        "Portishead".to_string(),
        "Massive Attack".to_string(),
        "Rihanna".to_string(),
    ];
    for (artist, count, seed) in [
        ("ar-Radiohead", 10, 42),
        ("ar-Drake", 10, 7),
        ("ar-Portishead", 6, 3),
        ("ar-Nobody", 10, 1),
    ] {
        let mix = artist_mix_in(&library, &index, artist, &similar, count, 3, seed);
        assert_eq!(
            mix,
            artist_mix_read_through(&library, artist, &similar, count, 3, seed),
            "{artist}"
        );
        assert_eq!(
            mix,
            artist_mix(&library, artist, &similar, count, 3, seed),
            "{artist}"
        );
    }
}

/// [`artist_mix`] with its two halves found by reading every song, as a plain reference for the
/// index.
fn artist_mix_read_through(
    songs: &[Node],
    artist: &str,
    similar: &[String],
    count: usize,
    max_per_artist: i32,
    seed: i32,
) -> Vec<Node> {
    let own: Vec<&Node> = songs.iter().filter(|song| artist_key(song) == artist).collect();
    let others: Vec<&Node> = songs_by_artists_read_through(songs, similar)
        .into_iter()
        .filter(|song| artist_key(song) != artist)
        .collect();
    let own_drawn = draw(&own, own.len(), i32::MAX, seed);
    let others_drawn = draw(&others, count, max_per_artist, seed.wrapping_add(1));
    let own_share = if others_drawn.is_empty() {
        count
    } else {
        count.div_ceil(2)
    };
    let mut own_taken = own_drawn.len().min(own_share);
    let others_taken = others_drawn.len().min(count - own_taken);
    own_taken = own_drawn.len().min(count - others_taken);
    let mut mix = Vec::new();
    let mut own_iter = own_drawn.into_iter().take(own_taken);
    let mut others_iter = others_drawn.into_iter().take(others_taken);
    loop {
        let a = own_iter.next();
        let b = others_iter.next();
        if a.is_none() && b.is_none() {
            break;
        }
        mix.extend(a);
        mix.extend(b);
    }
    mix
}
