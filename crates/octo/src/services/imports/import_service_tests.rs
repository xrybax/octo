//! Importing end to end, with Spotify's embed pages and Navidrome stood in for: links, files and
//! pasted lists, lists an earlier Spotify sign-in saved, have and missing, the trickle queue, the
//! playlist, and a fetched song finding its place.

use std::sync::Arc;

use octo_core::settings::AppSettings;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::import_playlists::PlaylistNavidrome;
use super::super::spotify_link_reader::tests::embed_page;
use super::super::test_support::{FakeNavidrome, row};
use super::super::trickle_queue::tests::moving_clock;
use super::super::trickle_worker::{OwnedSong, TrickleSeams};
use super::*;
use crate::services::library::quality_upgrade_worker::LibrarySongRow;

const THEIRS: &str = "37i9dQZF1DXcBWIGoYBM5M";

#[tokio::test]
async fn streaming_is_private_and_never_queues_downloads_or_writes_a_navidrome_playlist() {
    let rig = rig().await;
    let overview = rig.saved_from_spotify().await;
    let id = &Rig::list(&overview, "Road trip").id;
    assert!(!rig.service.set_streaming("bob", id, true).ok);
    assert!(rig.service.set_streaming("alice", id, true).ok);
    let list = rig.service.streaming_lists("ALICE").remove(0);
    let virtual_id = super::super::import_streaming::playlist_id(&list);
    assert_eq!(list.tracks.len(), 2);
    assert!(rig.service.streaming_list("bob", &virtual_id).is_none());
    assert!(rig.service.streaming_lists("").is_empty());
    rig.service.loop_once().await.unwrap();
    assert!(rig.queue.snapshot(None).is_empty());
    assert!(rig.navidrome.calls().is_empty());
    assert!(rig.calls().await.is_empty());

    // The old choices still do their own work, only when explicitly selected.
    assert!(rig.service.set_playlist("alice", id, true).await.ok);
    assert_eq!(rig.navidrome.playlist("pl1"), ["nd-halo"]);
    assert!(rig.service.set_get_missing("alice", id, true).ok);
    assert_eq!(rig.queue.snapshot(Some("alice")).len(), 1);
    assert!(rig.service.set_streaming("alice", id, false).ok);
    let saved = rig.store.get("alice", id).unwrap();
    assert!(saved.keep_playlist && saved.get_missing);
    assert!(rig.service.streaming_list("alice", &virtual_id).is_none());
}

#[tokio::test]
async fn a_streaming_list_that_gets_missing_songs_still_follows_its_link() {
    let rig = rig().await;
    let added = rig
        .service
        .add_link("alice", &format!("https://open.spotify.com/playlist/{THEIRS}"))
        .await
        .unwrap();
    let id = added.list_id.unwrap();
    assert!(rig.service.set_streaming("alice", &id, true).ok);
    rig.store.update("alice", &id, |list| {
        list.keep_playlist = true;
        list.get_missing = true;
    });
    let calls = rig.calls().await;
    *rig.now.lock() += TimeDelta::days(7);
    rig.service.loop_once().await.unwrap();
    assert!(
        rig.calls().await.len() > calls.len(),
        "a list that keeps a playlist or gets missing songs reads its link again, streaming or not"
    );
}

#[tokio::test]
async fn a_list_that_only_streams_never_reads_its_link_again() {
    let rig = rig().await;
    let added = rig
        .service
        .add_link("alice", &format!("https://open.spotify.com/playlist/{THEIRS}"))
        .await
        .unwrap();
    let id = added.list_id.unwrap();
    assert!(rig.service.set_streaming("alice", &id, true).ok);
    rig.store.update("alice", &id, |list| {
        list.keep_playlist = false;
        list.get_missing = false;
    });
    let calls = rig.calls().await;
    *rig.now.lock() += TimeDelta::days(7);
    rig.service.loop_once().await.unwrap();
    assert_eq!(
        rig.calls().await,
        calls,
        "a streaming-only list stays the snapshot it was"
    );
    assert!(rig.queue.snapshot(None).is_empty());
}

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/rust-migration/fixtures/state/imports.json"
);

struct Rig {
    server: MockServer,
    now: Arc<Mutex<DateTime<Utc>>>,
    store: Arc<ImportStore>,
    queue: Arc<TrickleQueue>,
    trickle: Arc<TrickleWorker>,
    service: Arc<ImportService>,
    library: Arc<Mutex<Vec<LibrarySongRow>>>,
    navidrome: Arc<FakeNavidrome>,
}

