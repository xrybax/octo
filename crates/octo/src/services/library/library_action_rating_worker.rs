//! `RatingActionRequest` and the worker (one shared instance: the controller enqueues into the
//! loop the host is running), and the ratings it did not act on.

use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use octo_core::settings::{LibraryAction, SettingsStore};
use octo_subsonic::SubsonicCredential;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use super::library_action_executor::{LibraryActionExecutor, LibraryActionRequest};
use super::library_action_journal::action_name;
use crate::services::family::MemberCredentials;
use crate::services::subsonic::SubsonicProxyService;

/// A rating that asked for an action, with the sign-in needed to clear it again.
///
/// The rater's own sign-in is carried rather than Octo's because Subsonic ratings are PER USER:
/// clearing with Octo's admin identity would clear the admin's rating and leave the user's in
/// place. It is the sign-in the rating came with: u with t and s as sent, a password turned into
/// a token made from it first, or an API key as it is. In memory only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RatingActionRequest {
    pub action: LibraryAction,
    pub navidrome_id: String,
    pub username: String,
    pub credential: SubsonicCredential,
}

/// Why a rating that asks for an action was not acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RatingIgnored {
    /// The library actions switch is off.
    LibraryActionsOff,
    /// Star ratings as commands are off.
    RatingsOff,
    /// Nobody could be named from the request's sign-in.
    UnknownUser,
    /// The rater is not on the allowed list.
    NotAllowed,
    /// Only answers in Review or Duplicates count, and the song is in neither.
    OutsideNotices,
    /// Rehearsal mode: the action was rehearsed and nothing changed.
    DryRun,
    /// Too many ratings were already waiting.
    QueueFull,
    /// Octo is shutting down.
    Stopping,
}

impl RatingIgnored {
    /// The reason in plain words, with the setting that changes it.
    pub fn reason(&self, username: &str) -> String {
        match self {
            RatingIgnored::LibraryActionsOff => {
                "Library actions are off. Turn on Enable library actions under Library actions.".into()
            }
            RatingIgnored::RatingsOff => {
                "Star ratings are off. Turn on Star ratings as commands under Library actions.".into()
            }
            RatingIgnored::UnknownUser => {
                "Octo could not tell who rated the song, so it did nothing.".into()
            }
            RatingIgnored::NotAllowed => format!(
                "{} is not on the allowed list. Add them to Who may trigger actions under Library actions.",
                if username.is_empty() { "This user" } else { username }
            ),
            RatingIgnored::OutsideNotices => {
                "The song is not in Review or Duplicates, and only answers there count. Set Where a star counts to Any song to count a star on every song.".into()
            }
            RatingIgnored::DryRun => {
                "Rehearsal mode is on, so nothing was changed. Turn off Rehearsal mode under Library actions.".into()
            }
            RatingIgnored::QueueFull => {
                "Too many ratings were waiting, so this one was dropped. Rate the song again in a minute.".into()
            }
            RatingIgnored::Stopping => "Octo was stopping, so the rating was dropped. Rate the song again.".into(),
        }
    }
}

/// A rating Octo did not act on, for the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredRating {
    pub at_utc: DateTime<Utc>,
    pub username: String,
    pub song_id: String,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub rating: i32,
    pub action: LibraryAction,
    pub reason: String,
}

/// Applies star-rating actions, off the request thread.
///
/// Separate from the playlist worker because the two triggers are independently configurable:
/// ratings can be on with playlists off, and a single worker with two early-return gates would
/// switch both off together.
pub struct LibraryActionRatingWorker {
    sender: mpsc::Sender<RatingActionRequest>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<RatingActionRequest>>,
    executor: Arc<LibraryActionExecutor>,
    /// A background `SubsonicProxyService`, used for each clear.
    proxy: SubsonicProxyService,
    /// The library action settings, read at every enqueue.
    settings: Arc<SettingsStore>,
    /// Fresh sign-ins for members: a member's rating is applied and cleared with a sign-in
    /// made from their current hidden password, not the one the rating arrived with.
    members: OnceLock<Arc<MemberCredentials>>,
    /// The last ratings that asked for an action and got none, newest last.
    ignored: Mutex<VecDeque<IgnoredRating>>,
}

