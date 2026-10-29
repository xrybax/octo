//! The places file: converting GeoNames rows, and looking up postal codes and cities in what
//! the converter wrote.

use std::path::{Path, PathBuf};

use super::*;

/// GeoNames rows, as `allCountries.txt` has them: country, postal code, place, admin name 1,
/// admin code 1, admin name 2, admin code 2, admin name 3, admin code 3, latitude, longitude,
/// accuracy.
pub(crate) const FIXTURE: &[&str] = &[
    "US\t90210\tBeverly Hills\tCalifornia\tCA\tLos Angeles\t037\t\t\t34.0901\t-118.4065\t4",
    "US\t02134\tAllston\tMassachusetts\tMA\tSuffolk\t025\t\t\t42.3539\t-71.1337\t4",
    "US\t62701\tSpringfield\tIllinois\tIL\tSangamon\t167\t\t\t39.8017\t-89.6436\t4",
    "US\t62702\tSpringfield\tIllinois\tIL\tSangamon\t167\t\t\t39.8201\t-89.6437\t4",
    "US\t62703\tSpringfield\tIllinois\tIL\tSangamon\t167\t\t\t39.7623\t-89.6271\t4",
    "US\t01101\tSpringfield\tMassachusetts\tMA\tHampden\t013\t\t\t42.1015\t-72.5898\t4",
    "US\t01103\tSpringfield\tMassachusetts\tMA\tHampden\t013\t\t\t42.1029\t-72.5887\t4",
    "GB\tSW1A\tLondon\tEngland\tENG\tGreater London\t11609024\t\t\t51.5\t-0.1397\t",
    "JP\t100-0001\tChiyoda\tTokyo To\t40\tChiyoda Ku\t1864529\t\t\t35.6895\t139.7536\t",
    "CH\t8001\tZürich\tKanton Zürich\tZH\tBezirk Zürich\t112\t\t\t47.3721\t8.5422\t1",
    "PL\t59-700\tRakowice\tLower Silesia\t72\t\t\t\t\t51.2447\t15.5347\t6",
    "PL\t59-700\tGolnice\tLower Silesia\t72\t\t\t\t\t51.3504\t15.5852\t6",
    "GB\tE16\tCanning Town\tEngland\tENG\t\t\t\t\t51.51\t0.01\t",
    "FJ\t0000\tTaveuni\t\t\t\t\t\t\t-16.8\t179.95\t",
    "CA\tT0A\tEastern Alberta (St. Paul)\tAlberta\tAB\t\t\t\t\t54.766\t-111.7174\t6",
];

/// The fixture rows as a places file in a temp folder.
pub(crate) fn fixture_file(dir: &Path) -> PathBuf {
    let path = dir.join("places.bin");
    write_places(&path, FIXTURE);
    path
}

/// `rows` converted into a places file at `path`.
pub(crate) fn write_places(path: &Path, rows: &[&str]) -> ConvertSummary {
    let text = rows.join("\n");
    let file = std::fs::File::create(path).expect("a places file");
    convert(text.as_bytes(), std::io::BufWriter::new(file)).expect("converted")
}

fn index() -> (tempfile::TempDir, PlaceIndex) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = fixture_file(dir.path());
    (dir, PlaceIndex::new(path))
}

fn place(code: &str, name: &str, state: &str, country: &str, latitude: f64, longitude: f64) -> Place {
    Place {
        code: code.into(),
        name: name.into(),
        state: state.into(),
        country: country.into(),
        latitude,
        longitude,
    }
}

// ---- Keys ----------------------------------------------------------------------------------

#[test]
fn codes_ignore_case_spaces_and_dashes_and_keep_leading_zeros() {
    assert_eq!(code_key("sw1a 1aa"), "SW1A1AA");
    assert_eq!(code_key(" SW1A1AA "), "SW1A1AA");
    assert_eq!(code_key("100-0001"), "1000001");
    assert_eq!(code_key("02134"), "02134");
}

#[test]
fn names_ignore_case_accents_and_punctuation() {
    assert_eq!(name_key("Zürich"), "zurich");
    assert_eq!(name_key("  WINSTON-SALEM "), "winston salem");
    assert_eq!(name_key("St. Paul"), "st paul");
}

