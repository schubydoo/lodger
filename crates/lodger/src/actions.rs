//! The VM lifecycle (PRD F4, TAD 4.3 and 9.5):
//!
//! - `POST /api/vms/{id}/actions/{action}`: start, shutdown, force-off,
//!   reboot, pause, and resume.
//! - `DELETE /api/vms/{id}`: removes the VM, and with `remove_volumes` its
//!   volumes that no other VM uses.
//! - `PATCH /api/vms/{id}`: switches autostart with `{"autostart": true}`.
//!
//! An action answers 204 once libvirt accepted the call. The new state
//! reaches the UI through the event on `/ws/events`, so the UI never guesses
//! it. Force off loses unsaved data in the guest, and delete loses the VM,
//! so their bodies must repeat the VM's name: `{"confirm": "<name>"}`. The
//! UI asks the user to type it, and the server checks it again. Every call
//! that reaches libvirt writes an audit row: `vm.lifecycle` for an action or
//! a delete, and `vm.edited` for autostart.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use lodger_virt::{Power, SkipReason};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::{self, Entry};
use crate::db::Session;
use crate::server::AppState;

const LIFECYCLE: &str = "vm.lifecycle";
const EDITED: &str = "vm.edited";

pub(crate) fn error(code: StatusCode, message: impl Into<String>) -> Response {
    (code, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

/// The optional JSON body. Only force off reads it.
#[derive(Debug, Default, Deserialize)]
pub struct Confirm {
    /// The VM's name, typed by the user.
    pub confirm: Option<String>,
}

/// The body of a delete.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteRequest {
    /// The VM's name, typed by the user.
    pub confirm: Option<String>,
    /// Also delete the volumes that no other VM uses.
    #[serde(default)]
    pub remove_volumes: bool,
}

/// The body of `PATCH /api/vms/{id}`. Autostart is the only setting yet.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub autostart: bool,
}

/// The answer to a delete: what happened to each disk.
#[derive(Debug, Serialize)]
pub struct Removal {
    pub removed: Vec<String>,
    pub skipped: Vec<Skipped>,
}

/// A disk that a delete kept.
#[derive(Debug, Serialize)]
pub struct Skipped {
    pub path: String,
    /// `used_by`, `shared`, `not_in_pool`, or `failed`.
    pub reason: &'static str,
    /// For `used_by`: the VM that uses the disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm: Option<String>,
    /// For `failed`: libvirt's message, unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl From<lodger_virt::Removal> for Removal {
    fn from(r: lodger_virt::Removal) -> Self {
        Self {
            removed: r.removed,
            skipped: r
                .skipped
                .into_iter()
                .map(|s| {
                    let (reason, vm, message) = match s.reason {
                        SkipReason::UsedBy(vm) => ("used_by", Some(vm), None),
                        SkipReason::Shared => ("shared", None, None),
                        SkipReason::NotInPool => ("not_in_pool", None, None),
                        SkipReason::Failed(message) => ("failed", None, Some(message)),
                    };
                    Skipped {
                        path: s.path,
                        reason,
                        vm,
                        message,
                    }
                })
                .collect(),
        }
    }
}

/// Parses an optional JSON body. An empty body gives the default. The
/// error is the message for a 400 answer.
fn body_or_default<T: Default + DeserializeOwned>(body: &Bytes) -> Result<T, String> {
    if body.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| format!("bad JSON body: {e}"))
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
    let confirm: Confirm = match body_or_default(&body) {
        Ok(confirm) => confirm,
        Err(message) => return error(StatusCode::BAD_REQUEST, message),
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
        Err(e) => {
            let row = |reason| by(Entry::failed(LIFECYCLE, reason));
            failure(&state, row, action.name(), &vm.name, e).await
        }
    }
}

