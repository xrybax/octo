//! getPlace and the octoPlaces extension, as the apps see them: a postal code in any spelling,
//! a city with and without its state, the nearest place to a coordinate, a place Octo does not
//! know, a request missing what it needs, both formats, a server without place data, and a
//! caller who is not signed in.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::Method;
use octo_core::places::{PlaceIndex, convert};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request as MockRequest};

use super::places::resolve_places_file;
use super::test_app::*;
use crate::app::AppState;
use crate::http::pipeline::App;

const ALICE: &str = "u=alice&t=good&s=salt&v=1.16.1&c=test";
const WRONG: &str = "u=alice&t=bad&s=salt&v=1.16.1&c=test";

/// GeoNames rows, as `allCountries.txt` has them (country, postal code, place, admin name 1,
/// admin code 1, admin name 2, admin code 2, admin name 3, admin code 3, latitude, longitude,
/// accuracy).
const ROWS: &[&str] = &[
    "US\t90210\tBeverly Hills\tCalifornia\tCA\tLos Angeles\t037\t\t\t34.0901\t-118.4065\t4",
    "US\t02134\tAllston\tMassachusetts\tMA\tSuffolk\t025\t\t\t42.3539\t-71.1337\t4",
    "US\t62701\tSpringfield\tIllinois\tIL\tSangamon\t167\t\t\t39.8017\t-89.6436\t4",
    "US\t62702\tSpringfield\tIllinois\tIL\tSangamon\t167\t\t\t39.8201\t-89.6437\t4",
    "US\t01101\tSpringfield\tMassachusetts\tMA\tHampden\t013\t\t\t42.1015\t-72.5898\t4",
    "GB\tSW1A\tLondon\tEngland\tENG\tGreater London\t11609024\t\t\t51.5\t-0.1397\t",
    "GB\tE16\tCanning Town\tEngland\tENG\t\t\t\t\t51.51\t0.01\t",
    "JE\tJE2\tSt Helier\t\t\t\t\t\t\t49.1858\t-2.11\t",
];

/// The rows above converted into `<dir>/places.bin` by the converter the image build runs.
pub(crate) fn write_fixture(dir: &Path) -> PathBuf {
    let path = dir.join("places.bin");
    let file = std::fs::File::create(&path).expect("a places file");
    convert(ROWS.join("\n").as_bytes(), std::io::BufWriter::new(file)).expect("converted");
    path
}

/// Navidrome accepts the token "good" and lists no extensions of its own.
async fn navidrome() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(|request: &MockRequest| {
            if query_of(request).get("t").map(String::as_str) != Some("good") {
                return navidrome_json(NAVIDROME_WRONG_PASSWORD);
            }
            match request.url.path() {
                "/rest/getOpenSubsonicExtensions" | "/rest/getOpenSubsonicExtensions.view" => navidrome_json(
                    r#"{"subsonic-response":{"status":"ok","version":"1.16.1","type":"navidrome","openSubsonic":true,"openSubsonicExtensions":[]}}"#,
                ),
                _ => navidrome_json(NAVIDROME_OK),
            }
        })
        .mount(&server)
        .await;
    server
}

struct Harness {
    app: App,
    _navidrome: MockServer,
    _dir: tempfile::TempDir,
}

/// The app over the fixture's place data, or over none.
async fn harness(with_data: bool) -> Harness {
    let navidrome = navidrome().await;
    let dir = tempfile::tempdir().expect("a temp dir");
    let state: AppState = if with_data {
        let path = write_fixture(dir.path());
        state_with(settings(&navidrome.uri()), |inner| {
            inner.places = Arc::new(PlaceIndex::new(path));
        })
    } else {
        state_with(settings(&navidrome.uri()), |_| {})
    };
    Harness {
        app: app(&state),
        _navidrome: navidrome,
        _dir: dir,
    }
}

async fn ask(h: &Harness, query: &str) -> Value {
    get(&h.app, &format!("/rest/{query}")).await.envelope()
}

fn error_code(answer: &Value) -> Value {
    assert_eq!(answer["status"], "failed", "{answer}");
    answer["error"]["code"].clone()
}

