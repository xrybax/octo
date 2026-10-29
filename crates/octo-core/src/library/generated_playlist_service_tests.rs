//! Genre and decade mixes (#54). The draw is what a listener hears, so its rules are tested
//! directly: it holds still for a period, never breaks the artist cap, and keeps its share for
//! new tracks only when asked to.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use indexmap::IndexMap;

use super::*;
use crate::settings::GenreSettings;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 25, 12, 0, 0).unwrap()
}

/// The round-trip date format: seven fraction digits and a Z.
fn round_trip_text(at: DateTime<Utc>) -> String {
    format!(
        "{}.{:07}Z",
        at.format("%Y-%m-%dT%H:%M:%S"),
        at.timestamp_subsec_nanos() / 100
    )
}

fn song(id: i32, artist: &str, play_count: Option<i32>, created: Option<DateTime<Utc>>) -> Node {
    let created = created.unwrap_or_else(|| now() - TimeDelta::days(730));
    let play_count = play_count.map_or_else(|| "null".to_string(), |count| count.to_string());
    Node::parse(&format!(
        r#"{{"id":"s{id}","title":"Song {id}","artist":"{artist}","artistId":"ar-{artist}","playCount":{play_count},"created":"{}","duration":200}}"#,
        round_trip_text(created)
    ))
    .expect("a song parses")
}

fn pool(count: i32, artists: i32) -> Vec<Node> {
    (0..count)
        .map(|i| song(i, &format!("Artist {}", i % artists), Some(5), None))
        .collect()
}

fn ids(songs: &[Node]) -> String {
    songs
        .iter()
        .map(|song| str_of(song, "id").unwrap_or_default())
        .collect::<Vec<_>>()
        .join(",")
}

fn set_play_count(song: &mut Node, count: i32) {
    song.as_object_mut()
        .expect("an object")
        .insert("playCount".into(), Node::Number(count.to_string()));
}

#[test]
fn select_is_deterministic_for_a_seed() {
    let pool = pool(200, 50);

    let first = select(&pool, 50, 3, 0, 30, now(), 42);
    let again = select(&pool, 50, 3, 0, 30, now(), 42);
    let other = select(&pool, 50, 3, 0, 30, now(), 43);

    assert_eq!(first.len(), 50);
    assert_eq!(ids(&first), ids(&again));
    assert_ne!(ids(&first), ids(&other));
}

#[test]
fn select_never_exceeds_the_artist_cap_even_when_that_leaves_the_mix_short() {
    let pool = pool(100, 4);

    let drawn = select(&pool, 50, 3, 0, 30, now(), 7);

    assert_eq!(drawn.len(), 12);
    let mut per_artist: IndexMap<&str, i32> = IndexMap::new();
    for song in &drawn {
        *per_artist.entry(str_of(song, "artist").unwrap()).or_insert(0) += 1;
    }
    assert!(per_artist.values().all(|count| *count == 3), "{per_artist:?}");
}

#[test]
fn select_new_share_fills_from_new_tracks_first() {
    let mut pool = pool(100, 100);
    for i in 0..10 {
        set_play_count(&mut pool[i * 10], 0);
    }

    let drawn = select(&pool, 20, 3, 50, 30, now(), 1);

    assert_eq!(drawn.len(), 20);
    assert_eq!(drawn.iter().filter(|song| is_new(song, now(), 30)).count(), 10);
}

#[test]
fn select_new_share_zero_ignores_newness() {
    let old = pool(100, 100);
    let mut fresh = pool(100, 100);
    for song in fresh.iter_mut().take(50) {
        set_play_count(song, 0);
    }

    assert_eq!(
        ids(&select(&old, 20, 3, 0, 30, now(), 9)),
        ids(&select(&fresh, 20, 3, 0, 30, now(), 9))
    );
}