#[test]
fn countries_are_two_letters() {
    assert_eq!(country_key(" us "), Some("US".into()));
    assert_eq!(country_key("USA"), None);
    assert_eq!(country_key("1A"), None);
    assert_eq!(country_key(""), None);
}

#[test]
fn uk_is_gb() {
    assert_eq!(country_key("UK"), Some("GB".into()));
    assert_eq!(country_key(" uk"), Some("GB".into()));
    let (_dir, index) = index();
    assert_eq!(
        index.by_code("UK", "SW1A 1AA").map(|p| (p.code, p.country)),
        Some(("SW1A".into(), "GB".into()))
    );
    assert_eq!(
        index.by_name("uk", "London", None).map(|p| p.country),
        Some("GB".into())
    );
}

#[test]
fn coarser_codes_are_the_parts_the_data_keeps() {
    assert_eq!(coarser_codes("GB", "SW1A1AA"), vec!["SW1A"]);
    assert_eq!(coarser_codes("GB", "M11AE"), vec!["M1"]);
    assert_eq!(coarser_codes("CA", "T0A1A0"), vec!["T0A"]);
    assert_eq!(coarser_codes("NL", "1012AB"), vec!["1012"]);
    assert_eq!(coarser_codes("IE", "D02X285"), vec!["D02"]);
    assert_eq!(coarser_codes("AR", "C1425ABC"), vec!["1425"]);
    assert_eq!(coarser_codes("BR", "01310100"), vec!["01310000"]);
    assert!(coarser_codes("BR", "01310000").is_empty());
    assert!(coarser_codes("US", "90210").is_empty());
    assert!(coarser_codes("GB", "SW1A").is_empty());
    assert_eq!(coarser_codes("US", "902101234"), vec!["90210"]);
    assert_eq!(coarser_codes("PR", "009011234"), vec!["00901"]);
    assert!(coarser_codes("US", "9021012").is_empty());
    assert!(coarser_codes("DE", "902101234").is_empty());
}

#[test]
fn a_zip_plus_4_is_found_by_its_five_digits() {
    let (_dir, index) = index();
    for asked in ["90210-1234", "902101234", "90210 1234"] {
        assert_eq!(
            index.by_code("US", asked).map(|p| p.code),
            Some("90210".into()),
            "{asked}"
        );
    }
    assert_eq!(index.by_code("US", "99999-1234"), None);
}

#[test]
fn coordinates_round_to_two_decimals_without_minus_zero() {
    assert_eq!(round2(34.0901), 34.09);
    assert_eq!(round2(-118.4065), -118.41);
    assert_eq!(round2(-0.001), 0.0);
    assert!(round2(-0.001).is_sign_positive());
}

// ---- Postal codes --------------------------------------------------------------------------

#[test]
fn a_postal_code_is_found_with_its_place() {
    let (_dir, index) = index();
    assert_eq!(
        index.by_code("US", "90210"),
        Some(place(
            "90210",
            "Beverly Hills",
            "California",
            "US",
            34.09,
            -118.41
        ))
    );
    assert_eq!(
        index.by_code("us", " 90210 ").map(|p| p.name),
        Some("Beverly Hills".into())
    );
}

#[test]
fn leading_zeros_are_part_of_the_code() {
    let (_dir, index) = index();
    assert_eq!(index.by_code("US", "02134").map(|p| p.code), Some("02134".into()));
    assert_eq!(index.by_code("US", "2134"), None);
}

#[test]
fn a_code_written_with_a_dash_is_found_either_way_and_answered_as_written() {
    let (_dir, index) = index();
    for asked in ["100-0001", "1000001", "100 0001"] {
        assert_eq!(
            index.by_code("JP", asked),
            Some(place("100-0001", "Chiyoda", "Tokyo To", "JP", 35.69, 139.75)),
            "{asked}"
        );
    }
}

#[test]
fn a_full_uk_postcode_is_found_by_its_outward_code_in_any_spelling() {
    let (_dir, index) = index();
    let london = Some(place("SW1A", "London", "England", "GB", 51.5, -0.14));
    assert_eq!(index.by_code("GB", "sw1a 1aa"), london);
    assert_eq!(index.by_code("GB", "SW1A1AA"), london);
    assert_eq!(index.by_code("gb", "SW1A"), london);
}

