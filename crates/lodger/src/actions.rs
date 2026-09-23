//! `POST /api/vms/{id}/actions/{action}`: start, shut down, and force off
//! (PRD F4, TAD 4.3 and 9.5).
//!
//! The answer is 204 once libvirt accepted the call. The new state reaches
//! the UI through the lifecycle event on `/ws/events`, so the UI never
//! guesses it. Force off loses unsaved data in the guest, so its body must
//! repeat the VM's name: `{"confirm": "<name>"}`. The UI asks the user to
//! type it, and the server checks it again. Every call that reaches libvirt
//! writes a `vm.lifecycle` audit row.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use lodger_virt::Power;
use serde::Deserialize;
use uuid::Uuid;

use crate::audit::{self, Entry};
use crate::db::Session;
use crate::server::AppState;

const LIFECYCLE: &str = "vm.lifecycle";

fn error(code: StatusCode, message: impl Into<String>) -> Response {
    (code, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

/// The optional JSON body. Only force off reads it.
#[derive(Debug, Default, Deserialize)]
pub struct Confirm {
    /// The VM's name, typed by the user.
    pub confirm: Option<String>,
}

/// `POST /api/vms/{id}/actions/{action}`.
pub async fn run(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    Path((id, action)): Path<(Uuid, String)>,
    body: Bytes,
) -> Response {
    let Some(action) = Power::parse(&action) else {
        return error(StatusCode::NOT_FOUND, "no such action");
    };
    let confirm = if body.is_empty() {
        Confirm::default()
    } else {
        match serde_json::from_slice::<Confirm>(&body) {
            Ok(confirm) => confirm,
            Err(e) => return error(StatusCode::BAD_REQUEST, format!("bad JSON body: {e}")),
        }
    };
    let Some(vm) = state.host.inventory().vms.remove(&id) else {
        return error(StatusCode::NOT_FOUND, "no such VM");
    };
    if action == Power::ForceOff && confirm.confirm.as_deref() != Some(vm.name.as_str()) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "type the VM's name to force it off",
        );
    }
    let Some(virt) = state.host.virt() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Lodger is not connected to libvirt",
        );
    };

    let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);
    let by = |entry: Entry| {
        let mut entry = entry
            .account(&session.username)
            .client_ip(ip)
            .target_vm(&vm.name);
        entry.detail.action = Some(action.name());
        entry
    };
    match virt.power(id, action).await {
        Ok(()) => {
            audit::log(&state.db, by(Entry::ok(LIFECYCLE))).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) if e.is_not_found() => {
            audit::log(&state.db, by(Entry::failed(LIFECYCLE, "no_such_vm"))).await;
            error(StatusCode::NOT_FOUND, "no such VM")
        }
        Err(e) if e.is_invalid_operation() => {
            audit::log(&state.db, by(Entry::failed(LIFECYCLE, "wrong_state"))).await;
            error(StatusCode::CONFLICT, e.to_string())
        }
        Err(e) => {
            eprintln!("lodger: {} {}: {e}", action.name(), vm.name);
            audit::log(&state.db, by(Entry::failed(LIFECYCLE, "libvirt_error"))).await;
            error(StatusCode::BAD_GATEWAY, e.to_string())
        }
    }
}
