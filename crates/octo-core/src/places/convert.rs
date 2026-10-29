//! GeoNames' postal code dump (`allCountries.txt`, tab-separated) into a places file.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};

use flate2::Compression;
use flate2::write::DeflateEncoder;

use super::{
    CITY_SECTION, GRID_SECTION, MAGIC, POSTAL_SECTION, RANGE_SECTION, code_key, country_key, distance_km,
    grid_cell, grid_key, name_key, round2,
};

/// About how many bytes of lines go in one block before it is deflated.
const BLOCK_BYTES: usize = 16 * 1024;

/// The longest field kept, in characters; longer ones are cut.
const FIELD_CHARS: usize = 120;

/// The most points a city's medoid is measured against: a city with more is measured against an
/// even spread of this many of them.
const MEDOID_SAMPLE: usize = 512;

/// What a conversion read and wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConvertSummary {
    /// Rows read.
    pub rows: usize,
    /// Rows left out: too few columns, no country, no code or no coordinates.
    pub skipped: usize,
    /// Postal codes written (one per country and code).
    pub codes: usize,
    /// Cities written (one per country, city, state and county).
    pub cities: usize,
    /// Points written to the grid (one per coordinate, rounded to 2 decimals).
    pub points: usize,
    /// Bytes written.
    pub bytes: u64,
}

/// One city: every postal code row with its country, name, state and county (admin code 2, else
/// admin name 2), and the coordinates of each.
struct City {
    code: String,
    name: String,
    state: String,
    state_code: String,
    points: Vec<(f64, f64)>,
}

/// Reads the GeoNames rows (country, postal code, place, admin name 1, admin code 1, admin
/// name 2, admin code 2, admin name 3, admin code 3, latitude, longitude, accuracy) and writes
/// the places file. A postal code listed more than once keeps its first row; a city's
/// coordinates are its postal codes' medoid (see [`medoid`]); of the postal codes at one rounded
/// coordinate, the first is the grid's point there.
pub fn convert(input: impl BufRead, output: impl Write) -> io::Result<ConvertSummary> {
    let mut summary = ConvertSummary::default();
    let mut codes: Vec<(String, String)> = Vec::new();
    let mut cities: HashMap<String, City> = HashMap::new();
    let mut points: Vec<(String, String)> = Vec::new();
    let mut pointed: HashSet<(i64, i64)> = HashSet::new();
    let mut rows: Option<(i64, i64)> = None;

    for line in input.lines() {
        let line = line?;
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        summary.rows += 1;
        let fields: Vec<&str> = line.split('\t').collect();
        let parsed = (fields.len() >= 11)
            .then(|| {
                let country = country_key(fields[0])?;
                let code = clean(fields[1]);
                let key = code_key(&code);
                let latitude = fields[9].trim().parse::<f64>().ok()?;
                let longitude = fields[10].trim().parse::<f64>().ok()?;
                let in_range = (-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude);
                (!key.is_empty() && in_range).then_some((country, code, key, latitude, longitude))
            })
            .flatten();
        let Some((country, code, key, latitude, longitude)) = parsed else {
            summary.skipped += 1;
            continue;
        };
        let name = clean(fields[2]);
        let state = clean(fields[3]);
        let state_code = clean(fields[4]);

        let written = if code == key { "" } else { code.as_str() };
        let (lat, lon) = (round2(latitude), round2(longitude));
        codes.push((
            format!("{country}|{key}"),
            format!("\t{written}\t{name}\t{state}\t{lat}\t{lon}"),
        ));
        // Hundredths of a degree, as whole numbers, so equal rounded coordinates are equal.
        if pointed.insert(((lat * 100.0).round() as i64, (lon * 100.0).round() as i64)) {
            let (row, _) = grid_cell(lat, lon);
            rows = Some(rows.map_or((row, row), |(south, north)| (south.min(row), north.max(row))));
            points.push((
                grid_key(lat, lon),
                format!("\t{country}\t{code}\t{name}\t{state}\t{lat}\t{lon}"),
            ));
        }

        let city_name = name_key(&name);
        if city_name.is_empty() {
            continue;
        }
        let state_name = name_key(if state.is_empty() { &state_code } else { &state });
        let county_code = code_key(fields[6]);
        let county = if county_code.is_empty() {
            name_key(fields[5])
        } else {
            county_code
        };
        cities
            .entry(format!("{country}|{city_name}|{state_name}|{county}"))
            .or_insert_with(|| City {
                code: code.clone(),
                name: name.clone(),
                state: state.clone(),
                state_code: state_code.clone(),
                points: Vec::new(),
            })
            .points
            .push((latitude, longitude));
    }

    // Stable, so the first row of a code listed twice is the one kept.
    codes.sort_by(|a, b| a.0.cmp(&b.0));
    codes.dedup_by(|later, earlier| later.0 == earlier.0);
    summary.codes = codes.len();

    let mut city_lines: Vec<(String, String)> = cities
        .into_iter()
        .map(|(key, city)| {
            let (latitude, longitude) = medoid(&city.points);
            let line = format!(
                "\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                city.code,
                city.name,
                city.state,
                city.state_code,
                round2(latitude),
                round2(longitude),
                city.points.len()
            );
            (key, line)
        })
        .collect();
    city_lines.sort_by(|a, b| a.0.cmp(&b.0));
    summary.cities = city_lines.len();

    // Stable: within a cell, points keep the order of their rows.
    points.sort_by(|a, b| a.0.cmp(&b.0));
    summary.points = points.len();

    let mut writer = BlockWriter::new(output);
    writer.raw(MAGIC)?;
    writer.section(POSTAL_SECTION, &codes)?;
    writer.section(CITY_SECTION, &city_lines)?;
    writer.section(GRID_SECTION, &points)?;
    if let Some((south, north)) = rows {
        writer.rows(south, north);
    }
    summary.bytes = writer.finish()?;
    Ok(summary)
}