/// `DELETE /api/vms/{id}`.
pub async fn delete(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Response {
    let request: DeleteRequest = match body_or_default(&body) {
        Ok(request) => request,
        Err(message) => return error(StatusCode::BAD_REQUEST, message),
    };
    let Some(vm) = state.host.inventory().vms.remove(&id) else {
        return error(StatusCode::NOT_FOUND, "no such VM");
    };
    if request.confirm.as_deref() != Some(vm.name.as_str()) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "type the VM's name to delete it",
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
        entry.detail.action = Some("delete");
        entry
    };
    match virt.delete(id, request.remove_volumes).await {
        Ok(report) => {
            audit::log(&state.db, by(Entry::ok(LIFECYCLE))).await;
            Json(Removal::from(report)).into_response()
        }
        Err(e) => {
            let row = |reason| by(Entry::failed(LIFECYCLE, reason));
            failure(&state, row, "delete", &vm.name, e).await
        }
    }
}

/// `PATCH /api/vms/{id}`.
pub async fn update(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Response {
    let settings: Settings = match serde_json::from_slice(&body) {
        Ok(settings) => settings,
        Err(e) => return error(StatusCode::BAD_REQUEST, format!("bad JSON body: {e}")),
    };
    let Some(vm) = state.host.inventory().vms.remove(&id) else {
        return error(StatusCode::NOT_FOUND, "no such VM");
    };
    let Some(virt) = state.host.virt() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Lodger is not connected to libvirt",
        );
    };
    let action = if settings.autostart {
        "autostart-on"
    } else {
        "autostart-off"
    };
    let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);
    let by = |entry: Entry| {
        let mut entry = entry
            .account(&session.username)
            .client_ip(ip)
            .target_vm(&vm.name);
        entry.detail.action = Some(action);
        entry
    };
    match virt.set_autostart(id, settings.autostart).await {
        Ok(()) => {
            audit::log(&state.db, by(Entry::ok(EDITED))).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            let row = |reason| by(Entry::failed(EDITED, reason));
            failure(&state, row, action, &vm.name, e).await
        }
    }
}

/// Audits a failed libvirt call with its reason code and builds the answer.
/// `row` builds the audit row for a reason code.
async fn failure(
    state: &AppState,
    row: impl FnOnce(&'static str) -> Entry,
    action: &str,
    vm: &str,
    e: lodger_virt::Error,
) -> Response {
    let (reason, code) = if e.is_not_found() {
        ("no_such_vm", StatusCode::NOT_FOUND)
    } else if e.is_invalid_operation() {
        ("wrong_state", StatusCode::CONFLICT)
    } else {
        eprintln!("lodger: {action} {vm}: {e}");
        ("libvirt_error", StatusCode::BAD_GATEWAY)
    };
    audit::log(&state.db, row(reason)).await;
    if reason == "no_such_vm" {
        error(code, "no such VM")
    } else {
        error_answer(code, &e)
    }
}

/// The answer for a failed libvirt call, with the explanation of a known
/// error.
pub(crate) fn error_answer(code: StatusCode, e: &lodger_virt::Error) -> Response {
    explained(code, e.to_string(), e.explanation())
}

/// The answer that says that Lodger has no libvirt connection now.
pub(crate) fn no_libvirt() -> Response {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Lodger is not connected to libvirt",
    )
}

/// The answer for a body that is not the expected JSON.
pub(crate) fn bad_json(e: &serde_json::Error) -> Response {
    error(StatusCode::BAD_REQUEST, format!("bad JSON body: {e}"))
}

/// What a pool or network call answers, and audits, when libvirt has no
/// such object.
pub(crate) struct NotFound {
    /// The fixed audit reason, such as `no_such_pool`.
    pub reason: &'static str,
    /// The answer text, such as `no such pool`.
    pub message: &'static str,
}

