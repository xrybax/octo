//! Looking a place up in a places file, a block at a time.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::SystemTime;

use flate2::read::DeflateDecoder;
use parking_lot::Mutex;
use tracing::warn;

use super::{
    CITY_SECTION, GRID_SECTION, MAGIC, NEAREST_KM, POSTAL_SECTION, Place, RANGE_SECTION, cell_key,
    cells_near, coarser_codes, code_key, country_key, distance_km, name_key, round2,
};

/// The trailer: the block list's offset and length, then the magic.
const TRAILER_BYTES: u64 = 24;

/// The largest block list read, in bytes: far above what a full GeoNames dump makes.
const MAX_LIST_BYTES: u64 = 16 * 1024 * 1024;

/// The largest block read, packed or unpacked, in bytes.
const MAX_BLOCK_BYTES: u64 = 1024 * 1024;

/// One block of a section: its first key, and where its bytes are.
#[derive(Debug)]
struct Block {
    first_key: String,
    offset: u64,
    length: u64,
}

/// The block list of every section, and the grid rows that hold points (`None` for a file
/// that does not say).
#[derive(Debug, Default)]
struct Blocks {
    postal: Vec<Block>,
    cities: Vec<Block>,
    grid: Vec<Block>,
    rows: Option<(i64, i64)>,
}

/// A block list, with the length and modification time of the file it was read from.
struct Loaded {
    length: u64,
    modified: Option<SystemTime>,
    blocks: Arc<Blocks>,
}

/// The places file and its block list, read the first time a lookup needs it and again when the
/// file's length or modification time changes. A file that is not there (a dev run, a test) or
/// cannot be read finds nothing, and says so in the log once.
pub struct PlaceIndex {
    path: PathBuf,
    loaded: Mutex<Option<Loaded>>,
    warned: AtomicBool,
    /// Blocks unpacked by every lookup so far, and how many of those a lookup had unpacked
    /// already.
    blocks_read: AtomicU64,
    blocks_reread: AtomicU64,
}

impl PlaceIndex {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        PlaceIndex {
            path: path.into(),
            loaded: Mutex::new(None),
            warned: AtomicBool::new(false),
            blocks_read: AtomicU64::new(0),
            blocks_reread: AtomicU64::new(0),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the file is there and its block list can be read: whether lookups can find
    /// anything at all.
    pub fn available(&self) -> bool {
        match self.open() {
            Ok(_) => true,
            Err(e) => {
                self.warn_once(&e, "octoPlaces is not listed and place lookups find nothing");
                false
            }
        }
    }

    /// The place with this postal code in `country` (ISO 3166-1 alpha-2, any case). The code
    /// is matched as a key (case, spaces and dashes ignored); a code the data holds only part of
    /// is found by its coarser code (see [`coarser_codes`]).
    pub fn by_code(&self, country: &str, code: &str) -> Option<Place> {
        let country = country_key(country)?;
        let key = code_key(code);
        if key.is_empty() {
            return None;
        }
        self.read(|reader, blocks| {
            for candidate in std::iter::once(key.clone()).chain(coarser_codes(&country, &key)) {
                let wanted = format!("{country}|{candidate}");
                let mut found = None;
                scan(reader, &blocks.postal, &wanted, |line_key, line| {
                    if line_key == wanted {
                        found = postal_place(&country, &candidate, line);
                    }
                    false
                })?;
                if found.is_some() {
                    return Ok(found);
                }
            }
            Ok(None)
        })
    }