/// The reference draws from the same pools and seeds: a mix holds exactly these songs.
#[test]
fn select_draws_the_reference_songs() {
    assert_eq!(
        ids(&select(&pool(200, 50), 50, 3, 0, 30, now(), 42)),
        "s18,s157,s171,s99,s152,s56,s142,s49,s91,s185,s83,s39,s76,s40,s113,s107,s84,s41,s183,s68,s198,s75,s136,s105,s85,s3,s189,s26,s137,s160,s127,s47,s161,s169,s93,s38,s120,s170,s65,s22,s92,s193,s151,s114,s35,s45,s72,s11,s80,s199"
    );
    assert_eq!(
        ids(&select(&pool(100, 4), 50, 3, 0, 30, now(), 7)),
        "s15,s14,s23,s31,s78,s49,s72,s32,s16,s85,s21,s26"
    );
    let mut fresh = pool(100, 100);
    for i in 0..10 {
        set_play_count(&mut fresh[i * 10], 0);
    }
    assert_eq!(
        ids(&select(&fresh, 20, 3, 50, 30, now(), 1)),
        "s21,s46,s68,s35,s80,s13,s39,s82,s93,s48,s19,s30,s20,s70,s50,s40,s0,s60,s90,s10"
    );
    assert_eq!(
        ids(&select(
            &pool(30, 10),
            10,
            2,
            0,
            30,
            now(),
            seed("alice", "genre:Rock", 1)
        )),
        "s12,s0,s20,s5,s8,s15,s22,s28,s27,s9"
    );
}

#[test]
fn is_new_never_played_or_recently_added() {
    assert!(is_new(&song(1, "A", None, None), now(), 30));
    assert!(is_new(&song(1, "A", Some(0), None), now(), 30));
    assert!(is_new(
        &song(1, "A", Some(4), Some(now() - TimeDelta::days(3))),
        now(),
        30
    ));
    assert!(!is_new(
        &song(1, "A", Some(4), Some(now() - TimeDelta::days(45))),
        now(),
        30
    ));
}

#[test]
fn apply_hysteresis_creates_at_20_keeps_down_to_10_drops_below() {
    let counts: IndexMap<String, i32> = [
        ("genre:New", 20),
        ("genre:Almost", 19),
        ("genre:Kept", 10),
        ("genre:Gone", 9),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();

    let active = apply_hysteresis(
        &counts,
        &["genre:Kept".to_string(), "genre:Gone".to_string()],
        20,
        10,
        20,
    );

    assert_eq!(active, ["genre:New", "genre:Kept"]);
}

#[test]
fn apply_hysteresis_shows_the_largest_first_up_to_the_limit() {
    let counts: IndexMap<String, i32> = [
        ("genre:B", 50),
        ("genre:A", 50),
        ("decade:1990", 400),
        ("genre:C", 30),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();

    assert_eq!(
        apply_hysteresis(&counts, &[], 20, 10, 3),
        ["decade:1990", "genre:A", "genre:B"]
    );
}

#[test]
fn parse_genres_one_entry_per_genre_whatever_the_case_and_never_a_year_or_a_blocked_value() {
    let rows = Node::parse(
        r#"[{"value":"Rock","songCount":120},{"value":"rock","songCount":4},{"value":"1990s","songCount":80},
            {"value":"Music","songCount":300},{"value":" ","songCount":10},{"value":"Trip-Hop","songCount":25}]"#,
    )
    .expect("parses");
    let Node::Array(rows) = rows else {
        panic!("an array")
    };

    let counts = parse_genres(Some(&rows), &GenreSettings::default().effective_blocklist());

    let expected: IndexMap<String, i32> = [("genre:Rock", 120), ("genre:Trip-Hop", 25)]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    assert_eq!(counts, expected);
}

#[test]
fn ids_are_octo_shaped_and_per_listener() {
    let alice = playlist_id("alice", "genre:Rock");

    assert_eq!(alice.len(), 22);
    assert!(alice.starts_with("og"));
    assert_eq!(alice, playlist_id(" ALICE ", "genre:Rock"));
    assert_ne!(alice, playlist_id("bob", "genre:Rock"));
    assert_ne!(alice, playlist_id("alice", "genre:Pop"));
    // The reference values.
    assert_eq!(alice, "ogCBXI0xBkBbo5YFCGFg0J");
    assert_eq!(playlist_id(" ALICE ", "decade:1990"), "oggfZ1V0xbMkjnZt0DRDBI");
}

#[test]
fn period_index_changes_once_per_period() {
    let start = Utc.with_ymd_and_hms(2026, 9, 25, 0, 0, 0).unwrap();

    assert_eq!(
        period_index(start, 24),
        period_index(start + TimeDelta::minutes(23 * 60 + 54), 24)
    );
    assert_eq!(
        period_index(start, 24) + 1,
        period_index(start + TimeDelta::hours(24), 24)
    );
    assert_ne!(seed("alice", "genre:Rock", 1), seed("alice", "genre:Rock", 2));
    // The reference values.
    assert_eq!(seed("alice", "genre:Rock", 1), -1611485292);
    assert_eq!(seed("alice", "genre:Rock", 2), -847570069);
    assert_eq!(seed("bob", "decade:1990", 20000), -1817182269);
    assert_eq!(dotnet_ticks(now()), 639259344000000000);
}

fn station(i: i32) -> Song {
    Song {
        id: format!("st{i}"),
        artist: format!("Radio Artist {i}"),
        title: format!("Radio Song {i}"),
        ..Default::default()
    }
}

fn insert(node: &mut Node, key: &str, value: Node) {
    node.as_object_mut().expect("an object").insert(key.into(), value);
}

#[test]
fn blend_puts_new_library_songs_from_the_end_and_never_repeats_one() {
    let songs: Vec<Song> = (0..10).map(station).collect();
    let mut candidates = vec![
        song(100, "Radio Artist 3", Some(0), None),
        song(101, "Library One", Some(0), None),
        song(102, "Library Two", Some(7), None),
        song(103, "Library Three", Some(0), None),
    ];
    insert(&mut candidates[0], "title", Node::String("Radio Song 3".into()));
    insert(&mut candidates[1], "suffix", Node::String("mp3".into()));
    insert(&mut candidates[1], "bitRate", Node::Number("320".into()));
    insert(
        &mut candidates[1],
        "isrc",
        Node::Array(vec![
            Node::String("USRC17607839".into()),
            Node::String("us-rc1-76-07840".into()),
        ]),
    );

    let blended = blend(&songs, &candidates, 2, 30, now()).expect("two were put in");

    assert_eq!(blended.len(), 10);
    assert_eq!(blended[9].id, "s101");
    assert_eq!(blended[4].id, "s103");
    assert!(blended[9].is_local);
    assert_eq!(blended[9].suffix.as_deref(), Some("mp3"));
    assert_eq!(blended[9].bit_rate, Some(320));
    // The library's own ISRCs go back out exactly as Navidrome sent them.
    assert_eq!(
        blended[9].isrcs_for_clients(),
        ["USRC17607839", "us-rc1-76-07840"]
    );
    assert!(blended[4].isrcs_for_clients().is_empty());
    assert_eq!(blended.iter().filter(|song| song.id.starts_with("st")).count(), 8);
}

#[test]
fn blend_nothing_new_leaves_the_station_alone() {
    let songs: Vec<Song> = (0..10).map(station).collect();

    assert!(blend(&songs, &[song(1, "Library", Some(3), None)], 2, 30, now()).is_none());
}

/// The state file reads and writes back byte for byte, compact.
#[test]
fn the_state_fixture_round_trips_byte_for_byte() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/rust-migration/fixtures/state/generated-playlists.json"
    ))
    .expect("the fixture is in the repo");
    let document: StateDocument = serde_json::from_str(&text).expect("the fixture reads");
    let users = document.users.as_ref().expect("users");
    assert_eq!(users["brandon"].counts["genre:R&B"], 12);
    assert_eq!(users["brandon"].kinds, "genre,decade");
    assert_eq!(crate::json::to_string(&document), text);
}

