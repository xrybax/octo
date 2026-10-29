use std::collections::HashMap;

use parking_lot::Mutex;

use crate::common::dotnet::{eq_ignore_case, is_blank};
use crate::common::song_identity::SongIdentity;
use crate::models::domain::album::Album;
use crate::models::domain::song::Song;
use crate::tagging::net::ignore_case_key;
use crate::tagging::release_chooser::ReleaseChooser;
use crate::tagging::release_distance::ReleaseDistance;
use crate::tagging::tag_plan::{ScoredCandidate, TagConfidence, TagPlan};

/// The album-level facts a walk settled on: from its first track matched to the album's own
/// release, or, when none was, from what its tracks were written with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettledRelease {
    pub release_id: Option<String>,
    pub release_group_id: Option<String>,
    pub music_brainz_album_title: Option<String>,
    pub label: Option<String>,
    pub catalog_number: Option<String>,
    pub barcode: Option<String>,
    pub release_type: Option<String>,
    pub release_status: Option<String>,
    pub release_country: Option<String>,
    pub year: Option<i32>,
    pub original_date: Option<String>,
    pub release_date: Option<String>,
    pub total_tracks: Option<i32>,
    pub total_discs: Option<i32>,
    pub album_artist_ids: Vec<String>,
    pub album_artist: Option<String>,
    pub is_compilation: bool,
    pub captured: bool,
}

impl SettledRelease {
    /// The album-level facts a song carries.
    pub fn of(song: &Song) -> Self {
        let text = |value: &Option<String>| value.clone().filter(|v| !is_blank(v));
        Self {
            release_id: text(&song.music_brainz_release_id),
            release_group_id: text(&song.music_brainz_release_group_id),
            music_brainz_album_title: text(&song.music_brainz_album_title),
            label: text(&song.label),
            catalog_number: text(&song.catalog_number),
            barcode: text(&song.barcode),
            release_type: text(&song.release_type),
            release_status: text(&song.release_status),
            release_country: text(&song.release_country),
            year: song.year.filter(|&y| y > 0),
            original_date: text(&song.original_date),
            release_date: text(&song.release_date),
            total_tracks: song.total_tracks.filter(|&n| n > 0),
            total_discs: song.total_discs.filter(|&n| n > 0),
            album_artist_ids: song.music_brainz_album_artist_ids.clone(),
            album_artist: text(&song.album_artist),
            is_compilation: song.is_compilation,
            captured: true,
        }
    }

    /// Each fact this one lacks, from `song`. Whether the album is a compilation stays as it is.
    fn fill_from(&mut self, song: &Song) {
        fn take<T>(mine: &mut Option<T>, theirs: Option<T>) {
            if mine.is_none() {
                *mine = theirs;
            }
        }
        let other = Self::of(song);
        take(&mut self.release_id, other.release_id);
        take(&mut self.release_group_id, other.release_group_id);
        take(&mut self.music_brainz_album_title, other.music_brainz_album_title);
        take(&mut self.label, other.label);
        take(&mut self.catalog_number, other.catalog_number);
        take(&mut self.barcode, other.barcode);
        take(&mut self.release_type, other.release_type);
        take(&mut self.release_status, other.release_status);
        take(&mut self.release_country, other.release_country);
        take(&mut self.year, other.year);
        take(&mut self.original_date, other.original_date);
        take(&mut self.release_date, other.release_date);
        take(&mut self.total_tracks, other.total_tracks);
        take(&mut self.total_discs, other.total_discs);
        take(&mut self.album_artist, other.album_artist);
        if self.album_artist_ids.is_empty() {
            self.album_artist_ids = other.album_artist_ids;
        }
    }
}

/// Where one track sits on the album a walk fetches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Listed {
    track: Option<i32>,
    disc: Option<i32>,
}

