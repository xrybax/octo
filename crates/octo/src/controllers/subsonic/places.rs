//! getPlace (octoPlaces): a postal code or a city as coordinates, for an app on a device with
//! no location service (the desktop app), and coordinates as the nearest place, for the apps'
//! "Live radio near you". Answered from the place data in the image (GeoNames), read a block at
//! a time; nothing is asked of any outside service.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::response::IntoResponse;
use axum::routing::get;
use tracing::error;

use super::caller_checks::{SubsonicCall, refuse_unless_signed_in};
use crate::app::AppState;
use crate::http::error::AppResult;
use crate::http::routes::RouteSet;

/// Overrides where the place data is read from.
pub const PLACES_FILE_ENV: &str = "OCTO_PLACES_FILE";

/// The place data's name beside the executable (the image has it at `/app/data/places.bin`).
const PLACES_FILE: &str = "data/places.bin";

/// The words of error 70.
const UNKNOWN_PLACE: &str = "Octo does not know that place.";

/// The words of error 10 for a code or a name without its country.
const MISSING_COUNTRY: &str = "Required parameter is missing: country";

pub fn routes() -> RouteSet {
    RouteSet::new().subsonic("getPlace", get(get_place).post(get_place))
}

/// Where the place data is: `OCTO_PLACES_FILE` when set; else `data/places.bin` beside the
/// executable when it is there (the image); else `places.bin` in the config folder.
pub fn places_file(config_dir: &Path) -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    resolve_places_file(
        std::env::var(PLACES_FILE_ENV).ok(),
        exe_dir.as_deref(),
        config_dir,
    )
}

/// [`places_file`] over explicit inputs.
pub fn resolve_places_file(env: Option<String>, exe_dir: Option<&Path>, config_dir: &Path) -> PathBuf {
    if let Some(path) = env.filter(|v| !v.trim().is_empty()) {
        return PathBuf::from(path);
    }
    exe_dir
        .map(|dir| dir.join(PLACES_FILE))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| config_dir.join("places.bin"))
}

/// `code` with `country`, or `name` with `country` and maybe `state`, or `latitude` and
/// `longitude` (decimal degrees, the nearest place within 30 km), in that order when several are
/// sent: the place, error 70 when Octo does not know it, error 10 without what it needs or with
/// coordinates that are not decimal degrees. The caller must be signed in (a family member too);
/// XML or JSON, as asked.
pub async fn get_place(State(state): State<AppState>, req: Request) -> AppResult {
    let call = match SubsonicCall::read(&state, req).await {
        Ok(call) => call,
        Err(response) => return Ok(response),
    };
    let format = call.format();
    if let Some(refused) = refuse_unless_signed_in(&state, &call, &format).await? {
        return Ok(refused);
    }
    let builder = &state.subsonic_response_builder;
    let given = |key: &str| {
        call.param_opt(key)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let refuse = |message: &str| Ok(builder.create_error(&format, 10, message).into_response());
    let (code, name, country) = (given("code"), given("name"), given("country"));
    let (latitude, longitude) = (given("latitude"), given("longitude"));
    let ask = if let Some(code) = code {
        let Some(country) = country else {
            return refuse(MISSING_COUNTRY);
        };
        Ask::Code { country, code }
    } else if let Some(name) = name {
        let Some(country) = country else {
            return refuse(MISSING_COUNTRY);
        };
        Ask::Name {
            country,
            name,
            state: given("state"),
        }
    } else if latitude.is_some() || longitude.is_some() {
        let (Some(latitude), Some(longitude)) = (
            latitude.as_deref().and_then(|v| degrees(v, 90.0)),
            longitude.as_deref().and_then(|v| degrees(v, 180.0)),
        ) else {
            return refuse("Required parameter is missing: latitude and longitude, in decimal degrees");
        };
        Ask::Near { latitude, longitude }
    } else {
        return refuse("Required parameter is missing: code, name, or latitude and longitude");
    };

    let places = Arc::clone(&state.places);
    let found = tokio::task::spawn_blocking(move || match ask {
        Ask::Code { country, code } => places.by_code(&country, &code),
        Ask::Name { country, name, state } => places.by_name(&country, &name, state.as_deref()),
        Ask::Near { latitude, longitude } => places.nearest(latitude, longitude),
    })
    .await
    .unwrap_or_else(|e| {
        error!("getPlace: the place lookup did not finish ({e}), so it answers error 70");
        None
    });
    Ok(match found {
        Some(place) => builder.create_place_response(&format, &place).into_response(),
        None => builder.create_error(&format, 70, UNKNOWN_PLACE).into_response(),
    })
}

/// What the caller asked for.
enum Ask {
    Code {
        country: String,
        code: String,
    },
    Name {
        country: String,
        name: String,
        state: Option<String>,
    },
    Near {
        latitude: f64,
        longitude: f64,
    },
}

/// A coordinate in decimal degrees, at most `limit` either side of zero, or `None`.
fn degrees(value: &str, limit: f64) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|degrees| degrees.is_finite() && degrees.abs() <= limit)
}
