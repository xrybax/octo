//! The words for library health, the same as the Octo app's (HealthWords.kt and FixWords.kt), so
//! a finding reads the same on the phone, the desktop and the dashboard: what each check is
//! called, what it means, and what to do about it.

use octo_core::common::dotnet::{is_blank, to_upper_invariant};

use super::keep_marks::TargetReason;
use super::library_health::{
    AlbumDifference, AlbumDifferenceValues, BestReason, DuplicateBasis, DuplicateSet, HealthCheck,
    HealthReport, IHealthFields, SplitAlbum, SplitBasis, SplitReason,
};

/// A count with its thousands split by commas.
pub fn n0(count: i64) -> String {
    let digits = count.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if count < 0 {
        out.push('-');
    }
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A count with its noun, "1 song" or "2,256 songs".
pub fn count_text(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", n0(count as i64))
    }
}

pub fn title(check: HealthCheck) -> &'static str {
    match check {
        HealthCheck::Duplicates => "Songs you have twice",
        HealthCheck::SplitAlbums => "Albums split apart",
        HealthCheck::NoLength => "Songs with no length",
        HealthCheck::NoTrackNumber => "No track number",
        HealthCheck::NoAlbumArtist => "No album artist",
        HealthCheck::NoCover => "No cover art",
        HealthCheck::NoYear => "No year",
        HealthCheck::NoGenre => "No genre",
    }
}

/// How much a check found, in its own terms.
pub fn count_label(check: HealthCheck, count: usize) -> String {
    match check {
        HealthCheck::Duplicates => count_text(
            count,
            "song with more than one copy",
            "songs with more than one copy",
        ),
        HealthCheck::SplitAlbums => count_text(count, "album", "albums"),
        _ => count_text(count, "song", "songs"),
    }
}

/// What a finding means for the listener.
pub fn meaning(check: HealthCheck) -> &'static str {
    match check {
        HealthCheck::Duplicates => {
            "The same recording is in your library more than once, so it shows up twice in your lists and takes up space."
        }
        HealthCheck::SplitAlbums => {
            "Each of these albums shows up as more than one album, because its songs' tags do not agree."
        }
        HealthCheck::NoLength => {
            "The server could not tell how long these songs are, which usually means the file is damaged or cut short."
        }
        HealthCheck::NoTrackNumber => {
            "These songs are on albums but have no track number, so their albums play out of order."
        }
        HealthCheck::NoAlbumArtist => {
            "Without an album artist, an album can be filed under each song's own artist and fall apart."
        }
        HealthCheck::NoCover => "These songs have no picture, so they show a blank square.",
        HealthCheck::NoYear => {
            "These songs have no year, so year filters leave them out and they sort last by year."
        }
        HealthCheck::NoGenre => "These songs have no genre, so genre pages and genre filters leave them out.",
    }
}

/// What to do about it by hand.
pub fn advice(check: HealthCheck) -> &'static str {
    match check {
        HealthCheck::Duplicates => {
            "The first copy of each is the one to keep: it sounds best, or has the fullest tags. Remove the others, unless a copy belongs to an album you want to keep whole."
        }
        HealthCheck::SplitAlbums => {
            "Give every song of the album the same album title, album artist and year with a tag editor, then let the server rescan."
        }
        HealthCheck::NoLength => "Play one to check. If it will not play, replace the file.",
        HealthCheck::NoCover => {
            "Add a picture to the files, or put a cover image in the album's folder, then let the server rescan."
        }
        _ => "Add the missing tag with a tag editor, then let the server rescan.",
    }
}

/// Why a set's copies were taken for one recording.
pub fn basis_words(basis: DuplicateBasis) -> &'static str {
    match basis {
        DuplicateBasis::Tags => "Tagged as the same recording",
        DuplicateBasis::TitleAndLength => "Same title, artist and length",
    }
}

/// Which copy to keep and why, for a set whose best copy sounds like `best`.
pub fn best_words(reason: BestReason, best: &str) -> String {
    match reason {
        BestReason::Sound => format!("Keep the first, {best}"),
        BestReason::Tags => "They sound alike; the first has fuller tags".to_string(),
        BestReason::None => "They sound alike".to_string(),
    }
}

/// One line for a set of copies: "3 copies. Same title, artist and length. Keep the first, FLAC,
/// 24-bit, 96 kHz."
pub fn set_summary<T>(set: &DuplicateSet<T>, fields: &dyn IHealthFields<T>) -> String {
    format!(
        "{} copies. {}. {}.",
        set.copies.len(),
        basis_words(set.basis),
        best_words(set.best_reason, &quality_text(set.best(), fields))
    )
}

