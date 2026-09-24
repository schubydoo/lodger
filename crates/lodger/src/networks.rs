//! The virtual networks API (PRD F8, TAD 4.3):
//!
//! - `GET /api/networks`: every network, sorted by name.
//! - `GET /api/networks/{id}`: one network with its mode, bridge, subnets,
//!   and the VMs that have a NIC on it.
//! - `GET /api/host-bridges`: the bridges on the host, for a bridge network.
//!   Lodger only reads them: it never creates or changes a host interface.
//! - `POST /api/networks`: a new NAT, isolated, or bridge network. Autostart
//!   is on unless the body says `"autostart": false`.
//! - `PATCH /api/networks/{id}`: `{"active": bool}` starts or stops the
//!   network, and `{"autostart": bool}` switches autostart.
//! - `DELETE /api/networks/{id}`: stops and undefines the network. The body
//!   must repeat its name.
//!
//! Every call that reaches libvirt writes a `network.*` audit row.

use std::net::SocketAddr;

use axum::Extension;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use lodger_core::model::Network;
use lodger_core::validate::{InputError, Name};
use lodger_core::xml::network::{NetworkXml, NewNetwork};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::actions::{error, error_answer};
use crate::audit::{self, Entry};
use crate::db::Session;
use crate::server::AppState;

/// A network with the facts that its XML and the VMs add.
#[derive(Debug, Serialize)]
pub struct NetworkDetail {
    #[serde(flatten)]
    pub network: Network,
    /// `nat`, `bridge`, or another forward mode. `None` means isolated.
    pub mode: Option<String>,
    /// The IPv4 subnets, such as `192.168.122.0/24`.
    pub subnets: Vec<String>,
    /// The VMs with a NIC on the network, sorted by name.
    pub used_by: Vec<String>,
}

/// The body of `POST /api/networks`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateNetwork {
    pub name: String,
    pub mode: Mode,
    /// The subnet of a NAT or isolated network.
    pub subnet: Option<String>,
    /// The host bridge of a bridge network.
    pub bridge: Option<String>,
    #[serde(default = "yes")]
    pub autostart: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Nat,
    Isolated,
    Bridge,
}

/// The body of `PATCH /api/networks/{id}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub active: Option<bool>,
    pub autostart: Option<bool>,
}

/// The body of `DELETE /api/networks/{id}`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Removal {
    /// The network's name, typed by the user.
    pub confirm: Option<String>,
}

fn no_libvirt() -> Response {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Lodger is not connected to libvirt",
    )
}

fn bad_json(e: &serde_json::Error) -> Response {
    error(StatusCode::BAD_REQUEST, format!("bad JSON body: {e}"))
}

/// `GET /api/networks`.
pub async fn list(State(state): State<AppState>) -> Json<Vec<Network>> {
    let mut networks: Vec<Network> = state.host.inventory().networks.into_values().collect();
    networks.sort_by(|a, b| a.name.cmp(&b.name).then(a.uuid.cmp(&b.uuid)));
    Json(networks)
}

/// `GET /api/host-bridges`.
pub async fn host_bridges() -> Json<Vec<String>> {
    Json(lodger_virt::host_bridges())
}

/// `GET /api/networks/{id}`.
pub async fn detail(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let Some(network) = state.host.inventory().networks.remove(&id) else {
        return error(StatusCode::NOT_FOUND, "no such network");
    };
    let Some(virt) = state.host.virt() else {
        return no_libvirt();
    };
    let facts = async {
        let xml = NetworkXml::parse(&virt.network_xml(id).await?)?;
        let used_by = virt.network_users(id).await?;
        Ok::<_, lodger_virt::Error>((xml, used_by))
    };
    match facts.await {
        Ok((xml, used_by)) => Json(NetworkDetail {
            mode: xml.forward_mode().map(str::to_owned),
            subnets: xml.subnets().iter().map(ToString::to_string).collect(),
            network,
            used_by,
        })
        .into_response(),
        Err(e) if e.is_not_found() => error(StatusCode::NOT_FOUND, "no such network"),
        Err(e) => error_answer(StatusCode::BAD_GATEWAY, &e),
    }
}