/// What one album walk has settled, so every track of the walk shares the album-level fields:
/// the album's own listing (each track's place, its track and disc count, its year and cover),
/// the release the first matched track settled on, its facts, each track as it was written,
/// and each track's measured loudness for the album gain at the end. Two tracks of one album
/// matched to two pressings, or to the album and a larger edition of it, would otherwise carry
/// two labels and two dates, and the library server would show two albums.
///
/// Shared by the tracks of a walk, so its state sits behind locks. `L` is the loudness type
/// (`octo_media::audio::Loudness`, which this crate cannot name).
#[derive(Debug)]
pub struct AlbumTagContext<L = ()> {
    catalog_album_id: Option<String>,
    album_title: String,
    album_artist: Option<String>,
    /// Each listed track's place, by its external id.
    listing: HashMap<String, Listed>,
    total_tracks: Option<i32>,
    total_discs: Option<i32>,
    year: Option<i32>,
    cover_url: Option<String>,
    settled: Mutex<SettledRelease>,
    /// Each placed track's path and loudness, keyed ignoring case, in the order first placed.
    loudness: Mutex<indexmap::IndexMap<String, (String, Option<L>)>>,
    /// Each placed track's path and the song as it was written, keyed ignoring case, in the
    /// order first placed.
    placed: Mutex<indexmap::IndexMap<String, (String, Song)>>,
}

impl<L: Clone> AlbumTagContext<L> {
    pub fn new(catalog_album_id: Option<&str>, album_title: &str, album_artist: Option<&str>) -> Self {
        Self {
            catalog_album_id: catalog_album_id.map(str::to_string),
            album_title: album_title.to_string(),
            album_artist: album_artist.map(str::to_string),
            listing: HashMap::new(),
            total_tracks: None,
            total_discs: None,
            year: None,
            cover_url: None,
            settled: Mutex::new(SettledRelease::default()),
            loudness: Mutex::new(indexmap::IndexMap::new()),
            placed: Mutex::new(indexmap::IndexMap::new()),
        }
    }

    /// The album's own listing: each track's place on it, how many tracks and discs it has, its
    /// year and its cover.
    pub fn with_listing(mut self, album: &Album) -> Self {
        for song in &album.songs {
            if let Some(id) = song.external_id.as_deref().filter(|id| !id.is_empty()) {
                self.listing.insert(
                    id.to_string(),
                    Listed {
                        track: song.track.filter(|&n| n > 0),
                        disc: song.disc_number.filter(|&n| n > 0),
                    },
                );
            }
        }
        self.total_tracks = (!album.songs.is_empty()).then_some(album.songs.len() as i32);
        self.total_discs = album
            .songs
            .iter()
            .filter_map(|song| song.disc_number)
            .max()
            .filter(|&n| n > 0);
        self.year = album.year.filter(|&y| y > 0);
        self.cover_url = album.cover_art_url.clone().filter(|url| !is_blank(url));
        self
    }

    pub fn catalog_album_id(&self) -> Option<&str> {
        self.catalog_album_id.as_deref()
    }

    pub fn album_title(&self) -> &str {
        &self.album_title
    }

    pub fn album_artist(&self) -> Option<&str> {
        self.album_artist.as_deref()
    }

    /// A copy of what the walk has settled so far.
    pub fn settled(&self) -> SettledRelease {
        self.settled.lock().clone()
    }

    pub fn captured(&self) -> bool {
        self.settled.lock().captured
    }

    pub fn release_id(&self) -> Option<String> {
        self.settled.lock().release_id.clone()
    }

    /// Whether the album's own listing holds this track.
    pub fn lists(&self, external_id: &str) -> bool {
        self.listing.contains_key(external_id)
    }

    /// Whether the song is filed under the walk's album.
    pub fn holds(&self, song: &Song) -> bool {
        SongIdentity::key(&song.album) == SongIdentity::key(&self.album_title)
    }