/// A heading for a set of copies: the song and its artist.
pub fn set_heading<T>(set: &DuplicateSet<T>, fields: &dyn IHealthFields<T>) -> String {
    let title = fields.title(set.best());
    match fields.artist(set.best()) {
        Some(artist) if !is_blank(artist) => format!("{title} by {artist}"),
        _ => title.to_string(),
    }
}

/// A heading for a split album.
pub fn album_heading<T>(album: &SplitAlbum<T>) -> String {
    let by = if is_blank(&album.artist) {
        album.title.trim().to_string()
    } else {
        format!("{} by {}", album.title.trim(), album.artist.trim())
    };
    format!("{by}, shown as {} albums", album.parts.len())
}

/// Why the parts are one album, when their album artists differ, then what they disagree on.
pub fn album_summary<T>(album: &SplitAlbum<T>) -> String {
    album
        .reasons
        .iter()
        .map(reason_words)
        .chain(album.differences.iter().map(difference_words))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Why parts by different album artists were taken for one album.
pub fn reason_words(reason: &SplitReason) -> String {
    match reason.basis {
        SplitBasis::SharedArtist if !is_blank(&reason.song) => format!(
            "One album filed under two artists: the part by {} has {} by {}.",
            reason.other.trim(),
            reason.song.trim(),
            reason.artist.trim()
        ),
        SplitBasis::SharedArtist => format!(
            "One album filed under two artists: the part by {} names {} too.",
            reason.other.trim(),
            reason.artist.trim()
        ),
        SplitBasis::SameRelease => "The parts are tagged as the same MusicBrainz release.".to_string(),
        SplitBasis::SameReleaseGroup => {
            "The parts are tagged as the same MusicBrainz release group.".to_string()
        }
        SplitBasis::SameBarcode => format!("The parts carry the same barcode, {}.", reason.value),
        SplitBasis::SameLabelAndYear => format!(
            "The parts came out on the same label in the same year: {}.",
            reason.value
        ),
    }
}

pub fn difference_words(difference: &AlbumDifferenceValues) -> String {
    let listed = difference
        .values
        .iter()
        .map(|value| {
            let text = value.trim();
            if text.is_empty() { "none" } else { text }
        })
        .collect::<Vec<_>>()
        .join(", ");
    // A space before or after the words is there but cannot be seen.
    let mut trimmed: Vec<&str> = difference.values.iter().map(|value| value.trim()).collect();
    trimmed.sort_unstable();
    trimmed.dedup();
    let space_only = trimmed.len() == 1;
    if space_only && difference.kind == AlbumDifference::Title {
        return "The album title has a stray space on some songs.".to_string();
    }
    if space_only && difference.kind == AlbumDifference::AlbumArtist {
        return "The album artist has a stray space on some songs.".to_string();
    }
    match difference.kind {
        AlbumDifference::Title => format!("The album title is written differently: {listed}."),
        AlbumDifference::AlbumArtist => format!("The album artist differs: {listed}."),
        AlbumDifference::Year => format!("The year differs: {listed}."),
        AlbumDifference::Other => {
            "Something else in the tags differs, often a release date or an album id on only some songs."
                .to_string()
        }
    }
}

/// How a copy sounds, in a few words: "FLAC, 24-bit, 96 kHz" or "MP3, 320 kbps". Only what the
/// server says.
pub fn quality_text<T>(song: &T, fields: &dyn IHealthFields<T>) -> String {
    quality_text_of(
        fields.format(song).as_deref(),
        fields.lossless(song),
        fields.bit_depth(song),
        fields.sample_rate(song),
        fields.bit_rate(song),
    )
}

pub fn quality_text_of(
    format: Option<&str>,
    lossless: bool,
    bit_depth: Option<i32>,
    sample_rate: Option<i32>,
    bit_rate: Option<i32>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(format) = format.filter(|format| !is_blank(format)) {
        parts.push(to_upper_invariant(format));
    }
    if lossless {
        if let Some(depth) = bit_depth.filter(|depth| *depth > 0) {
            parts.push(format!("{depth}-bit"));
        }
        if let Some(rate) = sample_rate.filter(|rate| *rate > 0) {
            parts.push(rate_text(rate));
        }
    } else if let Some(rate) = bit_rate.filter(|rate| *rate > 0) {
        parts.push(format!("{rate} kbps"));
    }
    if parts.is_empty() {
        "unknown quality".to_string()
    } else {
        parts.join(", ")
    }
}

/// 44100 as "44.1 kHz", 96000 as "96 kHz".
fn rate_text(hz: i32) -> String {
    let tenths = (hz + 50) / 100;
    if tenths % 10 == 0 {
        format!("{} kHz", tenths / 10)
    } else {
        format!("{}.{} kHz", tenths / 10, tenths % 10)
    }
}

/// The line under the page's title.
pub fn overview<T: Clone>(report: &HealthReport<T>) -> String {
    overview_and(report, 0)
}

/// [`overview`] with `more` findings of the server's own besides the app's checks.
pub fn overview_and<T: Clone>(report: &HealthReport<T>, more: usize) -> String {
    let checked = format!("Checked {}.", count_text(report.checked, "song", "songs"));
    let found = report.findings().len() + more;
    if found == 0 {
        return checked;
    }
    let kinds = if found == 1 {
        "One thing".to_string()
    } else {
        format!("{found} things")
    };
    format!("{checked} {kinds} could be better.")
}

// ---- tags against the audio (the server's own check) ------------------------------------------

pub const TAGS_TITLE: &str = "Tags that disagree with the audio";
pub const TAGS_MEANING: &str = "Octo listened to these songs, and their tags say something the audio is not: an album that names another take, a live or instrumental take titled plainly, or ids of another recording.";
pub const TAGS_ADVICE: &str = "Each song says what its audio is. Change the tags in your tagger, or turn on library actions to fix them here.";
pub const TAGS_FIX_MEANING: &str = "Use the audio's tags writes only what the audio shows: the plain album, the take in the title, and the ids of the one recording it is. Every change can be undone.";
pub const TAGS_NONE_SURE: &str = "None of these was decided by the exact length or a single match, so each one waits for you. Listen, then use the audio's tags on the ones that are right.";
pub const USE_THE_AUDIOS_TAGS: &str = "Use the audio's tags";

/// The button that fixes every sure finding at once.
pub fn tags_fix_all_label(sure: usize) -> String {
    if sure == 1 {
        "Fix the sure one".to_string()
    } else {
        format!("Fix all {} sure ones", n0(sure as i64))
    }
}

/// How far the tag check has got, in a line.
pub fn tag_check_line(state: &str, checked: usize, total: i32, per_hour: i32) -> String {
    let of = if total > 0 {
        format!(
            "{} of {} checked",
            n0(checked.min(total as usize) as i64),
            count_text(total as usize, "song", "songs")
        )
    } else {
        format!("{} checked", count_text(checked, "song", "songs"))
    };
    match state {
        "Off" => "The tag check is off.".to_string(),
        "Running" => format!("Checking tags against the audio, about {per_hour} an hour: {of}."),
        "Done" => format!("Tags checked against the audio: {of}."),
        _ => format!("Tag check {}: {of}.", to_lower_first(state)),
    }
}

fn to_lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// What the page says when there is nothing to fix.
pub const ALL_CLEAR: &str = "No second copies, no split albums and no missing tags. There is nothing to fix.";

// ---- fixes (FixWords.kt) ---------------------------------------------------------------------

pub const RECENTLY_REMOVED: &str = "Recently removed";
pub const PUT_BACK: &str = "Put back";
pub const LOOK_UP_TAGS: &str = "Look up tags";

/// What a fix button says for a check, for `count` items.
pub fn fix_all_label(check: HealthCheck, count: usize) -> String {
    match check {
        HealthCheck::Duplicates => format!("Fix {}", count_text(count, "song", "songs")),
        HealthCheck::SplitAlbums => format!("Join {}", count_text(count, "album", "albums")),
        HealthCheck::NoCover => format!("Find {}", count_text(count, "cover", "covers")),
        HealthCheck::NoLength => format!("Find higher quality for {}", count_text(count, "song", "songs")),
        HealthCheck::NoTrackNumber => format!("Look up {}", count_text(count, "song", "songs")),
        _ => format!("Fill in {} from their albums", count_text(count, "song", "songs")),
    }
}

/// How a fix for a check works, in a sentence, for the preview.
pub fn fix_meaning(check: HealthCheck) -> &'static str {
    match check {
        HealthCheck::Duplicates => {
            "Keeps the best sound of each song in the copy people starred, rated, played or put in playlists, fills in the tags it lacks from the others, and moves the others to the server's trash, where they can be put back. Their stars, ratings and playlist places move to the copy that stays."
        }
        HealthCheck::SplitAlbums => {
            "Gives the songs of the other parts the album tags of the largest part, album artist and all, so each album shows as one. When one part's album artist already names every part's artist, that part leads instead."
        }
        HealthCheck::NoCover => {
            "Looks for each album's cover and puts it inside songs that have no picture. A picture already there is never replaced."
        }
        HealthCheck::NoLength => {
            "Asks the server for a higher quality copy of each song, which takes the damaged file's place once it passes the server's checks."
        }
        HealthCheck::NoTrackNumber => {
            "Looks each song up the way a download is tagged, and shows what it found before anything is written."
        }
        _ => {
            "Fills in the tag where the rest of the album agrees on it. The others can be looked up the way a download is tagged, and every change is shown before it is written."
        }
    }
}