#[test]
fn kinds_and_staleness_follow_the_settings() {
    let settings = GeneratedPlaylistSettings {
        enabled: true,
        genres: true,
        decades: false,
        new_releases: false,
        rediscover: false,
        deep_cuts: false,
        ..Default::default()
    };
    let all = ExplicitFilter::All;
    assert_eq!(kinds_of(&settings, all, false), "genre,,");
    let state = UserMixes {
        counts_utc: now(),
        kinds: "genre,,".into(),
        ..Default::default()
    };
    assert!(!is_stale(&state, &settings, all, false, now()));
    assert!(is_stale(
        &state,
        &settings,
        all,
        false,
        now() + TimeDelta::hours(i64::from(settings.effective_refresh_hours()))
    ));
    assert!(is_stale(&UserMixes::default(), &settings, all, false, now()));
    // Made for you: on by default whatever the mixes switch says, and what decides each list is
    // part of the kinds, the explicit filter too.
    let defaults = GeneratedPlaylistSettings::default();
    assert_eq!(
        kinds_of(&defaults, all, false),
        ",,newReleases+rediscover+deepCuts,w8a50x0,m6"
    );
    assert_eq!(
        kinds_of(&defaults, ExplicitFilter::CleanOnly, false),
        ",,newReleases+rediscover+deepCuts,w8a50x2,m6"
    );
    // Skip covers and instrumentals is part of New Releases' kinds only while it is on.
    assert_eq!(
        kinds_of(&defaults, all, true),
        ",,newReleases+rediscover+deepCuts,w8a50x0s,m6"
    );
    let saved = UserMixes {
        counts_utc: now(),
        kinds: kinds_of(&defaults, all, false),
        ..Default::default()
    };
    assert!(!is_stale(&saved, &defaults, all, false, now()));
    assert!(is_stale(&saved, &defaults, all, true, now()));
    assert_eq!(
        kinds_of(
            &GeneratedPlaylistSettings {
                new_releases: false,
                ..defaults.clone()
            },
            all,
            false
        ),
        ",,rediscover+deepCuts,m6"
    );
    assert_eq!(
        describe("decade:1990"),
        ("decade".to_string(), "1990s".to_string())
    );
    assert_eq!(describe("genre:Rock"), ("genre".to_string(), "Rock".to_string()));
}

