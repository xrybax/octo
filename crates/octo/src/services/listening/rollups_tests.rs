use chrono::{FixedOffset, TimeZone, Utc};

use super::*;

/// Pacific daylight time, so local days differ from UTC ones.
fn pacific() -> FixedOffset {
    FixedOffset::west_opt(7 * 3600).unwrap()
}

/// 2026-10-20, a Tuesday, at 15:00 Pacific.
fn now() -> DateTime<Utc> {
    pacific()
        .with_ymd_and_hms(2026, 10, 20, 15, 0, 0)
        .unwrap()
        .with_timezone(&Utc)
}

/// A local time in Pacific.
fn at(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    pacific()
        .with_ymd_and_hms(2026, 10, day, hour, minute, 0)
        .unwrap()
        .with_timezone(&Utc)
}

fn play(artist: &str, title: &str, when: DateTime<Utc>, seconds: i64) -> Play {
    Play {
        song_id: format!("{artist}-{title}"),
        cover_id: format!("cover-{artist}-{title}"),
        artist: artist.into(),
        title: title.into(),
        album: "Album".into(),
        seconds,
        is_local: true,
        played_at: when,
        ..Default::default()
    }
}

fn rows(plays: &[Play]) -> Vec<(&str, &Play)> {
    plays.iter().map(|p| ("winters", p)).collect()
}

#[test]
fn ranges_are_read_from_their_numbers() {
    assert_eq!(range_of(Some("7"), 90), 7);
    assert_eq!(range_of(Some("90d"), 90), 90);
    assert_eq!(range_of(Some(" 30 "), 90), 30);
    assert_eq!(range_of(Some("14"), 90), DEFAULT_RANGE);
    assert_eq!(range_of(None, 90), DEFAULT_RANGE);
}

#[test]
fn a_year_is_offered_only_when_a_year_is_kept() {
    assert_eq!(range_of(Some("365"), 90), DEFAULT_RANGE);
    assert_eq!(range_of(Some("365"), 364), DEFAULT_RANGE);
    assert_eq!(range_of(Some("365d"), 365), 365);
    assert_eq!(ranges(90), [7, 30, 90]);
    assert_eq!(ranges(365), [7, 30, 90, 365]);
}

#[test]
fn a_play_written_with_no_platform_takes_its_apps() {
    let mut plays = Vec::new();
    for (app, platform) in [
        ("Amperfy 1.4", "unknown"),
        ("Amperfy 1.4", "iphone"),
        ("DSub", ""),
        ("Octo 1.9.0 (Android)", "unknown"),
        ("Unknown app", "unknown"),
    ] {
        let mut p = play("A", app, at(19, 10, 0), 60);
        p.app = app.into();
        p.platform = platform.into();
        plays.push(p);
    }
    let r = rollup(&rows(&plays), now(), 7, &pacific());
    let apps: Vec<(&str, &str, usize)> = r
        .apps
        .iter()
        .map(|a| (a.name.as_str(), a.platform.as_str(), a.plays))
        .collect();
    assert_eq!(
        apps,
        [
            ("Amperfy", "iphone", 2),
            ("DSub", "android", 1),
            ("Octo for Android", "android", 1),
            (UNKNOWN_APP, UNKNOWN_PLATFORM, 1),
        ]
    );
}

#[test]
fn a_range_is_today_and_the_days_before_it_in_local_time() {
    let plays = vec![
        // Monday 23:30 Pacific is already Tuesday in UTC: it still counts for Monday.
        play("Drake", "One", at(19, 23, 30), 1800),
        play("Drake", "Two", at(20, 9, 0), 3600),
        // The first day of the range, just after midnight.
        play("Drake", "Three", at(14, 0, 5), 600),
        // The day before the range starts.
        play("Drake", "Four", at(13, 23, 55), 600),
    ];
    let r = rollup(&rows(&plays), now(), 7, &pacific());
    assert_eq!(r.per_day.len(), 7);
    assert_eq!(r.per_day[0].day, "2026-10-14");
    assert_eq!(r.per_day[6].day, "2026-10-20");
    assert_eq!(r.per_day[5].hours, 0.5);
    assert_eq!(r.per_day[6].hours, 1.0);
    assert_eq!(r.plays, 3);
    assert!((r.hours - (1800.0 + 3600.0 + 600.0) / 3600.0).abs() < 0.01);
    assert_eq!(r.hours_before, 0.17);
}