async fn rig() -> Rig {
    rig_over(Arc::new(ImportStore::default())).await
}

async fn rig_over(store: Arc<ImportStore>) -> Rig {
    let server = MockServer::builder().start().await;
    Mock::given(path(format!("/embed/playlist/{THEIRS}")))
        .respond_with(ResponseTemplate::new(200).set_body_raw(embed_page(3, "Today's hits"), "text/html"))
        .mount(&server)
        .await;

    let (now, clock) = moving_clock();
    let settings = Arc::new(SettingsStore::for_tests(AppSettings::default()));
    let queue = Arc::new(TrickleQueue::default());
    let trickle = TrickleWorker::new(queue.clone(), settings.clone(), TrickleSeams::default());
    let library = Arc::new(Mutex::new(vec![
        row(
            "nd-teardrop",
            "Massive Attack",
            "Teardrop",
            Some(331),
            Some("Mezzanine"),
            "flac",
            900,
        ),
        row("nd-halo", "Beyonce", "Halo", Some(261), None, "mp3", 320),
    ]));
    let pages = library.clone();
    let matcher = Arc::new(ImportMatcher::with_pages(Arc::new(move |start, count| {
        let page: Vec<LibrarySongRow> = pages
            .lock()
            .iter()
            .skip(start as usize)
            .take(count as usize)
            .cloned()
            .collect();
        let n = page.len();
        Box::pin(async move { Some((page, n)) })
    })));
    let navidrome = Arc::new(FakeNavidrome::default());
    *navidrome.admin.lock() = "alice".into();
    let service = ImportService::new(ImportParts {
        store: store.clone(),
        queue: queue.clone(),
        trickle: trickle.clone(),
        matcher,
        playlists: ImportPlaylists::new(navidrome.clone() as Arc<dyn PlaylistNavidrome>),
        links: Arc::new(SpotifyLinkReader::with_base(
            SpotifyLinkReader::client(),
            &server.uri(),
        )),
        settings,
        tracker: None,
    });
    service.set_clock(clock);
    Rig {
        server,
        now,
        store,
        queue,
        trickle,
        service,
        library,
        navidrome,
    }
}

fn song(key: &str, title: &str, artist: &str, seconds: i32) -> ImportTrack {
    ImportTrack {
        key: key.into(),
        title: title.into(),
        artist: artist.into(),
        album: Some("Mezzanine".into()),
        seconds: Some(seconds),
        ..Default::default()
    }
}

impl Rig {
    /// Alice's Liked Songs and her Road trip playlist, as an earlier Spotify sign-in saved them,
    /// matched against the library.
    async fn saved_from_spotify(&self) -> ImportOverview {
        let angel = song("spotify:liked00000000000000002", "Angel", "Massive Attack", 379);
        self.store.save(ImportList {
            id: "spotify-liked".into(),
            owner: "alice".into(),
            name: "Liked Songs".into(),
            source: import_sources::SPOTIFY_LIKED.into(),
            total: 2,
            tracks: vec![
                song(
                    "spotify:liked00000000000000001",
                    "Teardrop",
                    "Massive Attack",
                    330,
                ),
                angel.clone(),
            ],
            ..Default::default()
        });
        let mut halo = song("spotify:road00000000000000001", "Halo", "Beyoncé", 261);
        halo.album = None;
        self.store.save(ImportList {
            id: "spotify-mine0000000000000000001".into(),
            owner: "alice".into(),
            name: "Road trip".into(),
            source: import_sources::SPOTIFY_PLAYLIST.into(),
            source_ref: Some("mine0000000000000000001".into()),
            snapshot_id: Some("s1".into()),
            total: 2,
            tracks: vec![halo, angel],
            ..Default::default()
        });
        self.service.match_and_follow("alice").await;
        self.service.overview("alice")
    }

    async fn calls(&self) -> Vec<String> {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| format!("{} {}", r.method, r.url.path()))
            .collect()
    }

    fn list<'a>(overview: &'a ImportOverview, name: &str) -> &'a ListSummary {
        overview.lists.iter().find(|l| l.name == name).expect("the list")
    }
}