#[test]
fn a_country_key_describes_itself_by_the_countrys_english_name() {
    assert_eq!(
        describe("country:ES"),
        ("country".to_string(), "Spain".to_string())
    );
    assert_eq!(
        describe("country:KR"),
        ("country".to_string(), "South Korea".to_string())
    );
    assert_eq!(country_key("JP"), "country:JP");
}

#[test]
fn country_mixes_are_part_of_the_kinds_only_when_on() {
    let all = ExplicitFilter::All;
    let mixes = GeneratedPlaylistSettings {
        enabled: true,
        new_releases: false,
        rediscover: false,
        deep_cuts: false,
        ..Default::default()
    };
    assert_eq!(kinds_of(&mixes, all, false), "genre,decade,");
    let countries = GeneratedPlaylistSettings {
        country_mixes: true,
        ..mixes.clone()
    };
    assert_eq!(kinds_of(&countries, all, false), "genre,decade,,country:");
    let some = GeneratedPlaylistSettings {
        countries: "es,jp".into(),
        ..countries.clone()
    };
    assert_eq!(kinds_of(&some, all, false), "genre,decade,,country:ES+JP");
    // Only countries is still mixes; the country switch without the mixes switch is nothing.
    let only = GeneratedPlaylistSettings {
        genres: false,
        decades: false,
        ..countries.clone()
    };
    assert!(mixes_on(&only) && country_mixes_on(&only));
    let off = GeneratedPlaylistSettings {
        enabled: false,
        ..countries
    };
    assert!(!mixes_on(&off) && !country_mixes_on(&off));
    assert_eq!(kinds_of(&off, all, false), ",,");
}

#[test]
fn countries_share_the_genre_thresholds_and_never_crowd_out_a_genre() {
    let settings = GeneratedPlaylistSettings {
        max_playlists: 2,
        max_country_playlists: 1,
        ..Default::default()
    };
    let counts: IndexMap<String, i32> = [
        ("country:US", 500),
        ("genre:Rock", 40),
        ("country:ES", 25),
        ("genre:Pop", 30),
        ("genre:Jazz", 21),
        ("country:FR", 12),
    ]
    .into_iter()
    .map(|(key, count)| (key.to_string(), count))
    .collect();

    assert_eq!(
        active_mixes(&counts, &[], &settings),
        ["genre:Rock", "genre:Pop", "country:US"]
    );
    // A country already shown stays down to RemoveBelow, as a genre does; one that never was
    // needs CreateAt.
    let wide = GeneratedPlaylistSettings {
        max_country_playlists: 5,
        ..settings.clone()
    };
    assert_eq!(
        active_mixes(&counts, &[], &wide),
        ["genre:Rock", "genre:Pop", "country:US", "country:ES"]
    );
    assert_eq!(
        active_mixes(&counts, &["country:FR".to_string()], &wide),
        [
            "genre:Rock",
            "genre:Pop",
            "country:US",
            "country:ES",
            "country:FR"
        ]
    );
    // Without countries the genres are what they always were.
    let genres: IndexMap<String, i32> = counts
        .iter()
        .filter(|(key, _)| key.starts_with("genre:"))
        .map(|(key, count)| (key.clone(), *count))
        .collect();
    assert_eq!(
        active_mixes(&genres, &[], &settings),
        apply_hysteresis(&genres, &[], 20, 10, 2)
    );
}

