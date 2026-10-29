//! Places: a postal code or a city turned into coordinates, and coordinates turned into the
//! nearest place, for the apps' "Live radio near you".
//!
//! The data is GeoNames' postal code dump (CC BY 4.0), converted once by [`convert`] into a
//! compact file of deflated blocks of sorted lines, and read by [`PlaceIndex`] a block at a
//! time: only the small block list stays in memory, never the places themselves.
//!
//! The file, `OCTOPLC1`:
//!
//! - 8 bytes: the magic `OCTOPLC1`;
//! - the blocks, each a raw deflate stream of whole lines, sorted by key across the section;
//! - the block list, plain text, one line per block: `section \t first key \t offset \t length`,
//!   where the section is `p` (postal codes), `c` (cities) or `g` (the grid), and one line
//!   `r \t south row \t north row`, the grid rows that hold points;
//! - 24 bytes: the block list's offset and length (little-endian u64 each), then the magic again.
//!
//! A postal code line is `COUNTRY|CODE \t code as written (empty when the same) \t place \t
//! state \t latitude \t longitude`; a city line is `COUNTRY|city|state|county \t a postal code
//! \t city \t state \t state code \t latitude \t longitude \t postal codes`; a grid line is
//! `cell \t country \t code as written \t place \t state \t latitude \t longitude`, one per
//! rounded coordinate, keyed by its [`GRID_DEGREES`] cell ([`grid_key`]). Keys are normalized by
//! [`code_key`] and [`name_key`], coordinates rounded to 2 decimals.

mod convert;
mod index;

#[cfg(test)]
#[path = "places_tests.rs"]
mod tests;

pub use convert::{ConvertSummary, convert};
pub use index::PlaceIndex;

use crate::common::romanize::romanized_words;

/// The first and last 8 bytes of a places file.
pub const MAGIC: &[u8; 8] = b"OCTOPLC1";

/// The section of postal codes in the block list.
const POSTAL_SECTION: &str = "p";
/// The section of cities in the block list.
const CITY_SECTION: &str = "c";
/// The section of grid cells in the block list.
const GRID_SECTION: &str = "g";
/// The block list's line of the grid rows that hold points.
const RANGE_SECTION: &str = "r";

/// The size of a grid cell, in degrees of latitude and of longitude.
pub const GRID_DEGREES: f64 = 0.5;

/// How far the nearest place may be, in kilometers.
pub const NEAREST_KM: f64 = 30.0;

/// The Earth's mean radius, in kilometers.
const EARTH_KM: f64 = 6371.0;

/// Grid rows (latitude) and columns (longitude).
const GRID_ROWS: i64 = (180.0 / GRID_DEGREES) as i64;
const GRID_COLUMNS: i64 = (360.0 / GRID_DEGREES) as i64;

/// One place, as getPlace answers it.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    /// The postal code as the data writes it: the one asked for, the coarser code it was found
    /// by (the outward code of a full UK postcode), or for a city the first of its codes.
    pub code: String,
    pub name: String,
    /// The first level subdivision (a US state, a Canadian province); empty when the data has
    /// none.
    pub state: String,
    /// ISO 3166-1 alpha-2, uppercase.
    pub country: String,
    pub latitude: f64,
    pub longitude: f64,
}

/// A postal code as a key: ASCII letters and digits only, uppercase. "sw1a 1aa" and "SW1A1AA"
/// are the same key; leading zeros stay.
pub fn code_key(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|ch| ch.to_ascii_uppercase())
        .collect()
}

/// A city or state name as a key: spelled in ASCII, lowercase, every run of other characters
/// one space. "Zürich", "ZURICH" and "zurich" are the same key.
pub fn name_key(name: &str) -> String {
    romanized_words(name)
}

/// A country code as a key: two ASCII letters, uppercase, or `None`. `UK` is `GB`, its ISO
/// code.
pub fn country_key(country: &str) -> Option<String> {
    let country = country.trim();
    (country.len() == 2 && country.chars().all(|ch| ch.is_ascii_alphabetic())).then(|| {
        match country.to_ascii_uppercase().as_str() {
            "UK" => "GB".to_string(),
            other => other.to_string(),
        }
    })
}