impl LibraryActionRatingWorker {
    const CAPACITY: usize = 256;

    /// How many ignored ratings are kept.
    pub const IGNORED_KEPT: usize = 50;

    pub fn new(
        executor: Arc<LibraryActionExecutor>,
        proxy: SubsonicProxyService,
        settings: Arc<SettingsStore>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(Self::CAPACITY);
        LibraryActionRatingWorker {
            sender,
            receiver: tokio::sync::Mutex::new(receiver),
            executor,
            proxy,
            settings,
            members: OnceLock::new(),
            ignored: Mutex::new(VecDeque::new()),
        }
    }

    /// Signs members in from their current hidden password whenever a queued rating is used.
    pub fn use_member_credentials(&self, members: Arc<MemberCredentials>) {
        let _ = self.members.set(members);
    }

    /// The request with a fresh sign-in when it was made by a member.
    fn refreshed(&self, mut request: RatingActionRequest) -> RatingActionRequest {
        let Some(members) = self.members.get() else {
            return request;
        };
        if let Some(user) = request.credential.user()
            && let Some(fresh) = members.fresh(user)
        {
            request.credential = fresh;
        }
        request
    }

    /// The switches and the allowed list, for a rating by `username` (None when nobody could be
    /// named).
    pub fn check(&self, username: Option<&str>) -> Result<(), RatingIgnored> {
        let settings = self.settings.current();
        let settings = &settings.library_actions;
        if !settings.enabled {
            return Err(RatingIgnored::LibraryActionsOff);
        }
        if !settings.ratings_enabled {
            return Err(RatingIgnored::RatingsOff);
        }
        let Some(username) = username.filter(|u| !u.trim().is_empty()) else {
            return Err(RatingIgnored::UnknownUser);
        };
        if !settings.is_allowed(Some(username)) {
            return Err(RatingIgnored::NotAllowed);
        }
        Ok(())
    }

    /// Queue a rating for action. The cheap gates run inline so a disabled feature costs one
    /// property read on the request path, and nothing that removes a file runs in the request.
    ///
    /// The channel is bounded: a full queue drops the new rating and says so.
    pub fn try_enqueue(&self, request: RatingActionRequest) -> Result<(), RatingIgnored> {
        self.check(Some(&request.username))?;
        match self.sender.try_send(request) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(RatingIgnored::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(RatingIgnored::Stopping),
        }
    }

    /// Keeps a rating that was not acted on, dropping the oldest past
    /// [`Self::IGNORED_KEPT`], and logs it.
    pub fn note_ignored(&self, rating: IgnoredRating) {
        info!(
            "A {} star rating of {} by {} was not acted on: {}",
            rating.rating,
            rating.song_id,
            if rating.username.is_empty() {
                "someone"
            } else {
                rating.username.as_str()
            },
            rating.reason
        );
        let mut ignored = self.ignored.lock();
        ignored.push_back(rating);
        while ignored.len() > Self::IGNORED_KEPT {
            ignored.pop_front();
        }
    }

    /// Fills in the artist and title of a kept rating once they are known. A rating already
    /// dropped from the list, or one named already, is left as it is.
    pub fn name_ignored(
        &self,
        username: &str,
        song_id: &str,
        at_utc: DateTime<Utc>,
        artist: &str,
        title: &str,
    ) {
        let mut ignored = self.ignored.lock();
        if let Some(rating) = ignored.iter_mut().rev().find(|r| {
            r.at_utc == at_utc && r.song_id == song_id && r.username == username && r.title.is_none()
        }) {
            rating.artist = Some(artist.to_string()).filter(|a| !a.is_empty());
            rating.title = Some(title.to_string()).filter(|t| !t.is_empty());
        }
    }

    /// The ratings not acted on, newest first.
    pub fn ignored(&self) -> Vec<IgnoredRating> {
        self.ignored.lock().iter().rev().cloned().collect()
    }

