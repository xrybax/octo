//! Every Octo list row says which group it belongs to (`octoGroup`): getPlaylists and getPlaylist
//! carry it in JSON and XML for Liked Songs, Made for you and every kind of mix. Chart and station
//! rows are checked where those lists are made (`top_songs_tests`, `route_tests::radio`).

use std::collections::HashMap;
use std::time::Duration;

use octo_core::settings::{AppSettings, GeneratedPlaylistSettings, SubsonicSettings};
use serde_json::Value;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::test_app::{app, get};
use crate::app::AppState;
use crate::http::pipeline::App;

const AUTH: &str = "u=alice&t=token&s=salt&v=1.16.1&c=test";

/// A Navidrome whose library holds 25 songs by each of two artists, a genre, a decade's songs
/// and one heart.
struct GroupsNavidrome;

fn songs_of(artist: &str, id: &str, year: i32, plays: i32, from: usize) -> Vec<String> {
    (from..from + 25)
        .map(|i| {
            // The first ten are loved long ago (Rediscover), the rest barely played (Deep Cuts).
            let (count, rating, played) = if i - from < 10 {
                (plays, 4, "2025-01-01T00:00:00Z")
            } else {
                (0, 0, "2026-10-01T00:00:00Z")
            };
            format!(
                r#"{{"id":"s{i}","title":"Song {i}","artist":"{artist}","artistId":"{id}","year":{year},"genre":"Rock","duration":200,"playCount":{count},"userRating":{rating},"played":"{played}","coverArt":"al-{id}"}}"#
            )
        })
        .collect()
}

fn library() -> String {
    let mut songs = songs_of("Bad Bunny", "arPR", 2019, 5, 0);
    songs.extend(songs_of("Rosalía", "arES", 2001, 4, 100));
    songs.join(",")
}