#[test]
fn a_canadian_code_is_found_by_its_first_three() {
    let (_dir, index) = index();
    assert_eq!(index.by_code("CA", "t0a 1a0").map(|p| p.code), Some("T0A".into()));
}

#[test]
fn a_code_listed_twice_keeps_its_first_place() {
    let (_dir, index) = index();
    assert_eq!(
        index.by_code("PL", "59-700").map(|p| p.name),
        Some("Rakowice".into())
    );
}

#[test]
fn an_unknown_code_country_or_blank_code_finds_nothing() {
    let (_dir, index) = index();
    assert_eq!(index.by_code("US", "99999"), None);
    assert_eq!(index.by_code("DE", "90210"), None);
    assert_eq!(index.by_code("USA", "90210"), None);
    assert_eq!(index.by_code("US", " - "), None);
}

// ---- Cities --------------------------------------------------------------------------------

#[test]
fn a_city_without_a_state_is_the_one_with_the_most_postal_codes() {
    let (_dir, index) = index();
    let found = index.by_name("US", "springfield", None).expect("a city");
    assert_eq!(found.state, "Illinois");
    assert_eq!(found.code, "62701");
    // 62701, the code nearest the other two (their medoid).
    assert_eq!((found.latitude, found.longitude), (39.8, -89.64));
}

#[test]
fn a_city_with_a_state_by_name_or_code() {
    let (_dir, index) = index();
    for state in ["MA", "ma", "Massachusetts", " massachusetts "] {
        let found = index.by_name("US", "Springfield", Some(state)).expect(state);
        assert_eq!(found.state, "Massachusetts", "{state}");
        assert_eq!(found.code, "01101");
        assert_eq!((found.latitude, found.longitude), (42.1, -72.59));
    }
    assert_eq!(index.by_name("US", "Springfield", Some("Texas")), None);
}

#[test]
fn a_city_is_found_without_its_accents() {
    let (_dir, index) = index();
    let found = index.by_name("ch", "zurich", None).expect("Zürich");
    assert_eq!(found, place("8001", "Zürich", "Kanton Zürich", "CH", 47.37, 8.54));
    assert_eq!(
        index.by_name("CH", "ZÜRICH", Some("zh")).map(|p| p.code),
        Some("8001".into())
    );
}

#[test]
fn a_city_name_is_matched_whole() {
    let (_dir, index) = index();
    assert_eq!(index.by_name("US", "Spring", None), None);
    assert_eq!(index.by_name("US", "Springfield Gardens", None), None);
    assert_eq!(index.by_name("US", "  ", None), None);
}

/// Towns that share a name and a state but not a county, and a town across the antimeridian.
const SAME_NAMES: &[&str] = &[
    "GB\tPO30\tNewport\tEngland\tENG\tIsle of Wight\tE06000046\t\t\t50.7\t-1.29\t",
    "GB\tPO30 5\tNewport\tEngland\tENG\tIsle of Wight\tE06000046\t\t\t50.69\t-1.3\t",
    "GB\tPO30 2\tNewport\tEngland\tENG\tIsle of Wight\tE06000046\t\t\t50.71\t-1.28\t",
    "GB\tTF10\tNewport\tEngland\tENG\tShropshire\tE06000051\t\t\t52.77\t-2.38\t",
    "GB\tTF10 7\tNewport\tEngland\tENG\tShropshire\tE06000051\t\t\t52.76\t-2.37\t",
    "US\t37064\tFranklin\tTennessee\tTN\tWilliamson\t187\t\t\t35.93\t-86.87\t4",
    "US\t37067\tFranklin\tTennessee\tTN\tWilliamson\t187\t\t\t35.91\t-86.82\t4",
    "US\t37069\tFranklin\tTennessee\tTN\tWilliamson\t187\t\t\t35.98\t-86.91\t4",
    "US\t37306\tFranklin\tTennessee\tTN\tFranklin\t051\t\t\t35.15\t-86.1\t4",
    "FJ\t0001\tRabi\tNorthern\tN\t\t\t\t\t-16.5\t179.98\t",
    "FJ\t0002\tRabi\tNorthern\tN\t\t\t\t\t-16.51\t-179.99\t",
    "FJ\t0003\tRabi\tNorthern\tN\t\t\t\t\t-16.49\t179.97\t",
];

