//! Whole copies of outside songs' previews. A player that reads a preview in many ranges (AVPlayer
//! steps through a long song's fragmented m4a before it plays) gets them from one copy made at
//! full speed instead of one trip to YouTube each, and the songs of a playlist kept ready start
//! from their copy at once. The bytes are the preview's own, so the size and type every listing
//! declared still hold.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Duration, Utc};
use futures::{StreamExt, TryStreamExt};
use octo_core::common::Clock;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::watch;
use tracing::{info, warn};

use super::store::free_bytes;
use crate::http::static_files::{RangeOutcome, parse_range};
use crate::services::i_download_service::{AudioStream, DirectStreamInfo};
use crate::services::you_tube::YouTubeResolver;

/// Copies unused for this long are swept, unless a playlist kept ready holds them.
const UNUSED_FOR: Duration = Duration::hours(24);
/// The copies made for plays never take more than this; the ones used longest ago go first.
pub const CAP_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// A fill with no new bytes for this long is given up.
const FILL_STALL: StdDuration = StdDuration::from_secs(30);
/// A request waiting on a filling copy with no new bytes for this long ends its body; longer than
/// a fill waits, so a fill that is still going never cuts a reader off.
const SERVE_STALL: StdDuration = StdDuration::from_secs(35);
/// How many fills run at once.
const FILLS_AT_ONCE: usize = 4;
/// How many of those may be for playlists kept ready, so the rest are always there for plays.
const KEPT_FILLS_AT_ONCE: usize = 2;
/// A range from the first byte that ends within this is a probe.
const PROBE_BYTES: u64 = 64 * 1024;
/// The index of the copies, beside them.
const INDEX: &str = "previews.json";
/// How much one read of a copy sends at most.
const READ_CHUNK: u64 = 64 * 1024;

/// Whether `id` is a YouTube video id: eleven letters, digits, dashes or underscores. Only such ids
/// name a copy's file.
pub fn is_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether a request reads a preview in pieces: a range from the first byte that ends within 64 KB
/// (a probe), or any range that starts past the first byte.
pub fn asks_for_pieces(range: &str) -> bool {
    let Some(spec) = range.trim().strip_prefix("bytes=") else {
        return false;
    };
    // Several ranges in one request go to the live answer.
    if spec.contains(',') {
        return false;
    }
    let (from, to) = spec.split_once('-').unwrap_or((spec, ""));
    match from.trim().parse::<u64>() {
        Ok(0) => to.trim().parse::<u64>().is_ok_and(|end| end < PROBE_BYTES),
        Ok(_) => true,
        // A suffix range (`bytes=-N`) reads the end.
        Err(_) => from.trim().is_empty() && !to.trim().is_empty(),
    }
}

/// Whether a preview the shim answered with `status` for `range` should be copied whole: an
/// answer with audio, to a request that reads the preview in pieces.
pub fn copy_after(status: u16, range: Option<&str>) -> bool {
    matches!(status, 200 | 206) && range.is_some_and(asks_for_pieces)
}

/// What asking for a copy came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Copying {
    /// The copy is complete.
    Ready,
    /// A fill is running for it, started now or before.
    Started,
    /// Every fill it may use is taken: ask again later.
    Busy,
    /// No copy can be made: not a video id, or the disk is nearly full.
    Refused,
}

/// A preview's whole stream, as a fill reads it.
pub struct WholePreview {
    pub content_type: String,
    pub total: u64,
    pub body: AudioStream,
}

/// Where a fill reads a whole preview from.
#[async_trait]
pub trait PreviewSource: Send + Sync {
    async fn open_whole(&self, video_id: &str) -> Option<WholePreview>;
}

/// The shim's stream from its first byte, which it fetches from YouTube at full speed.
#[async_trait]
impl PreviewSource for YouTubeResolver {
    async fn open_whole(&self, video_id: &str) -> Option<WholePreview> {
        let opened = self.open_stream(video_id, Some("bytes=0-")).await?;
        let range = opened.content_range.as_deref();
        let total = match opened.status_code {
            200 => opened.content_length,
            // Only a partial answer from the first byte is the whole preview.
            206 if range.is_some_and(|r| r.trim_start().starts_with("bytes 0-")) => range
                .and_then(|r| r.rsplit_once('/'))
                .and_then(|(_, total)| total.trim().parse().ok()),
            _ => None,
        }?;
        Some(WholePreview {
            content_type: opened.content_type,
            total,
            body: Box::pin(opened.response.bytes_stream().map_err(std::io::Error::other)),
        })
    }
}

