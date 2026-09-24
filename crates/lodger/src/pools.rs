//! The storage pools API (PRD F6 and flow 4.4, TAD 4.3):
//!
//! - `GET /api/pools`: every pool, sorted by name.
//! - `GET /api/pools/{id}`: one pool with its type, folder, NFS source, and
//!   the VMs that have a disk in it.
//! - `POST /api/pools`: a new directory or NFS pool. Autostart is on unless
//!   the body says `"autostart": false`.
//! - `PATCH /api/pools/{id}`: `{"active": bool}` starts or stops the pool, and
//!   `{"autostart": bool}` switches autostart.
//! - `DELETE /api/pools/{id}`: removes the pool. The body must repeat its
//!   name, and `"delete_files": true` also deletes every volume in it.
//!
//! Every call that reaches libvirt writes a `pool.*` audit row.

use std::net::SocketAddr;

use axum::Extension;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use lodger_core::model::Pool;
use lodger_core::validate::Name;
use lodger_core::xml::pool::{NewPool, PoolXml};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::actions::{NotFound, audited, bad_json, error, error_answer, no_libvirt};
use crate::audit::{self, Entry};
use crate::db::Session;
use crate::server::AppState;

/// The answer and the audit reason when libvirt has no such pool.
const NOT_FOUND: NotFound = NotFound {
    reason: "no_such_pool",
    message: "no such pool",
};

/// Where Lodger mounts an NFS pool when the request names no folder.
const NFS_MOUNT_ROOT: &str = "/var/lib/libvirt/pools";

/// A pool with the facts that its XML and the VMs add.
#[derive(Debug, Serialize)]
pub struct PoolDetail {
    #[serde(flatten)]
    pub pool: Pool,
    /// libvirt's pool type, such as `dir` or `netfs`.
    pub kind: Option<String>,
    /// The folder on the host.
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nfs: Option<NfsSource>,
    /// The VMs with a disk in the pool, sorted by name.
    pub used_by: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct NfsSource {
    pub host: String,
    pub export: String,
}

/// The body of `POST /api/pools`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePool {
    pub name: String,
    pub kind: Kind,
    /// The folder. Required for `dir`, optional for `nfs`.
    pub path: Option<String>,
    /// The NFS server.
    pub host: Option<String>,
    /// The NFS export on the server.
    pub export: Option<String>,
    #[serde(default = "yes")]
    pub autostart: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Dir,
    Nfs,
}

/// The body of `PATCH /api/pools/{id}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub active: Option<bool>,
    pub autostart: Option<bool>,
}

/// The body of `DELETE /api/pools/{id}`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Removal {
    /// The pool's name, typed by the user.
    pub confirm: Option<String>,
    #[serde(default)]
    pub delete_files: bool,
}

/// `GET /api/pools`.
pub async fn list(State(state): State<AppState>) -> Json<Vec<Pool>> {
    let mut pools: Vec<Pool> = state.host.inventory().pools.into_values().collect();
    pools.sort_by(|a, b| a.name.cmp(&b.name).then(a.uuid.cmp(&b.uuid)));
    Json(pools)
}

/// `GET /api/pools/{id}`.
pub async fn detail(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let Some(pool) = state.host.inventory().pools.remove(&id) else {
        return error(StatusCode::NOT_FOUND, "no such pool");
    };
    let Some(virt) = state.host.virt() else {
        return no_libvirt();
    };
    let facts = async {
        let xml = PoolXml::parse(&virt.pool_xml(id).await?)?;
        let used_by = virt.pool_users(id).await?;
        Ok::<_, lodger_virt::Error>((xml, used_by))
    };
    match facts.await {
        Ok((xml, used_by)) => Json(PoolDetail {
            kind: xml.pool_type().map(str::to_owned),
            path: xml.target_path().map(str::to_owned),
            nfs: xml.nfs_source().map(|(host, export)| NfsSource {
                host: host.to_owned(),
                export: export.to_owned(),
            }),
            pool,
            used_by,
        })
        .into_response(),
        Err(e) if e.is_not_found() => error(StatusCode::NOT_FOUND, "no such pool"),
        Err(e) => error_answer(StatusCode::BAD_GATEWAY, &e),
    }
}

