//! Deezer's pictures, without its stand-in for a missing one. A Deezer artist or album with no
//! picture of its own still names one: the same grey silhouette (or blank cover) for everyone,
//! at an address whose hash segment is empty (`.../images/artist//1000x1000-000000-80-0-0.jpg`)
//! or, on older answers, the hash of an empty file. Such a picture says nothing about the artist,
//! so it reads as no picture, and whoever shows the artist draws its own empty tile instead.

use serde_json::Value;

use crate::json::element::{ElementResult, str_prop};

/// The hash Deezer gives an image it has no file for: the MD5 of nothing.
const EMPTY_HASH: &str = "d41d8cd98f00b204e9800998ecf8427e";

/// Whether `url` is Deezer's stand-in for a missing picture: an `images/<kind>/` address whose
/// hash segment is empty or the empty file's hash. Any other address is a real picture.
pub fn is_stock_deezer_image(url: &str) -> bool {
    let Some(at) = url.find("/images/") else {
        return false;
    };
    let mut parts = url[at + "/images/".len()..].split('/');
    let (Some(_kind), Some(hash)) = (parts.next(), parts.next()) else {
        return false;
    };
    hash.is_empty() || hash.eq_ignore_ascii_case(EMPTY_HASH)
}

/// The first of `names` on `element` that holds a picture: a non-empty address. `None` when none
/// does, and when any of them is Deezer's stand-in: every size of a picture is the same picture,
/// and an address with no hash in it (`api.deezer.com/artist/<id>/image`) leads to the stand-in
/// too. Fails only when the element is not an object.
pub fn deezer_image<'a>(element: &'a Value, names: &[&str]) -> ElementResult<Option<&'a str>> {
    let mut found = None;
    for name in names {
        if let Some(url) = str_prop(element, name)?
            && !url.trim().is_empty()
        {
            if is_stock_deezer_image(url) {
                return Ok(None);
            }
            found = found.or(Some(url));
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_hash_is_the_stand_in() {
        assert!(is_stock_deezer_image(
            "https://cdn-images.dzcdn.net/images/artist//1000x1000-000000-80-0-0.jpg"
        ));
        assert!(is_stock_deezer_image(
            "https://e-cdns-images.dzcdn.net/images/cover//500x500-000000-80-0-0.jpg"
        ));
        assert!(is_stock_deezer_image(
            "https://cdn-images.dzcdn.net/images/artist/d41d8cd98f00b204e9800998ecf8427e/1000x1000-000000-80-0-0.jpg"
        ));
    }

    #[test]
    fn a_real_hash_is_a_picture() {
        assert!(!is_stock_deezer_image(
            "https://cdn-images.dzcdn.net/images/artist/4886905210739af3438990897bad3a98/1000x1000-000000-80-0-0.jpg"
        ));
        assert!(!is_stock_deezer_image("https://cdn/orchestra.jpg"));
        assert!(!is_stock_deezer_image("https://api.deezer.com/artist/92/image"));
    }

    #[test]
    fn a_stand_in_in_any_size_leaves_no_picture() {
        let stand_in = json!({
            "picture_xl": "https://cdn-images.dzcdn.net/images/artist//1000x1000-000000-80-0-0.jpg",
            "picture": "https://api.deezer.com/artist/5181383/image",
        });
        assert_eq!(
            deezer_image(&stand_in, &["picture_xl", "picture_big", "picture"]).unwrap(),
            None
        );
    }

    #[test]
    fn the_first_real_picture_wins() {
        let stand_in = json!({
            "picture_xl": "https://cdn-images.dzcdn.net/images/artist//1000x1000-000000-80-0-0.jpg",
            "picture_medium": "https://cdn-images.dzcdn.net/images/artist//250x250-000000-80-0-0.jpg",
        });
        assert_eq!(
            deezer_image(&stand_in, &["picture_xl", "picture_medium"]).unwrap(),
            None
        );
        let real = json!({
            "picture_xl": "",
            "picture_big": "https://cdn-images.dzcdn.net/images/artist/4886905210739af3438990897bad3a98/500x500-000000-80-0-0.jpg",
        });
        assert_eq!(
            deezer_image(&real, &["picture_xl", "picture_big"]).unwrap(),
            Some(
                "https://cdn-images.dzcdn.net/images/artist/4886905210739af3438990897bad3a98/500x500-000000-80-0-0.jpg"
            )
        );
    }
}