/// The point with the least total distance to the others, the first such point on a tie; with
/// more than [`MEDOID_SAMPLE`] points, the others measured are an even spread of that many.
/// Always one of the points, so two clusters of postal codes far apart never meet in a field
/// between them, and a city across the antimeridian stays there.
fn medoid(points: &[(f64, f64)]) -> (f64, f64) {
    let step = points.len().div_ceil(MEDOID_SAMPLE).max(1);
    let mut best: Option<(f64, (f64, f64))> = None;
    for &point in points {
        let total: f64 = points
            .iter()
            .step_by(step)
            .map(|&other| distance_km(point, other))
            .sum();
        if best.is_none_or(|(least, _)| total < least) {
            best = Some((total, point));
        }
    }
    best.map_or((0.0, 0.0), |(_, point)| point)
}

/// A field with no tabs or line breaks, trimmed and cut to [`FIELD_CHARS`].
fn clean(field: &str) -> String {
    field
        .trim()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(FIELD_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Writes the blocks and then the block list.
struct BlockWriter<W: Write> {
    out: W,
    at: u64,
    list: String,
}

impl<W: Write> BlockWriter<W> {
    fn new(out: W) -> Self {
        BlockWriter {
            out,
            at: 0,
            list: String::new(),
        }
    }

    fn raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.at += bytes.len() as u64;
        Ok(())
    }

    /// The sorted lines of one section, `(key, rest of the line)`, cut into blocks.
    fn section(&mut self, section: &str, lines: &[(String, String)]) -> io::Result<()> {
        let mut block = String::new();
        let mut first_key: Option<&str> = None;
        for (key, rest) in lines {
            if first_key.is_none() {
                first_key = Some(key);
            }
            block.push_str(key);
            block.push_str(rest);
            block.push('\n');
            if block.len() >= BLOCK_BYTES {
                self.block(section, first_key.take().unwrap_or_default(), &block)?;
                block.clear();
            }
        }
        if let Some(first) = first_key {
            self.block(section, first, &block)?;
        }
        Ok(())
    }

    fn block(&mut self, section: &str, first_key: &str, lines: &str) -> io::Result<()> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(lines.as_bytes())?;
        let packed = encoder.finish()?;
        self.list.push_str(&format!(
            "{section}\t{first_key}\t{}\t{}\n",
            self.at,
            packed.len()
        ));
        self.raw(&packed)
    }

    /// The grid's southernmost and northernmost rows, as a line of the block list.
    fn rows(&mut self, south: i64, north: i64) {
        self.list
            .push_str(&format!("{RANGE_SECTION}\t{south}\t{north}\n"));
    }

    /// The block list and the trailer; the bytes written in all.
    fn finish(mut self) -> io::Result<u64> {
        let list_at = self.at;
        let list = std::mem::take(&mut self.list);
        self.raw(list.as_bytes())?;
        self.raw(&list_at.to_le_bytes())?;
        self.raw(&(list.len() as u64).to_le_bytes())?;
        self.raw(MAGIC)?;
        self.out.flush()?;
        Ok(self.at)
    }
}