fn same_names() -> (tempfile::TempDir, PlaceIndex, ConvertSummary) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("places.bin");
    let summary = write_places(&path, SAME_NAMES);
    (dir, PlaceIndex::new(path), summary)
}

#[test]
fn towns_with_one_name_in_one_state_stay_apart_by_county() {
    let (_dir, index, summary) = same_names();
    // Two Newports, two Franklins and Rabi.
    assert_eq!(summary.cities, 5);
    // The Isle of Wight's Newport has more postal codes; its place is among them, never the
    // field halfway to Shropshire (about 51.53 north) that their mean would be.
    for state in [None, Some("England"), Some("ENG")] {
        let found = index.by_name("GB", "Newport", state).expect("Newport");
        assert_eq!((found.latitude, found.longitude), (50.7, -1.29), "{state:?}");
        assert_eq!(found.code, "PO30");
    }
    let franklin = index.by_name("US", "Franklin", Some("TN")).expect("Franklin");
    assert_eq!(franklin.code, "37064");
    assert_eq!((franklin.latitude, franklin.longitude), (35.93, -86.87));
}

#[test]
fn a_town_across_the_antimeridian_stays_there() {
    let (_dir, index, _) = same_names();
    let rabi = index.by_name("FJ", "Rabi", None).expect("Rabi");
    // The mean of its longitudes would be about 60 east.
    assert_eq!((rabi.latitude, rabi.longitude), (-16.5, 179.98));
}

// ---- Coordinates -------------------------------------------------------------------------

#[test]
fn the_nearest_of_several_places_is_the_one_found() {
    let (_dir, index) = index();
    assert_eq!(index.nearest(39.8, -89.64).map(|p| p.code), Some("62701".into()));
    assert_eq!(
        index.nearest(39.816, -89.641).map(|p| p.code),
        Some("62702".into())
    );
    assert_eq!(index.nearest(39.75, -89.62).map(|p| p.code), Some("62703".into()));
    assert_eq!(
        index.nearest(34.1, -118.4),
        Some(place(
            "90210",
            "Beverly Hills",
            "California",
            "US",
            34.09,
            -118.41
        ))
    );
}

#[test]
fn nothing_within_30_km_finds_nothing() {
    let (_dir, index) = index();
    assert_eq!(index.nearest(0.0, 0.0), None);
    // Springfield, Illinois is about 55 km west.
    assert_eq!(index.nearest(39.8, -89.0), None);
    // About 27 km north of 62702 (39.82, -89.64) is still near enough.
    assert_eq!(
        index.nearest(40.065, -89.64).map(|p| p.code),
        Some("62702".into())
    );
    assert!(distance_km((40.095, -89.64), (39.82, -89.64)) > NEAREST_KM);
    assert_eq!(index.nearest(40.095, -89.64), None);
}

#[test]
fn a_point_near_a_cell_edge_finds_a_place_in_the_neighboring_cell() {
    let (_dir, index) = index();
    // Canning Town (51.51, 0.01) is in the cell north east of this point.
    assert_ne!(grid_key(51.49, -0.01), grid_key(51.51, 0.01));
    assert_eq!(
        index.nearest(51.49, -0.01).map(|p| p.name),
        Some("Canning Town".into())
    );
    // Across the antimeridian: Taveuni is at 179.95 east.
    assert_ne!(grid_key(-16.8, -179.98), grid_key(-16.8, 179.95));
    assert_eq!(index.nearest(-16.8, -179.98).map(|p| p.code), Some("0000".into()));
}

#[test]
fn coordinates_off_the_globe_find_nothing() {
    let (_dir, index) = index();
    assert_eq!(index.nearest(91.0, 0.0), None);
    assert_eq!(index.nearest(0.0, -180.5), None);
    assert_eq!(index.nearest(f64::NAN, 0.0), None);
}