/// One complete copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    size: u64,
    content_type: String,
    last_use: DateTime<Utc>,
    /// A playlist kept ready holds it until then.
    #[serde(default)]
    kept_until: Option<DateTime<Utc>>,
}

impl Entry {
    fn kept(&self, now: DateTime<Utc>) -> bool {
        self.kept_until.is_some_and(|until| until > now)
    }
}

/// How far a fill has got.
#[derive(Debug, Clone, Default)]
struct Progress {
    total: Option<u64>,
    content_type: Option<String>,
    written: u64,
    /// The fill is over, whether the copy is complete or not.
    ended: bool,
}

struct Fill {
    progress: watch::Receiver<Progress>,
    kept_until: Option<DateTime<Utc>>,
}

#[derive(Default)]
struct State {
    index: HashMap<String, Entry>,
    fills: HashMap<String, Fill>,
}

/// The copies, in a folder of their own.
pub struct PreviewCopies {
    folder: PathBuf,
    source: Arc<dyn PreviewSource>,
    clock: Clock,
    /// No new fill starts while the disk has less free than this.
    min_free: Box<dyn Fn() -> u64 + Send + Sync>,
    cap: u64,
    state: Mutex<State>,
    /// Held from reading the index to writing it, so a save never overwrites a newer one.
    save_lock: Mutex<()>,
}

