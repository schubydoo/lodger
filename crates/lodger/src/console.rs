//! `/ws/vms/{id}/vnc`: relays a VM's VNC display to noVNC in the browser.
//!
//! The handler opens the display through libvirt before it upgrades the
//! request, so a missing VM or a stopped one answers with a plain HTTP
//! status that the page can show. After the upgrade, the relay copies bytes
//! both ways. When either side closes, the relay drops both, and the socket
//! to QEMU closes with it.
//!
//! Login protection comes in Task 2.5. Until then the Origin check keeps
//! other web pages out, as on `/ws/events`.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

use crate::server::AppState;
use crate::ws::same_origin;

/// The largest chunk read from QEMU at once.
const CHUNK: usize = 64 * 1024;

pub async fn vnc(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(virt) = state.host.virt() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "libvirt is not connected").into_response();
    };
    let socket = match virt.open_vnc(id).await {
        Ok(socket) => socket,
        Err(e) if e.is_not_found() => return StatusCode::NOT_FOUND.into_response(),
        // For example a VM that is not running, or one with no VNC display.
        Err(e) => return (StatusCode::CONFLICT, e.to_string()).into_response(),
    };
    let socket = match socket
        .set_nonblocking(true)
        .and_then(|()| tokio::net::UnixStream::from_std(socket))
    {
        Ok(socket) => socket,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    // noVNC may ask for the "binary" subprotocol.
    upgrade
        .protocols(["binary"])
        .on_upgrade(move |ws| relay(ws, socket))
}

/// Copies bytes between the WebSocket and the VNC socket until one side
/// closes, then drops both.
pub(crate) async fn relay<S>(ws: WebSocket, socket: S)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut from_vm, mut to_vm) = tokio::io::split(socket);
    let (mut to_browser, mut from_browser) = ws.split();

    let browser_to_vm = async {
        while let Some(Ok(message)) = from_browser.next().await {
            match message {
                Message::Binary(bytes) => {
                    if to_vm.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                // RFB is binary. Pings are answered by axum.
                _ => {}
            }
        }
    };
    let vm_to_browser = async {
        let mut buf = vec![0; CHUNK];
        loop {
            match from_vm.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let bytes = buf[..n].to_vec().into();
                    if to_browser.send(Message::Binary(bytes)).await.is_err() {
                        return;
                    }
                }
            }
        }
        // The VM side ended: tell the browser.
        let _ = to_browser.send(Message::Close(None)).await;
    };
    tokio::select! {
        () = browser_to_vm => {}
        () = vm_to_browser => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::Router;
    use axum::extract::ws::WebSocketUpgrade;
    use axum::routing::get;
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;
    use tokio_tungstenite::tungstenite::Message;

    use super::relay;

    /// Serves one relay whose VM side is `vm`, and returns its address.
    async fn serve(vm: UnixStream) -> String {
        let vm = Arc::new(Mutex::new(Some(vm)));
        let app = Router::new().route(
            "/relay",
            get(move |upgrade: WebSocketUpgrade| {
                let vm = vm.lock().unwrap().take().expect("one client only");
                async move { upgrade.on_upgrade(move |ws| relay(ws, vm)) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move { axum::serve(listener, app).await });
        addr
    }

    const GREETING: &[u8] = b"RFB 003.008\n";

    #[tokio::test]
    async fn bytes_pass_both_ways() {
        // `peer` plays QEMU's VNC server.
        let (vm, mut peer) = UnixStream::pair().unwrap();
        let addr = serve(vm).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/relay"))
            .await
            .unwrap();

        peer.write_all(GREETING).await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(got, Message::Binary(GREETING.to_vec().into()));

        ws.send(Message::Binary(GREETING.to_vec().into()))
            .await
            .unwrap();
        let mut buf = [0; GREETING.len()];
        tokio::time::timeout(Duration::from_secs(5), peer.read_exact(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf, GREETING);
    }

    #[tokio::test]
    async fn closing_the_browser_side_closes_the_vm_socket() {
        let (vm, mut peer) = UnixStream::pair().unwrap();
        let addr = serve(vm).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/relay"))
            .await
            .unwrap();
        ws.close(None).await.unwrap();
        // A read of 0 bytes means that the relay dropped its end.
        let mut buf = [0; 16];
        let n = tokio::time::timeout(Duration::from_secs(5), peer.read(&mut buf))
            .await
            .expect("the relay kept the VM socket open")
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn a_closed_vm_side_closes_the_websocket() {
        let (vm, peer) = UnixStream::pair().unwrap();
        let addr = serve(vm).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/relay"))
            .await
            .unwrap();
        drop(peer);
        let end = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await;
        assert!(end.is_ok(), "the WebSocket stayed open");
    }

    #[tokio::test]
    async fn text_messages_are_ignored() {
        let (vm, mut peer) = UnixStream::pair().unwrap();
        let addr = serve(vm).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/relay"))
            .await
            .unwrap();
        ws.send(Message::Text("not rfb".into())).await.unwrap();
        ws.send(Message::Binary(GREETING.to_vec().into()))
            .await
            .unwrap();
        // Only the binary message reaches the VM.
        let mut buf = [0; GREETING.len()];
        tokio::time::timeout(Duration::from_secs(5), peer.read_exact(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf, GREETING);
    }
}