#[test]
fn the_cells_searched_cover_30_km_at_every_latitude() {
    // At the equator the cell and its eight neighbors.
    assert_eq!(cells_near(0.2, 0.2).len(), 9);
    // Far north a column is narrower than 30 km, so more columns are read.
    assert!(cells_near(70.2, 10.2).len() > 9);
    // At a pole, the whole row.
    assert!(cells_near(89.99, 0.0).len() >= 720);
    for (lat, lon) in [
        (0.0, 0.0),
        (45.49, 7.99),
        (64.9, -150.1),
        (-33.5, 151.0),
        (78.2, 15.6),
    ] {
        let cells = cells_near(lat, lon);
        // Points 29 km away in eight directions all fall in a cell that is read.
        for bearing in 0..8 {
            let angle = f64::from(bearing) * std::f64::consts::FRAC_PI_4;
            let dlat = 29.0 / 111.19 * angle.cos();
            let dlon = 29.0 / (111.19 * lat.to_radians().cos()) * angle.sin();
            let (plat, plon) = (lat + dlat, lon + dlon);
            assert!(
                distance_km((lat, lon), (plat, plon)) < 30.0,
                "{lat},{lon} {bearing}"
            );
            assert!(
                cells.contains(&grid_cell(plat, plon)),
                "{lat},{lon} bearing {bearing}"
            );
        }
    }
}

// ---- The file ------------------------------------------------------------------------------

#[test]
fn a_missing_file_finds_nothing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let index = PlaceIndex::new(dir.path().join("places.bin"));
    assert_eq!(index.by_code("US", "90210"), None);
    assert_eq!(index.by_name("US", "Springfield", None), None);
}

#[test]
fn a_file_that_is_not_a_places_file_finds_nothing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("places.bin");
    std::fs::write(&path, b"US|90210\tBeverly Hills\n".repeat(10)).expect("written");
    assert_eq!(PlaceIndex::new(&path).by_code("US", "90210"), None);
}

#[test]
fn a_file_put_in_place_later_is_read_then() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let index = PlaceIndex::new(dir.path().join("places.bin"));
    assert_eq!(index.by_code("US", "90210"), None);
    fixture_file(dir.path());
    assert_eq!(index.by_code("US", "90210").map(|p| p.code), Some("90210".into()));
}

// ---- The converter -------------------------------------------------------------------------

#[test]
fn the_converter_counts_what_it_kept_and_left_out() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut rows = FIXTURE.to_vec();
    rows.push("US\t12345\tNowhere\tNew York\tNY\t\t\t\t\tnorth\t-73.9\t");
    rows.push("US\t12346\tFar Out\tNew York\tNY\t\t\t\t\t95.0\t-73.9\t");
    rows.push("US\t\tNo Code\tNew York\tNY\t\t\t\t\t40.0\t-73.9\t");
    rows.push("USA\t10001\tNew York\tNew York\tNY\t\t\t\t\t40.0\t-73.9\t");
    rows.push("US\t10001\tToo short");
    rows.push("");
    let summary = write_places(&dir.path().join("places.bin"), &rows);
    assert_eq!(summary.rows, FIXTURE.len() + 5);
    assert_eq!(summary.skipped, 5);
    // Every fixture code once (59-700 is listed twice).
    assert_eq!(summary.codes, FIXTURE.len() - 1);
    // Beverly Hills, Allston, Springfield (Illinois), Springfield (Massachusetts), London,
    // Chiyoda, Zürich, Rakowice, Golnice, Canning Town, Taveuni and Eastern Alberta.
    assert_eq!(summary.cities, 12);
    // Every row but 01103, at the same rounded coordinate as 01101.
    assert_eq!(summary.points, FIXTURE.len() - 1);
    let bytes = std::fs::read(dir.path().join("places.bin")).expect("read");
    assert_eq!(summary.bytes, bytes.len() as u64);
    assert_eq!(&bytes[..8], MAGIC);
    assert_eq!(&bytes[bytes.len() - 8..], MAGIC);
}

