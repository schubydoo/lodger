//! The REST API. Reads come from the inventory cache (TAD section 5).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use lodger_core::model::{HostInfo, Vm, VmState};
use lodger_virt::ConnState;
use serde::Serialize;
use uuid::Uuid;

use crate::server::AppState;

/// The connection state as JSON, for the UI banner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Connection {
    /// `connecting`, `connected`, or `disconnected`.
    pub state: &'static str,
    /// Why Lodger is disconnected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<ConnState> for Connection {
    fn from(state: ConnState) -> Self {
        match state {
            ConnState::Connecting => Self {
                state: "connecting",
                error: None,
            },
            ConnState::Connected => Self {
                state: "connected",
                error: None,
            },
            ConnState::Disconnected { error } => Self {
                state: "disconnected",
                error: Some(error),
            },
        }
    }
}

/// `GET /api/health`: for monitoring. It needs no login (TAD 4.3), so it
/// answers with fixed words only. Error details can name paths on the host;
/// they go to the log.
#[derive(Debug, Serialize)]
pub struct Health {
    /// `ok`, `degraded` while libvirt is not connected, or `error` when the
    /// database fails.
    pub status: &'static str,
    /// `ok` or `error`.
    pub database: &'static str,
    /// `connecting`, `connected`, or `disconnected`.
    pub libvirt: &'static str,
}

pub async fn health(State(state): State<AppState>) -> (StatusCode, Json<Health>) {
    let libvirt = Connection::from(state.host.state()).state;
    let (code, status, database) = match state.db.ping().await {
        Err(e) => {
            eprintln!("lodger: health check: the database failed: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "error", "error")
        }
        Ok(()) if libvirt == "connected" => (StatusCode::OK, "ok", "ok"),
        Ok(()) => (StatusCode::OK, "degraded", "ok"),
    };
    (
        code,
        Json(Health {
            status,
            database,
            libvirt,
        }),
    )
}

/// `GET /api/host`.
#[derive(Debug, Serialize)]
pub struct Host {
    pub connection: Connection,
    /// `None` while Lodger is not connected, or when the read failed.
    pub info: Option<HostInfo>,
    /// Why `info` is `None` although Lodger is connected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info_error: Option<String>,
    pub vms: VmCounts,
    pub pools: usize,
    pub networks: usize,
}

#[derive(Debug, Serialize)]
pub struct VmCounts {
    pub total: usize,
    pub running: usize,
}

pub async fn host(State(state): State<AppState>) -> Json<Host> {
    // Take the state, the inventory, and the connections at one moment,
    // before the libvirt call waits.
    let connection = state.host.state().into();
    let inventory = state.host.inventory();
    let (info, info_error) = match state.host.virt() {
        Some(virt) => match virt.host_info().await {
            Ok(info) => (Some(info), None),
            Err(e) => (None, Some(e.to_string())),
        },
        None => (None, None),
    };
    Json(Host {
        connection,
        info,
        info_error,
        vms: VmCounts {
            total: inventory.vms.len(),
            running: inventory
                .vms
                .values()
                .filter(|vm| vm.state == VmState::Running)
                .count(),
        },
        pools: inventory.pools.len(),
        networks: inventory.networks.len(),
    })
}

/// `GET /api/vms`: every VM, sorted by name.
pub async fn vms(State(state): State<AppState>) -> Json<Vec<Vm>> {
    let mut vms: Vec<Vm> = state.host.inventory().vms.into_values().collect();
    vms.sort_by(|a, b| a.name.cmp(&b.name).then(a.uuid.cmp(&b.uuid)));
    Json(vms)
}

/// `GET /api/vms/{id}`. API paths use the UUID, so a VM that another tool
/// deletes and defines again under the same name is a different VM.
pub async fn vm(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vm>, StatusCode> {
    state
        .host
        .inventory()
        .vms
        .remove(&id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}