#[tokio::test]
async fn a_postal_code_comes_back_with_its_place_in_json() {
    let h = harness(true).await;
    let reply = get(
        &h.app,
        &format!("/rest/getPlace?{ALICE}&f=json&code=90210&country=US"),
    )
    .await;
    assert_eq!(reply.media_type().as_deref(), Some("application/json"));
    assert_eq!(
        reply.json(),
        json!({"subsonic-response": {
            "status": "ok",
            "version": "1.16.1",
            "type": "octo",
            "openSubsonic": true,
            "place": {
                "code": "90210",
                "name": "Beverly Hills",
                "state": "California",
                "country": "US",
                "latitude": 34.09,
                "longitude": -118.41
            }
        }})
    );
}

#[tokio::test]
async fn a_postal_code_comes_back_in_xml_by_default() {
    let h = harness(true).await;
    let reply = get(
        &h.app,
        &format!("/rest/getPlace.view?{ALICE}&code=02134&country=us"),
    )
    .await;
    assert!(
        reply.media_type().is_some_and(|t| t.contains("xml")),
        "{:?}",
        reply.media_type()
    );
    assert_eq!(
        reply.text(),
        concat!(
            "<subsonic-response status=\"ok\" version=\"1.16.1\" type=\"octo\" openSubsonic=\"true\" xmlns=\"http://subsonic.org/restapi\">\n",
            "  <place code=\"02134\" name=\"Allston\" state=\"Massachusetts\" country=\"US\" latitude=\"42.35\" longitude=\"-71.13\" />\n",
            "</subsonic-response>"
        )
    );
}

#[tokio::test]
async fn a_place_without_a_state_leaves_it_out() {
    let h = harness(true).await;
    let answer = ask(&h, &format!("getPlace?{ALICE}&f=json&code=je2%204ab&country=JE")).await;
    assert_eq!(
        answer["place"],
        json!({"code": "JE2", "name": "St Helier", "country": "JE", "latitude": 49.19, "longitude": -2.11})
    );
}

#[tokio::test]
async fn a_uk_postcode_is_found_however_it_is_written() {
    let h = harness(true).await;
    for code in ["sw1a%201aa", "SW1A1AA", "SW1A+1AA", "sw1a"] {
        let answer = ask(&h, &format!("getPlace?{ALICE}&f=json&code={code}&country=gb")).await;
        assert_eq!(answer["status"], "ok", "{code}: {answer}");
        assert_eq!(answer["place"]["code"], "SW1A", "{code}");
        assert_eq!(answer["place"]["name"], "London", "{code}");
    }
}

#[tokio::test]
async fn a_leading_zero_is_part_of_the_code() {
    let h = harness(true).await;
    let answer = ask(&h, &format!("getPlace?{ALICE}&f=json&code=2134&country=US")).await;
    assert_eq!(error_code(&answer), 70);
}

#[tokio::test]
async fn a_city_with_and_without_its_state() {
    let h = harness(true).await;
    let biggest = ask(
        &h,
        &format!("getPlace?{ALICE}&f=json&name=springfield&country=US"),
    )
    .await;
    assert_eq!(biggest["place"]["state"], "Illinois", "{biggest}");
    assert_eq!(biggest["place"]["code"], "62701");
    assert_eq!(biggest["place"]["latitude"], 39.8);

    for state in ["MA", "Massachusetts"] {
        let answer = ask(
            &h,
            &format!("getPlace?{ALICE}&f=json&name=Springfield&state={state}&country=US"),
        )
        .await;
        assert_eq!(
            answer["place"],
            json!({"code": "01101", "name": "Springfield", "state": "Massachusetts", "country": "US", "latitude": 42.1, "longitude": -72.59}),
            "{state}"
        );
    }
    let elsewhere = ask(
        &h,
        &format!("getPlace?{ALICE}&f=json&name=Springfield&state=TX&country=US"),
    )
    .await;
    assert_eq!(error_code(&elsewhere), 70);
}

#[tokio::test]
async fn coordinates_find_the_nearest_of_several_places() {
    let h = harness(true).await;
    let answer = ask(
        &h,
        &format!("getPlace?{ALICE}&f=json&latitude=39.819&longitude=-89.6431"),
    )
    .await;
    assert_eq!(
        answer["place"],
        json!({"code": "62702", "name": "Springfield", "state": "Illinois", "country": "US", "latitude": 39.82, "longitude": -89.64}),
        "{answer}"
    );
    let answer = ask(
        &h,
        &format!("getPlace?{ALICE}&f=json&latitude=39.79&longitude=-89.65"),
    )
    .await;
    assert_eq!(answer["place"]["code"], "62701", "{answer}");
    let xml = get(
        &h.app,
        &format!("/rest/getPlace?{ALICE}&latitude=34.1&longitude=-118.4"),
    )
    .await;
    assert!(
        xml.text()
            .contains("<place code=\"90210\" name=\"Beverly Hills\""),
        "{}",
        xml.text()
    );
}