#[test]
fn the_converter_cuts_many_rows_into_blocks_that_are_all_found() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let rows: Vec<String> = (0..20_000)
        .map(|n| {
            format!(
                "DE\t{n:05}\tOrt {n}\tLand {}\t{:02}\t\t\t\t\t{}\t{}\t",
                n % 16,
                n % 16,
                47.0 + f64::from(n) / 10_000.0,
                6.0 + f64::from(n) / 5_000.0
            )
        })
        .collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let path = dir.path().join("places.bin");
    let summary = write_places(&path, &rows);
    assert_eq!((summary.codes, summary.cities), (20_000, 20_000));

    let index = PlaceIndex::new(&path);
    let blocks = std::fs::File::open(&path)
        .and_then(|mut file| index::read_blocks_for_tests(&mut file))
        .expect("a block list");
    assert!(blocks.0 > 10 && blocks.1 > 10 && blocks.2 > 1, "{blocks:?}");
    for n in [0, 1, 4_999, 10_000, 15_432, 19_999] {
        let code = format!("{n:05}");
        let found = index.by_code("DE", &code).unwrap_or_else(|| panic!("{code}"));
        assert_eq!(found.name, format!("Ort {n}"));
        assert_eq!(found.latitude, round2(47.0 + f64::from(n) / 10_000.0));
        let city = index
            .by_name("DE", &format!("ort {n}"), None)
            .unwrap_or_else(|| panic!("{n}"));
        assert_eq!(city.code, code);
    }
    assert_eq!(index.by_code("DE", "20000"), None);
    assert_eq!(index.by_code("DE", "00000 "), index.by_code("DE", "00000"));
}

#[test]
fn a_lookup_unpacks_one_or_two_blocks() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let rows: Vec<String> = (0..50_000)
        .map(|n| format!("FR\t{n:05}\tVille {n}\tRégion\t11\t\t\t\t\t48.85\t2.35\t"))
        .collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let path = dir.path().join("places.bin");
    write_places(&path, &rows);
    let index = PlaceIndex::new(&path);
    for n in (0..50_000).step_by(997) {
        let before = index.blocks_read();
        index.by_code("FR", &format!("{n:05}")).expect("found");
        let read = index.blocks_read() - before;
        // The block that holds it, and the one before when it is a block's first line.
        assert!((1..=2).contains(&read), "{n}: {read} blocks");
    }
    let before = index.blocks_read();
    index.nearest(48.85, 2.35).expect("found");
    assert!(index.blocks_read() - before <= 2);
}

/// A grid of points 0.05 degrees of latitude and 0.07 of longitude apart, from 44 north and 5
/// east: 61 rows by 58, over many grid cells.
fn spread_rows() -> Vec<String> {
    (0..61)
        .flat_map(|i| {
            (0..58).map(move |j| {
                let latitude = 44.0 + f64::from(i) * 0.05;
                let longitude = 5.0 + f64::from(j) * 0.07;
                format!("FR\t{i:02}{j:02}0\tLieu {i} {j}\tRégion\t11\t\t\t\t\t{latitude}\t{longitude}\t")
            })
        })
        .collect()
}

/// Coordinates spread over and around an area, the same every run.
fn coordinates(count: usize, south: f64, north: f64, west: f64, east: f64) -> Vec<(f64, f64)> {
    let mut state: u64 = 0x5eed;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..count)
        .map(|_| (south + next() * (north - south), west + next() * (east - west)))
        .collect()
}

#[test]
fn the_nearest_point_over_many_cells_is_the_nearest_of_all() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let rows = spread_rows();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let path = dir.path().join("places.bin");
    write_places(&path, &rows);
    let grid_blocks = std::fs::File::open(&path)
        .and_then(|mut file| index::read_blocks_for_tests(&mut file))
        .expect("a block list")
        .2;
    assert!(grid_blocks > 3, "{grid_blocks} grid blocks");
    let points: Vec<(f64, f64)> = (0..61)
        .flat_map(|i| {
            (0..58).map(move |j| {
                (
                    round2(44.0 + f64::from(i) * 0.05),
                    round2(5.0 + f64::from(j) * 0.07),
                )
            })
        })
        .collect();
    let index = PlaceIndex::new(&path);
    let mut found_some = 0;
    for (latitude, longitude) in coordinates(300, 43.5, 47.5, 4.4, 9.6) {
        let nearest = points
            .iter()
            .map(|&point| distance_km((latitude, longitude), point))
            .fold(f64::INFINITY, f64::min);
        let before = index.blocks_read();
        let found = index.nearest(latitude, longitude);
        let read = index.blocks_read() - before;
        assert!(
            read <= grid_blocks as u64,
            "{latitude},{longitude}: {read} blocks"
        );
        match found {
            Some(place) => {
                found_some += 1;
                let km = distance_km((latitude, longitude), (place.latitude, place.longitude));
                assert!(
                    (km - nearest).abs() < 1e-9,
                    "{latitude},{longitude}: {km} km, not {nearest}"
                );
            }
            None => assert!(nearest > NEAREST_KM, "{latitude},{longitude}: {nearest} km"),
        }
    }
    assert!(found_some > 150, "{found_some}");
    // No lookup unpacked a block twice.
    assert_eq!(index.blocks_reread(), 0);
}