impl PreviewCopies {
    pub fn new(
        folder: PathBuf,
        source: Arc<dyn PreviewSource>,
        clock: Clock,
        min_free: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Arc<Self> {
        Self::with_cap(folder, source, clock, min_free, CAP_BYTES)
    }

    /// As `new`, with the copies made for plays held under `cap` bytes.
    pub fn with_cap(
        folder: PathBuf,
        source: Arc<dyn PreviewSource>,
        clock: Clock,
        min_free: impl Fn() -> u64 + Send + Sync + 'static,
        cap: u64,
    ) -> Arc<Self> {
        let copies = PreviewCopies {
            folder,
            source,
            clock,
            min_free: Box::new(min_free),
            cap,
            state: Mutex::new(State::default()),
            save_lock: Mutex::new(()),
        };
        copies.load();
        Arc::new(copies)
    }

    /// Reads the index, keeping the copies whose file is there at its size. A copy the index
    /// does not list (an index that could not be read lists none) is listed again from its file;
    /// unfinished fills, and files no video id names, go.
    fn load(&self) {
        let _ = std::fs::create_dir_all(&self.folder);
        let mut index: HashMap<String, Entry> = std::fs::read(self.folder.join(INDEX))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        index.retain(|id, entry| {
            is_video_id(id) && std::fs::metadata(self.copy_path(id)).is_ok_and(|m| m.len() == entry.size)
        });
        let now = self.clock.now();
        if let Ok(dir) = std::fs::read_dir(&self.folder) {
            for item in dir.flatten() {
                let path = item.path();
                let kind = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if kind == "part" || (kind == "preview" && !is_video_id(&stem)) {
                    let _ = std::fs::remove_file(&path);
                } else if kind == "preview" && !index.contains_key(&stem) {
                    let Ok(meta) = std::fs::metadata(&path) else {
                        continue;
                    };
                    index.insert(
                        stem,
                        Entry {
                            size: meta.len(),
                            content_type: sniff_type(&path).to_string(),
                            last_use: now,
                            kept_until: None,
                        },
                    );
                }
            }
        }
        self.state.lock().index = index;
    }

    fn copy_path(&self, id: &str) -> PathBuf {
        self.folder.join(format!("{id}.preview"))
    }

    fn part_path(&self, id: &str) -> PathBuf {
        self.folder.join(format!("{id}.part"))
    }

    /// Writes the index as it is now. Blocking file work: async callers use `save`.
    fn persist(&self) {
        let _one = self.save_lock.lock();
        let index = self.state.lock().index.clone();
        let Ok(json) = serde_json::to_vec_pretty(&index) else {
            return;
        };
        let temp = self.folder.join(format!("{INDEX}.tmp"));
        if std::fs::write(&temp, json).is_ok() {
            let _ = std::fs::rename(&temp, self.folder.join(INDEX));
        }
    }

    /// Writes the index as it is now, off the async threads.
    pub async fn save(self: &Arc<Self>) {
        let copies = Arc::clone(self);
        let _ = tokio::task::spawn_blocking(move || copies.persist()).await;
    }

    /// Starts one fill of this video's preview for a play, unless a copy is there or filling.
    /// `expected_total` is the size the play's own answer gave; a stream of another size makes no
    /// copy. False when no fill could start.
    pub fn start(self: &Arc<Self>, video_id: &str, expected_total: Option<u64>) -> bool {
        matches!(
            self.begin(video_id, expected_total, None),
            Copying::Ready | Copying::Started
        )
    }

    /// Makes sure this video has a copy held until `until`, for a playlist kept ready: an existing
    /// copy is held, else a fill starts.
    pub fn start_kept(self: &Arc<Self>, video_id: &str, until: DateTime<Utc>) -> Copying {
        if self.is_ready(video_id) {
            self.keep(video_id, until);
            return Copying::Ready;
        }
        self.begin(video_id, None, Some(until))
    }

    fn begin(
        self: &Arc<Self>,
        video_id: &str,
        expected: Option<u64>,
        kept_until: Option<DateTime<Utc>>,
    ) -> Copying {
        if !is_video_id(video_id) {
            return Copying::Refused;
        }
        if free_bytes(&self.folder).is_some_and(|free| free < (self.min_free)()) {
            return Copying::Refused;
        }
        let (tx, rx) = watch::channel(Progress::default());
        {
            let mut state = self.state.lock();
            if state.index.contains_key(video_id) {
                return Copying::Ready;
            }
            if let Some(fill) = state.fills.get_mut(video_id) {
                if kept_until > fill.kept_until {
                    fill.kept_until = kept_until;
                }
                return Copying::Started;
            }
            let kept_running = state.fills.values().filter(|f| f.kept_until.is_some()).count();
            if state.fills.len() >= FILLS_AT_ONCE
                || (kept_until.is_some() && kept_running >= KEPT_FILLS_AT_ONCE)
            {
                return Copying::Busy;
            }
            state.fills.insert(
                video_id.to_string(),
                Fill {
                    progress: rx,
                    kept_until,
                },
            );
        }
        let copies = Arc::clone(self);
        let id = video_id.to_string();
        tokio::spawn(async move { copies.fill(id, expected, tx).await });
        Copying::Started
    }

    async fn fill(self: Arc<Self>, id: String, expected: Option<u64>, tx: watch::Sender<Progress>) {
        let began = Instant::now();
        let part = self.part_path(&id);
        let outcome = self.write_part(&id, expected, &part, &tx).await;
        let outcome = match outcome {
            Ok(done) => tokio::fs::rename(&part, self.copy_path(&id))
                .await
                .map(|()| done)
                .map_err(|e| format!("the copy could not be kept: {e}")),
            Err(why) => Err(why),
        };
        let now = self.clock.now();
        {
            let mut state = self.state.lock();
            let kept_until = state.fills.get(&id).and_then(|f| f.kept_until);
            if let Ok((size, content_type)) = &outcome {
                state.index.insert(
                    id.clone(),
                    Entry {
                        size: *size,
                        content_type: content_type.clone(),
                        last_use: now,
                        kept_until,
                    },
                );
            }
            state.fills.remove(&id);
        }
        match outcome {
            Ok((size, _)) => {
                info!(
                    "Preview copy of {id} ready ({size} bytes, {} ms)",
                    began.elapsed().as_millis()
                );
                self.save().await;
            }
            Err(why) => {
                let _ = tokio::fs::remove_file(&part).await;
                warn!("Preview copy of {id} given up: {why}");
            }
        }
        tx.send_modify(|p| p.ended = true);
    }

    /// Writes the whole preview into its unfinished file, telling readers each step.
    async fn write_part(
        &self,
        id: &str,
        expected: Option<u64>,
        part: &Path,
        tx: &watch::Sender<Progress>,
    ) -> Result<(u64, String), String> {
        let whole = tokio::time::timeout(FILL_STALL, self.source.open_whole(id))
            .await
            .map_err(|_| "the shim did not answer".to_string())?
            .ok_or_else(|| "the shim had no stream".to_string())?;
        if let Some(expected) = expected
            && expected != whole.total
        {
            return Err(format!(
                "the stream is {} bytes, not the {expected} the play was sent",
                whole.total
            ));
        }
        if whole.total == 0 {
            return Err("the stream is empty".to_string());
        }
        tx.send_modify(|p| {
            p.total = Some(whole.total);
            p.content_type = Some(whole.content_type.clone());
        });
        let mut file = tokio::fs::File::create(part).await.map_err(|e| e.to_string())?;
        let mut body = whole.body;
        let mut written = 0u64;
        while written < whole.total {
            let chunk = match tokio::time::timeout(FILL_STALL, body.next()).await {
                Err(_) => return Err("no new bytes for 30 s".to_string()),
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(e.to_string()),
                Ok(Some(Ok(chunk))) => chunk,
            };
            let room = (whole.total - written).min(chunk.len() as u64) as usize;
            file.write_all(&chunk[..room]).await.map_err(|e| e.to_string())?;
            // Readers open the same file: a byte is told about only once it is written through.
            file.flush().await.map_err(|e| e.to_string())?;
            written += room as u64;
            tx.send_modify(|p| p.written = written);
        }
        file.sync_data().await.map_err(|e| e.to_string())?;
        if written != whole.total {
            return Err(format!("the stream ended at {written} of {} bytes", whole.total));
        }
        Ok((whole.total, whole.content_type))
    }

    /// The answer to a request from the copy, or None when it cannot come from there: no copy, a
    /// copy of another size than the song's `confirmed_size`, a range past the end (the live
    /// answer gives that), or a range whose first byte a filling copy has not reached.
    pub async fn serve(
        &self,
        video_id: &str,
        range: Option<&str>,
        confirmed_size: Option<u64>,
    ) -> Option<DirectStreamInfo> {
        if !is_video_id(video_id) || range.is_some_and(|r| r.contains(',')) {
            return None;
        }
        let now = self.clock.now();
        let (ready, filling) = {
            let mut state = self.state.lock();
            let ready = state.index.get_mut(video_id).map(|entry| {
                entry.last_use = now;
                entry.clone()
            });
            let filling = match ready {
                Some(_) => None,
                None => state.fills.get(video_id).map(|f| f.progress.clone()),
            };
            (ready, filling)
        };
        if let Some(entry) = ready {
            if confirmed_size.is_some_and(|size| size != entry.size) {
                return None;
            }
            let file = tokio::fs::File::open(self.copy_path(video_id)).await.ok()?;
            return answer(file, entry.size, entry.content_type, range, None).await;
        }
        let progress = filling?;
        let seen = progress.borrow().clone();
        let (Some(total), Some(content_type)) = (seen.total, seen.content_type) else {
            return None;
        };
        if seen.ended {
            // Over since it was looked up: complete if the index has it now.
            let entry = self.state.lock().index.get(video_id).cloned()?;
            if confirmed_size.is_some_and(|size| size != entry.size) {
                return None;
            }
            let file = tokio::fs::File::open(self.copy_path(video_id)).await.ok()?;
            return answer(file, entry.size, entry.content_type, range, None).await;
        }
        if confirmed_size.is_some_and(|size| size != total) {
            return None;
        }
        let file = match tokio::fs::File::open(self.part_path(video_id)).await {
            Ok(file) => file,
            // Complete since it was looked up: the file has its own name now.
            Err(_) => tokio::fs::File::open(self.copy_path(video_id)).await.ok()?,
        };
        answer(file, total, content_type, range, Some(progress)).await
    }

    /// Holds this video's copy until `until`, for a playlist kept ready. Kept in memory: the
    /// caller saves once it has held all it holds (`save`).
    pub fn keep(&self, video_id: &str, until: DateTime<Utc>) {
        let mut state = self.state.lock();
        if let Some(entry) = state.index.get_mut(video_id)
            && entry.kept_until.is_none_or(|kept| kept < until)
        {
            entry.kept_until = Some(until);
        }
    }

    /// Whether this video's copy is complete.
    pub fn is_ready(&self, video_id: &str) -> bool {
        self.state.lock().index.contains_key(video_id)
    }

    /// Whether a new copy for a playlist kept ready fits under `kept_cap` bytes, counting the kept
    /// copies and the kept fills running.
    pub fn kept_room(&self, kept_cap: u64) -> bool {
        self.kept_bytes() < kept_cap
    }

    /// The bytes held for playlists kept ready, fills running included.
    pub fn kept_bytes(&self) -> u64 {
        let now = self.clock.now();
        let state = self.state.lock();
        let kept: u64 = state.index.values().filter(|e| e.kept(now)).map(|e| e.size).sum();
        let filling: u64 = state
            .fills
            .values()
            .filter(|f| f.kept_until.is_some())
            .map(|f| f.progress.borrow().total.unwrap_or(0))
            .sum();
        kept + filling
    }

    /// The complete copies and their bytes, for the dashboard.
    pub fn sizes(&self) -> (usize, u64) {
        let state = self.state.lock();
        (state.index.len(), state.index.values().map(|e| e.size).sum())
    }

    /// A copy no playlist holds goes when unused for a day, then the ones used longest ago while
    /// they take more than the cap. A copy a playlist holds stays until its hold ends. A file still
    /// being read when it goes is read on to its end. Blocking file work.
    pub fn sweep(&self) {
        let now = self.clock.now();
        let gone = {
            let mut state = self.state.lock();
            for entry in state.index.values_mut() {
                if entry.kept_until.is_some_and(|until| until <= now) {
                    entry.kept_until = None;
                }
            }
            let mut gone: Vec<String> = state
                .index
                .iter()
                .filter(|(_, e)| !e.kept(now) && now - e.last_use > UNUSED_FOR)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &gone {
                state.index.remove(id);
            }
            let mut unkept: Vec<(String, DateTime<Utc>, u64)> = state
                .index
                .iter()
                .filter(|(_, e)| !e.kept(now))
                .map(|(id, e)| (id.clone(), e.last_use, e.size))
                .collect();
            unkept.sort_by_key(|(_, last_use, _)| *last_use);
            let mut used: u64 = unkept.iter().map(|(_, _, size)| size).sum();
            for (id, _, size) in unkept {
                if used <= self.cap {
                    break;
                }
                state.index.remove(&id);
                used -= size;
                gone.push(id);
            }
            gone
        };
        for id in &gone {
            let _ = std::fs::remove_file(self.copy_path(id));
        }
        self.persist();
    }
}

/// The type of a copy whose index entry was lost: WebM starts with its EBML mark, anything else is
/// the m4a YouTube serves first.
fn sniff_type(path: &Path) -> &'static str {
    use std::io::Read;
    let mut head = [0u8; 4];
    let webm = std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| head == [0x1A, 0x45, 0xDF, 0xA3]);
    if webm { "audio/webm" } else { "audio/mp4" }
}