    /// The city called `name` in `country`. With `state` (its name or its code, as the data
    /// writes them), only a city in that state; of the cities that match, the one with the most
    /// postal codes, which is the largest of them.
    pub fn by_name(&self, country: &str, name: &str, state: Option<&str>) -> Option<Place> {
        let country = country_key(country)?;
        let city = name_key(name);
        if city.is_empty() {
            return None;
        }
        let state = state.map(name_key).filter(|s| !s.is_empty());
        let prefix = format!("{country}|{city}|");
        self.read(|reader, blocks| {
            let mut best: Option<(u32, Place)> = None;
            scan(reader, &blocks.cities, &prefix, |line_key, line| {
                let Some(rest) = line_key.strip_prefix(prefix.as_str()) else {
                    return false;
                };
                let state_key = rest.split('|').next().unwrap_or_default();
                if let Some((count, place, state_code)) = city_place(&country, line) {
                    let wanted = state
                        .as_deref()
                        .is_none_or(|s| s == state_key || s == name_key(&state_code));
                    if wanted && best.as_ref().is_none_or(|(most, _)| count > *most) {
                        best = Some((count, place));
                    }
                }
                true
            })?;
            Ok(best.map(|(_, place)| place))
        })
    }

    /// The place nearest a coordinate (decimal degrees), at most [`NEAREST_KM`] away, or `None`.
    /// Only the grid cells that can hold such a place are read (see [`cells_near`]), none outside
    /// the rows that hold points, in key order, so each block is unpacked once.
    pub fn nearest(&self, latitude: f64, longitude: f64) -> Option<Place> {
        let valid = (-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude);
        if !valid {
            return None;
        }
        self.read(|reader, blocks| {
            let mut cells: Vec<String> = cells_near(latitude, longitude)
                .into_iter()
                .filter(|(row, _)| {
                    blocks
                        .rows
                        .is_none_or(|(south, north)| (south..=north).contains(row))
                })
                .map(|(row, column)| cell_key(row, column))
                .collect();
            cells.sort_unstable();
            cells.dedup();
            let mut best: Option<(f64, Place)> = None;
            for cell in cells {
                scan(reader, &blocks.grid, &cell, |line_key, line| {
                    if line_key != cell {
                        return false;
                    }
                    if let Some(place) = grid_place(line) {
                        let km = distance_km((latitude, longitude), (place.latitude, place.longitude));
                        if km <= NEAREST_KM && best.as_ref().is_none_or(|(nearest, _)| km < *nearest) {
                            best = Some((km, place));
                        }
                    }
                    true
                })?;
            }
            Ok(best.map(|(_, place)| place))
        })
    }

    /// How many blocks the lookups so far have unpacked.
    #[cfg(test)]
    pub(super) fn blocks_read(&self) -> u64 {
        self.blocks_read.load(Ordering::Relaxed)
    }

    /// How many of those a lookup had unpacked before.
    #[cfg(test)]
    pub(super) fn blocks_reread(&self) -> u64 {
        self.blocks_reread.load(Ordering::Relaxed)
    }

    /// Runs `lookup` over the open file and its block list; `None` (logged once) when the file
    /// is missing or unreadable.
    fn read(&self, lookup: impl FnOnce(&mut Reader, &Blocks) -> io::Result<Option<Place>>) -> Option<Place> {
        let outcome = self.open().and_then(|(mut file, blocks)| {
            let mut reader = Reader::new(&mut file);
            let found = lookup(&mut reader, &blocks);
            self.blocks_read
                .fetch_add(reader.read.len() as u64, Ordering::Relaxed);
            self.blocks_reread.fetch_add(reader.reread, Ordering::Relaxed);
            found
        });
        match outcome {
            Ok(place) => place,
            Err(e) => {
                self.warn_once(&e, "place lookups find nothing");
                None
            }
        }
    }

    /// Logs, the first time only, that the place data cannot be read and what that means.
    fn warn_once(&self, e: &io::Error, meaning: &str) {
        if !self.warned.swap(true, Ordering::Relaxed) {
            warn!(
                "The place data at {} cannot be read ({e}), so {meaning}",
                self.path.display()
            );
        }
    }