#[test]
fn far_north_each_block_is_unpacked_once_and_past_the_data_none_is() {
    let dir = tempfile::tempdir().expect("a temp dir");
    // Points all around the globe at 82 north, where a grid column is about 8 km wide.
    let rows: Vec<String> = (0..2_000)
        .map(|n| {
            let longitude = -180.0 + f64::from(n) * 0.18;
            format!("GL\t{n:04}\tPunkt {n}\t\t\t\t\t\t\t82.1\t{longitude}\t")
        })
        .collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let path = dir.path().join("places.bin");
    write_places(&path, &rows);
    let grid_blocks = std::fs::File::open(&path)
        .and_then(|mut file| index::read_blocks_for_tests(&mut file))
        .expect("a block list")
        .2;
    assert!(grid_blocks > 1, "{grid_blocks} grid blocks");
    let index = PlaceIndex::new(&path);
    for longitude in [-179.9, -90.0, 0.05, 45.3, 179.9] {
        assert!(cells_near(82.2, longitude).len() > 20);
        let before = index.blocks_read();
        assert!(index.nearest(82.2, longitude).is_some(), "{longitude}");
        let read = index.blocks_read() - before;
        assert!(
            read <= grid_blocks as u64,
            "{longitude}: {read} blocks of {grid_blocks}"
        );
    }
    assert_eq!(index.blocks_reread(), 0);
    // At the poles every column of a row is a cell, and no point is in any of them.
    for (latitude, longitude) in [(89.99, 0.0), (90.0, 120.0), (-89.99, 0.0), (0.0, 0.0)] {
        let before = index.blocks_read();
        assert_eq!(index.nearest(latitude, longitude), None);
        assert_eq!(index.blocks_read(), before, "{latitude},{longitude}");
    }
}

// ---- Damaged files ---------------------------------------------------------------------------

fn deflated(text: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(text).expect("deflated");
    encoder.finish().expect("deflated")
}

/// A places file written by hand: the magic, the packed postal code blocks `(first key, packed)`,
/// the block list (as `list` rewrites it) and the trailer, which names `list_at` as the block
/// list's offset when given.
fn hand_made(path: &Path, blocks: &[(&str, Vec<u8>)], list: impl Fn(String) -> String, list_at: Option<u64>) {
    let mut bytes = MAGIC.to_vec();
    let mut lines = String::new();
    for (first_key, packed) in blocks {
        lines.push_str(&format!("p\t{first_key}\t{}\t{}\n", bytes.len(), packed.len()));
        bytes.extend_from_slice(packed);
    }
    let lines = list(lines);
    let at = bytes.len() as u64;
    bytes.extend_from_slice(lines.as_bytes());
    bytes.extend_from_slice(&list_at.unwrap_or(at).to_le_bytes());
    bytes.extend_from_slice(&(lines.len() as u64).to_le_bytes());
    bytes.extend_from_slice(MAGIC);
    std::fs::write(path, bytes).expect("written");
}

const BEVERLY_HILLS: &[u8] = b"US|90210\t\tBeverly Hills\tCalifornia\t34.09\t-118.41\n";

#[test]
fn a_hand_made_file_is_read_like_a_converted_one() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("places.bin");
    hand_made(&path, &[("US|90210", deflated(BEVERLY_HILLS))], |list| list, None);
    let index = PlaceIndex::new(&path);
    assert!(index.available());
    assert_eq!(
        index.by_code("US", "90210").map(|p| p.name),
        Some("Beverly Hills".into())
    );
}

