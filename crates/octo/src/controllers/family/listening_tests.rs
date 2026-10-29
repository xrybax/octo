//! Listening on the Family page: each member's own, and everyone's for managers.

use super::*;
use crate::services::listening::Play;

fn listened(id: &str) -> Play {
    Play {
        song_id: id.into(),
        cover_id: id.into(),
        artist: "Kavinsky".into(),
        title: format!("Song {id}"),
        album: "OutRun".into(),
        seconds: 240,
        is_local: true,
        played_at: Utc::now() - chrono::TimeDelta::hours(2),
        ..Default::default()
    }
}

async fn with_plays() -> Fam {
    let f = fam().await;
    assert!(f.state.listening.record("alex", listened("alex-1")));
    assert!(f.state.listening.record("alex", listened("alex-2")));
    assert!(f.state.listening.record("sam", listened("sam-1")));
    f
}

#[tokio::test]
async fn listening_needs_a_sign_in() {
    let f = with_plays().await;
    assert_eq!(
        f.get("/api/family/me/listening", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.get("/api/family/listening", None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_member_sees_their_own_listening_and_nobody_elses() {
    let f = with_plays().await;
    let alex = f.alex();
    let mine = f.get("/api/family/me/listening?range=7", Some(&alex)).await;
    assert_eq!(mine.status, StatusCode::OK, "{}", mine.body);
    let mine = mine.json();
    assert_eq!(mine["user"], "alex");
    assert_eq!(mine["range"], 7);
    assert_eq!(mine["everyone"], false);
    assert!(mine["people"].is_null());
    assert_eq!(mine["rollup"]["plays"], 2);
    // Asking for someone else still answers with their own.
    let asked = f
        .get("/api/family/me/listening?user=sam", Some(&alex))
        .await
        .json();
    assert_eq!(asked["user"], "alex");
    assert_eq!(asked["rollup"]["plays"], 2);
    // Everyone's is for managers.
    let refused = f.get("/api/family/listening?user=sam", Some(&alex)).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_manager_sees_everyone_and_each_person() {
    let f = with_plays().await;
    let sam = f.sam();
    let all = f.get("/api/family/listening?range=30", Some(&sam)).await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.body);
    let all = all.json();
    assert_eq!(all["everyone"], true);
    assert_eq!(all["rollup"]["plays"], 3);
    let names: Vec<&str> = all["people"]
        .as_array()
        .expect("people")
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert_eq!(names, ["alex", "sam"]);
    let alex = f.get("/api/family/listening?user=alex", Some(&sam)).await.json();
    assert_eq!(alex["user"], "alex");
    assert_eq!(alex["rollup"]["plays"], 2);
    // The owner manages the family too.
    let owner = f.get("/api/family/listening", Some(&f.owner_cookie())).await;
    assert_eq!(owner.status, StatusCode::OK, "{}", owner.body);
}
