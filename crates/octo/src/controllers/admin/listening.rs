//! The dashboard's Listening page: everyone's plays, or one person's, and clearing them.

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::json;
use tracing::info;

use super::plumbing::{error_json, ok, query_value};
use crate::app::AppState;
use crate::middleware::admin_sign_in_gate::{Caller, caller_of};
use crate::services::admin::{AdminCaller, AdminCallerKind};
use crate::services::listening::{self, rollups};

/// The words a caller who may not see listening is answered with.
pub const SIGN_IN_FOR_LISTENING: &str = "Sign in with your Navidrome admin account to see listening.";
/// The words the dashboard is answered with while its sign-in is off.
pub const SIGN_IN_OFF_HIDES_LISTENING: &str = "Listening is hidden while the dashboard sign-in is off, since anyone who can open this dashboard could see it.";

/// Whose listening a caller sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Everyone's, with the people grid.
    Everyone,
    /// Only this listener's own.
    Only(String),
}

/// Whose listening a caller of the admin API may see, or the words they are refused with:
/// the dashboard and a Navidrome admin's app see everyone, a listener's app sees itself, and
/// nobody else sees any.
pub fn scope_of(caller: Option<&AdminCaller>) -> Result<Scope, &'static str> {
    let Some(caller) = caller else {
        return Err(SIGN_IN_FOR_LISTENING);
    };
    match caller.kind {
        AdminCallerKind::Dashboard | AdminCallerKind::NavidromeAdmin => Ok(Scope::Everyone),
        AdminCallerKind::Listener => caller
            .user
            .clone()
            .filter(|user| !user.trim().is_empty())
            .map(Scope::Only)
            .ok_or(SIGN_IN_FOR_LISTENING),
        AdminCallerKind::SignInOff => Err(SIGN_IN_OFF_HIDES_LISTENING),
        AdminCallerKind::Recovery | AdminCallerKind::FamilyMember | AdminCallerKind::FamilyManager => {
            Err(SIGN_IN_FOR_LISTENING)
        }
    }
}

/// Whether a caller may clear listening history: the dashboard or a Navidrome admin.
pub fn may_clear(caller: Option<&AdminCaller>) -> bool {
    caller.is_some_and(|caller| {
        matches!(
            caller.kind,
            AdminCallerKind::Dashboard | AdminCallerKind::NavidromeAdmin
        )
    })
}

/// `GET /api/admin/listening?range=7|30|90|365[&user=]`: everyone's listening with the people
/// grid, or one person's. The Octo app with a listener's own sign-in sees that listener only.
pub async fn get_listening(
    State(state): State<AppState>,
    caller: Caller,
    RawQuery(query): RawQuery,
) -> Response {
    let scope = match scope_of(caller_of(&caller)) {
        Ok(scope) => scope,
        Err(words) => return error_json(StatusCode::FORBIDDEN, words),
    };
    let keep_days = state.settings.current().listening.effective_keep_days();
    let days = rollups::range_of(query_value(query.as_deref(), "range").as_deref(), keep_days);
    let (user, everyone) = match scope {
        Scope::Everyone => (query_value(query.as_deref(), "user"), true),
        Scope::Only(user) => (Some(user), false),
    };
    let log = state.listening.clone();
    let answer = tokio::task::spawn_blocking(move || {
        let mut page = listening::page(&log, days, user.as_deref(), everyone);
        page["everyone"] = json!(everyone);
        page["keepDays"] = json!(keep_days);
        page["ranges"] = json!(rollups::ranges(keep_days));
        page
    })
    .await;
    match answer {
        Ok(page) => ok(&page),
        Err(_) => error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The listening log could not be read.",
        ),
    }
}

/// `POST /api/admin/listening/clear[?user=]`: removes one person's plays, or everyone's.
pub async fn clear_listening(
    State(state): State<AppState>,
    caller: Caller,
    RawQuery(query): RawQuery,
) -> Response {
    if !may_clear(caller_of(&caller)) {
        return error_json(StatusCode::FORBIDDEN, SIGN_IN_FOR_LISTENING);
    }
    let user = query_value(query.as_deref(), "user");
    let who = AdminCaller::describe(caller_of(&caller));
    let log = state.listening.clone();
    let target = user.clone();
    let removed = tokio::task::spawn_blocking(move || log.clear(target.as_deref()))
        .await
        .unwrap_or(0);
    match user.as_deref() {
        Some(user) => info!("Listening: {who} cleared {user}'s history ({removed} plays)"),
        None => info!("Listening: {who} cleared everyone's history ({removed} plays)"),
    }
    ok(&json!({ "cleared": removed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(kind: AdminCallerKind, user: Option<&str>) -> AdminCaller {
        AdminCaller::new(user.map(str::to_string), kind)
    }

    #[test]
    fn each_kind_of_caller_sees_what_it_may() {
        use AdminCallerKind::*;
        assert_eq!(
            scope_of(Some(&caller(Dashboard, Some("admin")))),
            Ok(Scope::Everyone)
        );
        assert_eq!(
            scope_of(Some(&caller(NavidromeAdmin, Some("admin")))),
            Ok(Scope::Everyone)
        );
        assert_eq!(
            scope_of(Some(&caller(Listener, Some("alex")))),
            Ok(Scope::Only("alex".into()))
        );
        assert_eq!(
            scope_of(Some(&caller(Listener, None))),
            Err(SIGN_IN_FOR_LISTENING)
        );
        assert_eq!(
            scope_of(Some(&caller(SignInOff, None))),
            Err(SIGN_IN_OFF_HIDES_LISTENING)
        );
        for kind in [Recovery, FamilyMember, FamilyManager] {
            assert_eq!(
                scope_of(Some(&caller(kind, Some("x")))),
                Err(SIGN_IN_FOR_LISTENING),
                "{kind:?}"
            );
        }
        assert_eq!(scope_of(None), Err(SIGN_IN_FOR_LISTENING));
    }

    #[test]
    fn only_the_dashboard_and_an_admin_app_clear() {
        use AdminCallerKind::*;
        assert!(may_clear(Some(&caller(Dashboard, Some("admin")))));
        assert!(may_clear(Some(&caller(NavidromeAdmin, Some("admin")))));
        for kind in [Listener, SignInOff, Recovery, FamilyMember, FamilyManager] {
            assert!(!may_clear(Some(&caller(kind, Some("x")))), "{kind:?}");
        }
        assert!(!may_clear(None));
    }
}