    /// Whether a release's album is the walk's album by name, not another edition of it.
    pub fn names_album(&self, album: Option<&str>) -> bool {
        album
            .filter(|album| !is_blank(album))
            .is_some_and(|album| ReleaseDistance::names_same_album(&self.album_title, album))
    }

    /// Whether a release names an album, and it is not the walk's: another edition, or another
    /// album holding the same recording.
    pub fn names_other_album(&self, album: Option<&str>) -> bool {
        album.is_some_and(|album| !is_blank(album)) && !self.names_album(album)
    }

    /// Files a listed track under the walk's album at its place there, with the album's own
    /// track and disc count, and its album artist, year and cover where the song has none. The
    /// song a track id resolves to carries whatever album last listed that recording, and the
    /// check of a landed file names whichever release it heard first, which for a recording on
    /// several editions can be another of them; that release's ids go too. False for a track
    /// the listing does not hold, which is left as it is.
    pub fn place(&self, external_id: &str, song: &mut Song) -> bool {
        let Some(listed) = self.listing.get(external_id) else {
            return false;
        };
        if self.names_other_album(song.music_brainz_album_title.as_deref()) {
            song.music_brainz_release_id = None;
            song.music_brainz_release_group_id = None;
            song.music_brainz_album_title = None;
            song.music_brainz_release_track_id = None;
            song.music_brainz_album_artist_ids.clear();
        }
        song.album = self.album_title.clone();
        song.track = listed.track;
        song.disc_number = listed.disc;
        if self.total_tracks.is_some() {
            song.total_tracks = self.total_tracks;
        }
        if self.total_discs.is_some() {
            song.total_discs = self.total_discs;
        }
        if song.album_artist.as_deref().is_none_or(is_blank) {
            song.album_artist = self.album_artist.clone().filter(|artist| !is_blank(artist));
        }
        if song.year.is_none() {
            song.year = self.year;
        }
        if song.cover_art_url_large.as_deref().is_none_or(is_blank) {
            song.cover_art_url_large = self.cover_url.clone();
        }
        true
    }

    /// Sets a path's loudness. Paths match ignoring case, and a path already there keeps the
    /// spelling it was first placed with.
    pub fn set_loudness(&self, path: &str, value: Option<L>) {
        let mut loudness = self.loudness.lock();
        match loudness.get_mut(&ignore_case_key(path)) {
            Some(entry) => entry.1 = value,
            None => {
                loudness.insert(ignore_case_key(path), (path.to_string(), value));
            }
        }
    }

    /// Every placed track's path and loudness, in the order they were first placed.
    pub fn loudness(&self) -> Vec<(String, Option<L>)> {
        self.loudness.lock().values().cloned().collect()
    }

    /// Remembers a placed track as it was written, for the walk's last pass. Paths match
    /// ignoring case, and a path already there keeps its place and spelling.
    pub fn set_placed(&self, path: &str, song: &Song) {
        let mut placed = self.placed.lock();
        match placed.get_mut(&ignore_case_key(path)) {
            Some(entry) => entry.1 = song.clone(),
            None => {
                placed.insert(ignore_case_key(path), (path.to_string(), song.clone()));
            }
        }
    }

    /// Remember the first track's release facts, once a plan set them from the album's own
    /// release. A release of another album, a larger edition holding the same recording among
    /// them, settles nothing.
    pub fn capture(&self, plan: &TagPlan, song: &Song) {
        let mut settled = self.settled.lock();
        let Some(chosen) = plan.chosen.as_ref() else {
            return;
        };
        if settled.captured || !plan.album_from_candidate() {
            return;
        }
        if !self.holds(song) || self.names_other_album(chosen.candidate.album_title()) {
            return;
        }
        let mut facts = SettledRelease::of(song);
        if facts.release_id.is_none() {
            facts.release_id = chosen.candidate.release_id.clone();
        }
        if facts.release_group_id.is_none() {
            facts.release_group_id = chosen.candidate.release_group_id.clone();
        }
        *settled = facts;
    }