#[tokio::test]
async fn lists_saved_from_a_spotify_account_stay_matched_and_are_never_read_again() {
    let rig = rig().await;
    let overview = rig.saved_from_spotify().await;
    let liked = Rig::list(&overview, "Liked Songs");
    assert_eq!((liked.total, liked.have, liked.missing), (2, 1, 1));
    assert!(!liked.can_refresh);
    assert_eq!(liked.partial.as_deref(), Some(ACCOUNT_LIST_NOTE));
    let road = Rig::list(&overview, "Road trip");
    assert_eq!((road.have, road.missing), (1, 1));
    assert!(!road.can_refresh);

    let json = serde_json::to_value(&overview).expect("json");
    assert!(json.get("spotify").is_none(), "{json}");
    assert!(json.get("reading").is_none(), "{json}");

    // Read again: matched against the library, nothing asked of Spotify.
    let id = liked.id.clone();
    let again = rig.service.refresh("alice", &id).await.expect("answered");
    assert!(again.ok, "{}", again.message);
    assert_eq!(again.message, "Matched against your library again.");

    // Kept and stale, the loop still leaves them as they are.
    rig.service.set_get_missing("alice", &id, true);
    rig.service.set_playlist("alice", &id, true).await;
    *rig.now.lock() += TimeDelta::days(2);
    rig.service.loop_once().await.expect("a pass");
    let calls = rig.calls().await;
    assert!(calls.is_empty(), "{calls:?}");
    let kept = rig.store.get("alice", &id).expect("kept");
    assert_eq!(kept.source, import_sources::SPOTIFY_LIKED);
    assert_eq!(kept.tracks.len(), 2);
}

#[tokio::test]
async fn a_saved_file_with_a_spotify_accounts_list_loads_and_keeps_it() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("imports.json");
    std::fs::copy(FIXTURE, &path).expect("copies the fixture");
    let rig = rig_over(Arc::new(ImportStore::new(Some(path.clone())))).await;
    let overview = rig.service.overview("brandon");
    let liked = overview
        .lists
        .iter()
        .find(|l| l.source == import_sources::SPOTIFY_LIKED)
        .expect("the saved Liked Songs");
    assert!(liked.keep_playlist && liked.get_missing);
    assert!(!liked.can_refresh);
    assert_eq!(liked.partial.as_deref(), Some(ACCOUNT_LIST_NOTE));

    // It can be removed like any list.
    let removed = rig.service.remove("brandon", &liked.id);
    assert!(removed.ok, "{}", removed.message);
    assert!(
        !ImportStore::new(Some(path))
            .lists_of("brandon")
            .iter()
            .any(|l| l.source == import_sources::SPOTIFY_LIKED)
    );
}

#[tokio::test]
async fn fetching_a_list_queues_its_missing_songs_and_stopping_takes_them_back() {
    let rig = rig().await;
    let overview = rig.saved_from_spotify().await;
    let liked = overview
        .lists
        .iter()
        .find(|l| l.source == import_sources::SPOTIFY_LIKED)
        .expect("liked")
        .id
        .clone();
    let road = Rig::list(&overview, "Road trip").id.clone();

    assert_eq!(rig.service.set_get_missing("alice", &liked, true).count, 1);
    // Angel is the only song Road trip misses, and Liked Songs already queued it.
    assert_eq!(rig.service.set_get_missing("alice", &road, true).count, 0);
    assert_eq!(rig.queue.snapshot(Some("alice")).len(), 1);
    let detail = rig.service.detail("alice", &road).expect("detail");
    let angel = detail.tracks.iter().find(|t| t.title == "Angel").expect("angel");
    assert_eq!(angel.state, import_track_states::QUEUED);

    // Angel is still wanted by Road trip, so stopping Liked Songs leaves it queued.
    assert_eq!(rig.service.set_get_missing("alice", &liked, false).count, 0);
    assert_eq!(rig.service.set_get_missing("alice", &road, false).count, 1);
    assert!(rig.queue.snapshot(Some("alice")).is_empty());
}

