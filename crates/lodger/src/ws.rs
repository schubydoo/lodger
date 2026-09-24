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
//! - `stats`: the live stats of every running VM, in `vms`, every 5 seconds.
//!   Only a client that sent `{"subscribe":"stats"}` gets them, until it
//!   sends `{"unsubscribe":"stats"}` or closes.
//!
//! The browser sends only those 2 messages. The server ignores anything
//! else, and any text over 256 bytes.
//! Each client has its own task and its own receiver. A slow client
//! therefore delays only itself. When it falls more than the hub's capacity
//! behind, it skips the old events and gets `resync`.

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use lodger_core::model::VmStats;
use lodger_virt::Event;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use uuid::Uuid;

use crate::api::Connection;
use crate::auth::{self, SocketQuery};
use crate::server::AppState;
use crate::stats::Snapshot;

#[derive(Debug, PartialEq, Serialize)]
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
    Stats {
        vms: Vec<VmStats>,
    },
}

/// A message from the browser. As an enum, it accepts an object with exactly
/// one key, so `{"subscribe":"stats","x":1}` fails to parse.
#[derive(Debug, PartialEq, Eq, Deserialize)]
enum Request {
    #[serde(rename = "subscribe")]
    Subscribe(Topic),
    #[serde(rename = "unsubscribe")]
    Unsubscribe(Topic),
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Topic {
    Stats,
}

/// The browser's request in `text`, or `None` for anything else.
fn request(text: &str) -> Option<Request> {
    if text.len() > 256 {
        return None;
    }
    serde_json::from_str(text).ok()
}

/// The next stats snapshot for a subscribed client. Without a subscription,
/// it never finishes, so `select!` waits on the other branches.
async fn next_stats(stats: &mut Option<watch::Receiver<Snapshot>>) -> Option<Vec<VmStats>> {
    match stats {
        Some(rx) => {
            rx.changed().await.ok()?;
            Some(rx.borrow_and_update().as_ref().clone())
        }
        None => std::future::pending().await,
    }
}

pub async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SocketQuery>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let key = match auth::open_socket(&state, &headers, &query).await {
        Ok(key) => key,
        Err(response) => return *response,
    };
    upgrade.on_upgrade(move |socket| forward(socket, state, key))
}

/// The close message when the socket's session ends: code 1008, "policy
/// violation", which tells the page not to count it as a network failure.
pub(crate) fn session_over() -> Message {
    Message::Close(Some(CloseFrame {
        code: close_code::POLICY,
        reason: "the session ended".into(),
    }))
}

async fn forward(mut socket: WebSocket, state: AppState, key: [u8; 32]) {
    let ended = auth::session_ended(state.clone(), key);
    tokio::pin!(ended);
    // Subscribe first, then mark the current values as seen, so the client
    // gets only what changes after it connects.
    let mut events = state.host.subscribe();
    let mut reloads = state.host.watch_reloads();
    let mut conn = state.host.watch_state();
    reloads.mark_unchanged();
    conn.mark_unchanged();
    let mut stats = None;

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
            () = &mut ended => {
                let _ = socket.send(session_over()).await;
                return;
            },
            Some(vms) = next_stats(&mut stats) => Update::Stats { vms },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(Message::Text(text))) => {
                    match request(&text) {
                        Some(Request::Subscribe(Topic::Stats)) if stats.is_none() => {
                            stats = Some(state.stats.subscribe());
                        }
                        Some(Request::Unsubscribe(Topic::Stats)) => stats = None,
                        _ => {}
                    }
                    continue;
                }
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

    use super::{Request, Topic, Update, request, update_for};
    use crate::api::Connection;

    #[test]
    fn only_the_two_requests_parse() {
        assert_eq!(
            request(r#"{"subscribe":"stats"}"#),
            Some(Request::Subscribe(Topic::Stats))
        );
        assert_eq!(
            request(r#"{"unsubscribe":"stats"}"#),
            Some(Request::Unsubscribe(Topic::Stats))
        );
        for other in [
            "hello",
            r#"{"subscribe":"cpu"}"#,
            r#"{"subscribe":"stats","x":1}"#,
            r#"{"type":"stats"}"#,
        ] {
            assert_eq!(request(other), None, "{other}");
        }
        let long = format!(r#"{{"subscribe":"stats"}}{}"#, " ".repeat(300));
        assert_eq!(request(&long), None);
    }

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
