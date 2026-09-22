//! `/ws/events`: tells the browser which data to fetch again.
//!
//! Each message is one JSON object with a `type` field:
//!
//! - `vm`, `pool`, `network`: the object with this `id` changed.
//! - `resync`: changes were missed, so fetch everything again. The server
//!   sends it when this client falls behind the events, and after the
//!   inventory loads again, for example after a reconnect.
//! - `connection`: the libvirt connection state changed, with the same
//!   fields as `connection` in `GET /api/host`.
//!
//! Each client has its own task and its own receiver. A slow client
//! therefore delays only itself. When it falls more than the hub's capacity
//! behind, it skips the old events and gets `resync`.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use lodger_virt::Event;
use serde::Serialize;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::api::Connection;
use crate::server::AppState;

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Update {
    Vm {
        id: Uuid,
    },
    Pool {
        id: Uuid,
    },
    Network {
        id: Uuid,
    },
    Resync,
    Connection {
        #[serde(flatten)]
        connection: Connection,
    },
}

pub async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade.on_upgrade(move |socket| forward(socket, state))
}

/// Browsers apply no same-origin rule to WebSocket connections, so any page that the
/// user opens could connect here. A browser always sends `Origin` on a
/// WebSocket upgrade, so an upgrade with an `Origin` from another host is
/// refused. A request without `Origin` does not come from a page, for
/// example `websocat` on the host, and passes. Login comes in Task 2.3.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let origin_host = origin
        .to_str()
        .ok()
        .and_then(|o| o.parse::<Uri>().ok())
        .and_then(|uri| uri.authority().cloned());
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    match (origin_host, host) {
        (Some(origin), Some(host)) => origin.as_str().eq_ignore_ascii_case(host),
        _ => false,
    }
}

async fn forward(mut socket: WebSocket, state: AppState) {
    // Subscribe first, then mark the current values as seen, so the client
    // gets only what changes after it connects.
    let mut events = state.host.subscribe();
    let mut reloads = state.host.watch_reloads();
    let mut conn = state.host.watch_state();
    reloads.mark_unchanged();
    conn.mark_unchanged();

    loop {
        let update = tokio::select! {
            received = events.recv() => {
                // The supervisor lives as long as the server, so the channel
                // closes only at shutdown.
                if matches!(received, Err(RecvError::Closed)) {
                    return;
                }
                match update_for(received) {
                    Some(update) => update,
                    None => continue,
                }
            },
            Ok(()) = reloads.changed() => Update::Resync,
            Ok(()) = conn.changed() => Update::Connection {
                connection: conn.borrow_and_update().clone().into(),
            },
            incoming = socket.recv() => match incoming {
                // The browser sends nothing. Ignore anything but a close.
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => continue,
            },
        };
        let text = serde_json::to_string(&update).expect("an Update always serializes");
        if socket.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }
}

/// The message for one result from the event receiver. `None` means that
/// the client needs no message.
pub(crate) fn update_for(received: Result<Event, RecvError>) -> Option<Update> {
    match received {
        Ok(Event::Domain { id, .. }) => Some(Update::Vm { id }),
        Ok(Event::Pool { id, .. }) => Some(Update::Pool { id }),
        Ok(Event::Network { id, .. }) => Some(Update::Network { id }),
        // The supervisor never passes a close on. The state change reports it.
        Ok(Event::Closed { .. }) => None,
        Err(RecvError::Lagged(_)) => Some(Update::Resync),
        // `forward` stops before it asks for a message.
        Err(RecvError::Closed) => None,
    }
}

#[cfg(test)]
mod tests {
    use lodger_virt::{DomainChange, Event};
    use tokio::sync::broadcast::error::RecvError;
    use uuid::Uuid;

    use super::{Update, update_for};
    use crate::api::Connection;

    #[test]
    fn each_event_maps_to_its_message() {
        let id = Uuid::from_u128(7);
        let domain = Event::Domain {
            id,
            change: DomainChange::Reboot,
        };
        assert_eq!(update_for(Ok(domain)), Some(Update::Vm { id }));
        let pool = Event::Pool { id, event: 2 };
        assert_eq!(update_for(Ok(pool)), Some(Update::Pool { id }));
        let net = Event::Network { id, event: 2 };
        assert_eq!(update_for(Ok(net)), Some(Update::Network { id }));
        assert_eq!(update_for(Ok(Event::Closed { reason: 1 })), None);
        assert_eq!(update_for(Err(RecvError::Lagged(3))), Some(Update::Resync));
        assert_eq!(update_for(Err(RecvError::Closed)), None);
    }

    fn headers(pairs: &[(&'static str, &str)]) -> axum::http::HeaderMap {
        pairs
            .iter()
            .map(|(k, v)| (axum::http::HeaderName::from_static(k), v.parse().unwrap()))
            .collect()
    }

    #[test]
    fn only_a_page_from_this_server_may_connect() {
        use super::same_origin;
        let host = ("host", "127.0.0.1:8460");
        assert!(same_origin(&headers(&[
            host,
            ("origin", "http://127.0.0.1:8460")
        ])));
        assert!(same_origin(&headers(&[
            host,
            ("origin", "HTTP://127.0.0.1:8460")
        ])));
        assert!(
            same_origin(&headers(&[host])),
            "no Origin: not a browser page"
        );
        assert!(!same_origin(&headers(&[
            host,
            ("origin", "http://evil.example")
        ])));
        assert!(!same_origin(&headers(&[
            host,
            ("origin", "http://127.0.0.1:9999")
        ])));
        assert!(!same_origin(&headers(&[host, ("origin", "null")])));
        assert!(!same_origin(&headers(&[(
            "origin",
            "http://127.0.0.1:8460"
        )])));
    }

    #[test]
    fn messages_are_small_json_objects() {
        let id = Uuid::from_u128(7);
        let json = |u: &Update| serde_json::to_value(u).unwrap();
        assert_eq!(
            json(&Update::Vm { id }),
            serde_json::json!({"type": "vm", "id": "00000000-0000-0000-0000-000000000007"})
        );
        assert_eq!(json(&Update::Resync), serde_json::json!({"type": "resync"}));
        let down = Update::Connection {
            connection: Connection::from(lodger_virt::ConnState::Disconnected {
                error: "gone".into(),
            }),
        };
        assert_eq!(
            json(&down),
            serde_json::json!({"type": "connection", "state": "disconnected", "error": "gone"})
        );
    }
}