/// Checks a create request and builds the new network, or the answer that
/// rejects it. `existing` holds the name and XML of every network.
fn new_network(
    request: &CreateNetwork,
    existing: &[(String, String)],
    host_bridges: &[String],
) -> Result<NewNetwork, Box<Response>> {
    let invalid = |e: InputError| Box::new(error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()));
    let name = Name::parse("Network name", &request.name).map_err(invalid)?;
    if existing.iter().any(|(other, _)| *other == request.name) {
        return Err(Box::new(error(
            StatusCode::CONFLICT,
            format!("a network called {:?} exists already", request.name),
        )));
    }
    let missing = |field: &'static str| invalid(InputError::Empty { field });
    let network = match request.mode {
        Mode::Nat | Mode::Isolated => {
            let subnet = request.subnet.as_deref().ok_or_else(|| missing("Subnet"))?;
            if matches!(request.mode, Mode::Nat) {
                NewNetwork::nat(name, subnet)
            } else {
                NewNetwork::isolated(name, subnet)
            }
            .map_err(invalid)?
        }
        Mode::Bridge => {
            let bridge = request
                .bridge
                .as_deref()
                .ok_or_else(|| missing("Host bridge"))?;
            NewNetwork::bridge(name, bridge, host_bridges).map_err(invalid)?
        }
    };
    // A network whose XML Lodger cannot read cannot clash with anything that
    // Lodger could tell.
    let parsed: Vec<(&str, NetworkXml)> = existing
        .iter()
        .filter_map(|(name, xml)| Some((name.as_str(), NetworkXml::parse(xml).ok()?)))
        .collect();
    network
        .check_against(parsed.iter().map(|(name, xml)| (*name, xml)))
        .map_err(invalid)?;
    Ok(network)
}

/// `POST /api/networks`.
pub async fn create(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    body: Bytes,
) -> Response {
    let request: CreateNetwork = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(e) => return bad_json(&e),
    };
    let Some(virt) = state.host.virt() else {
        return no_libvirt();
    };
    let existing = match virt.network_xmls().await {
        Ok(existing) => existing,
        Err(e) => return error_answer(StatusCode::BAD_GATEWAY, &e),
    };
    let network = match new_network(&request, &existing, &lodger_virt::host_bridges()) {
        Ok(network) => network,
        Err(response) => return *response,
    };
    let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);
    let row = |entry: Entry| {
        entry
            .account(&session.username)
            .client_ip(ip)
            .target_network(network.name.as_str())
    };
    match virt
        .create_network(network.to_xml(), request.autostart)
        .await
    {
        Ok(id) => {
            audit::log(&state.db, row(Entry::ok("network.created"))).await;
            (StatusCode::CREATED, Json(serde_json::json!({ "uuid": id }))).into_response()
        }
        Err(e) => {
            eprintln!("lodger: create network {}: {e}", network.name);
            audit::log(
                &state.db,
                row(Entry::failed("network.created", "libvirt_error")),
            )
            .await;
            error_answer(StatusCode::BAD_GATEWAY, &e)
        }
    }
}

/// `PATCH /api/networks/{id}`.
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
    let Some(network) = state.host.inventory().networks.remove(&id) else {
        return error(StatusCode::NOT_FOUND, "no such network");
    };
    let Some(virt) = state.host.virt() else {
        return no_libvirt();
    };
    let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);
    let row = |entry: Entry| {
        entry
            .account(&session.username)
            .client_ip(ip)
            .target_network(&network.name)
    };
    if let Some(active) = change.active {
        let event = if active {
            "network.started"
        } else {
            "network.stopped"
        };
        let call = virt.set_network_active(id, active);
        if let Err(response) = audited(&state, row, event, None, call).await {
            return *response;
        }
    }
    if let Some(on) = change.autostart {
        let action = if on { "autostart-on" } else { "autostart-off" };
        let call = virt.set_network_autostart(id, on);
        if let Err(response) = audited(&state, row, "network.edited", Some(action), call).await {
            return *response;
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `DELETE /api/networks/{id}`.
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
    let Some(network) = state.host.inventory().networks.remove(&id) else {
        return error(StatusCode::NOT_FOUND, "no such network");
    };
    if request.confirm.as_deref() != Some(network.name.as_str()) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "type the network's name to delete it",
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
            .target_network(&network.name)
    };
    match audited(
        &state,
        row,
        "network.deleted",
        None,
        virt.delete_network(id),
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(response) => *response,
    }
}

/// Runs one libvirt call and writes its audit row, with a fixed reason code
/// on failure. The error is the answer to send.
async fn audited(
    state: &AppState,
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
                ("no_such_network", StatusCode::NOT_FOUND)
            } else if e.is_invalid_operation() {
                ("wrong_state", StatusCode::CONFLICT)
            } else {
                eprintln!("lodger: {event}: {e}");
                ("libvirt_error", StatusCode::BAD_GATEWAY)
            };
            audit::log(&state.db, with_action(Entry::failed(event, reason))).await;
            Err(Box::new(if reason == "no_such_network" {
                error(code, "no such network")
            } else {
                error_answer(code, &e)
            }))
        }
    }
}