    /// Keep a track on the walk's album. When its best candidate is another album holding the
    /// same recording (a larger edition, a compilation) and a candidate on the walk's album is
    /// ranked too, that one takes its place, as sure as its own distance makes it. When the
    /// walk's album is one of two releases too close to call, the walk settles which.
    pub fn keep_on_album(&self, plan: &mut TagPlan) -> bool {
        let Some(chosen) = plan.chosen.clone() else {
            return false;
        };
        if self.names_album(chosen.candidate.album_title()) {
            let other_album_next = plan
                .ranked
                .get(1)
                .is_some_and(|next| self.names_other_album(next.candidate.album_title()));
            if plan.confidence != TagConfidence::Ambiguous || !other_album_next {
                return false;
            }
            plan.confidence = ReleaseChooser::confidence_for(chosen.distance);
            plan.notes.push(format!(
                "'{}' is the album being fetched, which settles the two",
                self.album_title
            ));
            return true;
        }
        if !self.names_other_album(chosen.candidate.album_title()) {
            return false;
        }
        let Some(found) = plan
            .ranked
            .iter()
            .find(|scored| {
                self.names_album(scored.candidate.album_title()) && same_recording(&chosen, scored)
            })
            .cloned()
        else {
            return false;
        };
        plan.notes.push(format!(
            "kept on '{}', the album being fetched, over {}",
            self.album_title,
            TagPlan::describe(&chosen.candidate)
        ));
        plan.confidence = ReleaseChooser::confidence_for(found.distance);
        plan.chosen = Some(found);
        true
    }

    /// Steer a later track onto the walk's release: when its best candidate is another pressing
    /// of the same group, the candidate on the settled release takes its place.
    pub fn prefer_settled_release(&self, plan: &mut TagPlan) -> bool {
        let settled = self.settled.lock().clone();
        let (true, Some(release_id)) = (settled.captured, settled.release_id.as_deref()) else {
            return false;
        };
        let Some(chosen) = plan.chosen.as_ref() else {
            return false;
        };
        let same = |a: Option<&str>, b: Option<&str>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => eq_ignore_case(a, b),
            _ => false,
        };
        if same(chosen.candidate.release_id.as_deref(), Some(release_id)) {
            return false;
        }
        if !same(
            chosen.candidate.release_group_id.as_deref(),
            settled.release_group_id.as_deref(),
        ) {
            return false;
        }
        let Some(found) = plan
            .ranked
            .iter()
            .find(|s| same(s.candidate.release_id.as_deref(), Some(release_id)))
            .cloned()
        else {
            return false;
        };
        let note = format!(
            "the walk's release was kept over another pressing of the same album ({})",
            chosen.candidate.release_date.as_deref().unwrap_or("?")
        );
        plan.chosen = Some(found);
        plan.notes.push(note);
        true
    }

    /// Copy the settled album-level facts onto a track of the same album.
    pub fn pin(&self, song: &mut Song) {
        let settled = self.settled.lock().clone();
        if settled.captured && self.holds(song) {
            self.pin_with(&settled, song);
        }
    }

    fn pin_with(&self, facts: &SettledRelease, song: &mut Song) {
        let same_release = match (
            song.music_brainz_release_id.as_deref(),
            facts.release_id.as_deref(),
        ) {
            (Some(mine), Some(settled)) => eq_ignore_case(mine, settled),
            (mine, settled) => mine.is_none_or(is_blank) && settled.is_none(),
        };
        // A release track id names a track of the song's own release, which is not the walk's.
        if !same_release {
            song.music_brainz_release_track_id = None;
        }
        song.music_brainz_release_id = facts.release_id.clone();
        song.music_brainz_release_group_id = facts.release_group_id.clone();
        song.music_brainz_album_title = facts.music_brainz_album_title.clone();
        song.label = facts.label.clone();
        song.catalog_number = facts.catalog_number.clone();
        song.barcode = facts.barcode.clone();
        song.release_type = facts.release_type.clone();
        song.release_status = facts.release_status.clone();
        song.release_country = facts.release_country.clone();
        if facts.year.is_some() {
            song.year = facts.year;
        }
        song.original_date = facts.original_date.clone();
        song.release_date = facts.release_date.clone();
        if let Some(total) = self.total_tracks.or(facts.total_tracks) {
            song.total_tracks = Some(total);
        }
        if let Some(discs) = facts.total_discs.or(self.total_discs) {
            song.total_discs = Some(discs);
        }
        song.music_brainz_album_artist_ids = facts.album_artist_ids.clone();
        if let Some(album_artist) = &facts.album_artist {
            song.album_artist = Some(album_artist.clone());
        }
        song.is_compilation = facts.is_compilation;
    }

    /// The album-level facts every track of the walk ends with: the settled release's, or, when
    /// no track matched the album's own release, each fact from the first placed track that has
    /// it. None before any track of the album was placed.
    pub fn walk_facts(&self) -> Option<SettledRelease> {
        let settled = self.settled();
        if settled.captured {
            return Some(settled);
        }
        let placed = self.placed.lock();
        let mut songs = placed
            .values()
            .map(|(_, song)| song)
            .filter(|song| self.holds(song));
        let mut facts = SettledRelease::of(songs.next()?);
        for song in songs {
            facts.fill_from(song);
        }
        Some(facts)
    }

    /// Every placed track of the album whose album-level facts are not the walk's, with its path
    /// and the song as it should now be written. For the walk's last pass: a track tagged before
    /// the walk settled its release was written without it.
    pub fn unsettled(&self) -> Vec<(String, Song)> {
        let Some(facts) = self.walk_facts() else {
            return Vec::new();
        };
        let placed: Vec<(String, Song)> = self.placed.lock().values().cloned().collect();
        placed
            .into_iter()
            .filter(|(_, song)| self.holds(song))
            .filter_map(|(path, song)| {
                let mut pinned = song.clone();
                self.pin_with(&facts, &mut pinned);
                let changed = SettledRelease::of(&song) != SettledRelease::of(&pinned)
                    || song.music_brainz_release_track_id != pinned.music_brainz_release_track_id;
                changed.then_some((path, pinned))
            })
            .collect()
    }
}