#[test]
fn songs_count_under_known_countries_and_only_the_ones_asked_for() {
    let songs = [Some("ES"), Some("ES"), Some(""), None, Some("JP"), Some("US")];
    let any = count_countries(songs, &[]);
    assert_eq!(
        any.into_iter().collect::<Vec<_>>(),
        [
            ("country:ES".to_string(), 2),
            ("country:JP".to_string(), 1),
            ("country:US".to_string(), 1)
        ]
    );
    let some = count_countries(songs, &["JP", "ES"]);
    assert_eq!(some.keys().collect::<Vec<_>>(), ["country:ES", "country:JP"]);
}

#[test]
fn numbers_read_as_json_node_reads_them() {
    let node = Node::parse(r#"{"a":5,"b":5.9,"c":1e3,"d":"5","e":null,"f":99999999999}"#).unwrap();
    assert_eq!(nullable_int_of(&node, "a"), Some(5));
    assert_eq!(nullable_int_of(&node, "b"), Some(5));
    assert_eq!(nullable_int_of(&node, "c"), Some(1000));
    assert_eq!(nullable_int_of(&node, "d"), None);
    assert_eq!(nullable_int_of(&node, "e"), None);
    assert_eq!(nullable_int_of(&node, "f"), Some(i32::MAX));
    assert_eq!(int_of(&node, "missing"), 0);
}

// ---- Year and artist mixes ------------------------------------------------------------------

#[test]
fn a_year_key_describes_itself_and_an_artist_key_by_its_artist() {
    assert_eq!(
        describe(&year_key(1994)),
        ("year".to_string(), "1994".to_string())
    );
    assert_eq!(year_key(1994), "year:1994");
    assert_eq!(
        describe(&artist_mix_key("ar-1")),
        ("artist".to_string(), "ar-1".to_string())
    );
    assert_eq!(list_group("year"), "year");
    assert_eq!(list_group("artist"), "artist");
    assert_eq!(
        list_group(crate::library::for_you_lists::DEEP_CUTS_KIND),
        "forYou"
    );
    assert_eq!(list_group(crate::library::for_you_lists::LIKED_KIND), "liked");
}

#[test]
fn year_and_artist_mixes_are_part_of_the_kinds_only_when_on() {
    let all = ExplicitFilter::All;
    let mixes = GeneratedPlaylistSettings {
        enabled: true,
        new_releases: false,
        rediscover: false,
        deep_cuts: false,
        ..Default::default()
    };
    assert_eq!(kinds_of(&mixes, all, false), "genre,decade,");
    let years = GeneratedPlaylistSettings {
        years: true,
        ..mixes.clone()
    };
    assert_eq!(kinds_of(&years, all, false), "genre,decade,,year");
    let artists = GeneratedPlaylistSettings {
        artists: true,
        artist_count: 7,
        ..years.clone()
    };
    assert_eq!(kinds_of(&artists, all, false), "genre,decade,,year,artist7");
    // Years or artists alone are still mixes; their switches without the mixes switch are not.
    let only = GeneratedPlaylistSettings {
        genres: false,
        decades: false,
        ..artists.clone()
    };
    assert!(mixes_on(&only) && year_mixes_on(&only) && artist_mixes_on(&only));
    let off = GeneratedPlaylistSettings {
        enabled: false,
        ..artists
    };
    assert!(!mixes_on(&off) && !year_mixes_on(&off) && !artist_mixes_on(&off));
    assert_eq!(kinds_of(&off, all, false), ",,");
}

#[test]
fn years_are_counted_from_the_walk_each_song_under_its_own_year() {
    let walk: Vec<Node> = [
        r#"{"id":"1","year":1994}"#,
        r#"{"id":"2","year":1994}"#,
        r#"{"id":"3","year":2003}"#,
        r#"{"id":"4","year":0}"#,
        r#"{"id":"5"}"#,
        r#"{"id":"6","year":"1994"}"#,
        r#"{"id":"7","year":1994.0}"#,
    ]
    .iter()
    .map(|text| Node::parse(text).expect("a song parses"))
    .collect();

    let counts = count_years(&walk);

    assert_eq!(
        counts.into_iter().collect::<Vec<_>>(),
        [("year:1994".to_string(), 3), ("year:2003".to_string(), 1)]
    );
}

#[test]
fn years_rank_by_songs_newest_first_on_ties_with_their_own_cap_and_thresholds() {
    let settings = GeneratedPlaylistSettings {
        max_playlists: 1,
        max_year_playlists: 3,
        ..Default::default()
    };
    let counts: IndexMap<String, i32> = [
        ("genre:Rock", 40),
        ("genre:Pop", 30),
        ("year:1994", 30),
        ("year:2001", 30),
        ("year:1980", 60),
        ("year:2010", 25),
        ("year:2020", 15),
    ]
    .into_iter()
    .map(|(key, count)| (key.to_string(), count))
    .collect();

    // Years never push a genre out, and a genre never pushes a year out.
    assert_eq!(
        active_mixes(&counts, &[], &settings),
        ["genre:Rock", "year:1980", "year:2001", "year:1994"]
    );
    // A year already shown stays down to RemoveBelow; one that never was needs CreateAt.
    let wide = GeneratedPlaylistSettings {
        max_year_playlists: 30,
        ..settings.clone()
    };
    assert_eq!(
        active_mixes(&counts, &[], &wide),
        ["genre:Rock", "year:1980", "year:2001", "year:1994", "year:2010"]
    );
    assert_eq!(
        active_mixes(&counts, &["year:2020".to_string()], &wide),
        [
            "genre:Rock",
            "year:1980",
            "year:2001",
            "year:1994",
            "year:2010",
            "year:2020"
        ]
    );
    // MaxYearPlaylists is at least 1 and at most 30.
    let none = GeneratedPlaylistSettings {
        max_year_playlists: 0,
        ..settings
    };
    assert_eq!(active_mixes(&counts, &[], &none), ["genre:Rock", "year:1980"]);
}

fn ranked(count: usize) -> Vec<String> {
    (1..=count)
        .map(|rank| artist_mix_key(&format!("ar{rank}")))
        .collect()
}

fn began(keys: &[(&str, i64)]) -> IndexMap<String, DateTime<Utc>> {
    keys.iter()
        .map(|(key, days)| (artist_mix_key(key), now() - TimeDelta::days(*days)))
        .collect()
}

#[test]
fn an_artist_joins_in_the_top_count_and_never_below() {
    let kept = kept_artist_mixes(&ranked(8), &IndexMap::new(), 5, now());
    assert_eq!(kept, ranked(5));
}

#[test]
fn an_artist_mix_stays_at_ranks_six_to_eight_and_goes_at_nine_only_after_seven_days() {
    // ar1..ar5 lead now; the old mixes ar6, ar7 and ar8 are ranked 6 to 8, ar9 9th.
    let previous = began(&[("ar6", 30), ("ar7", 30), ("ar8", 30), ("ar9", 30)]);
    let kept = kept_artist_mixes(&ranked(9), &previous, 5, now());
    assert_eq!(kept, ranked(8));

    // At rank 9 a mix younger than seven days stays; at seven days it goes.
    let young = began(&[("ar9", 6)]);
    let kept = kept_artist_mixes(&ranked(9), &young, 5, now());
    assert_eq!(
        kept,
        ranked(5)
            .into_iter()
            .chain([artist_mix_key("ar9")])
            .collect::<Vec<_>>()
    );
    let week = began(&[("ar9", 7)]);
    assert_eq!(kept_artist_mixes(&ranked(9), &week, 5, now()), ranked(5));
}

#[test]
fn an_artist_mix_whose_artist_left_the_ranking_stays_its_first_week_after_the_ranked() {
    let previous = began(&[("gone-new", 2), ("gone-old", 8), ("gone-newer", 1)]);
    let kept = kept_artist_mixes(&ranked(3), &previous, 3, now());
    assert_eq!(
        kept,
        ranked(3)
            .into_iter()
            .chain([artist_mix_key("gone-new"), artist_mix_key("gone-newer")])
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_state_keeps_artist_mixes_and_leaves_them_out_of_the_file_while_there_are_none() {
    let mut mixes = UserMixes {
        counts_utc: now(),
        kinds: "genre,,".into(),
        ..Default::default()
    };
    let without = crate::json::to_string(&mixes);
    assert!(!without.contains("ArtistMixes"), "{without}");

    mixes.artist_mixes.insert(
        artist_mix_key("ar1"),
        ArtistMix {
            name: "Radiohead".into(),
            since: now(),
            songs: vec![r#"{"id":"s1"}"#.into()],
        },
    );
    let text = crate::json::to_string(&mixes);
    assert!(
        text.contains(r#""ArtistMixes":{"artist:ar1":{"Name":"Radiohead","Since":"#),
        "{text}"
    );
    let read: UserMixes = serde_json::from_str(&text).expect("the state reads");
    assert_eq!(read, mixes);
    // A file from before artist mixes reads with none.
    let old: UserMixes = serde_json::from_str(&without).expect("an older state reads");
    assert!(old.artist_mixes.is_empty());
}
