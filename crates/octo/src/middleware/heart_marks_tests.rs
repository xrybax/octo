use chrono::TimeZone;

use super::*;

fn marks() -> Vec<(String, DateTime<Utc>)> {
    vec![(
        "ext-a".to_string(),
        Utc.with_ymd_and_hms(2026, 10, 14, 22, 0, 0).unwrap(),
    )]
}

#[test]
fn a_marked_song_anywhere_in_a_json_answer_reads_as_a_favorite() {
    let body = br#"{"subsonic-response":{"status":"ok","album":{"id":"al-1","song":[{"id":"ext-a","title":"A"},{"id":"nd-b","title":"B"}]}}}"#;

    let marked = mark(body, &marks()).expect("changed");
    let value: Value = serde_json::from_slice(&marked).expect("JSON");
    let songs = &value["subsonic-response"]["album"]["song"];
    assert_eq!(songs[0]["starred"], "2026-10-14T22:00:00.000Z");
    assert!(songs[1].get("starred").is_none());
    assert!(value["subsonic-response"]["album"].get("starred").is_none());
}

#[test]
fn a_single_song_and_xml_answers_are_marked_too() {
    let json = br#"{"subsonic-response":{"status":"ok","song":{"id":"ext-a","title":"A"}}}"#;
    let marked: Value = serde_json::from_slice(&mark(json, &marks()).expect("changed")).expect("JSON");
    assert_eq!(
        marked["subsonic-response"]["song"]["starred"],
        "2026-10-14T22:00:00.000Z"
    );

    let xml = br#"<subsonic-response xmlns="http://subsonic.org/restapi" status="ok"><searchResult3><song id="ext-a" title="A"></song><song id="nd-b" title="B"></song></searchResult3></subsonic-response>"#;
    let marked = String::from_utf8(mark(xml, &marks()).expect("changed")).expect("UTF-8");
    assert!(
        marked.contains(r#"id="ext-a" title="A" starred="2026-10-14T22:00:00.000Z""#),
        "{marked}"
    );
    assert_eq!(marked.matches("starred=").count(), 1, "{marked}");
}

#[test]
fn a_song_that_says_when_it_was_starred_keeps_it_and_nothing_else_changes() {
    let starred =
        br#"{"subsonic-response":{"status":"ok","song":{"id":"ext-a","starred":"2020-01-01T00:00:00Z"}}}"#;
    assert!(mark(starred, &marks()).is_none());
    let folder =
        br#"{"subsonic-response":{"status":"ok","directory":{"child":[{"id":"ext-a","isDir":true}]}}}"#;
    assert!(mark(folder, &marks()).is_none());
    let failed = br#"{"subsonic-response":{"status":"failed","song":{"id":"ext-a"}}}"#;
    assert!(mark(failed, &marks()).is_none());
    assert!(mark(b"not an answer", &marks()).is_none());
}
