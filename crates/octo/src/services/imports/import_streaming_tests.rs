use super::*;
use crate::services::imports::ImportStore;

fn snapshot() -> ImportList {
    ImportList {
        id: "file-road-trip".into(),
        owner: "alice".into(),
        name: "Road trip".into(),
        stream_without_downloading: true,
        tracks: vec![ImportTrack {
            title: "Angel".into(),
            artist: "Massive Attack".into(),
            seconds: Some(379),
            album: Some("Mezzanine".into()),
            isrc: Some("GBAAA9800320".into()),
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn streaming_identity_and_choice_survive_restart_and_explicit_reimport() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("imports.json");
    let list = snapshot();
    let id = playlist_id(&list);
    let store = ImportStore::new(Some(path.clone()));
    store.save(list.clone());
    drop(store);
    let store = ImportStore::new(Some(path));
    let routing = preview_routing(&list.tracks[0]);
    let preview_id = ExternalIdRegistry::make_short_id(&routing);
    assert_eq!(store.preview_routing(&preview_id).unwrap(), routing);
    let mut reread = list.clone();
    reread.name = "Renamed".into();
    reread.stream_without_downloading = false;
    reread.tracks[0].library_id = Some("nd-angel".into());
    let saved = store.save(reread);
    assert!(saved.stream_without_downloading);
    assert_eq!(playlist_id(&saved), id);
    assert_eq!(id.len(), 22);
    let mut other = list.clone();
    other.owner = "bob".into();
    assert_ne!(playlist_id(&other), id);
    other.owner = "ALICE".into();
    assert_eq!(playlist_id(&other), id);
    assert!(!ImportList::default().stream_without_downloading);
}

#[test]
fn preview_ids_and_routing_survive_restart_without_network_or_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("external-ids.json");
    let list = snapshot();
    let registry = ExternalIdRegistry::new(Some(path.clone()));
    let song = preview_song(&registry, &list.tracks[0]);
    let id = song.id;
    assert_eq!(id.len(), 22);
    assert_eq!(song.external_provider.as_deref(), Some("soulseek"));
    let routing = registry.lookup(&id).unwrap().snapshot();
    assert_eq!(routing.isrc, list.tracks[0].isrc);
    assert_eq!(routing.duration, Some(379));
    registry.pin_video(&id, "video123456", false);
    registry.flush();
    drop(registry);
    let registry = ExternalIdRegistry::new(Some(path));
    assert!(registry.lookup(&id).is_some());
    assert_eq!(preview_song(&registry, &list.tracks[0]).id, id);
    assert_eq!(
        registry.lookup(&id).unwrap().snapshot().you_tube_id.as_deref(),
        Some("video123456")
    );
}

#[test]
fn row_counts_actual_imported_entries_including_duplicates_not_unread_source_tracks() {
    let mut list = snapshot();
    list.total = 999;
    list.tracks.push(list.tracks[0].clone());
    let fields = playlist_fields(&list);
    assert_eq!(fields["songCount"], 2);
    assert_eq!(fields["duration"], 758);
    assert_eq!(fields["public"], false);
    assert_eq!(fields["readonly"], true);
}