/// The answer for `range` of a copy `total` bytes long, read from `file`. With `growing`, the
/// copy is still filling: the body follows it as it grows.
async fn answer(
    mut file: tokio::fs::File,
    total: u64,
    content_type: String,
    range: Option<&str>,
    growing: Option<watch::Receiver<Progress>>,
) -> Option<DirectStreamInfo> {
    if total == 0 {
        return None;
    }
    let (status, from, to) = match range.map(|r| parse_range(r, total)) {
        None | Some(RangeOutcome::Whole) => (200, 0, total - 1),
        Some(RangeOutcome::Partial(from, to)) => (206, from, to),
        Some(RangeOutcome::Unsatisfiable) => return None,
    };
    if growing.as_ref().is_some_and(|p| from > p.borrow().written) {
        return None;
    }
    if from > 0 {
        file.seek(SeekFrom::Start(from)).await.ok()?;
    }
    let reader = Reader {
        file,
        pos: from,
        end: to,
        total,
        growing,
    };
    let body = futures::stream::unfold(Some(reader), |reader| async move {
        let mut reader = reader?;
        match reader.next().await {
            Ok(Some(bytes)) => Some((Ok(bytes), Some(reader))),
            Ok(None) => None,
            Err(e) => Some((Err(e), None)),
        }
    });
    Some(DirectStreamInfo {
        audio_stream: Box::pin(body),
        content_type,
        content_length: Some(to - from + 1),
        quality: Some("youtube-m4a (copy)".to_string()),
        status_code: status,
        content_range: (status == 206).then(|| format!("bytes {from}-{to}/{total}")),
    })
}

