//! The storage volumes API (PRD F7, TAD 4.3). A volume has no UUID, so the
//! pool's UUID and the volume's name identify it:
//!
//! - `GET /api/pools/{id}/volumes`: every volume of a running pool, sorted by
//!   name, each with the VMs that use it.
//! - `POST /api/pools/{id}/volumes`: a new qcow2 or raw volume, from
//!   `{"name", "format", "capacity_bytes"}`.
//! - `DELETE /api/pools/{id}/volumes/{name}`: deletes the volume, unless a VM
//!   or a qcow2 overlay in the pool uses it.
//!
//! Every call that reaches libvirt with a change writes a `volume.*` audit row.

use std::net::SocketAddr;

use axum::Extension;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use lodger_core::validate::{Name, check_text};
use lodger_core::xml::volume::{NewVolume, VolumeFormat};
use serde::Deserialize;
use uuid::Uuid;

use crate::actions::{NotFound, audited, bad_json, error, error_answer, no_libvirt};
use crate::audit::Entry;
use crate::db::Session;
use crate::server::AppState;

/// The answer and the audit reason when libvirt has no such pool or volume.
const NOT_FOUND: NotFound = NotFound {
    reason: "no_such_volume",
    message: "no such pool or volume",
};

/// The body of `POST /api/pools/{id}/volumes`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateVolume {
    pub name: String,
    pub format: Format,
    pub capacity_bytes: u64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Qcow2,
    Raw,
}

/// `GET /api/pools/{id}/volumes`.
pub async fn list(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    if !state.host.inventory().pools.contains_key(&id) {
        return error(StatusCode::NOT_FOUND, "no such pool");
    }
    let Some(virt) = state.host.virt() else {
        return no_libvirt();
    };
    match virt.volumes(id).await {
        Ok(volumes) => Json(volumes).into_response(),
        Err(e) if e.is_not_found() => error(StatusCode::NOT_FOUND, "no such pool"),
        Err(e) if e.is_invalid_operation() => error_answer(StatusCode::CONFLICT, &e),
        Err(e) => error_answer(StatusCode::BAD_GATEWAY, &e),
    }
}

/// `POST /api/pools/{id}/volumes`. The checks run before any change: the
/// name, the size, and a name that the pool has already.
pub async fn create(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Response {
    let request: CreateVolume = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(e) => return bad_json(&e),
    };
    let Some(pool) = state.host.inventory().pools.remove(&id) else {
        return error(StatusCode::NOT_FOUND, "no such pool");
    };
    let invalid = |e: lodger_core::validate::InputError| {
        error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string())
    };
    let format = match request.format {
        Format::Qcow2 => VolumeFormat::Qcow2,
        Format::Raw => VolumeFormat::Raw,
    };
    let volume = match Name::parse("Volume name", &request.name)
        .and_then(|name| NewVolume::new(name, format, request.capacity_bytes))
    {
        Ok(volume) => volume,
        Err(e) => return invalid(e),
    };
    let Some(virt) = state.host.virt() else {
        return no_libvirt();
    };
    let existing = match virt.volume_names(id).await {
        Ok(existing) => existing,
        Err(e) if e.is_not_found() => return error(StatusCode::NOT_FOUND, "no such pool"),
        Err(e) if e.is_invalid_operation() => return error_answer(StatusCode::CONFLICT, &e),
        Err(e) => return error_answer(StatusCode::BAD_GATEWAY, &e),
    };
    if let Err(e) = volume.check_against(&pool.name, existing.iter().map(String::as_str)) {
        return error(StatusCode::CONFLICT, e.to_string());
    }
    let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);
    let name = volume.name.as_str().to_owned();
    let row = |entry: Entry| {
        entry
            .account(&session.username)
            .client_ip(ip)
            .target_volume(&pool.name, &name)
    };
    match audited(&state, NOT_FOUND, row, "volume.created", None, async {
        virt.create_volume(id, volume.to_xml()).await
    })
    .await
    {
        Ok(()) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "name": name })),
        )
            .into_response(),
        Err(response) => *response,
    }
}

/// `DELETE /api/pools/{id}/volumes/{name}`. A volume that a VM uses stays,
/// and the answer names the VMs.
pub async fn remove(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    Path((id, name)): Path<(Uuid, String)>,
) -> Response {
    // The name comes from the URL, and a NUL byte there would make the virt
    // crate panic. Only NUL is refused: a volume from another tool may have a
    // name outside Lodger's own rules, and it must stay deletable.
    if let Err(e) = check_text("Volume name", &name) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string());
    }
    let Some(pool) = state.host.inventory().pools.remove(&id) else {
        return error(StatusCode::NOT_FOUND, "no such pool");
    };
    let Some(virt) = state.host.virt() else {
        return no_libvirt();
    };
    let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);
    let row = |entry: Entry| {
        entry
            .account(&session.username)
            .client_ip(ip)
            .target_volume(&pool.name, &name)
    };
    match audited(&state, NOT_FOUND, row, "volume.deleted", None, async {
        virt.delete_volume(id, name.clone()).await
    })
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(response) => *response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_create_body_names_its_format_in_snake_case() {
        let body: CreateVolume =
            serde_json::from_str(r#"{"name":"d.qcow2","format":"qcow2","capacity_bytes":1048576}"#)
                .unwrap();
        assert!(matches!(body.format, Format::Qcow2));
        assert!(
            serde_json::from_str::<CreateVolume>(
                r#"{"name":"d","format":"vmdk","capacity_bytes":1}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<CreateVolume>(
                r#"{"name":"d","format":"raw","capacity_bytes":1,"x":1}"#
            )
            .is_err()
        );
    }
}
