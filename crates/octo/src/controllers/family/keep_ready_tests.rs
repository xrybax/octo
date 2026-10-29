//! Playlists kept ready on the Family page: a member chooses their own, nobody else's.

use super::*;

#[tokio::test]
async fn keeping_playlists_ready_needs_a_sign_in() {
    let f = fam().await;
    assert_eq!(
        f.get("/api/family/me/keep-ready", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.put("/api/family/me/keep-ready", None, json!({ "lists": [] }))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_member_chooses_their_own_lists_whoever_the_body_names() {
    let f = fam().await;
    let alex = f.alex();
    let mine = f.get("/api/family/me/keep-ready", Some(&alex)).await;
    assert_eq!(mine.status, StatusCode::OK, "{}", mine.body);
    let mine = mine.json();
    assert_eq!(mine["user"], "alex");
    assert_eq!(mine["mostPlayed"], true);
    assert!(mine["people"].is_null());

    let saved = f
        .put(
            "/api/family/me/keep-ready",
            Some(&alex),
            json!({ "user": "sam", "lists": [], "mostPlayed": false }),
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    assert_eq!(saved.json()["user"], "alex");
    assert_eq!(saved.json()["mostPlayed"], false);
    // Sam's own choice is untouched.
    assert_eq!(
        f.get("/api/family/me/keep-ready", Some(&f.sam())).await.json()["mostPlayed"],
        true
    );
}

#[tokio::test]
async fn a_list_that_is_not_the_members_is_refused() {
    let f = fam().await;
    let alex = f.alex();
    let refused = f
        .put(
            "/api/family/me/keep-ready",
            Some(&alex),
            json!({ "lists": ["of-not-alexs"] }),
        )
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(
        refused.body.contains("not one of your playlists"),
        "{}",
        refused.body
    );
}