#[tokio::test]
async fn a_playlist_is_kept_and_a_fetched_song_takes_its_place() {
    let rig = rig().await;
    let overview = rig.saved_from_spotify().await;
    let road = Rig::list(&overview, "Road trip").id.clone();
    let made = rig.service.set_playlist("alice", &road, true).await;
    assert!(made.ok, "{}", made.message);
    assert_eq!(rig.navidrome.playlist("pl1"), ["nd-halo"]);
    let overview = rig.service.overview("alice");
    assert_eq!(
        overview
            .lists
            .iter()
            .find(|l| l.id == road)
            .and_then(|l| l.playlist_id.clone())
            .as_deref(),
        Some("pl1")
    );

    // The trickle brings Angel; the next pass matches it and puts it in its place.
    rig.service.set_get_missing("alice", &road, true);
    rig.library.lock().push(row(
        "nd-angel",
        "Massive Attack",
        "Angel",
        Some(379),
        Some("Mezzanine"),
        "flac",
        900,
    ));
    rig.trickle.configure(|seams| {
        seams.owned = Arc::new(|_| Box::pin(async { Ok(Some(OwnedSong(Some("nd-angel".into())))) }));
        seams.people_downloading = Arc::new(|| false);
        seams.soulseek_out = Arc::new(|| Box::pin(async { false }));
    });
    rig.trickle.tick().await;
    rig.trickle.drain().await;
    rig.service.loop_once().await.expect("a pass");

    assert_eq!(rig.navidrome.playlist("pl1"), ["nd-halo", "nd-angel"]);
    let overview = rig.service.overview("alice");
    let list = overview.lists.iter().find(|l| l.id == road).expect("road");
    assert_eq!((list.have, list.missing), (2, 0));
}

#[tokio::test]
async fn a_list_comes_from_a_link_or_a_file() {
    let rig = rig().await;
    let link = rig
        .service
        .add_link("bob", &format!("https://open.spotify.com/playlist/{THEIRS}?si=x"))
        .await
        .expect("answered");
    assert!(link.ok, "{}", link.message);
    assert_eq!(link.count, 3);

    let csv =
        "Track Name,Artist Name(s),Track Duration (ms)\nTeardrop,Massive Attack,330000\nNope,Nobody,1000\n";
    let file = rig.service.add_file("bob", "Gym.csv", csv.as_bytes()).await;
    assert!(file.ok, "{}", file.message);
    let overview = rig.service.overview("bob");
    let gym = Rig::list(&overview, "Gym");
    assert_eq!((gym.have, gym.missing), (1, 1));
    assert!(!gym.can_refresh);
    assert!(rig.service.overview("alice").lists.is_empty());

    let bad = rig.service.add_file("bob", "x.csv", b"a,b\n1,2\n").await;
    assert!(!bad.ok);
}

#[tokio::test]
async fn one_person_keeps_at_most_500_lists_and_a_file_of_the_same_name_still_replaces_its_list() {
    let rig = rig().await;
    for i in 0..ImportService::MAX_LISTS {
        rig.store.save(ImportList {
            id: format!("file-{i}"),
            owner: "bob".into(),
            name: format!("List {i}"),
            source: import_sources::FILE.into(),
            ..Default::default()
        });
    }
    let csv = "Track Name,Artist Name(s)\nTeardrop,Massive Attack\n";
    let refused = rig.service.add_file("bob", "Gym.csv", csv.as_bytes()).await;
    assert!(!refused.ok);
    assert_eq!(
        refused.message,
        "That would make more than 500 lists, the most Octo keeps for one person. Remove some first."
    );
    assert!(rig.service.add_file("alice", "Gym.csv", csv.as_bytes()).await.ok);
}

// ---- "Keep as playlist" after a failure -------------------------------------------------------

/// A playlist Navidrome would not take is tried again with a growing wait, not every pass, and
/// never leaves another empty playlist behind.
#[tokio::test]
async fn a_playlist_navidrome_would_not_take_waits_longer_each_time() {
    let rig = rig().await;
    let csv = "Track Name,Artist Name(s)\nTeardrop,Massive Attack\n";
    let added = rig.service.add_file("alice", "Gym.csv", csv.as_bytes()).await;
    let id = added.list_id.expect("an id");
    *rig.navidrome.fail_add.lock() = true;

    let refused = rig.service.set_playlist("alice", &id, true).await;
    assert!(!refused.ok);
    assert!(rig.navidrome.playlists.lock().is_empty(), "nothing left behind");
    assert_eq!(rig.store.get("alice", &id).and_then(|l| l.playlist_id), None);
    let creates = || {
        rig.navidrome
            .calls()
            .iter()
            .filter(|c| c.starts_with("create"))
            .count()
    };
    assert_eq!(creates(), 1);

    // Passes inside the first minute try nothing.
    for _ in 0..3 {
        *rig.now.lock() += TimeDelta::seconds(15);
        rig.service.loop_once().await.expect("a pass");
    }
    assert_eq!(creates(), 1);
    // After it, one try; then two minutes before the next.
    *rig.now.lock() += TimeDelta::seconds(30);
    rig.service.loop_once().await.expect("a pass");
    assert_eq!(creates(), 2);
    *rig.now.lock() += TimeDelta::seconds(90);
    rig.service.loop_once().await.expect("a pass");
    assert_eq!(creates(), 2);
    *rig.now.lock() += TimeDelta::seconds(31);
    rig.service.loop_once().await.expect("a pass");
    assert_eq!(creates(), 3);
    assert!(rig.navidrome.playlists.lock().is_empty());

    // Once Navidrome takes it, the playlist is made and the waits are over.
    *rig.navidrome.fail_add.lock() = false;
    let made = rig.service.set_playlist("alice", &id, true).await;
    assert!(made.ok, "{}", made.message);
    assert_eq!(rig.navidrome.playlists.lock().len(), 1);
}