/// The coarser codes to try, in order, when a country's data holds only part of its postal
/// codes, or a code is written longer than the data keeps it: the five digits of a ZIP+4 (the
/// US and the territories that use its ZIP codes), the outward code of a UK, Channel Islands or
/// Isle of Man postcode, the forward sortation area of a Canadian one, the four digits of a
/// Dutch one, the routing key of an Eircode, the letters of a Maltese one, the four digits of an
/// Argentine CPA, and the zero-filled codes GeoNames keeps for Brazil, Chile and China.
pub fn coarser_codes(country: &str, key: &str) -> Vec<String> {
    let len = key.len();
    let all_digits = key.chars().all(|ch| ch.is_ascii_digit());
    let coarser = match country {
        "US" | "PR" | "VI" | "GU" | "AS" | "MP" if len == 9 && all_digits => Some(key[..5].to_string()),
        "GB" | "JE" | "GG" | "IM" if len >= 5 => Some(key[..len - 3].to_string()),
        "CA" if len == 6 => Some(key[..3].to_string()),
        "NL" if len == 6 => Some(key[..4].to_string()),
        "IE" if len == 7 => Some(key[..3].to_string()),
        "MT" if len > 3 => Some(key[..3].to_string()),
        "AR" if len == 8
            && key[1..5].chars().all(|ch| ch.is_ascii_digit())
            && key[..1].chars().all(|ch| ch.is_ascii_alphabetic()) =>
        {
            Some(key[1..5].to_string())
        }
        "BR" if len == 8 && all_digits => Some(format!("{}000", &key[..5])),
        "CL" if len == 7 && all_digits => Some(format!("{}0000", &key[..3])),
        "CN" if len == 6 && all_digits => Some(format!("{}00", &key[..4])),
        _ => None,
    };
    coarser.into_iter().filter(|c| c != key).collect()
}

/// The grid cell of a coordinate: its row from the south pole and its column from the
/// antimeridian, the column wrapping around.
pub fn grid_cell(latitude: f64, longitude: f64) -> (i64, i64) {
    let row = ((latitude + 90.0) / GRID_DEGREES).floor() as i64;
    let column = ((longitude + 180.0) / GRID_DEGREES).floor() as i64;
    (row.clamp(0, GRID_ROWS - 1), column.rem_euclid(GRID_COLUMNS))
}

/// A grid cell as a key: `row|column`.
pub fn grid_key(latitude: f64, longitude: f64) -> String {
    let (row, column) = grid_cell(latitude, longitude);
    cell_key(row, column)
}

fn cell_key(row: i64, column: i64) -> String {
    format!("{row}|{column}")
}

/// The cells (row, column) that hold every point within [`NEAREST_KM`] of a coordinate: its own
/// and its neighbors, as many columns either side as that distance spans at each row's latitude
/// (more near the poles, the whole row at a pole), each cell once.
pub fn cells_near(latitude: f64, longitude: f64) -> Vec<(i64, i64)> {
    let (row, column) = grid_cell(latitude, longitude);
    let km_per_degree = EARTH_KM.to_radians();
    let rows_either_side = (NEAREST_KM / (km_per_degree * GRID_DEGREES)).ceil() as i64;
    let mut cells = Vec::new();
    for r in (row - rows_either_side).max(0)..=(row + rows_either_side).min(GRID_ROWS - 1) {
        // The row's edge nearest a pole, where a degree of longitude is shortest.
        let south = r as f64 * GRID_DEGREES - 90.0;
        let poleward = south.abs().max((south + GRID_DEGREES).abs()).min(90.0);
        let km_per_column = km_per_degree * poleward.to_radians().cos() * GRID_DEGREES;
        let either_side = if km_per_column > 0.0 {
            (NEAREST_KM / km_per_column).ceil() as i64
        } else {
            GRID_COLUMNS
        };
        if either_side * 2 + 1 >= GRID_COLUMNS {
            cells.extend((0..GRID_COLUMNS).map(|c| (r, c)));
        } else {
            cells.extend(
                (column - either_side..=column + either_side).map(|c| (r, c.rem_euclid(GRID_COLUMNS))),
            );
        }
    }
    cells
}

/// The great circle distance between two coordinates (latitude, longitude), in kilometers.
pub fn distance_km(from: (f64, f64), to: (f64, f64)) -> f64 {
    let (lat1, lat2) = (from.0.to_radians(), to.0.to_radians());
    let dlat = lat2 - lat1;
    let dlon = (to.1 - from.1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_KM * a.sqrt().min(1.0).asin()
}

/// A coordinate rounded to 2 decimals, never minus zero.
pub(crate) fn round2(value: f64) -> f64 {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded == 0.0 { 0.0 } else { rounded }
}
