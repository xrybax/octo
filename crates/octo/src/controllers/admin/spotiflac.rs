//! `POST /api/admin/spotiflac/test`: the SpotiFLAC page's Test. It checks what the page holds,
//! saved or not: that SpotiFLAC answers at the address with the token, that it has an extension
//! to download with, and that Octo can make the folder downloads land in. Nothing is downloaded.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;

use super::plumbing::{SECRET_PLACEHOLDER, error_json, ok};
use crate::app::AppState;
use crate::services::spotiflac::Connection;

/// What the page sends: its fields as they are.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SpotiflacTestRequest {
    pub url: Option<String>,
    pub token: Option<String>,
    pub folder: Option<String>,
    pub enabled: Option<bool>,
}

/// The token the Test sends: the page only ever holds the placeholder for a saved token, and
/// testing it means the saved one, but only at the saved address, so a saved token is never sent
/// to an address someone typed. Anything other than the placeholder is the token as typed.
fn test_token(
    typed: Option<&str>,
    typed_url: &str,
    saved: &str,
    saved_url: &str,
) -> Result<String, &'static str> {
    match typed {
        Some(token) if token == SECRET_PLACEHOLDER => {
            if typed_url.eq_ignore_ascii_case(saved_url) {
                Ok(saved.trim().to_string())
            } else {
                Err("Retype the SpotiFLAC web token for the new address.")
            }
        }
        Some(token) => Ok(token.trim().to_string()),
        None => Ok(String::new()),
    }
}

pub async fn test_spotiflac(State(state): State<AppState>, body: Bytes) -> Response {
    let request: SpotiflacTestRequest = if body.is_empty() {
        SpotiflacTestRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(request) => request,
            Err(_) => {
                return error_json(
                    StatusCode::BAD_REQUEST,
                    "The Test needs the page's fields as JSON.",
                );
            }
        }
    };
    let saved = state.settings.current().spotiflac.clone();
    let url = octo_core::settings::normalize_server_url(request.url.as_deref().unwrap_or(""));
    let token = match test_token(
        request.token.as_deref(),
        &url,
        &saved.token,
        &saved.effective_url(),
    ) {
        Ok(token) => token,
        Err(words) => {
            return ok(&serde_json::json!({
                "ok": false,
                "message": words,
                "checks": [{ "state": "fail", "text": words }],
            }));
        }
    };
    let folder = octo_core::settings::SpotiflacSettings {
        folder: request.folder.unwrap_or_default(),
        ..Default::default()
    }
    .effective_folder();
    let fallback = state
        .settings
        .raw("Library:DownloadPath")
        .unwrap_or_else(|| "/music".to_string());
    let download_path = state.navidrome_identity.effective_download_path(&fallback);
    let report = state
        .spotiflac
        .test(
            &Connection { url, token },
            request.enabled.unwrap_or(saved.enabled),
            folder.as_deref(),
            &download_path,
        )
        .await;
    ok(&report.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_token_goes_only_to_the_saved_address() {
        assert_eq!(
            test_token(
                Some(SECRET_PLACEHOLDER),
                "http://spotiflac:8000",
                "saved",
                "http://spotiflac:8000"
            ),
            Ok("saved".to_string())
        );
        assert_eq!(
            test_token(
                Some(SECRET_PLACEHOLDER),
                "http://evil.example",
                "saved",
                "http://spotiflac:8000"
            ),
            Err("Retype the SpotiFLAC web token for the new address.")
        );
        assert_eq!(
            test_token(
                Some(" typed "),
                "http://evil.example",
                "saved",
                "http://spotiflac:8000"
            ),
            Ok("typed".to_string())
        );
        assert_eq!(
            test_token(None, "http://x", "saved", "http://x"),
            Ok(String::new())
        );
    }
}