    /// The open file and its block list: the one read before while the file's length and
    /// modification time are the same, else read again.
    fn open(&self) -> io::Result<(File, Arc<Blocks>)> {
        let mut file = File::open(&self.path)?;
        let meta = file.metadata()?;
        let (length, modified) = (meta.len(), meta.modified().ok());
        if let Some(loaded) = self.loaded.lock().as_ref()
            && loaded.length == length
            && loaded.modified == modified
        {
            return Ok((file, Arc::clone(&loaded.blocks)));
        }
        let blocks = Arc::new(read_blocks(&mut file)?);
        *self.loaded.lock() = Some(Loaded {
            length,
            modified,
            blocks: Arc::clone(&blocks),
        });
        Ok((file, blocks))
    }
}

fn bad(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// The block list, after checking both magics.
fn read_blocks(file: &mut File) -> io::Result<Blocks> {
    let size = file.seek(SeekFrom::End(0))?;
    if size < MAGIC.len() as u64 + TRAILER_BYTES {
        return Err(bad("not a places file (too short)"));
    }
    let mut head = [0u8; 8];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut head)?;
    let mut trailer = [0u8; TRAILER_BYTES as usize];
    file.seek(SeekFrom::Start(size - TRAILER_BYTES))?;
    file.read_exact(&mut trailer)?;
    if &head != MAGIC || &trailer[16..] != MAGIC {
        return Err(bad("not a places file"));
    }
    let word = |at: usize| {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&trailer[at..at + 8]);
        u64::from_le_bytes(bytes)
    };
    let (list_at, list_len) = (word(0), word(8));
    if list_len > MAX_LIST_BYTES
        || list_at < MAGIC.len() as u64
        || list_at.saturating_add(list_len) > size - TRAILER_BYTES
    {
        return Err(bad("the block list is out of the file"));
    }
    let mut list = String::new();
    file.seek(SeekFrom::Start(list_at))?;
    file.by_ref().take(list_len).read_to_string(&mut list)?;

    let mut blocks = Blocks::default();
    for line in list.lines() {
        let mut fields = line.split('\t');
        if fields.next() == Some(RANGE_SECTION) {
            let (Some(Ok(south)), Some(Ok(north))) = (
                fields.next().map(str::parse::<i64>),
                fields.next().map(str::parse::<i64>),
            ) else {
                return Err(bad("the block list's grid rows are not numbers"));
            };
            blocks.rows = Some((south, north));
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(section), Some(first_key), Some(offset), Some(length)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(bad("a block list line is short"));
        };
        let (Ok(offset), Ok(length)) = (offset.parse::<u64>(), length.parse::<u64>()) else {
            return Err(bad("a block list line has no offset"));
        };
        if length > MAX_BLOCK_BYTES || offset < MAGIC.len() as u64 || offset.saturating_add(length) > list_at
        {
            return Err(bad("a block is out of the file"));
        }
        let block = Block {
            first_key: first_key.to_string(),
            offset,
            length,
        };
        match section {
            POSTAL_SECTION => blocks.postal.push(block),
            CITY_SECTION => blocks.cities.push(block),
            GRID_SECTION => blocks.grid.push(block),
            _ => {}
        }
    }
    Ok(blocks)
}

/// How many blocks each section has: postal codes, cities, the grid.
#[cfg(test)]
pub(super) fn read_blocks_for_tests(file: &mut File) -> io::Result<(usize, usize, usize)> {
    let blocks = read_blocks(file)?;
    Ok((blocks.postal.len(), blocks.cities.len(), blocks.grid.len()))
}

/// The open places file, with the last block it unpacked, so the cells of one nearest lookup
/// that share a block unpack it once; the offsets of the blocks it unpacked, and how many times
/// it unpacked one of them again.
struct Reader<'a> {
    file: &'a mut File,
    last: Option<(u64, String)>,
    read: Vec<u64>,
    reread: u64,
}

impl<'a> Reader<'a> {
    fn new(file: &'a mut File) -> Self {
        Reader {
            file,
            last: None,
            read: Vec::new(),
            reread: 0,
        }
    }