#[tokio::test]
async fn a_listeners_list_is_one_request_until_an_approver_says_yes() {
    use crate::services::family::test_support::{World, member};
    use crate::services::imports::ImportFamily;
    use octo_core::family::abilities::Preset;
    use octo_core::family::request::{RequestKind, RequestState};

    let rig = rig().await;
    let overview = rig.saved_from_spotify().await;
    let road = Rig::list(&overview, "Road trip").id.clone();
    let world = World::new(None);
    world.family.upsert_member(member("alice", Preset::Listener, 6));
    let (requests, _fetcher) = world.list_requests();
    rig.service.use_family(ImportFamily {
        gate: world.gate(),
        requests: Arc::downgrade(&requests),
    });

    let asked = rig.service.set_get_missing("alice", &road, true);
    assert!(asked.ok, "{}", asked.message);
    assert!(asked.message.starts_with("Asked for a copy"), "{}", asked.message);
    assert!(
        rig.queue.snapshot(Some("alice")).is_empty(),
        "nothing fetched yet"
    );
    let made = requests.requests_of("alice");
    assert_eq!(made.len(), 1);
    assert_eq!(
        (made[0].kind, made[0].state),
        (RequestKind::Import, RequestState::Pending)
    );
    assert_eq!(made[0].title, "Road trip");
    // Choosing songs of the list asks the same request again.
    rig.service.get_songs("alice", &road, &["any".to_string()]);
    assert_eq!(requests.requests_of("alice").len(), 1);

    // Approved: the list fetches like anyone's.
    let approved = rig.service.approve_family_list("alice", &road);
    assert!(approved.ok, "{}", approved.message);
    assert_eq!(rig.queue.snapshot(Some("alice")).len(), 1);
}

#[tokio::test]
async fn an_account_that_only_saves_cannot_fetch_a_list() {
    use crate::services::family::test_support::{World, member};
    use crate::services::imports::ImportFamily;
    use octo_core::family::abilities::Preset;

    let rig = rig().await;
    let overview = rig.saved_from_spotify().await;
    let road = Rig::list(&overview, "Road trip").id.clone();
    let world = World::new(None);
    let mut alice = member("alice", Preset::Listener, 6);
    alice
        .edits
        .insert("AddToLibrary".into(), serde_json::json!("SaveOnly"));
    world.family.upsert_member(alice);
    let (requests, _fetcher) = world.list_requests();
    rig.service.use_family(ImportFamily {
        gate: world.gate(),
        requests: Arc::downgrade(&requests),
    });
    let refused = rig.service.set_get_missing("alice", &road, true);
    assert!(!refused.ok);
    assert_eq!(
        refused.message,
        crate::services::family::library_write_gate::SAVE_ONLY_MESSAGE
    );
    assert!(requests.requests_of("alice").is_empty());
    assert!(rig.queue.snapshot(Some("alice")).is_empty());
}

#[tokio::test]
async fn members_who_add_at_once_fetch_lists_as_always() {
    use crate::services::family::test_support::{World, member};
    use crate::services::imports::ImportFamily;
    use octo_core::family::abilities::Preset;

    let rig = rig().await;
    let overview = rig.saved_from_spotify().await;
    let road = Rig::list(&overview, "Road trip").id.clone();
    let world = World::new(None);
    world.family.upsert_member(member("alice", Preset::Member, 6));
    let (requests, _fetcher) = world.list_requests();
    rig.service.use_family(ImportFamily {
        gate: world.gate(),
        requests: Arc::downgrade(&requests),
    });
    assert_eq!(rig.service.set_get_missing("alice", &road, true).count, 1);
    assert!(requests.requests_of("alice").is_empty());
}