/// Reads one range of a copy, waiting on a filling copy for bytes it has not got yet.
struct Reader {
    file: tokio::fs::File,
    pos: u64,
    /// The last byte to send.
    end: u64,
    total: u64,
    growing: Option<watch::Receiver<Progress>>,
}

impl Reader {
    async fn next(&mut self) -> std::io::Result<Option<Bytes>> {
        if self.pos > self.end {
            return Ok(None);
        }
        let mut limit = self.end + 1;
        if let Some(progress) = self.growing.as_mut() {
            loop {
                let seen = progress.borrow().clone();
                if seen.written > self.pos || seen.written >= self.total {
                    limit = limit.min(seen.written);
                    break;
                }
                if seen.ended {
                    return Err(std::io::Error::other("the copy ended short"));
                }
                match tokio::time::timeout(SERVE_STALL, progress.changed()).await {
                    Ok(Ok(())) => {}
                    // The fill is over: what it wrote is all there is.
                    Ok(Err(_)) => {
                        let written = progress.borrow().written;
                        if written > self.pos {
                            limit = limit.min(written);
                            break;
                        }
                        return Err(std::io::Error::other("the copy ended short"));
                    }
                    Err(_) => return Err(std::io::Error::other("the copy stopped growing")),
                }
            }
        }
        let want = (limit - self.pos).min(READ_CHUNK) as usize;
        let mut buf = vec![0u8; want];
        let read = self.file.read(&mut buf).await?;
        if read == 0 {
            return Err(std::io::Error::other("the copy ended short"));
        }
        buf.truncate(read);
        self.pos += read as u64;
        Ok(Some(Bytes::from(buf)))
    }
}