#[test]
fn the_week_grid_holds_minutes_per_local_hour_monday_first() {
    let plays = vec![
        play("A", "x", at(19, 9, 30), 1800),
        play("A", "y", at(19, 9, 50), 600),
        play("A", "z", at(18, 22, 0), 300),
    ];
    let r = rollup(&rows(&plays), now(), 7, &pacific());
    assert_eq!(r.grid.len(), 7);
    assert!(r.grid.iter().all(|row| row.len() == 24));
    // Monday 9 AM: 30 + 10 minutes.
    assert_eq!(r.grid[0][9], 40.0);
    // Sunday 10 PM.
    assert_eq!(r.grid[6][22], 5.0);
    assert_eq!(r.grid.iter().flatten().sum::<f64>(), 45.0);
}

#[test]
fn plays_less_than_twenty_minutes_apart_are_one_session() {
    let plays = vec![
        play("A", "1", at(19, 10, 0), 600),
        // Starts 15 minutes after the first ended.
        play("A", "2", at(19, 10, 25), 600),
        play("A", "3", at(19, 10, 35), 300),
        // 25 minutes after: a new session.
        play("A", "4", at(19, 11, 5), 300),
        play("A", "5", at(18, 20, 0), 1200),
    ];
    let r = rollup(&rows(&plays), now(), 7, &pacific());
    let session = r.longest_session.expect("a session");
    assert_eq!(session.minutes, 40);
    assert_eq!(session.plays, 3);
    assert_eq!(session.start, at(19, 10, 0));
    assert_eq!(session.who, None);
}

#[test]
fn two_peoples_plays_never_make_one_session() {
    let mine = play("A", "1", at(19, 10, 0), 600);
    let theirs = play("B", "2", at(19, 10, 12), 600);
    let r = rollup(&[("winters", &mine), ("alex", &theirs)], now(), 7, &pacific());
    let session = r.longest_session.expect("a session");
    assert_eq!(session.minutes, 10);
    assert_eq!(session.plays, 1);
    assert!(session.who.is_some());
}

#[test]
fn streaks_count_days_in_a_row_and_the_one_reaching_today() {
    let plays = vec![
        play("A", "1", at(1, 12, 0), 60),
        play("A", "2", at(2, 12, 0), 60),
        play("A", "3", at(3, 12, 0), 60),
        play("A", "4", at(4, 23, 0), 60),
        play("A", "5", at(10, 12, 0), 60),
        // Nothing today yet: the run up to yesterday is the current one.
        play("A", "6", at(18, 12, 0), 60),
        play("A", "7", at(19, 12, 0), 60),
    ];
    let r = rollup(&rows(&plays), now(), 30, &pacific());
    assert_eq!(r.best_streak, 4);
    assert_eq!(r.current_streak, 2);
    let gone = vec![play("A", "1", at(15, 12, 0), 60)];
    assert_eq!(rollup(&rows(&gone), now(), 30, &pacific()).current_streak, 0);
}

#[test]
fn a_change_is_shown_only_when_the_range_before_is_mostly_on_record() {
    // On record since long before the range before: the change is shown.
    let plays = vec![
        play("A", "old", at(1, 12, 0), 60),
        play("A", "before", at(10, 12, 0), 3600),
        play("A", "now", at(18, 12, 0), 5400),
    ];
    let r = rollup(&rows(&plays), now(), 7, &pacific());
    assert_eq!(r.change.kind, "change");
    assert_eq!(r.change.percent, Some(50));

    // On record for only half of the range before: "new", with the first day.
    let late = vec![
        play("A", "before", at(11, 12, 0), 3600),
        play("A", "now", at(18, 12, 0), 5400),
    ];
    let r = rollup(&rows(&late), now(), 7, &pacific());
    assert_eq!(r.change.kind, "new");
    assert_eq!(r.change.percent, None);
    assert_eq!(r.change.since.as_deref(), Some("2026-10-11"));

    // Exactly three quarters of it is enough.
    let edge_day = at(7, 0, 0) + TimeDelta::seconds((0.25 * 7.0 * 86_400.0) as i64);
    let edge = vec![
        play("A", "before", edge_day, 3600),
        play("A", "now", at(18, 12, 0), 3600),
    ];
    assert_eq!(rollup(&rows(&edge), now(), 7, &pacific()).change.kind, "change");

    // Nothing at all before: never a change.
    let fresh = vec![
        play("A", "old", at(1, 12, 0), 60),
        play("A", "now", at(18, 12, 0), 60),
    ];
    assert_eq!(rollup(&rows(&fresh), now(), 7, &pacific()).change.kind, "new");
}