    /// Ratings waiting to be applied.
    pub fn pending(&self) -> usize {
        self.sender.max_capacity() - self.sender.capacity()
    }

    /// The worker loop: applies queued ratings one at a time until shutdown.
    pub async fn run(self: Arc<Self>, stopping: CancellationToken) -> anyhow::Result<()> {
        let mut receiver = self.receiver.lock().await;
        loop {
            let request = tokio::select! {
                request = receiver.recv() => match request {
                    Some(request) => request,
                    None => break,
                },
                _ = stopping.cancelled() => break,
            };
            let request = self.refreshed(request);
            // Per-item handling is mandatory: one failed action must not stop the worker.
            match self.executor.apply(Self::to_action_request(&request)).await {
                Ok(outcome) => {
                    info!(
                        "Library action {} from a rating by {}: {} - {}",
                        action_name(request.action),
                        request.username,
                        outcome.state.name(),
                        outcome.detail.as_deref().unwrap_or("")
                    );

                    // Only clear a rating the action actually consumed. Leaving it set on a
                    // failure means the user can see it did not take, rather than the rating
                    // vanishing and nothing having happened. Keep never clears it: five stars is
                    // also what someone who loves a track gives it, and Keep changed nothing that
                    // needs undoing.
                    if outcome.consumed() && request.action != LibraryAction::Keep {
                        self.clear_rating(&request).await;
                    }
                }
                Err(e) => error!(
                    "Library action from a rating failed for {}: {e:#}",
                    request.navidrome_id
                ),
            }
        }
        Ok(())
    }

    /// The executor's request, with the rater's own sign-in, so a replaced song keeps the
    /// favorite they gave it.
    pub fn to_action_request(request: &RatingActionRequest) -> LibraryActionRequest {
        LibraryActionRequest::new(
            request.action,
            request.navidrome_id.clone(),
            request.username.clone(),
        )
        .with_credential(Some(request.credential.clone()))
    }