/// Runs one libvirt call on a pool or a network and writes its audit row,
/// with a fixed reason code on failure. The error is the answer to send.
pub(crate) async fn audited(
    state: &AppState,
    not_found: NotFound,
    row: impl Fn(Entry) -> Entry,
    event: &'static str,
    action: Option<&'static str>,
    call: impl Future<Output = Result<(), lodger_virt::Error>>,
) -> Result<(), Box<Response>> {
    let with_action = |mut entry: Entry| {
        entry.detail.action = action;
        row(entry)
    };
    match call.await {
        Ok(()) => {
            audit::log(&state.db, with_action(Entry::ok(event))).await;
            Ok(())
        }
        Err(e) => {
            let (reason, code) = if e.is_not_found() {
                (not_found.reason, StatusCode::NOT_FOUND)
            } else if matches!(e, lodger_virt::Error::InUse(_)) {
                ("in_use", StatusCode::CONFLICT)
            } else if e.is_invalid_operation() {
                ("wrong_state", StatusCode::CONFLICT)
            } else {
                eprintln!("lodger: {event}: {e}");
                ("libvirt_error", StatusCode::BAD_GATEWAY)
            };
            audit::log(&state.db, with_action(Entry::failed(event, reason))).await;
            Err(Box::new(if reason == not_found.reason {
                error(code, not_found.message)
            } else {
                error_answer(code, &e)
            }))
        }
    }
}

/// An error answer. A known libvirt error also carries its `cause`, its
/// `fix`, and the `commands` of the fix (PRD R10). The `error` text stays
/// as libvirt wrote it.
fn explained(
    code: StatusCode,
    message: String,
    explanation: Option<&lodger_virt::Explanation>,
) -> Response {
    let mut body = serde_json::json!({ "error": message });
    if let Some(known) = explanation {
        body["cause"] = known.cause.into();
        body["fix"] = known.fix.into();
        body["commands"] = known.commands.into();
    }
    (code, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use lodger_virt::{SkipReason, Skipped};

    use super::{Removal, error_answer, explained};

    async fn body_of(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn a_known_error_carries_its_cause_fix_and_commands() {
        let message = "libvirt: internal error: unable to execute QEMU command 'getfd': \
                       No file descriptor supplied via SCM_RIGHTS";
        let known = lodger_virt::explain(message);
        let response = explained(StatusCode::BAD_GATEWAY, message.into(), known);
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_of(response).await;
        assert_eq!(body["error"], message);
        assert!(body["cause"].as_str().unwrap().contains("AppArmor"));
        assert!(body["fix"].as_str().unwrap().contains("/dev/vhost-net rw,"));
        assert_eq!(body["commands"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_failed_call_answers_with_the_explanation_of_its_error() {
        let known = lodger_virt::Error::WrongState(
            "unable to execute QEMU command 'getfd': No file descriptor supplied via SCM_RIGHTS",
        );
        let body = body_of(error_answer(StatusCode::BAD_GATEWAY, &known)).await;
        assert_eq!(body["commands"].as_array().unwrap().len(), 2);
        let unknown = lodger_virt::Error::WrongState("the VM is paused");
        let body = body_of(error_answer(StatusCode::CONFLICT, &unknown)).await;
        assert_eq!(body, serde_json::json!({ "error": "the VM is paused" }));
    }

    #[tokio::test]
    async fn an_unknown_error_keeps_its_text_and_nothing_else() {
        let message = "libvirt: operation failed: something new";
        let response = explained(StatusCode::BAD_GATEWAY, message.into(), None);
        assert_eq!(
            body_of(response).await,
            serde_json::json!({ "error": message })
        );
    }

    #[test]
    fn every_skip_reason_has_its_json_shape() {
        let skipped = |path: &str, reason| Skipped {
            path: path.into(),
            reason,
        };
        let report = lodger_virt::Removal {
            removed: vec!["/p/gone.img".into()],
            skipped: vec![
                skipped("/p/a.img", SkipReason::UsedBy("web".into())),
                skipped("/p/b.img", SkipReason::Shared),
                skipped("pool/c.img", SkipReason::NotInPool),
                skipped("/p/d.img", SkipReason::Failed("busy".into())),
            ],
        };
        assert_eq!(
            serde_json::to_value(Removal::from(report)).unwrap(),
            serde_json::json!({
                "removed": ["/p/gone.img"],
                "skipped": [
                    { "path": "/p/a.img", "reason": "used_by", "vm": "web" },
                    { "path": "/p/b.img", "reason": "shared" },
                    { "path": "pool/c.img", "reason": "not_in_pool" },
                    { "path": "/p/d.img", "reason": "failed", "message": "busy" },
                ],
            })
        );
    }
}