/// Whether two candidates are the same recording: the same recording id, or, when either has
/// none, the other no worse on anything that tells recordings apart.
fn same_recording(chosen: &ScoredCandidate, other: &ScoredCandidate) -> bool {
    let id = |scored: &ScoredCandidate| scored.candidate.recording_id.clone().filter(|id| !is_blank(id));
    if let (Some(mine), Some(theirs)) = (id(chosen), id(other)) {
        return eq_ignore_case(&mine, &theirs);
    }
    ["title", "artist", "take", "length", "fingerprint"]
        .iter()
        .all(|key| other.penalty_of(key).unwrap_or(0.0) <= chosen.penalty_of(key).unwrap_or(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tagging::tag_evidence::{ReleaseCandidate, TagSource};
    use crate::tagging::tag_plan::{ScoredCandidate, TagConfidence};

    fn scored(release: &str, group: &str, date: &str) -> ScoredCandidate {
        ScoredCandidate::new(
            ReleaseCandidate {
                release_id: Some(release.into()),
                release_group_id: Some(group.into()),
                release_title: Some("Mezzanine".into()),
                release_date: Some(date.into()),
                ..ReleaseCandidate::new(TagSource::Fingerprint, "Teardrop", "Massive Attack")
            },
            0.0,
            Vec::new(),
        )
    }

    fn plan(chosen: ScoredCandidate, ranked: Vec<ScoredCandidate>) -> TagPlan {
        TagPlan {
            confidence: TagConfidence::Strong,
            chosen: Some(chosen),
            ranked,
            ..Default::default()
        }
    }

    /// Two tracks of one walk share the release facts (the download itself is tested with the
    /// download service): the first track's facts are captured, a second track's other
    /// pressing of the same group is steered onto the settled release, and its song is pinned.
    #[test]
    fn a_walk_captures_steers_and_pins() {
        let context: AlbumTagContext = AlbumTagContext::new(Some("1"), "Mezzanine", Some("Massive Attack"));
        let first = plan(scored("r-1998", "g", "1998-04-20"), vec![]);
        let mut song = Song {
            album: "mezzanine".into(),
            label: Some("Virgin".into()),
            catalog_number: Some("CDV 2851".into()),
            year: Some(1998),
            total_tracks: Some(11),
            ..Default::default()
        };
        context.capture(&first, &song);
        assert!(context.captured());
        assert_eq!(context.release_id().as_deref(), Some("r-1998"));

        // Captured once: a later track changes nothing.
        song.label = Some("Other".into());
        context.capture(&first, &song);
        assert_eq!(context.settled().label.as_deref(), Some("Virgin"));

        let mut second = plan(
            scored("r-2019", "G", "2019-08-23"),
            vec![
                scored("r-2019", "g", "2019-08-23"),
                scored("R-1998", "g", "1998-04-20"),
            ],
        );
        assert!(context.prefer_settled_release(&mut second));
        assert_eq!(
            second
                .chosen
                .as_ref()
                .and_then(|c| c.candidate.release_id.as_deref()),
            Some("R-1998")
        );
        assert_eq!(
            second.notes,
            ["the walk's release was kept over another pressing of the same album (2019-08-23)"]
        );
        // Already on the settled release: nothing to do.
        assert!(!context.prefer_settled_release(&mut second));

        let mut other_album = plan(
            scored("r-x", "other", "2000"),
            vec![scored("r-1998", "g", "1998")],
        );
        assert!(!context.prefer_settled_release(&mut other_album));

        let mut track = Song {
            album: "Mezzanine".into(),
            year: None,
            label: Some("Peer label".into()),
            ..Default::default()
        };
        context.pin(&mut track);
        assert_eq!(track.label.as_deref(), Some("Virgin"));
        assert_eq!(track.catalog_number.as_deref(), Some("CDV 2851"));
        assert_eq!(track.year, Some(1998));
        assert_eq!(track.total_tracks, Some(11));
        assert_eq!(track.music_brainz_release_id.as_deref(), Some("r-1998"));

        let mut elsewhere = Song {
            album: "Another".into(),
            ..Default::default()
        };
        context.pin(&mut elsewhere);
        assert_eq!(elsewhere.label, None);
    }

    #[test]
    fn nothing_is_captured_from_a_weak_plan_or_another_album() {
        let context: AlbumTagContext = AlbumTagContext::new(None, "Mezzanine", None);
        let weak = TagPlan {
            confidence: TagConfidence::Low,
            ..plan(scored("r", "g", "1998"), vec![])
        };
        let song = Song {
            album: "Mezzanine".into(),
            ..Default::default()
        };
        context.capture(&weak, &song);
        assert!(!context.captured());
        let other = Song {
            album: "Collected".into(),
            ..Default::default()
        };
        context.capture(&plan(scored("r", "g", "1998"), vec![]), &other);
        assert!(!context.captured());
    }

    #[test]
    fn loudness_is_keyed_ignoring_case() {
        let context: AlbumTagContext<f64> = AlbumTagContext::new(None, "A", None);
        context.set_loudness("/m/A/01.flac", Some(-9.0));
        context.set_loudness("/m/a/01.FLAC", Some(-8.0));
        context.set_loudness("/m/A/02.flac", None);
        assert_eq!(
            context.loudness(),
            [
                ("/m/A/01.flac".to_string(), Some(-8.0)),
                ("/m/A/02.flac".to_string(), None)
            ]
        );
    }

    const ALBUM: &str = "Future Nostalgia";
    const EDITION: &str = "Future Nostalgia (The Moonlight Edition)";

    /// One recording on one album's release, at this distance.
    fn on(album: &str, release: &str, recording: &str, distance: f64) -> ScoredCandidate {
        ScoredCandidate::new(
            ReleaseCandidate {
                release_id: Some(release.into()),
                release_group_id: Some(format!("g-{release}")),
                release_title: Some(album.into()),
                recording_id: Some(recording.into()),
                ..ReleaseCandidate::new(TagSource::Fingerprint, "Levitating", "Dua Lipa")
            },
            distance,
            Vec::new(),
        )
    }

    /// A walk over the 13 tracks of the album, listed as t1 to t13.
    fn nostalgia() -> AlbumTagContext {
        let album = Album {
            title: ALBUM.into(),
            artist: "Dua Lipa".into(),
            year: Some(2020),
            cover_art_url: Some("https://cdn.example/fn.jpg".into()),
            songs: (1..=13)
                .map(|n| Song {
                    external_id: Some(format!("t{n}")),
                    track: Some(n),
                    disc_number: Some(1),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        AlbumTagContext::new(Some("fn"), ALBUM, Some("Dua Lipa")).with_listing(&album)
    }

    #[test]
    fn a_listed_track_is_filed_at_its_place_on_the_album_being_fetched() {
        let context = nostalgia();
        // As the larger edition listed the same recording last, with the release a check heard.
        let mut song = Song {
            album: EDITION.into(),
            track: Some(18),
            disc_number: Some(1),
            total_tracks: Some(19),
            music_brainz_release_id: Some("r-moon".into()),
            music_brainz_release_group_id: Some("g-moon".into()),
            music_brainz_album_title: Some(EDITION.into()),
            music_brainz_release_track_id: Some("t-moon-18".into()),
            ..Default::default()
        };
        assert!(context.place("t5", &mut song));
        assert_eq!(song.album, ALBUM);
        assert_eq!((song.track, song.disc_number), (Some(5), Some(1)));
        assert_eq!((song.total_tracks, song.total_discs), (Some(13), Some(1)));
        assert_eq!(song.album_artist.as_deref(), Some("Dua Lipa"));
        assert_eq!(song.year, Some(2020));
        assert_eq!(
            song.cover_art_url_large.as_deref(),
            Some("https://cdn.example/fn.jpg")
        );
        assert_eq!(song.music_brainz_release_id, None);
        assert_eq!(song.music_brainz_release_group_id, None);
        assert_eq!(song.music_brainz_release_track_id, None);

        // A track the album does not list is left as it is.
        let mut other = Song {
            album: EDITION.into(),
            track: Some(18),
            ..Default::default()
        };
        assert!(!context.place("t99", &mut other));
        assert_eq!((other.album.as_str(), other.track), (EDITION, Some(18)));
    }

    #[test]
    fn a_larger_edition_gives_way_to_the_album_and_settles_nothing() {
        let context = nostalgia();
        let edition = on(EDITION, "r-moon", "rec-lev", 0.01);
        let album = on(ALBUM, "r-fn", "REC-LEV", 0.04);
        let another_take = on(ALBUM, "r-fn", "rec-remix", 0.02);
        let mut kept = plan(edition.clone(), vec![edition.clone(), another_take, album]);
        assert!(context.keep_on_album(&mut kept));
        let chosen = kept.chosen.clone().expect("a choice");
        assert_eq!(chosen.candidate.recording_id.as_deref(), Some("REC-LEV"));
        assert_eq!(kept.confidence, TagConfidence::Strong);
        assert_eq!(
            kept.notes,
            [
                "kept on 'Future Nostalgia', the album being fetched, over 'Future Nostalgia (The Moonlight Edition)'"
            ]
        );
        // On the album already: nothing to do.
        assert!(!context.keep_on_album(&mut kept));

        // The edition alone stays the choice, but settles nothing for the walk.
        let mut alone = plan(edition.clone(), vec![edition]);
        assert!(!context.keep_on_album(&mut alone));
        let song = Song {
            album: ALBUM.into(),
            label: Some("Edition label".into()),
            ..Default::default()
        };
        context.capture(&alone, &song);
        assert!(!context.captured());
        context.capture(&kept, &song);
        assert!(context.captured());
    }

    #[test]
    fn the_album_being_fetched_settles_two_releases_too_close_to_call() {
        let context = nostalgia();
        let album = on(ALBUM, "r-fn", "rec", 0.03);
        let edition = on(EDITION, "r-moon", "rec", 0.04);
        let mut close = TagPlan {
            confidence: TagConfidence::Ambiguous,
            ..plan(album.clone(), vec![album.clone(), edition.clone()])
        };
        assert!(context.keep_on_album(&mut close));
        assert_eq!(close.confidence, TagConfidence::Strong);

        // Two releases both named for the album are left to the chooser.
        let mut both = TagPlan {
            confidence: TagConfidence::Ambiguous,
            ..plan(album.clone(), vec![album, on(ALBUM, "r-fn-2", "rec", 0.04)])
        };
        assert!(!context.keep_on_album(&mut both));
        assert_eq!(both.confidence, TagConfidence::Ambiguous);
    }

    #[test]
    fn the_last_pass_brings_a_track_tagged_before_the_walk_settled_onto_its_facts() {
        let context = nostalgia();
        let early = Song {
            album: ALBUM.into(),
            track: Some(1),
            total_tracks: Some(13),
            total_discs: Some(1),
            year: Some(2020),
            label: Some("Catalog label".into()),
            ..Default::default()
        };
        context.set_placed("/m/01.flac", &early);
        let settled = Song {
            album: ALBUM.into(),
            track: Some(2),
            total_tracks: Some(13),
            total_discs: Some(1),
            year: Some(2020),
            label: Some("Warner".into()),
            catalog_number: Some("FN-13".into()),
            original_date: Some("2020-03-27".into()),
            music_brainz_release_id: Some("r-fn".into()),
            music_brainz_release_group_id: Some("g-fn".into()),
            music_brainz_album_title: Some(ALBUM.into()),
            music_brainz_album_artist_ids: vec!["a-dua".into()],
            ..Default::default()
        };
        context.capture(&plan(on(ALBUM, "r-fn", "rec-2", 0.0), vec![]), &settled);
        assert!(context.captured());
        context.set_placed("/m/02.flac", &settled);
        let mut later = Song {
            album: ALBUM.into(),
            track: Some(3),
            ..Default::default()
        };
        context.pin(&mut later);
        context.set_placed("/m/03.flac", &later);

        let unsettled = context.unsettled();
        assert_eq!(unsettled.len(), 1, "{unsettled:?}");
        let (path, song) = &unsettled[0];
        assert_eq!(path, "/m/01.flac");
        assert_eq!(SettledRelease::of(song), SettledRelease::of(&settled));
        assert_eq!(song.track, Some(1));
    }

    #[test]
    fn with_nothing_settled_each_fact_comes_from_the_first_track_that_has_it() {
        let context = nostalgia();
        let first = Song {
            album: ALBUM.into(),
            total_tracks: Some(13),
            total_discs: Some(1),
            year: Some(2020),
            ..Default::default()
        };
        let second = Song {
            year: Some(2021),
            label: Some("Warner".into()),
            ..first.clone()
        };
        context.set_placed("/m/01.flac", &first);
        context.set_placed("/m/02.flac", &second);
        let facts = context.walk_facts().expect("facts");
        assert_eq!((facts.year, facts.label.as_deref()), (Some(2020), Some("Warner")));

        let unsettled = context.unsettled();
        assert_eq!(unsettled.len(), 2);
        assert!(
            unsettled
                .iter()
                .all(|(_, song)| song.year == Some(2020) && song.label.as_deref() == Some("Warner"))
        );
    }
}
