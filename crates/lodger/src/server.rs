//! The HTTP server: the API, the WebSocket, reserved prefixes, then the
//! embedded web UI.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use axum::routing::{any, get};
use lodger_virt::Host;

use crate::assets::{self, Embedded};
use crate::{api, ws};

/// What every handler can reach.
#[derive(Debug, Clone)]
pub struct AppState {
    pub host: Arc<Host>,
}

/// Builds the router. Paths under `/api` and `/ws` without a handler answer
/// 404, and they never fall through to the web UI.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/host", get(api::host))
        .route("/api/vms", get(api::vms))
        .route("/api/vms/{id}", get(api::vm))
        .route("/ws/events", get(ws::events))
        .route("/api", any(reserved))
        .route("/api/{*rest}", any(reserved))
        .route("/ws", any(reserved))
        .route("/ws/{*rest}", any(reserved))
        .fallback(web_ui)
        .with_state(state)
}

async fn reserved() -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn web_ui(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    assets::respond(&Embedded, &method, uri.path(), &headers)
}

/// Starts the libvirt supervisor for `uri`, binds `listen`, prints the bound
/// address, and serves until Ctrl-C or SIGTERM. A libvirt that cannot be
/// reached does not stop the server: the API reports it as disconnected.
pub async fn serve(listen: SocketAddr, uri: &str) -> Result<(), String> {
    let host = Host::start(uri).map_err(|e| format!("cannot use {uri:?}: {e}"))?;
    let state = AppState {
        host: Arc::new(host),
    };
    let fail = |e: std::io::Error| format!("cannot serve on {listen}: {e}");
    let listener = tokio::net::TcpListener::bind(listen).await.map_err(fail)?;
    println!(
        "lodger listening on http://{}",
        listener.local_addr().map_err(fail)?
    );
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(fail)
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install the SIGTERM handler");
    tokio::select! {
        _ = ctrl_c => {}
        _ = term.recv() => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use futures_util::StreamExt;
    use lodger_virt::{ConnState, Host, Virt};
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::tungstenite::Message;

    use super::{AppState, router};
    use crate::ws::Update;

    const TEST_URI: &str = "test:///default";

    /// Serves the router on a free local port and waits until the host is
    /// connected.
    async fn serve() -> (String, Arc<Host>) {
        serve_uri(TEST_URI).await
    }

    async fn serve_uri(uri: &str) -> (String, Arc<Host>) {
        let host = Arc::new(Host::start(uri).unwrap());
        let mut state = host.watch_state();
        tokio::time::timeout(
            Duration::from_secs(5),
            state.wait_for(|s| *s == ConnState::Connected),
        )
        .await
        .unwrap()
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let app = router(AppState {
            host: Arc::clone(&host),
        });
        tokio::spawn(async move { axum::serve(listener, app).await });
        (addr, host)
    }

    /// A plain HTTP/1.1 GET. Returns the status code and the body.
    async fn get(addr: &str, path: &str) -> (u16, String) {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        let status = out[9..12].parse().unwrap();
        let body = out.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        (status, body.to_string())
    }

    /// A test driver loaded from a file. It gives each connection a state
    /// of its own, so no other test can change what these tests compare.
    /// `Drop` removes the file, also after a failed assertion.
    struct PrivateDriver(std::path::PathBuf);

    impl PrivateDriver {
        fn new(test: &str, domains: &[&str]) -> Self {
            let path =
                std::env::temp_dir().join(format!("lodger-{test}-{}.xml", std::process::id()));
            let body: String = domains.iter().map(|d| domain_xml(d)).collect();
            std::fs::write(&path, format!("<node>{body}</node>")).unwrap();
            Self(path)
        }

        fn uri(&self) -> String {
            format!("test://{}", self.0.display())
        }
    }

    impl Drop for PrivateDriver {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn domain_xml(name: &str) -> String {
        format!(
            "<domain type='test'><name>{name}</name><memory>65536</memory>\
             <os><type>hvm</type></os></domain>"
        )
    }

    #[tokio::test]
    async fn vms_lists_every_domain_with_its_state() {
        let driver = PrivateDriver::new("list", &["alpha", "beta"]);
        let (addr, host) = serve_uri(&driver.uri()).await;
        let (status, body) = get(&addr, "/api/vms").await;
        assert_eq!(status, 200, "{body}");
        let listed: Vec<Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(listed.len(), host.inventory().vms.len());
        for vm in host.inventory().vms.values() {
            let row = listed
                .iter()
                .find(|row| row["uuid"] == vm.uuid.to_string())
                .unwrap_or_else(|| panic!("{} is missing", vm.name));
            assert_eq!(row, &serde_json::to_value(vm).unwrap());
        }
        // Sorted by name. The test driver starts file domains as running.
        let names: Vec<_> = listed.iter().map(|row| &row["name"]).collect();
        assert_eq!(names, ["alpha", "beta"]);
        assert!(listed.iter().all(|row| row["state"] == "running"));
    }

    #[tokio::test]
    async fn one_vm_by_uuid() {
        let driver = PrivateDriver::new("one", &["alpha"]);
        let (addr, host) = serve_uri(&driver.uri()).await;
        let vm = host.inventory().vms.into_values().next().unwrap();
        let (status, body) = get(&addr, &format!("/api/vms/{}", vm.uuid)).await;
        assert_eq!(status, 200);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            serde_json::to_value(&vm).unwrap()
        );
        let missing = "/api/vms/00000000-0000-0000-0000-000000000000";
        assert_eq!(get(&addr, missing).await.0, 404);
        assert_eq!(get(&addr, "/api/vms/not-a-uuid").await.0, 400);
    }

    #[tokio::test]
    async fn host_reports_the_connection_and_the_counts() {
        let driver = PrivateDriver::new("host", &["alpha", "beta"]);
        let (addr, _host) = serve_uri(&driver.uri()).await;
        let (status, body) = get(&addr, "/api/host").await;
        assert_eq!(status, 200);
        let json: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            json["connection"],
            serde_json::json!({"state": "connected"})
        );
        assert_eq!(json["info"]["cpus"], 16);
        assert_eq!(json["vms"], serde_json::json!({"total": 2, "running": 2}));
        assert_eq!(json["pools"], 0);
        assert_eq!(json["networks"], 0);
    }

    #[tokio::test]
    async fn unknown_api_and_ws_paths_stay_404() {
        let (addr, _host) = serve().await;
        assert_eq!(get(&addr, "/api/missing").await.0, 404);
        assert_eq!(get(&addr, "/ws/missing").await.0, 404);
        assert_eq!(get(&addr, "/api").await.0, 404);
    }

    #[tokio::test]
    async fn a_lifecycle_event_reaches_a_websocket_client() {
        let (addr, _host) = serve().await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
            .await
            .unwrap();

        let outside = Virt::open(TEST_URI).await.unwrap();
        let name = "lodger-spike-ws-lifecycle";
        let id = outside
            .job(move |c| {
                let domain = c.define_domain_xml(&domain_xml(name))?;
                domain.create()?;
                domain.uuid()
            })
            .await
            .unwrap();

        let want = serde_json::to_value(Update::Vm { id }).unwrap();
        let start = Instant::now();
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("no message within 5 seconds")
                .unwrap()
                .unwrap();
            if let Message::Text(text) = msg
                && serde_json::from_str::<Value>(&text).unwrap() == want
            {
                break;
            }
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        outside
            .job(move |c| {
                let domain = c.lookup_domain_by_name(name)?;
                domain.destroy()?;
                domain.undefine()
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_page_from_another_origin_cannot_open_the_socket() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let (addr, _host) = serve().await;
        let mut req = format!("ws://{addr}/ws/events")
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("origin", "http://evil.example".parse().unwrap());
        let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
        let tokio_tungstenite::tungstenite::Error::Http(resp) = err else {
            panic!("expected an HTTP refusal, got {err:?}");
        };
        assert_eq!(resp.status(), 403);

        let mut req = format!("ws://{addr}/ws/events")
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("origin", format!("http://{addr}").parse().unwrap());
        assert!(tokio_tungstenite::connect_async(req).await.is_ok());
    }

    #[tokio::test]
    async fn the_server_ignores_client_text_and_ends_on_close() {
        use futures_util::SinkExt;
        let (addr, _host) = serve().await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
            .await
            .unwrap();
        ws.send(Message::Text("hello".into())).await.unwrap();
        ws.send(Message::Close(None)).await.unwrap();
        // The server answers the close and ends the stream. Events from the
        // other tests, which share the test driver, may arrive first.
        let end = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(Ok(_)) = ws.next().await {}
        })
        .await;
        assert!(end.is_ok(), "the server did not close the socket");
    }

    /// A client that reads nothing must not slow the server down. Each
    /// client has its own receiver, so the flood below only fills that
    /// client's socket, and the API keeps answering. How many events reach
    /// the hub depends on timing, so the `resync` for a client that falls
    /// behind is tested on `update_for` in `ws.rs`.
    ///
    /// The flood uses a test driver loaded from a file, which has state of
    /// its own. The other tests share `test:///default`, and 4000 events
    /// there would make their clients fall behind too.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_client_that_reads_nothing_does_not_block_the_server() {
        let driver = PrivateDriver::new("flood", &[]);
        let (addr, host) = serve_uri(&driver.uri()).await;
        let (_stuck, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
            .await
            .unwrap();
        // A private driver gives each connection its own state, so the flood
        // runs on the host's read connection, the one with the callbacks.
        let virt = host.virt().unwrap();
        let flood = virt.read(|c| {
            let domain = c.define_domain_xml(&domain_xml("lodger-spike-ws-flood"))?;
            for _ in 0..2000 {
                domain.create()?;
                domain.destroy()?;
            }
            domain.undefine()
        });
        let api = async {
            let mut slowest = Duration::ZERO;
            for _ in 0..20 {
                let start = Instant::now();
                assert_eq!(get(&addr, "/api/vms").await.0, 200);
                slowest = slowest.max(start.elapsed());
            }
            slowest
        };
        let (flooded, slowest) = tokio::join!(flood, api);
        flooded.unwrap();
        assert!(
            slowest < Duration::from_secs(1),
            "slowest API answer: {slowest:?}"
        );
    }
}