/// Who keeps the marks, in words, for the copy the fix keeps the id of.
fn marked_copy(reason: TargetReason) -> &'static str {
    match reason {
        TargetReason::Marks => "the copy people starred, rated or played",
        TargetReason::Playlists => "the copy your playlists list",
        TargetReason::Oldest => "the oldest copy, the likeliest to hold plays and favorites",
        TargetReason::First => "the first copy",
    }
}

/// Why the fix moves the best sound into another copy.
pub fn kept_marks_moved(audio_quality: &str, reason: TargetReason) -> String {
    format!(
        "Keeps the best-sounding copy ({audio_quality}), moved into {}, so its marks and playlist places stay.",
        marked_copy(reason)
    )
}

/// Why the fix keeps a copy that does not sound best.
pub fn kept_marks_as_is(quality: &str, reason: TargetReason) -> String {
    format!(
        "Keeps {} ({quality}) as it is. The best-sounding copy is a different format, and moving it in would lose those marks.",
        marked_copy(reason)
    )
}

/// Said on a set in the list, where the plan shown is the best-sound one.
pub const MARKS_PICKED_IN_PREVIEW: &str = "Fix these copies first checks which copy people starred, rated, played or put in playlists, and keeps the best sound in that one.";

/// Said on a set in the list while every rated copy is kept.
pub const RATED_KEPT_IN_PREVIEW: &str = "Fix these copies first checks who starred or rated each copy: every copy someone starred or rated stays, and only the others go.";