    /// The lines of `block`, unpacked. A block cut short, larger than [`MAX_BLOCK_BYTES`]
    /// unpacked or not UTF-8 is an error, never part of a block.
    fn text(&mut self, block: &Block) -> io::Result<&str> {
        if self
            .last
            .as_ref()
            .is_none_or(|(offset, _)| *offset != block.offset)
        {
            let mut packed = Vec::new();
            self.file.seek(SeekFrom::Start(block.offset))?;
            self.file.by_ref().take(block.length).read_to_end(&mut packed)?;
            if packed.len() as u64 != block.length {
                return Err(bad("a block is cut off"));
            }
            let mut unpacked = Vec::new();
            DeflateDecoder::new(packed.as_slice())
                .take(MAX_BLOCK_BYTES + 1)
                .read_to_end(&mut unpacked)?;
            if unpacked.len() as u64 > MAX_BLOCK_BYTES {
                return Err(bad("a block is larger than 1 MB unpacked"));
            }
            let text = String::from_utf8(unpacked).map_err(|_| bad("a block is not UTF-8"))?;
            if self.read.contains(&block.offset) {
                self.reread += 1;
            }
            self.read.push(block.offset);
            self.last = Some((block.offset, text));
        }
        Ok(self.last.as_ref().map_or("", |(_, text)| text.as_str()))
    }
}

/// Visits the lines of a section whose key is `from` or after it, in order, while `visit`
/// returns true: `visit(key, rest of the line)`.
fn scan(
    reader: &mut Reader,
    blocks: &[Block],
    from: &str,
    mut visit: impl FnMut(&str, &str) -> bool,
) -> io::Result<()> {
    // The first block that can hold `from`: the last one whose first key is before it.
    let start = blocks
        .partition_point(|block| block.first_key.as_str() < from)
        .saturating_sub(1);
    for block in &blocks[start.min(blocks.len())..] {
        let text = reader.text(block)?;
        for line in text.lines() {
            let (key, rest) = line.split_once('\t').unwrap_or((line, ""));
            if key < from {
                continue;
            }
            if !visit(key, rest) {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// A postal code line: written code (empty when the key), place, state, latitude, longitude.
fn postal_place(country: &str, key: &str, rest: &str) -> Option<Place> {
    let fields: Vec<&str> = rest.split('\t').collect();
    let [written, name, state, latitude, longitude, ..] = fields.as_slice() else {
        return None;
    };
    Some(Place {
        code: if written.is_empty() { key } else { *written }.to_string(),
        name: name.to_string(),
        state: state.to_string(),
        country: country.to_string(),
        latitude: latitude.parse().ok()?,
        longitude: longitude.parse().ok()?,
    })
}

/// A city line: a postal code, city, state, state code, latitude, longitude, postal codes.
/// The place with its count of postal codes and its state code.
fn city_place(country: &str, rest: &str) -> Option<(u32, Place, String)> {
    let fields: Vec<&str> = rest.split('\t').collect();
    let [code, name, state, state_code, latitude, longitude, count, ..] = fields.as_slice() else {
        return None;
    };
    let place = Place {
        code: code.to_string(),
        name: name.to_string(),
        state: state.to_string(),
        country: country.to_string(),
        latitude: latitude.parse().ok()?,
        longitude: longitude.parse().ok()?,
    };
    Some((count.parse().ok()?, place, state_code.to_string()))
}

/// A grid line: country, code as written, place, state, latitude, longitude.
fn grid_place(rest: &str) -> Option<Place> {
    let fields: Vec<&str> = rest.split('\t').collect();
    let [country, code, name, state, latitude, longitude, ..] = fields.as_slice() else {
        return None;
    };
    Some(Place {
        code: code.to_string(),
        name: name.to_string(),
        state: state.to_string(),
        country: country.to_string(),
        latitude: round2(latitude.parse().ok()?),
        longitude: round2(longitude.parse().ok()?),
    })
}
