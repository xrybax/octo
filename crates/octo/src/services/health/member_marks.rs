//! Family members' marks on library songs, read and written through Navidrome signed in as each
//! member, with a sign-in made at the moment of use from their current hidden password.

use std::sync::Arc;

use futures::future::{BoxFuture, join_all};
use octo_core::settings::SettingsStore;
use serde_json::Value;

use super::keep_marks::MemberMarks;
use crate::services::family::{FamilyStore, MemberCredentials};
use crate::services::library::navidrome_playlist_api::SongMarks;

/// [`MemberMarks`] through Navidrome's Subsonic API, one call per member.
pub struct NavidromeMemberMarks {
    settings: Arc<SettingsStore>,
    http: reqwest::Client,
    store: Arc<FamilyStore>,
    credentials: Arc<MemberCredentials>,
}

impl NavidromeMemberMarks {
    pub fn new(
        settings: Arc<SettingsStore>,
        http: reqwest::Client,
        store: Arc<FamilyStore>,
        credentials: Arc<MemberCredentials>,
    ) -> Self {
        NavidromeMemberMarks {
            settings,
            http,
            store,
            credentials,
        }
    }

    /// The answer's envelope to `endpoint` called as `member`, whatever its status; None when
    /// there was no answer to read.
    async fn answer(&self, member: &str, endpoint: &str, parameters: &[(&str, &str)]) -> Option<Value> {
        let base = self.settings.current().subsonic.url.clone().unwrap_or_default();
        if base.trim().is_empty() {
            return None;
        }
        let credential = self.credentials.fresh(member)?;
        let query = credential.parameters(parameters);
        let url = format!("{}/rest/{endpoint}", base.trim_end_matches('/'));
        let response = self.http.get(&url).query(&query).send().await.ok()?;
        let body: Value = response.json().await.ok()?;
        body.get("subsonic-response").cloned()
    }

    /// The answer's envelope when Navidrome said ok to `endpoint` called as `member`.
    async fn call(&self, member: &str, endpoint: &str, parameters: &[(&str, &str)]) -> Option<Value> {
        let envelope = self.answer(member, endpoint, parameters).await?;
        is_ok(&envelope).then_some(envelope)
    }
}

fn is_ok(envelope: &Value) -> bool {
    envelope.get("status").and_then(Value::as_str) == Some("ok")
}

/// Navidrome's "not found" error code: the song is not in a library the member can see.
const NOT_FOUND: i64 = 70;

/// One member's marks from any getSong answer: their marks when it said ok, none when the song
/// is not in a library they can see, and None when the answer says nothing either way.
pub fn member_marks_from(envelope: &Value) -> Option<SongMarks> {
    if is_ok(envelope) {
        return parse_member_song(envelope);
    }
    (envelope.pointer("/error/code").and_then(Value::as_i64) == Some(NOT_FOUND)).then(SongMarks::default)
}

/// One member's marks from a getSong answer: a star is a `starred` date, the rating is
/// `userRating`.
pub fn parse_member_song(envelope: &Value) -> Option<SongMarks> {
    let song = envelope.get("song")?;
    let number = |name: &str| match song.get(name) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    };
    Some(SongMarks {
        starred: song.get("starred").is_some_and(|starred| !starred.is_null()),
        rating: i32::try_from(number("userRating")).unwrap_or(0),
        plays: number("playCount"),
    })
}

impl MemberMarks for NavidromeMemberMarks {
    fn marks_of<'a>(&'a self, song_id: &'a str) -> BoxFuture<'a, Vec<(String, Option<SongMarks>)>> {
        Box::pin(async move {
            if !self.settings.current().family.enabled {
                return Vec::new();
            }
            let members: Vec<String> = self
                .store
                .members()
                .into_iter()
                .map(|member| member.username)
                .collect();
            join_all(members.iter().map(|member| async move {
                let envelope = self.answer(member, "getSong", &[("id", song_id)]).await;
                (member.clone(), envelope.as_ref().and_then(member_marks_from))
            }))
            .await
        })
    }

    fn set_rating<'a>(&'a self, member: &'a str, song_id: &'a str, rating: i32) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let rating = rating.clamp(0, 5).to_string();
            self.call(member, "setRating", &[("id", song_id), ("rating", &rating)])
                .await
                .is_some()
        })
    }

    fn set_starred<'a>(&'a self, member: &'a str, song_id: &'a str, starred: bool) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let endpoint = if starred { "star" } else { "unstar" };
            self.call(member, endpoint, &[("id", song_id)]).await.is_some()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_members_star_rating_and_plays_come_from_their_get_song_answer() {
        let envelope = json!({"status": "ok", "song": {
            "id": "A", "starred": "2026-10-01T10:00:00Z", "userRating": 4, "playCount": 7,
        }});
        assert_eq!(
            parse_member_song(&envelope),
            Some(SongMarks {
                starred: true,
                rating: 4,
                plays: 7
            })
        );
        let plain = json!({"status": "ok", "song": {"id": "A"}});
        assert_eq!(parse_member_song(&plain), Some(SongMarks::default()));
        assert_eq!(parse_member_song(&json!({"status": "ok"})), None);
    }

    #[test]
    fn a_song_a_member_cannot_see_has_no_marks_and_any_other_failure_is_unread() {
        let hidden = json!({"status": "failed", "error": {"code": 70, "message": "not found"}});
        assert_eq!(member_marks_from(&hidden), Some(SongMarks::default()));
        let refused = json!({"status": "failed", "error": {"code": 40, "message": "wrong password"}});
        assert_eq!(member_marks_from(&refused), None);
        let ok = json!({"status": "ok", "song": {"id": "A", "userRating": 2}});
        assert_eq!(member_marks_from(&ok).map(|marks| marks.rating), Some(2));
    }
}