#[test]
fn top_artists_and_songs_are_by_plays_with_their_covers() {
    let mut plays = Vec::new();
    for minute in 0..3 {
        plays.push(play("Drake", "Nonstop", at(19, 10, minute * 4), 200));
    }
    plays.push(play("drake", "Passionfruit", at(19, 11, 0), 300));
    plays.push(play("Kavinsky", "Nightcall", at(19, 12, 0), 250));
    plays.push(play("Kavinsky", "Nightcall", at(19, 12, 5), 250));
    let r = rollup(&rows(&plays), now(), 7, &pacific());
    assert_eq!(r.top_artists[0].name, "Drake");
    assert_eq!(r.top_artists[0].plays, 4);
    assert_eq!(r.top_artists[0].minutes, 15.0);
    assert_eq!(r.top_artists[0].cover_id, "cover-Drake-Nonstop");
    assert_eq!(r.top_artists[1].name, "Kavinsky");
    assert_eq!(r.top_songs[0].title, "Nonstop");
    assert_eq!(r.top_songs[0].plays, 3);
    assert_eq!(r.top_songs[1].title, "Nightcall");
    assert_eq!(r.unique_songs, 3);
    assert_eq!(r.artists, 2);
}

#[test]
fn genres_merge_spellings_and_untagged_hours_are_apart() {
    let mut plays = vec![
        play("A", "1", at(19, 10, 0), 3600),
        play("A", "2", at(19, 11, 0), 1800),
        play("A", "3", at(19, 12, 0), 1800),
        play("A", "4", at(19, 13, 0), 900),
    ];
    plays[0].genre = Some("Hip-Hop".into());
    plays[1].genre = Some("hip hop".into());
    plays[2].genre = Some("Rap".into());
    let r = rollup(&rows(&plays), now(), 7, &pacific());
    assert_eq!(
        r.genres,
        vec![
            GenreHours {
                name: "Hip-Hop".into(),
                hours: 1.5
            },
            GenreHours {
                name: "Rap".into(),
                hours: 0.5
            },
        ]
    );
    assert_eq!(r.untagged_hours, 0.25);
}

#[test]
fn apps_are_grouped_by_name_and_platform_without_versions() {
    let mut plays = Vec::new();
    for (app, platform) in [
        ("Octo 1.9.0 (Android)", "android"),
        ("Octo 1.9.1 (Android)", "android"),
        ("Octo", "iphone"),
        ("Symfonium 12.3.0", "android"),
    ] {
        let mut p = play("A", app, at(19, 10, 0), 60);
        p.app = app.into();
        p.platform = platform.into();
        plays.push(p);
    }
    let r = rollup(&rows(&plays), now(), 7, &pacific());
    let names: Vec<(&str, usize)> = r.apps.iter().map(|a| (a.name.as_str(), a.plays)).collect();
    assert_eq!(
        names,
        [("Octo for Android", 2), ("Octo for iPhone", 1), ("Symfonium", 1)]
    );
    assert_eq!(r.apps[0].share, 0.5);
    assert_eq!(app_name("", "unknown"), UNKNOWN_APP);
    assert_eq!(app_name("Octo 1.9.0", "unknown"), "Octo");
}

#[test]
fn library_share_and_hearted_songs_newest_first() {
    let mut plays = vec![
        play("A", "1", at(19, 10, 0), 60),
        play("A", "2", at(19, 11, 0), 60),
        play("A", "2", at(19, 12, 0), 60),
        play("A", "3", at(19, 13, 0), 60),
    ];
    plays[0].is_local = false;
    plays[1].hearted = true;
    plays[2].hearted = true;
    plays[3].hearted = true;
    plays[3].is_local = false;
    let r = rollup(&rows(&plays), now(), 7, &pacific());
    assert_eq!(r.library_share, 0.5);
    let hearted: Vec<&str> = r.hearted.iter().map(|h| h.title.as_str()).collect();
    assert_eq!(hearted, ["3", "2"]);
    assert!(!r.hearted[0].local);
}

#[test]
fn nothing_played_is_an_empty_page() {
    let r = rollup(&[], now(), 90, &Utc);
    assert_eq!(r.plays, 0);
    assert_eq!(r.hours, 0.0);
    assert_eq!(r.per_day.len(), 90);
    assert!(r.top_artists.is_empty());
    assert!(r.longest_session.is_none());
    assert_eq!(r.change.kind, "new");
    assert_eq!(r.change.since, None);
    assert_eq!(r.first_play, None);
}

#[test]
fn a_persons_card_has_their_hours_genre_and_days() {
    let mut plays = vec![
        play("A", "1", at(19, 10, 0), 3600),
        play("A", "2", at(20, 9, 0), 1800),
        play("A", "3", at(1, 9, 0), 1800),
    ];
    plays[0].genre = Some("Rock".into());
    let card = person("Winters", &plays, now(), 7, &pacific());
    assert_eq!(card.name, "Winters");
    assert_eq!(card.hours, 1.5);
    assert_eq!(card.plays, 2);
    assert_eq!(card.top_genre.as_deref(), Some("Rock"));
    assert_eq!(card.per_day, [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.5]);
}