/// Checks a create request and builds the new pool, or the answer that
/// rejects it. `existing` holds the name and XML of every pool.
fn new_pool(request: &CreatePool, existing: &[(String, String)]) -> Result<NewPool, Box<Response>> {
    let invalid = |e: lodger_core::validate::InputError| {
        Box::new(error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
    };
    let name = Name::parse("Pool name", &request.name).map_err(invalid)?;
    if existing.iter().any(|(other, _)| *other == request.name) {
        return Err(Box::new(error(
            StatusCode::CONFLICT,
            format!("a pool called {:?} exists already", request.name),
        )));
    }
    let missing = |field: &str| {
        Box::new(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("{field} is empty"),
        ))
    };
    let pool = match request.kind {
        Kind::Dir => {
            let path = request
                .path
                .as_deref()
                .ok_or_else(|| missing("Pool folder"))?;
            NewPool::dir(name, path).map_err(invalid)?
        }
        Kind::Nfs => {
            let host = request
                .host
                .as_deref()
                .ok_or_else(|| missing("NFS server"))?;
            let export = request
                .export
                .as_deref()
                .ok_or_else(|| missing("NFS export"))?;
            let path = request
                .path
                .clone()
                .unwrap_or_else(|| format!("{NFS_MOUNT_ROOT}/{}", request.name));
            NewPool::nfs(name, host, export, &path).map_err(invalid)?
        }
    };
    // A pool whose XML Lodger cannot read cannot clash with anything that
    // Lodger could tell.
    let parsed: Vec<(&str, PoolXml)> = existing
        .iter()
        .filter_map(|(name, xml)| Some((name.as_str(), PoolXml::parse(xml).ok()?)))
        .collect();
    pool.check_against(parsed.iter().map(|(name, xml)| (*name, xml)))
        .map_err(invalid)?;
    Ok(pool)
}

/// `POST /api/pools`.
pub async fn create(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    body: Bytes,
) -> Response {
    let request: CreatePool = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(e) => return bad_json(&e),
    };
    let Some(virt) = state.host.virt() else {
        return no_libvirt();
    };
    let existing = match virt.pool_xmls().await {
        Ok(existing) => existing,
        Err(e) => return error_answer(StatusCode::BAD_GATEWAY, &e),
    };
    let pool = match new_pool(&request, &existing) {
        Ok(pool) => pool,
        Err(response) => return *response,
    };
    let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);
    let row = |entry: Entry| {
        entry
            .account(&session.username)
            .client_ip(ip)
            .target_pool(pool.name.as_str())
    };
    match virt.create_pool(pool.to_xml(), request.autostart).await {
        Ok(id) => {
            audit::log(&state.db, row(Entry::ok("pool.created"))).await;
            (StatusCode::CREATED, Json(serde_json::json!({ "uuid": id }))).into_response()
        }
        Err(e) => {
            eprintln!("lodger: create pool {}: {e}", pool.name);
            audit::log(
                &state.db,
                row(Entry::failed("pool.created", "libvirt_error")),
            )
            .await;
            error_answer(StatusCode::BAD_GATEWAY, &e)
        }
    }
}

/// `PATCH /api/pools/{id}`.
pub async fn change(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Response {
    let change: Change = match serde_json::from_slice(&body) {
        Ok(change) => change,
        Err(e) => return bad_json(&e),
    };
    if change.active.is_none() && change.autostart.is_none() {
        return error(
            StatusCode::BAD_REQUEST,
            "the body needs \"active\" or \"autostart\"",
        );
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
            .target_pool(&pool.name)
    };
    if let Some(active) = change.active {
        let event = if active {
            "pool.started"
        } else {
            "pool.stopped"
        };
        if let Err(response) = audited(&state, NOT_FOUND, row, event, None, async {
            virt.set_pool_active(id, active).await
        })
        .await
        {
            return *response;
        }
    }
    if let Some(on) = change.autostart {
        let action = if on { "autostart-on" } else { "autostart-off" };
        if let Err(response) = audited(&state, NOT_FOUND, row, "pool.edited", Some(action), async {
            virt.set_pool_autostart(id, on).await
        })
        .await
        {
            return *response;
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `DELETE /api/pools/{id}`.
pub async fn remove(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Response {
    let request: Removal = if body.is_empty() {
        Removal::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(request) => request,
            Err(e) => return bad_json(&e),
        }
    };
    let Some(pool) = state.host.inventory().pools.remove(&id) else {
        return error(StatusCode::NOT_FOUND, "no such pool");
    };
    if request.confirm.as_deref() != Some(pool.name.as_str()) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "type the pool's name to remove it",
        );
    }
    let Some(virt) = state.host.virt() else {
        return no_libvirt();
    };
    let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);
    let row = |entry: Entry| {
        entry
            .account(&session.username)
            .client_ip(ip)
            .target_pool(&pool.name)
    };
    let action = if request.delete_files {
        "delete-files"
    } else {
        "keep-files"
    };
    match audited(
        &state,
        NOT_FOUND,
        row,
        "pool.deleted",
        Some(action),
        async { virt.remove_pool(id, request.delete_files).await },
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(response) => *response,
    }
}