#[tokio::test]
async fn coordinates_with_nothing_within_30_km_are_error_70() {
    let h = harness(true).await;
    for (latitude, longitude) in [("0", "0"), ("39.8", "-89.0"), ("-89.99", "0")] {
        let answer = ask(
            &h,
            &format!("getPlace?{ALICE}&f=json&latitude={latitude}&longitude={longitude}"),
        )
        .await;
        assert_eq!(error_code(&answer), 70, "{latitude},{longitude}");
        assert_eq!(answer["error"]["message"], "Octo does not know that place.");
    }
}

#[tokio::test]
async fn a_point_near_a_cell_edge_finds_a_place_in_the_neighboring_cell() {
    let h = harness(true).await;
    let answer = ask(
        &h,
        &format!("getPlace?{ALICE}&f=json&latitude=51.49&longitude=-0.01"),
    )
    .await;
    assert_eq!(answer["place"]["name"], "Canning Town", "{answer}");
    assert_eq!(answer["place"]["code"], "E16");
}

#[tokio::test]
async fn coordinates_that_are_not_decimal_degrees_are_error_10() {
    let h = harness(true).await;
    for query in [
        "&latitude=abc&longitude=0",
        "&latitude=39.8&longitude=north",
        "&latitude=91&longitude=0",
        "&latitude=0&longitude=-180.5",
        "&latitude=NaN&longitude=0",
        "&latitude=inf&longitude=0",
        "&latitude=39.8",
        "&longitude=-89.64",
        "&latitude=39%2C8&longitude=-89.64",
    ] {
        let answer = ask(&h, &format!("getPlace?{ALICE}&f=json{query}")).await;
        assert_eq!(error_code(&answer), 10, "{query}");
        assert_eq!(
            answer["error"]["message"],
            "Required parameter is missing: latitude and longitude, in decimal degrees",
            "{query}"
        );
    }
}

#[tokio::test]
async fn a_code_comes_before_a_name_and_a_name_before_coordinates() {
    let h = harness(true).await;
    let near_london = "latitude=51.49&longitude=-0.01";
    let answer = ask(
        &h,
        &format!("getPlace?{ALICE}&f=json&code=90210&name=Springfield&country=US&{near_london}"),
    )
    .await;
    assert_eq!(answer["place"]["code"], "90210", "{answer}");
    let answer = ask(
        &h,
        &format!("getPlace?{ALICE}&f=json&name=Springfield&country=US&{near_london}"),
    )
    .await;
    assert_eq!(answer["place"]["code"], "62701", "{answer}");
    // A code without its country is not passed over for the coordinates.
    let answer = ask(&h, &format!("getPlace?{ALICE}&f=json&code=90210&{near_london}")).await;
    assert_eq!(error_code(&answer), 10);
    assert_eq!(
        answer["error"]["message"],
        "Required parameter is missing: country"
    );
}

#[tokio::test]
async fn a_form_post_is_read_like_a_query() {
    let h = harness(true).await;
    let reply = send(
        &h.app,
        Method::POST,
        "/rest/getPlace",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        format!("{ALICE}&f=json&name=Beverly%20Hills&country=US"),
    )
    .await;
    assert_eq!(reply.envelope()["place"]["code"], "90210", "{}", reply.text());
}

#[tokio::test]
async fn an_unknown_place_is_error_70_in_both_formats() {
    let h = harness(true).await;
    let answer = ask(&h, &format!("getPlace?{ALICE}&f=json&code=99999&country=US")).await;
    assert_eq!(error_code(&answer), 70);
    assert_eq!(answer["error"]["message"], "Octo does not know that place.");
    let answer = ask(&h, &format!("getPlace?{ALICE}&f=json&name=Atlantis&country=US")).await;
    assert_eq!(error_code(&answer), 70);
    let answer = ask(&h, &format!("getPlace?{ALICE}&f=json&code=90210&country=USA")).await;
    assert_eq!(error_code(&answer), 70);

    let xml = get(&h.app, &format!("/rest/getPlace?{ALICE}&code=99999&country=US")).await;
    assert_eq!(
        xml.text(),
        concat!(
            "<subsonic-response status=\"failed\" version=\"1.16.1\" xmlns=\"http://subsonic.org/restapi\">\n",
            "  <error code=\"70\" message=\"Octo does not know that place.\" />\n",
            "</subsonic-response>"
        )
    );
}