fn envelope(inner: &str) -> String {
    format!(r#"{{"subsonic-response":{{"status":"ok","version":"1.16.1"{inner}}}}}"#)
}

impl Respond for GroupsNavidrome {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request
            .url
            .path()
            .trim_matches('/')
            .trim_end_matches(".view")
            .to_string();
        let query: HashMap<String, String> = request.url.query_pairs().into_owned().collect();
        let json = query.get("f").map(String::as_str) == Some("json");
        let body = match path.as_str() {
            "rest/getPlaylists" if !json => {
                return ResponseTemplate::new(200).set_body_raw(
                    r#"<subsonic-response xmlns="http://subsonic.org/restapi" status="ok" version="1.16.1"><playlists></playlists></subsonic-response>"#,
                    "application/xml",
                );
            }
            "rest/getPlaylists" => envelope(r#","playlists":{"playlist":[]}"#),
            "rest/ping" if !json => {
                return ResponseTemplate::new(200).set_body_raw(
                    r#"<subsonic-response xmlns="http://subsonic.org/restapi" status="ok" version="1.16.1"></subsonic-response>"#,
                    "application/xml",
                );
            }
            "rest/getGenres" => envelope(r#","genres":{"genre":[{"value":"Rock","songCount":50}]}"#),
            "rest/getSongsByGenre" if query.get("offset").map(String::as_str) == Some("0") => {
                envelope(&format!(r#","songsByGenre":{{"song":[{}]}}"#, library()))
            }
            "rest/getSongsByGenre" => envelope(r#","songsByGenre":{"song":[]}"#),
            "rest/getRandomSongs" => match query.get("fromYear").map(String::as_str) {
                Some("1990") => envelope(&format!(
                    r#","randomSongs":{{"song":[{}]}}"#,
                    songs_of("Nineties", "ar90", 1995, 1, 500).join(",")
                )),
                Some("2019") => envelope(&format!(
                    r#","randomSongs":{{"song":[{}]}}"#,
                    songs_of("Bad Bunny", "arPR", 2019, 5, 0).join(",")
                )),
                _ => envelope(r#","randomSongs":{"song":[]}"#),
            },
            "rest/search3" if query.get("songOffset").map(String::as_str) == Some("0") => {
                envelope(&format!(r#","searchResult3":{{"song":[{}]}}"#, library()))
            }
            "rest/search3" => envelope(r#","searchResult3":{"song":[]}"#),
            "rest/getStarred2" => envelope(
                r#","starred2":{"song":[{"id":"s3","title":"Song 3","artist":"Bad Bunny","duration":200,"starred":"2026-10-01T00:00:00Z"}]}"#,
            ),
            _ => envelope(""),
        };
        ResponseTemplate::new(200).set_body_raw(body, "application/json")
    }
}

async fn groups_app() -> (MockServer, App) {
    let server = MockServer::builder().start().await;
    Mock::given(any())
        .respond_with(GroupsNavidrome)
        .mount(&server)
        .await;
    let state = AppState::for_tests(AppSettings {
        subsonic: SubsonicSettings {
            url: Some(server.uri()),
            auto_detect_download_path: false,
            ..Default::default()
        },
        generated_playlists: GeneratedPlaylistSettings {
            enabled: true,
            years: true,
            artists: true,
            artist_count: 1,
            new_releases: false,
            popular_now: false,
            charts: String::new(),
            ..Default::default()
        },
        ..Default::default()
    });
    (server, app(&state))
}

/// The Octo rows of getPlaylists as (name, octoGroup, octoList), once the mixes are counted.
async fn octo_rows(app: &App) -> Vec<(String, String, Option<String>)> {
    let mut rows = Vec::new();
    for _ in 0..100 {
        let body = get(app, &format!("/rest/getPlaylists.view?{AUTH}&f=json"))
            .await
            .json();
        rows = body["subsonic-response"]["playlists"]["playlist"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter(|row| row["id"].as_str().is_some_and(|id| id.starts_with("og")))
                    .map(|row| {
                        (
                            row["name"].as_str().unwrap_or_default().to_string(),
                            row["octoGroup"].as_str().unwrap_or("(none)").to_string(),
                            row["octoList"].as_str().map(str::to_string),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if rows.iter().any(|(_, group, _)| group == "artist") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    rows
}

#[tokio::test]
async fn every_octo_list_row_says_its_group_in_json_and_xml() {
    let (_server, app) = groups_app().await;

    let rows = octo_rows(&app).await;

    let groups: Vec<(&str, &str)> = rows
        .iter()
        .map(|(name, group, _)| (name.as_str(), group.as_str()))
        .collect();
    assert_eq!(
        groups,
        [
            ("Liked Songs", "liked"),
            ("Rediscover", "forYou"),
            ("Deep Cuts", "forYou"),
            ("Rock Mix", "genre"),
            ("1990s Mix", "decade"),
            ("2019 Mix", "year"),
            ("2001 Mix", "year"),
            ("Bad Bunny Mix", "artist"),
        ]
    );
    // octoList is as it was: Made for you and Liked Songs only.
    let lists: Vec<Option<&str>> = rows.iter().map(|(_, _, list)| list.as_deref()).collect();
    assert_eq!(
        lists,
        [
            Some("liked"),
            Some("rediscover"),
            Some("deepCuts"),
            None,
            None,
            None,
            None,
            None
        ]
    );

    let xml = get(&app, &format!("/rest/getPlaylists.view?{AUTH}")).await.text();
    for (name, group, _) in &rows {
        let row = xml
            .split("<playlist ")
            .find(|row| row.contains(&format!(r#"name="{name}""#)))
            .unwrap_or_else(|| panic!("{name} in {xml}"));
        assert!(row.contains(&format!(r#"octoGroup="{group}""#)), "{name}: {row}");
    }
}

#[tokio::test]
async fn get_playlist_carries_the_group_of_a_year_and_an_artist_mix() {
    let (_server, app) = groups_app().await;
    octo_rows(&app).await;
    let body = get(&app, &format!("/rest/getPlaylists.view?{AUTH}&f=json"))
        .await
        .json();
    let id_of = |name: &str| -> String {
        body["subsonic-response"]["playlists"]["playlist"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["name"] == name))
            .and_then(|row| row["id"].as_str())
            .unwrap_or_else(|| panic!("{name} is listed"))
            .to_string()
    };

    // A year mix keeps the artist cap of 3; an artist mix never caps its own artist.
    for (name, group, songs) in [("2019 Mix", "year", 3), ("Bad Bunny Mix", "artist", 25)] {
        let id = id_of(name);
        let detail = get(&app, &format!("/rest/getPlaylist.view?{AUTH}&f=json&id={id}"))
            .await
            .json();
        let playlist: &Value = &detail["subsonic-response"]["playlist"];
        assert_eq!(playlist["name"], name);
        assert_eq!(playlist["octoGroup"], group, "{name}");
        assert!(playlist.get("octoList").is_none(), "{name}");
        let entries = playlist["entry"].as_array().expect("entries");
        assert_eq!(entries.len(), songs, "{name}");
        assert!(
            entries.iter().all(|song| song["artist"] == "Bad Bunny"),
            "{name}: {entries:?}"
        );

        let xml = get(&app, &format!("/rest/getPlaylist.view?{AUTH}&id={id}"))
            .await
            .text();
        assert!(xml.contains(&format!(r#"octoGroup="{group}""#)), "{name}: {xml}");
    }
}