#[test]
fn offsets_out_of_the_file_find_nothing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("places.bin");
    let block = || vec![("US|90210", deflated(BEVERLY_HILLS))];
    // The block list's offset past the end, and inside the magic.
    for list_at in [1_000_000, u64::MAX - 4, 3] {
        hand_made(&path, &block(), |list| list, Some(list_at));
        let index = PlaceIndex::new(&path);
        assert!(!index.available(), "{list_at}");
        assert_eq!(index.by_code("US", "90210"), None, "{list_at}");
    }
    // A block past the block list, one inside the magic, a length that overflows, and an
    // offset that is not a number.
    for line in [
        "p\tUS|90210\t5000\t10\n",
        "p\tUS|90210\t0\t10\n",
        "p\tUS|90210\t8\t18446744073709551615\n",
        "p\tUS|90210\teight\t10\n",
        "p\tUS|90210\n",
        "r\tsouth\t300\n",
    ] {
        hand_made(&path, &block(), |_| line.to_string(), None);
        let index = PlaceIndex::new(&path);
        assert!(!index.available(), "{line}");
        assert_eq!(index.by_code("US", "90210"), None, "{line}");
    }
}

#[test]
fn a_block_larger_than_1_mb_unpacked_is_an_error_not_cut() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("places.bin");
    let line = b"US|00000\t\tSomewhere\t\t1\t1\n";
    let lines = |count: usize| {
        let mut text = Vec::new();
        for _ in 0..count {
            text.extend_from_slice(line);
        }
        text
    };
    let just_under = (1024 * 1024) / line.len();
    hand_made(
        &path,
        &[("US|00000", deflated(&lines(just_under)))],
        |list| list,
        None,
    );
    assert_eq!(
        PlaceIndex::new(&path).by_code("US", "00000").map(|p| p.name),
        Some("Somewhere".into())
    );
    let packed = deflated(&lines(just_under + 1));
    assert!(packed.len() < 1024 * 1024, "{} bytes packed", packed.len());
    hand_made(&path, &[("US|00000", packed)], |list| list, None);
    let index = PlaceIndex::new(&path);
    assert!(index.available());
    assert_eq!(index.by_code("US", "00000"), None);
}

#[test]
fn a_block_that_is_not_utf_8_finds_nothing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("places.bin");
    hand_made(
        &path,
        &[(
            "US|90210",
            deflated(b"US|90210\t\tBeverly \xff Hills\tCalifornia\t34.09\t-118.41\n"),
        )],
        |list| list,
        None,
    );
    assert_eq!(PlaceIndex::new(&path).by_code("US", "90210"), None);
    // Not a deflate stream at all.
    hand_made(&path, &[("US|90210", BEVERLY_HILLS.to_vec())], |list| list, None);
    assert_eq!(PlaceIndex::new(&path).by_code("US", "90210"), None);
}

#[test]
fn a_file_cut_off_finds_nothing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = fixture_file(dir.path());
    let bytes = std::fs::read(&path).expect("read");
    for keep in [bytes.len() - 1, bytes.len() - 30, bytes.len() / 2, 20] {
        std::fs::write(&path, &bytes[..keep]).expect("written");
        let index = PlaceIndex::new(&path);
        assert!(!index.available(), "{keep}");
        assert_eq!(index.by_code("US", "90210"), None, "{keep}");
        assert_eq!(index.nearest(34.1, -118.4), None, "{keep}");
    }
}

#[test]
fn a_file_replaced_is_read_again() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = fixture_file(dir.path());
    let index = PlaceIndex::new(&path);
    assert!(index.by_code("US", "90210").is_some());
    write_places(&path, &["DE\t10115\tBerlin\tBerlin\tBE\t\t\t\t\t52.53\t13.38\t"]);
    assert_eq!(index.by_code("US", "90210"), None);
    assert_eq!(
        index.by_code("DE", "10115").map(|p| p.name),
        Some("Berlin".into())
    );
    std::fs::remove_file(&path).expect("removed");
    assert!(!index.available());
    assert_eq!(index.by_code("DE", "10115"), None);
}

#[test]
fn the_data_is_available_only_when_it_can_be_read() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let index = PlaceIndex::new(dir.path().join("places.bin"));
    assert!(!index.available());
    fixture_file(dir.path());
    assert!(index.available());
    std::fs::write(
        dir.path().join("places.bin"),
        b"not a places file at all, just text",
    )
    .expect("written");
    assert!(!index.available());
}