    /// Put the rating back to 0, signed in as the rater.
    ///
    /// Three things stop this looping. rating=0 is inert by construction, because the handler
    /// only acts on 1 to 5. This call goes out through the relay, which builds an outbound
    /// request and never re-enters Octo's own routing table. And the journal would make a
    /// re-entry for the same action, id and file content a no-op anyway.
    async fn clear_rating(&self, request: &RatingActionRequest) {
        let cleared = self
            .proxy
            .relay(
                "rest/setRating",
                &request
                    .credential
                    .parameters(&[("id", request.navidrome_id.as_str()), ("rating", "0")]),
            )
            .await;
        if let Err(e) = cleared {
            // A rating left set is cosmetic, and the journal stops it triggering the action a
            // second time.
            info!("Could not clear the rating on {}: {e}", request.navidrome_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use md5::{Digest, Md5};
    use octo_core::settings::{AppSettings, FamilySettings, LibraryActionSettings, SubsonicSettings};
    use parking_lot::Mutex;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::any};

    use super::*;
    use crate::services::common::test_fakes::until;
    use crate::services::family::member_credentials::tests::member;
    use crate::services::family::{FamilyStore, FamilyVault};
    use crate::services::library::library_action_test_support::{Extras, asked_about, executor, store};

    fn sign_in(pairs: &[(&str, &str)]) -> SubsonicCredential {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        SubsonicCredential::from(pairs.iter().map(|(k, v)| (k, v))).expect("a sign-in")
    }

    fn rating(action: LibraryAction, id: &str) -> RatingActionRequest {
        RatingActionRequest {
            action,
            navidrome_id: id.into(),
            username: "alice".into(),
            credential: sign_in(&[("u", "alice"), ("t", "t"), ("s", "s")]),
        }
    }

    /// Records the sign-in of every rating put back to 0, by id.
    #[derive(Clone, Default)]
    struct RatingNavidrome {
        cleared: Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>,
    }

    impl RatingNavidrome {
        fn ids(&self) -> Vec<String> {
            self.cleared.lock().iter().map(|(id, _)| id.clone()).collect()
        }
    }

    impl Respond for RatingNavidrome {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let query: Vec<(String, String)> = request.url.query_pairs().into_owned().collect();
            let value = |key: &str| query.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
            if request.url.path().ends_with("/rest/setRating") && value("rating").as_deref() == Some("0") {
                let sign_in = query
                    .iter()
                    .filter(|(k, _)| matches!(k.as_str(), "u" | "t" | "s" | "p" | "apiKey"))
                    .cloned()
                    .collect();
                self.cleared
                    .lock()
                    .push((value("id").unwrap_or_default(), sign_in));
            }
            ResponseTemplate::new(200)
                .set_body_raw(r#"{"subsonic-response":{"status":"ok"}}"#, "application/json")
        }
    }

    fn on(navidrome: &MockServer) -> AppSettings {
        AppSettings {
            library_actions: LibraryActionSettings {
                enabled: true,
                review_enabled: true,
                ratings_enabled: true,
                dry_run: true,
                allowed_users: vec!["alice".into(), "alex".into()],
                ..Default::default()
            },
            subsonic: SubsonicSettings {
                url: Some(navidrome.uri()),
                ..Default::default()
            },
            family: FamilySettings {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    async fn navidrome() -> (MockServer, RatingNavidrome) {
        let navidrome = MockServer::builder().start().await;
        let responder = RatingNavidrome::default();
        Mock::given(any())
            .respond_with(responder.clone())
            .mount(&navidrome)
            .await;
        (navidrome, responder)
    }

    /// Runs the worker over `requests` (Delete is not switched on, so each is Skipped, which
    /// is consumed, and its rating cleared) and answers the sign-ins the clears carried.
    async fn cleared_with(
        requests: Vec<RatingActionRequest>,
        members: Option<Arc<MemberCredentials>>,
    ) -> Vec<(String, Vec<(String, String)>)> {
        let (navidrome, responder) = navidrome().await;
        let settings = store(on(&navidrome));
        let worker = Arc::new(LibraryActionRatingWorker::new(
            executor(&settings, Extras::default()),
            SubsonicProxyService::new(settings.clone()),
            settings,
        ));
        if let Some(members) = members {
            worker.use_member_credentials(members);
        }
        let stopping = CancellationToken::new();
        let running = tokio::spawn(worker.clone().run(stopping.clone()));
        let count = requests.len();
        for request in requests {
            assert_eq!(worker.try_enqueue(request), Ok(()));
        }
        until(|| responder.cleared.lock().len() == count).await;
        stopping.cancel();
        let _ = running.await;
        responder.cleared.lock().clone()
    }

    fn value<'a>(sign_in: &'a [(String, String)], key: &str) -> Option<&'a str> {
        sign_in.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Five stars is also what someone who loves a track gives it, and Keep changed nothing
    /// that needs undoing, so its rating stays. Any other consumed rating is cleared.
    #[tokio::test]
    async fn rating_worker_never_clears_a_keep_but_clears_another_consumed_rating() {
        let (navidrome, responder) = navidrome().await;
        let settings = store(on(&navidrome));
        let worker = Arc::new(LibraryActionRatingWorker::new(
            executor(
                &settings,
                Extras {
                    notices: Some(asked_about("alice", "nd-keep")),
                    ..Default::default()
                },
            ),
            SubsonicProxyService::new(settings.clone()),
            settings,
        ));
        let stopping = CancellationToken::new();
        let running = tokio::spawn(worker.clone().run(stopping.clone()));

        assert_eq!(worker.try_enqueue(rating(LibraryAction::Keep, "nd-keep")), Ok(()));
        // Delete is not switched on, so this one is Skipped: consumed, and its rating cleared.
        assert_eq!(
            worker.try_enqueue(rating(LibraryAction::Delete, "nd-delete")),
            Ok(())
        );
        until(|| !responder.cleared.lock().is_empty()).await;
        stopping.cancel();
        let _ = running.await;

        assert_eq!(responder.ids(), ["nd-delete"]);
    }

    /// Each sign-in a rating can come with clears the rating as that person: u, t and s as
    /// sent, the token a password was turned into, or the API key itself.
    #[tokio::test]
    async fn the_rating_is_cleared_with_the_sign_in_it_came_with() {
        let password = sign_in(&[("u", "alice"), ("p", "enc:736563726574")]).without_password();
        let held = password.parameters(&[]);
        let requests = vec![
            rating(LibraryAction::Delete, "nd-token"),
            RatingActionRequest {
                credential: password.clone(),
                ..rating(LibraryAction::Delete, "nd-password")
            },
            RatingActionRequest {
                credential: sign_in(&[("apiKey", "alice-key")]),
                ..rating(LibraryAction::Delete, "nd-key")
            },
        ];
        let cleared = cleared_with(requests, None).await;

        let by_id = |id: &str| {
            cleared
                .iter()
                .find(|(cleared, _)| cleared == id)
                .map(|(_, sign_in)| sign_in.clone())
                .expect("cleared")
        };
        let token = by_id("nd-token");
        assert_eq!(value(&token, "u"), Some("alice"));
        assert_eq!((value(&token, "t"), value(&token, "s")), (Some("t"), Some("s")));

        let from_password = by_id("nd-password");
        assert_eq!(value(&from_password, "u"), Some("alice"));
        assert_eq!(value(&from_password, "p"), None);
        let salt = value(&from_password, "s").expect("a salt");
        assert_eq!(salt, held["s"]);
        let expected = hex::encode(Md5::digest(format!("secret{salt}").as_bytes()));
        assert_eq!(value(&from_password, "t"), Some(expected.as_str()));

        let key = by_id("nd-key");
        assert_eq!(value(&key, "apiKey"), Some("alice-key"));
        assert_eq!(value(&key, "u"), None);
    }

    /// A member's rating is cleared with a sign-in made from their current hidden password, not
    /// the one it arrived with.
    #[tokio::test]
    async fn a_members_rating_is_cleared_with_a_fresh_sign_in() {
        let family = store(AppSettings {
            family: FamilySettings {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let members = Arc::new(FamilyStore::in_memory());
        let vault = Arc::new(FamilyVault::in_memory());
        members.upsert_member(member("alex", vault.seal("hidden").expect("sealed")));
        let credentials = Arc::new(MemberCredentials::new(members, vault, family));
        let request = RatingActionRequest {
            username: "alex".into(),
            credential: sign_in(&[("u", "alex"), ("t", "old"), ("s", "old-salt")]),
            ..rating(LibraryAction::Delete, "nd-member")
        };

        let cleared = cleared_with(vec![request], Some(credentials)).await;

        let sign_in = &cleared[0].1;
        assert_eq!(value(sign_in, "u"), Some("alex"));
        let salt = value(sign_in, "s").expect("a salt");
        assert_ne!(salt, "old-salt");
        assert_eq!(
            value(sign_in, "t"),
            Some(octo_core::family::codes::subsonic_token("hidden", salt).as_str())
        );
    }

    /// The action a rating asks for carries the rater's own sign-in.
    #[test]
    fn rating_carries_the_raters_sign_in() {
        let request = LibraryActionRatingWorker::to_action_request(&RatingActionRequest {
            action: LibraryAction::BetterQuality,
            navidrome_id: "nd-1".into(),
            username: "alice".into(),
            credential: sign_in(&[("u", "alice"), ("t", "token"), ("s", "salt")]),
        });

        assert_eq!(request.action, LibraryAction::BetterQuality);
        assert_eq!(request.navidrome_id, "nd-1");
        assert_eq!(request.username, "alice");
        let credential = request.credential.expect("a sign-in");
        assert_eq!(credential.user(), Some("alice"));
        let parameters = credential.parameters(&[]);
        assert_eq!(parameters["t"], "token");
        assert_eq!(parameters["s"], "salt");
    }

    /// The cheap gates, each with its reason, and the full queue that drops the new rating.
    #[tokio::test]
    async fn the_gates_run_inline_and_a_full_queue_drops_the_new_rating() {
        let settings = store(AppSettings {
            library_actions: LibraryActionSettings {
                enabled: false,
                ratings_enabled: false,
                allowed_users: vec!["alice".into()],
                ..Default::default()
            },
            ..Default::default()
        });
        let worker = LibraryActionRatingWorker::new(
            executor(&settings, Extras::default()),
            SubsonicProxyService::new(settings.clone()),
            settings.clone(),
        );
        assert_eq!(
            worker.try_enqueue(rating(LibraryAction::Delete, "nd-1")),
            Err(RatingIgnored::LibraryActionsOff)
        );

        let mut on = (*settings.current()).clone();
        on.library_actions.enabled = true;
        settings.set(on.clone());
        assert_eq!(
            worker.try_enqueue(rating(LibraryAction::Delete, "nd-1")),
            Err(RatingIgnored::RatingsOff)
        );

        on.library_actions.ratings_enabled = true;
        settings.set(on);
        assert_eq!(worker.check(None), Err(RatingIgnored::UnknownUser));
        assert_eq!(
            worker.try_enqueue(RatingActionRequest {
                username: "mallory".into(),
                ..rating(LibraryAction::Delete, "nd-1")
            }),
            Err(RatingIgnored::NotAllowed)
        );
        for _ in 0..256 {
            assert_eq!(worker.try_enqueue(rating(LibraryAction::Delete, "nd-1")), Ok(()));
        }
        assert_eq!(
            worker.try_enqueue(rating(LibraryAction::Delete, "nd-1")),
            Err(RatingIgnored::QueueFull)
        );
        assert_eq!(worker.pending(), 256);
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// Every reason names the setting that changes it, in plain words.
    #[test]
    fn each_reason_says_what_to_change() {
        let cases = [
            (
                RatingIgnored::LibraryActionsOff,
                "Library actions are off. Turn on Enable library actions under Library actions.",
            ),
            (
                RatingIgnored::RatingsOff,
                "Star ratings are off. Turn on Star ratings as commands under Library actions.",
            ),
            (
                RatingIgnored::UnknownUser,
                "Octo could not tell who rated the song, so it did nothing.",
            ),
            (
                RatingIgnored::NotAllowed,
                "mallory is not on the allowed list. Add them to Who may trigger actions under Library actions.",
            ),
            (
                RatingIgnored::OutsideNotices,
                "The song is not in Review or Duplicates, and only answers there count. Set Where a star counts to Any song to count a star on every song.",
            ),
            (
                RatingIgnored::DryRun,
                "Rehearsal mode is on, so nothing was changed. Turn off Rehearsal mode under Library actions.",
            ),
            (
                RatingIgnored::QueueFull,
                "Too many ratings were waiting, so this one was dropped. Rate the song again in a minute.",
            ),
            (
                RatingIgnored::Stopping,
                "Octo was stopping, so the rating was dropped. Rate the song again.",
            ),
        ];
        for (reason, words) in cases {
            let text = reason.reason("mallory");
            assert_eq!(text, words);
            assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'), "{text}");
        }
        assert!(
            RatingIgnored::NotAllowed
                .reason("")
                .starts_with("This user is not")
        );
    }

    /// The last fifty are kept, newest first.
    #[test]
    fn the_last_fifty_ignored_ratings_are_kept_newest_first() {
        let settings = store(AppSettings::default());
        let worker = LibraryActionRatingWorker::new(
            executor(&settings, Extras::default()),
            SubsonicProxyService::new(settings.clone()),
            settings,
        );
        assert!(worker.ignored().is_empty());
        for n in 0..60 {
            worker.note_ignored(IgnoredRating {
                at_utc: Utc::now(),
                username: "alice".into(),
                song_id: format!("nd-{n}"),
                artist: None,
                title: None,
                rating: 1,
                action: LibraryAction::Delete,
                reason: RatingIgnored::RatingsOff.reason("alice"),
            });
        }
        let kept = worker.ignored();
        assert_eq!(kept.len(), LibraryActionRatingWorker::IGNORED_KEPT);
        assert_eq!(kept[0].song_id, "nd-59");
        assert_eq!(kept[49].song_id, "nd-10");
    }
}
