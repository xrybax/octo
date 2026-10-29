//! getPlace (octoPlaces): a postal code or a city as coordinates, in XML or JSON.

use octo_core::places::Place;
use serde_json::{Map, Value, json};

use super::{SUBSONIC_NAMESPACE, SUBSONIC_VERSION, SubsonicReply, SubsonicResponseBuilder, envelope};

/// getPlace: the coordinates of a postal code or a city, for the apps' "Live radio near you"
/// on a device that has no location service.
pub const PLACES_EXTENSION: &str = "octoPlaces";
pub const PLACES_EXTENSION_VERSION: i32 = 1;

impl SubsonicResponseBuilder {
    /// `place`: code, name, state (left out when the data has none), country, latitude and
    /// longitude, in the format asked for.
    pub fn create_place_response(&self, format: &str, place: &Place) -> SubsonicReply {
        let mut fields = Map::new();
        fields.insert("code".into(), json!(place.code));
        fields.insert("name".into(), json!(place.name));
        if !place.state.is_empty() {
            fields.insert("state".into(), json!(place.state));
        }
        fields.insert("country".into(), json!(place.country));
        fields.insert("latitude".into(), json!(place.latitude));
        fields.insert("longitude".into(), json!(place.longitude));
        let place = Value::Object(fields);
        if format == "json" {
            return self.create_json_response(json!({
                "status": "ok",
                "version": SUBSONIC_VERSION,
                "type": "octo",
                "openSubsonic": true,
                "place": place,
            }));
        }
        SubsonicReply::xml(
            &envelope("ok")
                .attr("type", "octo")
                .attr("openSubsonic", "true")
                .child(super::json_shape_to_xml(
                    Some(SUBSONIC_NAMESPACE),
                    "place",
                    &place,
                )),
        )
    }
}