#[tokio::test]
async fn a_plain_list_is_read_whichever_way_round_names_the_library() {
    let rig = rig().await;
    // Written Title - Artist: read that way, both songs are in the library.
    let backward = "Teardrop - Massive Attack\nHalo - Beyonce\nNope - Nobody\n";
    let file = rig
        .service
        .add_file("bob", "Backward.txt", backward.as_bytes())
        .await;
    assert!(file.ok, "{}", file.message);
    let detail = rig
        .service
        .detail("bob", file.list_id.as_deref().unwrap_or_default())
        .expect("the list");
    assert_eq!(detail.list.name, "Backward");
    assert_eq!((detail.list.have, detail.list.missing), (2, 1));
    assert_eq!(detail.tracks[0].artist, "Massive Attack");
    assert_eq!(detail.tracks[0].title, "Teardrop");

    // Written Artist - Title: kept that way round.
    let forward = rig
        .service
        .add_text(
            "bob",
            Some("Forward"),
            "Massive Attack - Teardrop\nBeyonce - Halo\n",
        )
        .await;
    assert!(forward.ok, "{}", forward.message);
    let detail = rig
        .service
        .detail("bob", forward.list_id.as_deref().unwrap_or_default())
        .expect("the list");
    assert_eq!((detail.list.have, detail.list.missing), (2, 0));
    assert_eq!(detail.tracks[1].artist, "Beyonce");
}

#[tokio::test]
async fn pasted_text_is_a_list_of_its_own_name_and_says_what_it_needs() {
    let rig = rig().await;
    let pasted = rig.service.add_text("bob", None, "1. Beyonce - Halo\n").await;
    assert!(pasted.ok, "{}", pasted.message);
    assert_eq!(pasted.message, "Added Pasted list, 1 songs.");
    assert_eq!(Rig::list(&rig.service.overview("bob"), "Pasted list").have, 1);
    // The same name again replaces it.
    let again = rig
        .service
        .add_text("bob", Some("  "), "Massive Attack - Teardrop\nBeyonce - Halo\n")
        .await;
    assert!(again.ok, "{}", again.message);
    assert_eq!(rig.service.overview("bob").lists.len(), 1);

    let empty = rig
        .service
        .add_text("bob", Some("Nothing"), "\n# only a note\n")
        .await;
    assert!(!empty.ok);
    assert_eq!(empty.message, crate::services::imports::text_list::NO_SONGS);
}

#[tokio::test]
async fn the_overview_says_who_approves_only_when_a_member_waits_for_approval() {
    use crate::services::family::test_support::{World, member};
    use crate::services::imports::ImportFamily;
    use octo_core::family::abilities::Preset;

    let rig = rig().await;
    let world = World::new(None);
    world.family.upsert_member(member("alice", Preset::Listener, 6));
    world.family.upsert_member(member("bob", Preset::Member, 7));
    let mut quick = member("cara", Preset::Listener, 8);
    quick.edits.insert("AutoApprove".into(), serde_json::json!(true));
    world.family.upsert_member(quick);
    let (requests, _fetcher) = world.list_requests();
    rig.service.use_family(ImportFamily {
        gate: world.gate(),
        requests: Arc::downgrade(&requests),
    });

    let alice = serde_json::to_value(rig.service.overview("alice")).expect("json");
    assert_eq!(
        alice["approval"],
        serde_json::json!({"needed": true, "by": "winters"})
    );
    for (user, why) in [
        ("bob", "adds songs at once"),
        ("cara", "requests approved at once"),
        ("winters", "the owner"),
        ("stranger", "outside the family"),
    ] {
        let view = serde_json::to_value(rig.service.overview(user)).expect("json");
        assert!(view.get("approval").is_none(), "{user} ({why}): {view}");
    }

    // With Family off, nobody waits.
    let mut off = (*world.settings.current()).clone();
    off.family.enabled = false;
    world.settings.set(off);
    let alice = serde_json::to_value(rig.service.overview("alice")).expect("json");
    assert!(alice.get("approval").is_none(), "{alice}");
}