#[tokio::test]
async fn a_request_missing_what_it_needs_is_error_10() {
    let h = harness(true).await;
    for (query, missing) in [
        ("", "code, name, or latitude and longitude"),
        ("&code=%20&name=", "code, name, or latitude and longitude"),
        ("&country=US", "code, name, or latitude and longitude"),
        ("&code=90210", "country"),
        ("&name=Springfield&state=IL", "country"),
    ] {
        let answer = ask(&h, &format!("getPlace?{ALICE}&f=json{query}")).await;
        assert_eq!(error_code(&answer), 10, "{query}");
        assert_eq!(
            answer["error"]["message"],
            format!("Required parameter is missing: {missing}"),
            "{query}"
        );
    }
}

#[tokio::test]
async fn without_place_data_every_place_is_unknown() {
    let h = harness(false).await;
    let answer = ask(
        &h,
        &format!("getPlace?{ALICE}&f=json&latitude=39.8&longitude=-89.64"),
    )
    .await;
    assert_eq!(error_code(&answer), 70);
    let answer = ask(&h, &format!("getPlace?{ALICE}&f=json&code=90210&country=US")).await;
    assert_eq!(error_code(&answer), 70);
    let answer = ask(
        &h,
        &format!("getPlace?{ALICE}&f=json&name=Springfield&country=US"),
    )
    .await;
    assert_eq!(error_code(&answer), 70);
}

#[tokio::test]
async fn a_caller_who_is_not_signed_in_is_refused() {
    let h = harness(true).await;
    let answer = ask(&h, &format!("getPlace?{WRONG}&f=json&code=90210&country=US")).await;
    assert_eq!(error_code(&answer), 40);
    assert!(answer.get("place").is_none());
}

/// The octoPlaces entries of the extension list.
async fn places_listed(h: &Harness) -> Vec<Value> {
    let answer = ask(h, &format!("getOpenSubsonicExtensions?{ALICE}&f=json")).await;
    answer["openSubsonicExtensions"]
        .as_array()
        .expect("a list")
        .iter()
        .filter(|e| e["name"] == "octoPlaces")
        .cloned()
        .collect()
}

#[tokio::test]
async fn the_extension_is_listed_only_with_place_data() {
    let h = harness(true).await;
    assert_eq!(
        places_listed(&h).await,
        vec![json!({"name": "octoPlaces", "versions": [1]})]
    );
    // Without it an app keeps to its own lookup instead of asking for places Octo cannot know.
    let h = harness(false).await;
    assert_eq!(places_listed(&h).await, Vec::<Value>::new());
}

#[tokio::test]
async fn the_extension_is_listed_once_the_place_data_is_there() {
    let navidrome = navidrome().await;
    let dir = tempfile::tempdir().expect("a temp dir");
    let data_dir = dir.path().to_path_buf();
    let path = data_dir.join("places.bin");
    let state = state_with(settings(&navidrome.uri()), |inner| {
        inner.places = Arc::new(PlaceIndex::new(&path));
    });
    let h = Harness {
        app: app(&state),
        _navidrome: navidrome,
        _dir: dir,
    };
    assert!(places_listed(&h).await.is_empty());
    std::fs::write(&path, b"not place data").expect("written");
    assert!(places_listed(&h).await.is_empty());
    write_fixture(&data_dir);
    assert_eq!(places_listed(&h).await.len(), 1);
}

#[test]
fn the_data_file_is_the_override_then_the_image_then_the_config_folder() {
    let image = tempfile::tempdir().expect("a temp dir");
    let config = Path::new("/config");
    assert_eq!(
        resolve_places_file(Some("/elsewhere/places.bin".into()), Some(image.path()), config),
        PathBuf::from("/elsewhere/places.bin")
    );
    assert_eq!(
        resolve_places_file(Some(" ".into()), Some(image.path()), config),
        config.join("places.bin")
    );
    std::fs::create_dir_all(image.path().join("data")).expect("a data folder");
    std::fs::write(image.path().join("data/places.bin"), b"").expect("written");
    assert_eq!(
        resolve_places_file(None, Some(image.path()), config),
        image.path().join("data/places.bin")
    );
    assert_eq!(resolve_places_file(None, None, config), config.join("places.bin"));
}