/// The switch beside Fix on the duplicates page.
pub const KEEP_RATED_LABEL: &str = "Keep every rated copy";

/// What the switch does, in one sentence.
pub const KEEP_RATED_MEANING: &str = "When on, a copy anyone starred or rated is never removed: only copies nobody starred or rated go to the trash.";

/// The rest of what the switch does, for those who open it.
pub const KEEP_RATED_MORE: &str = "Everyone's stars and ratings count, Family members' too. A song where every copy is starred or rated is left as it is. Plays and playlist places do not keep a copy by themselves: playlist places move to the copy that stays, and plays stay with the copy that goes, since Navidrome cannot give plays to another copy. From any other app: give the copies to keep a heart, then add any copy of a song to the Fix duplicates action playlist (switch the action on under Library actions); the switch applies there too.";

/// Why a set is left alone while every rated copy is kept.
pub const ALL_RATED: &str = "Every copy is starred or rated, so all of them stay and nothing changes.";

/// The rated copies a plan keeps besides the one it keeps first.
pub fn also_keeps_rated(count: usize) -> String {
    format!(
        "Also keeps {} someone starred or rated, as {}.",
        count_text(count, "other copy", "other copies"),
        if count == 1 { "it is" } else { "they are" }
    )
}

/// The sets a preview leaves alone because every copy is starred or rated.
pub fn skipped_rated(count: usize) -> String {
    format!(
        "Leaves {} as {}: every copy is starred or rated.",
        count_text(count, "song", "songs"),
        if count == 1 { "it is" } else { "they are" }
    )
}

/// The marks on one copy for the preview: "Starred, rated 4 stars", "Rated 5 stars by 2
/// people", or None when nobody starred or rated it. `unread` says some marks could not be read.
pub fn marks_words(
    starred: usize,
    best_rating: i32,
    rated: usize,
    people: usize,
    unread: bool,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if starred > 0 {
        parts.push("starred".to_string());
    }
    if rated > 0 {
        parts.push(format!(
            "rated {}",
            count_text(best_rating.max(0) as usize, "star", "stars")
        ));
    }
    let said = if parts.is_empty() {
        None
    } else {
        let mut text = parts.join(", ");
        if people > 1 {
            text.push_str(&format!(" by {}", count_text(people, "person", "people")));
        }
        Some(upper_first(&text))
    };
    match (said, unread) {
        (said, false) => said,
        (Some(said), true) => Some(format!("{said}; some marks could not be read")),
        (None, true) => Some("Some marks could not be read, so it stays".to_string()),
    }
}

fn upper_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
