//! The HTTP server: the API, the WebSocket, reserved prefixes, then the
//! embedded web UI.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware;
use axum::response::Response;
use axum::routing::{any, delete, get, post};
use ipnet::IpNet;
use lodger_virt::Host;

use crate::assets::{self, AssetSource, Embedded};
use crate::config::Config;
use crate::db::Db;
use crate::setup::{self, Setup, SetupToken};
use crate::stats::Stats;
use crate::throttle::Throttle;
use crate::tickets::Tickets;
use crate::{accounts, actions, api, auth, console, networks, pools, security, volumes, ws};

/// What every handler can reach.
#[derive(Debug, Clone)]
pub struct AppState {
    pub host: Arc<Host>,
    pub db: Db,
    pub setup: Arc<Setup>,
    pub throttle: Arc<Throttle>,
    /// Proxies whose forwarded client address Lodger believes (TAD 7.5).
    pub trusted_proxies: Arc<Vec<IpNet>>,
    /// Single-use WebSocket tickets.
    pub tickets: Arc<Tickets>,
    /// The origin of `public_url`, for the Origin check (TAD 7.4).
    pub public_origin: Option<Arc<str>>,
    /// The Content-Security-Policy for every response.
    pub csp: Arc<HeaderValue>,
    /// Live VM stats for the sockets that subscribe.
    pub stats: Stats,
}

impl AppState {
    pub fn new(
        host: Arc<Host>,
        db: Db,
        setup: Option<SetupToken>,
        trusted_proxies: Vec<IpNet>,
        public_origin: Option<String>,
    ) -> Self {
        let page = Embedded.get(assets::FALLBACK).map_or_else(
            || assets::STUB_PAGE.to_owned(),
            |a| String::from_utf8_lossy(&a.bytes).into_owned(),
        );
        Self {
            csp: Arc::new(security::csp(&page, public_origin.as_deref())),
            public_origin: public_origin.map(Arc::from),
            stats: Stats::new(Arc::clone(&host), crate::stats::PERIOD),
            host,
            db,
            setup: Arc::new(std::sync::Mutex::new(setup)),
            throttle: Arc::default(),
            tickets: Arc::default(),
            trusted_proxies: Arc::new(trusted_proxies),
        }
    }
}

/// Builds the router. Paths under `/api` and `/ws` without a handler answer
/// 404, and they never fall through to the web UI.
///
/// Every API route needs a live session (TAD 7.2), except health, setup,
/// and login. New routes go into `protected`, so the guard is the default.
/// The guard also checks `X-CSRF-Token` on state-changing requests. The
/// WebSocket routes check the Origin, the session, and a ticket themselves
/// (`auth::open_socket`), because an upgrade is a GET.
///
/// Around everything: state-changing requests must come from a Lodger page, and
/// every response gets the security headers (`security.rs`).
pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/host", get(api::host))
        .route("/api/vms", get(api::vms))
        .route(
            "/api/vms/{id}",
            get(api::vm).delete(actions::delete).patch(actions::update),
        )
        .route("/api/vms/{id}/actions/{action}", post(actions::run))
        .route("/api/pools", get(pools::list).post(pools::create))
        .route(
            "/api/pools/{id}",
            get(pools::detail)
                .patch(pools::change)
                .delete(pools::remove),
        )
        .route(
            "/api/pools/{id}/volumes",
            get(volumes::list).post(volumes::create),
        )
        .route("/api/pools/{id}/volumes/{name}", delete(volumes::remove))
        .route("/api/networks", get(networks::list).post(networks::create))
        .route(
            "/api/networks/{id}",
            get(networks::detail)
                .patch(networks::change)
                .delete(networks::remove),
        )
        .route("/api/host-bridges", get(networks::host_bridges))
        .route("/api/session", get(auth::current).delete(auth::logout))
        .route("/api/ws-tickets", post(auth::issue_ticket))
        .route("/api/accounts", get(accounts::list).post(accounts::create))
        .route("/api/accounts/{id}", delete(accounts::delete))
        .route("/api/account/password", post(accounts::change_password))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_session,
        ));
    Router::new()
        .route("/api/health", get(api::health))
        .route("/api/setup", get(setup::status).post(setup::claim))
        .route("/api/session", post(auth::login))
        .merge(protected)
        .route("/ws/events", get(ws::events))
        .route("/ws/vms/{id}/vnc", get(console::vnc))
        .route("/api", any(reserved))
        .route("/api/{*rest}", any(reserved))
        .route("/ws", any(reserved))
        .route("/ws/{*rest}", any(reserved))
        .fallback(web_ui)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::require_same_origin,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::add_headers,
        ))
        .with_state(state)
}

async fn reserved() -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn web_ui(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    assets::respond(&Embedded, &method, uri.path(), &headers)
}

/// Opens the database, starts the libvirt supervisor, binds the listen
/// address, prints it, and serves until Ctrl-C or SIGTERM. A libvirt that
/// cannot be reached does not stop the server: the API reports it as
/// disconnected. A database that cannot be opened does.
pub async fn serve(config: Config) -> Result<(), String> {
    // First, so that a bad certificate stops the start before anything else.
    let tls = config
        .tls
        .as_ref()
        .map(crate::tls::server_config)
        .transpose()?;
    let db = Db::open(&config.state_dir).await?;
    tokio::task::spawn_blocking(crate::passwords::prepare)
        .await
        .map_err(|e| format!("cannot prepare the password check: {e}"))?;
    let setup = open_setup(&db).await?;
    let host = Host::start(&config.uri).map_err(|e| format!("cannot use {:?}: {e}", config.uri))?;
    tokio::spawn(crate::audit::delete_old_rows(
        db.clone(),
        std::time::Duration::from_secs(24 * 60 * 60),
    ));
    let state = AppState::new(
        Arc::new(host),
        db,
        setup,
        config.trusted_proxies.clone(),
        config.public_url.clone(),
    );
    let listen = config.listen;
    let fail = |e: std::io::Error| format!("cannot serve on {listen}: {e}");
    let listener = tokio::net::TcpListener::bind(listen).await.map_err(fail)?;
    // The handlers must exist before the address line: a supervisor or a test
    // may send SIGTERM as soon as it reads that line.
    let shutdown =
        shutdown_signal().map_err(|e| format!("cannot install the signal handlers: {e}"))?;
    // stdout carries only the address line, which scripts and tests read.
    // Everything else goes to stderr, which systemd sends to the journal.
    let scheme = if tls.is_some() { "https" } else { "http" };
    println!(
        "lodger listening on {scheme}://{}",
        listener.local_addr().map_err(fail)?
    );
    eprintln!("{}", summary(&config));
    if !listen.ip().is_loopback() && tls.is_none() {
        // TAD section 7.4: without TLS, passwords and session cookies would
        // cross the network in clear text. Browsers also keep the Secure
        // session cookie only over HTTPS or loopback.
        eprintln!(
            "lodger: WARNING: listening on {listen}, which is not loopback, without TLS. Set \
             tls_cert and tls_key, or put Lodger behind a reverse proxy with TLS and listen on \
             127.0.0.1 instead."
        );
    }
    // The TCP peer's address, which the login's client-IP rule needs.
    let app = router(state).into_make_service_with_connect_info::<SocketAddr>();
    match tls {
        Some(tls) => {
            use axum::serve::ListenerExt;
            // `tap_io` does nothing to the stream. axum gives a custom
            // listener the peer address for `ConnectInfo` only in this wrapper.
            let listener = crate::tls::TlsListener::new(listener, tls)
                .map_err(fail)?
                .tap_io(|_| {});
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown)
                .await
        }
        None => {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown)
                .await
        }
    }
    .map_err(fail)
}

/// Makes a setup token when no account exists and writes it to the log.
async fn open_setup(db: &Db) -> Result<Option<SetupToken>, String> {
    if db.account_count().await? > 0 {
        return Ok(None);
    }
    let (text, token) = SetupToken::generate(std::time::Instant::now())?;
    eprintln!(
        "lodger: setup token: {text} (open Lodger in a browser and enter it to create the first \
         account; it works once and for 60 minutes, and a restart writes a new one)"
    );
    Ok(Some(token))
}

/// One line that says which settings are in force.
fn summary(config: &Config) -> String {
    format!(
        "lodger: libvirt {}, state directory {}, public URL {}, {} trusted proxies, TLS {}",
        config.uri,
        config.state_dir.display(),
        config.public_url.as_deref().unwrap_or("not set"),
        config.trusted_proxies.len(),
        config
            .tls
            .as_ref()
            .map_or_else(|| "off".to_owned(), |t| t.cert.display().to_string())
    )
}

/// Installs the SIGINT and SIGTERM handlers now, and returns the future that
/// ends on the first of them. `tokio::signal::ctrl_c` and a handler created
/// inside the returned future would install only on the first poll, and
/// axum polls the shutdown future in a task of its own, maybe later. A
/// signal in between would kill the process without a clean shutdown.
fn shutdown_signal() -> std::io::Result<impl std::future::Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    })
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

    use uuid::Uuid;

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
        serve_host(host).await
    }

    /// The session cookie of each test server, by address. `get` and
    /// `post_json` send it, so tests of protected routes run logged in.
    static COOKIES: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, String>>,
    > = std::sync::LazyLock::new(Default::default);

    /// Serves `state` on a free local port, with the TCP peer address that
    /// the login needs.
    async fn start(state: AppState) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let app = router(state).into_make_service_with_connect_info::<std::net::SocketAddr>();
        tokio::spawn(async move { axum::serve(listener, app).await });
        addr
    }

    /// Creates an account and a session straight in the database, and
    /// returns the session cookie.
    async fn log_in_directly(state: &AppState) -> String {
        use sha2::Digest;
        assert!(
            state
                .db
                .create_first_account("tester".into(), "not a real hash".into())
                .await
                .unwrap()
        );
        let account = state
            .db
            .find_account("tester".into())
            .await
            .unwrap()
            .unwrap();
        let token = "f".repeat(64);
        state
            .db
            .create_session(crate::db::NewSession {
                token_sha256: sha2::Sha256::digest(token.as_bytes()).into(),
                account_id: account.id,
                csrf_token: "csrf".into(),
                client_ip: "127.0.0.1".into(),
                user_agent: None,
            })
            .await
            .unwrap();
        format!("{}={token}", crate::auth::COOKIE)
    }

    /// Serves the router for `host` as it is, connected or not, logged in.
    async fn serve_host(host: Arc<Host>) -> (String, Arc<Host>) {
        let state = AppState::new(
            Arc::clone(&host),
            crate::db::Db::in_memory().await,
            None,
            vec![],
            None,
        );
        let cookie = log_in_directly(&state).await;
        let addr = start(state).await;
        COOKIES.lock().unwrap().insert(addr.clone(), cookie);
        (addr, host)
    }

    /// Serves `test:///default` with setup open and no account. Returns the
    /// address, the token text, and the state.
    async fn serve_setup() -> (String, String, AppState) {
        serve_setup_at(None).await
    }

    /// [`serve_setup`] with a `public_url` origin.
    async fn serve_setup_at(public_origin: Option<&str>) -> (String, String, AppState) {
        let host = Arc::new(Host::start(TEST_URI).unwrap());
        let (text, token) = crate::setup::SetupToken::generate(std::time::Instant::now()).unwrap();
        let state = AppState::new(
            host,
            crate::db::Db::in_memory().await,
            Some(token),
            vec![],
            public_origin.map(str::to_owned),
        );
        let addr = start(state.clone()).await;
        (addr, text, state)
    }

    /// One HTTP/1.1 request. Returns the status and the whole response.
    async fn raw(
        addr: &str,
        method: &str,
        path: &str,
        headers: &[String],
        body: &str,
    ) -> (u16, String) {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
        for h in headers {
            req.push_str(h);
            req.push_str("\r\n");
        }
        req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        (out[9..12].parse().unwrap(), out)
    }

    fn body_of(response: &str) -> String {
        response
            .split_once("\r\n\r\n")
            .map(|(_, b)| b)
            .unwrap_or("")
            .to_string()
    }

    fn cookie_for(addr: &str) -> Vec<String> {
        COOKIES
            .lock()
            .unwrap()
            .get(addr)
            .map(|c| vec![format!("Cookie: {c}")])
            .unwrap_or_default()
    }

    /// A GET with the server's test session, if it has one.
    async fn get(addr: &str, path: &str) -> (u16, String) {
        let (status, out) = raw(addr, "GET", path, &cookie_for(addr), "").await;
        (status, body_of(&out))
    }

    /// A GET with the given cookie header, or none.
    async fn get_as(addr: &str, path: &str, cookie: Option<&str>) -> u16 {
        let headers: Vec<String> = cookie.map(|c| format!("Cookie: {c}")).into_iter().collect();
        raw(addr, "GET", path, &headers, "").await.0
    }

    /// What a browser sends on a request from a Lodger page.
    const SAME_ORIGIN: &str = "Sec-Fetch-Site: same-origin";

    /// A POST from a Lodger page with a JSON body and the server's test
    /// session, if any.
    async fn post_json(addr: &str, path: &str, body: &Value) -> (u16, String) {
        let mut headers = cookie_for(addr);
        headers.push("Content-Type: application/json".into());
        headers.push(SAME_ORIGIN.into());
        let (status, out) = raw(addr, "POST", path, &headers, &body.to_string()).await;
        (status, body_of(&out))
    }

    const GOOD_PASSWORD: &str = "correct horse battery staple";

    fn claim(token: &str, username: &str, password: &str) -> Value {
        serde_json::json!({"token": token, "username": username, "password": password})
    }

    #[tokio::test]
    async fn setup_without_the_token_creates_no_account() {
        let (addr, _token, state) = serve_setup().await;
        let wrong = claim("00000000000000000000000000000000", "admin", GOOD_PASSWORD);
        let (status, body) = post_json(&addr, "/api/setup", &wrong).await;
        assert_eq!(status, 403, "{body}");
        let missing = serde_json::json!({"username": "admin", "password": GOOD_PASSWORD});
        assert_eq!(post_json(&addr, "/api/setup", &missing).await.0, 422);
        assert_eq!(state.db.account_count().await.unwrap(), 0);
        // Setup stays open.
        assert_eq!(get(&addr, "/api/setup").await.0, 200);
    }

    #[tokio::test]
    async fn setup_refuses_a_page_from_another_origin() {
        let (addr, token, state) = serve_setup().await;
        let body = claim(&token, "admin", GOOD_PASSWORD).to_string();
        let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
        let req = format!(
            "POST /api/setup HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
             Origin: http://evil.example\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        assert!(out.starts_with("HTTP/1.1 403"), "{out}");
        assert_eq!(state.db.account_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn closed_setup_answers_404_whatever_the_body() {
        let (addr, _host) = serve().await;
        assert_eq!(
            post_json(&addr, "/api/setup", &serde_json::json!({}))
                .await
                .0,
            404
        );
        // No body and no Content-Type at all.
        let (status, out) = raw(&addr, "POST", "/api/setup", &[SAME_ORIGIN.into()], "").await;
        assert_eq!(status, 404, "{out}");
    }

    #[tokio::test]
    async fn a_weak_password_or_a_bad_name_creates_no_account() {
        let (addr, token, state) = serve_setup().await;
        let short = post_json(&addr, "/api/setup", &claim(&token, "admin", "too short")).await;
        assert_eq!(short.0, 422);
        assert!(short.1.contains("at least 15"), "{}", short.1);
        let common = post_json(
            &addr,
            "/api/setup",
            &claim(&token, "admin", "1q2w3e4r5t6y7u8i"),
        )
        .await;
        assert_eq!(common.0, 422);
        let bad = post_json(
            &addr,
            "/api/setup",
            &claim(&token, "bad name", GOOD_PASSWORD),
        )
        .await;
        assert_eq!(bad.0, 422);
        assert_eq!(state.db.account_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_first_claim_creates_the_account_and_closes_setup() {
        let (addr, token, state) = serve_setup().await;
        assert_eq!(get(&addr, "/api/setup").await.0, 200);
        let (status, body) =
            post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        assert_eq!(status, 201, "{body}");
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["username"],
            "admin"
        );
        assert_eq!(state.db.account_count().await.unwrap(), 1);
        // The token is used up, and setup is gone.
        assert_eq!(get(&addr, "/api/setup").await.0, 404);
        let again = post_json(&addr, "/api/setup", &claim(&token, "second", GOOD_PASSWORD)).await;
        assert_eq!(again.0, 404);
        assert_eq!(state.db.account_count().await.unwrap(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_claims_create_exactly_one_account() {
        let (addr, token, state) = serve_setup().await;
        let claims = (0..8).map(|n| {
            let addr = addr.clone();
            let body = claim(&token, &format!("admin{n}"), GOOD_PASSWORD);
            tokio::spawn(async move { post_json(&addr, "/api/setup", &body).await.0 })
        });
        let mut statuses = Vec::new();
        for c in claims {
            statuses.push(c.await.unwrap());
        }
        statuses.sort_unstable();
        assert_eq!(
            statuses.iter().filter(|s| **s == 201).count(),
            1,
            "{statuses:?}"
        );
        assert!(
            statuses.iter().all(|s| *s == 201 || *s == 404),
            "{statuses:?}"
        );
        assert_eq!(state.db.account_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn an_expired_token_fails() {
        let (addr, _, state) = serve_setup().await;
        let past = std::time::Instant::now()
            .checked_sub(crate::setup::TOKEN_LIFETIME + std::time::Duration::from_secs(1))
            .unwrap();
        let (text, token) = crate::setup::SetupToken::generate(past).unwrap();
        *state.setup.lock().unwrap() = Some(token);
        let (status, body) =
            post_json(&addr, "/api/setup", &claim(&text, "admin", GOOD_PASSWORD)).await;
        assert_eq!(status, 403);
        assert!(body.contains("expired"), "{body}");
        assert_eq!(get(&addr, "/api/setup").await.0, 404);
        assert_eq!(state.db.account_count().await.unwrap(), 0);
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
    async fn health_answers_with_fixed_words() {
        let (addr, _host) = serve().await;
        let (status, body) = get(&addr, "/api/health").await;
        assert_eq!(status, 200);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            serde_json::json!({"status": "ok", "database": "ok", "libvirt": "connected"})
        );
    }

    #[tokio::test]
    async fn health_is_degraded_while_libvirt_is_down() {
        let missing = std::env::temp_dir().join("lodger-no-such-driver.xml");
        let host = Arc::new(Host::start(&format!("test://{}", missing.display())).unwrap());
        let (addr, _host) = serve_host(host).await;
        let (status, body) = get(&addr, "/api/health").await;
        assert_eq!(status, 200);
        let json: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["status"], "degraded");
        // No error text: the endpoint needs no login.
        assert!(json["libvirt"] == "connecting" || json["libvirt"] == "disconnected");
        assert!(!body.contains("lodger-no-such-driver"), "{body}");
    }

    #[tokio::test]
    async fn protected_endpoints_answer_401_without_a_session() {
        let (addr, host) = serve().await;
        // The built-in domain: other tests add and remove their own.
        let vm = host
            .inventory()
            .vms
            .into_values()
            .find(|vm| vm.name == "test")
            .unwrap();
        let forged = format!("{}={}", crate::auth::COOKIE, "0".repeat(64));
        for path in [
            "/api/host",
            "/api/vms",
            &format!("/api/vms/{}", vm.uuid),
            "/api/session",
        ] {
            assert_eq!(get_as(&addr, path, None).await, 401, "{path}");
            assert_eq!(get_as(&addr, path, Some(&forged)).await, 401, "{path}");
            // The test session works.
            assert_eq!(get(&addr, path).await.0, 200, "{path}");
        }
        // Open without a session: health, setup (closed here), and the app.
        assert_eq!(get_as(&addr, "/api/health", None).await, 200);
        assert_eq!(get_as(&addr, "/api/setup", None).await, 404);
        assert_eq!(get_as(&addr, "/", None).await, 200);
    }

    /// Logs in through the API. Returns the status, the cookie, and the body.
    async fn login(addr: &str, username: &str, password: &str) -> (u16, Option<String>, String) {
        let body = serde_json::json!({"username": username, "password": password}).to_string();
        let (status, out) = raw(
            addr,
            "POST",
            "/api/session",
            &["Content-Type: application/json".into(), SAME_ORIGIN.into()],
            &body,
        )
        .await;
        let cookie = out
            .lines()
            .find_map(|l| l.strip_prefix("set-cookie: "))
            .and_then(|v| v.split(';').next())
            .map(str::to_owned);
        (status, cookie, body_of(&out))
    }

    #[tokio::test]
    async fn login_then_logout_ends_the_session_everywhere() {
        let (addr, token, _state) = serve_setup().await;
        let created = post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        assert_eq!(created.0, 201);

        let (status, cookie, body) = login(&addr, "ADMIN", GOOD_PASSWORD).await;
        assert_eq!(status, 200, "{body}");
        let cookie = cookie.expect("the login sets the cookie");
        assert!(cookie.starts_with("__Host-lodger_sid="), "{cookie}");
        let info: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(info["username"], "admin");
        assert_eq!(info["csrf_token"].as_str().unwrap().len(), 64);

        for path in ["/api/session", "/api/host", "/api/vms"] {
            assert_eq!(get_as(&addr, path, Some(&cookie)).await, 200, "{path}");
        }
        let csrf = info["csrf_token"].as_str().unwrap();
        let (status, out) = raw(
            &addr,
            "DELETE",
            "/api/session",
            &[
                format!("Cookie: {cookie}"),
                SAME_ORIGIN.into(),
                format!("X-CSRF-Token: {csrf}"),
            ],
            "",
        )
        .await;
        assert_eq!(status, 204);
        assert!(out.to_lowercase().contains("max-age=0"), "{out}");
        // The same cookie now fails on every endpoint at once.
        for path in ["/api/session", "/api/host", "/api/vms"] {
            assert_eq!(get_as(&addr, path, Some(&cookie)).await, 401, "{path}");
        }
    }

    #[tokio::test]
    async fn a_wrong_password_and_an_unknown_user_get_the_same_answer() {
        let (addr, token, _state) = serve_setup().await;
        post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        let wrong = login(&addr, "admin", "correct horse battery stable").await;
        let nobody = login(&addr, "nobody", GOOD_PASSWORD).await;
        assert_eq!((wrong.0, nobody.0), (401, 401));
        assert_eq!(wrong.2, nobody.2);
        assert!(wrong.1.is_none() && nobody.1.is_none());
    }

    #[tokio::test]
    async fn an_unsafe_request_from_another_origin_fails_with_403() {
        let (addr, token, _state) = serve_setup().await;
        let setup = claim(&token, "admin", GOOD_PASSWORD).to_string();
        let login_body =
            serde_json::json!({"username": "admin", "password": GOOD_PASSWORD}).to_string();
        let json = "Content-Type: application/json".to_string();
        let refused = [
            // No browser header at all, as from a plain script.
            vec![],
            vec!["Origin: http://evil.example".into()],
            // A sibling subdomain: the same site, but not the same origin.
            vec![
                "Sec-Fetch-Site: same-site".into(),
                "Origin: https://evil.lodger.lan".into(),
            ],
            vec!["Sec-Fetch-Site: cross-site".into()],
            // Origin equal to Host is not enough: Host can be forged.
            vec![format!("Origin: http://{addr}")],
        ];
        for extra in &refused {
            let mut headers = vec![json.clone()];
            headers.extend(extra.iter().cloned());
            for (path, body) in [("/api/setup", &setup), ("/api/session", &login_body)] {
                let (status, _) = raw(&addr, "POST", path, &headers, body).await;
                assert_eq!(status, 403, "{path} with {extra:?}");
            }
        }
        // Nothing ran: setup is still open.
        assert_eq!(get_as(&addr, "/api/setup", None).await, 200);
    }

    #[tokio::test]
    async fn an_origin_equal_to_public_url_passes() {
        let (addr, token, _state) = serve_setup_at(Some("https://lodger.lan")).await;
        let (status, _) = raw(
            &addr,
            "POST",
            "/api/setup",
            &[
                "Content-Type: application/json".into(),
                "Origin: https://lodger.lan".into(),
            ],
            &claim(&token, "admin", GOOD_PASSWORD).to_string(),
        )
        .await;
        assert_eq!(status, 201);
    }

    #[tokio::test]
    async fn logout_needs_the_csrf_token_of_its_own_session() {
        let (addr, token, _state) = serve_setup().await;
        post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        let (_, cookie, _) = login(&addr, "admin", GOOD_PASSWORD).await;
        let cookie = cookie.unwrap();
        // A second session's token does not fit the first session.
        let (_, _, other) = login(&addr, "admin", GOOD_PASSWORD).await;
        let other: Value = serde_json::from_str(&other).unwrap();
        let other_csrf = other["csrf_token"].as_str().unwrap();
        for csrf in [None, Some("0".repeat(64)), Some(other_csrf.to_owned())] {
            let mut headers = vec![format!("Cookie: {cookie}"), SAME_ORIGIN.into()];
            headers.extend(csrf.map(|t| format!("X-CSRF-Token: {t}")));
            let (status, _) = raw(&addr, "DELETE", "/api/session", &headers, "").await;
            assert_eq!(status, 403, "{headers:?}");
        }
        assert_eq!(get_as(&addr, "/api/session", Some(&cookie)).await, 200);
    }

    #[tokio::test]
    async fn every_response_carries_the_security_headers() {
        let (addr, _host) = serve().await;
        for (method, path) in [
            ("GET", "/"),
            ("GET", "/api/health"),
            ("GET", "/api/missing"),
            ("POST", "/api/session"),
        ] {
            let (_, out) = raw(&addr, method, path, &[], "").await;
            let out = out.to_lowercase();
            for header in [
                "content-security-policy: default-src 'self'; script-src 'self'",
                "referrer-policy: no-referrer",
                "x-content-type-options: nosniff",
            ] {
                assert!(out.contains(header), "{method} {path} lacks {header}");
            }
            assert!(out.contains("frame-ancestors 'none'"), "{method} {path}");
        }
    }

    /// A logged-in browser tab: its cookie and CSRF token.
    struct Tab {
        cookie: String,
        csrf: String,
    }

    async fn log_in_tab(addr: &str, username: &str, password: &str) -> Tab {
        let (status, cookie, body) = login(addr, username, password).await;
        assert_eq!(status, 200, "{body}");
        let info: Value = serde_json::from_str(&body).unwrap();
        Tab {
            cookie: cookie.unwrap(),
            csrf: info["csrf_token"].as_str().unwrap().to_owned(),
        }
    }

    /// One API call from `tab`, as a Lodger page makes it.
    async fn call(
        addr: &str,
        tab: &Tab,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut headers = vec![
            format!("Cookie: {}", tab.cookie),
            SAME_ORIGIN.into(),
            format!("X-CSRF-Token: {}", tab.csrf),
        ];
        if body.is_some() {
            headers.push("Content-Type: application/json".into());
        }
        let text = body.map(|b| b.to_string()).unwrap_or_default();
        let (status, out) = raw(addr, method, path, &headers, &text).await;
        let body = body_of(&out);
        (status, serde_json::from_str(&body).unwrap_or(Value::Null))
    }

    /// A server with the account `admin` and one logged-in tab for it.
    async fn serve_admin() -> (String, Tab) {
        let (addr, token, _state) = serve_setup().await;
        let created = post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        assert_eq!(created.0, 201);
        let tab = log_in_tab(&addr, "admin", GOOD_PASSWORD).await;
        (addr, tab)
    }

    const OTHER_PASSWORD: &str = "a different good passphrase";

    #[tokio::test]
    async fn a_common_or_short_password_fails_with_a_clear_message() {
        let (addr, tab) = serve_admin().await;
        // On the list (`common.txt` line 1), in another case: the check
        // ignores case.
        let common = "DEMON1Q2W3E4R5T";
        for (path, body) in [
            (
                "/api/accounts",
                serde_json::json!({"username": "second", "password": common}),
            ),
            (
                "/api/account/password",
                serde_json::json!({"current_password": GOOD_PASSWORD, "new_password": common}),
            ),
        ] {
            let (status, answer) = call(&addr, &tab, "POST", path, Some(body)).await;
            assert_eq!(status, 422, "{path}");
            assert_eq!(
                answer["error"],
                "the password is on the list of the most common passwords. Choose another one",
                "{path}"
            );
        }
        let short = serde_json::json!({"username": "second", "password": "too short"});
        let (status, answer) = call(&addr, &tab, "POST", "/api/accounts", Some(short)).await;
        assert_eq!(status, 422);
        assert!(
            answer["error"].as_str().unwrap().contains("at least 15"),
            "{answer}"
        );
    }

    #[tokio::test]
    async fn a_second_account_logs_in_with_full_rights() {
        let (addr, tab) = serve_admin().await;
        let new = serde_json::json!({"username": "second", "password": OTHER_PASSWORD});
        let (status, created) = call(&addr, &tab, "POST", "/api/accounts", Some(new.clone())).await;
        assert_eq!(status, 201, "{created}");
        // The name is unique without regard to case.
        let again = serde_json::json!({"username": "SECOND", "password": OTHER_PASSWORD});
        assert_eq!(
            call(&addr, &tab, "POST", "/api/accounts", Some(again))
                .await
                .0,
            409
        );

        let second = log_in_tab(&addr, "second", OTHER_PASSWORD).await;
        assert_eq!(call(&addr, &second, "GET", "/api/vms", None).await.0, 200);
        let (_, list) = call(&addr, &second, "GET", "/api/accounts", None).await;
        let names: Vec<(&str, bool)> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|a| (a["username"].as_str().unwrap(), a["you"].as_bool().unwrap()))
            .collect();
        assert_eq!(names, [("admin", false), ("second", true)]);
        assert!(list[0].get("password_hash").is_none(), "{list}");
    }

    #[tokio::test]
    async fn deleting_the_last_account_fails_and_a_delete_ends_its_sessions() {
        let (addr, tab) = serve_admin().await;
        let (_, list) = call(&addr, &tab, "GET", "/api/accounts", None).await;
        let admin_id = list[0]["id"].as_i64().unwrap();
        let (status, answer) = call(
            &addr,
            &tab,
            "DELETE",
            &format!("/api/accounts/{admin_id}"),
            None,
        )
        .await;
        assert_eq!(status, 409);
        assert!(
            answer["error"].as_str().unwrap().contains("last account"),
            "{answer}"
        );
        assert_eq!(call(&addr, &tab, "GET", "/api/vms", None).await.0, 200);

        let new = serde_json::json!({"username": "second", "password": OTHER_PASSWORD});
        let (_, created) = call(&addr, &tab, "POST", "/api/accounts", Some(new)).await;
        let second_id = created["id"].as_i64().unwrap();
        let second = log_in_tab(&addr, "second", OTHER_PASSWORD).await;
        let path = format!("/api/accounts/{second_id}");
        assert_eq!(call(&addr, &tab, "DELETE", &path, None).await.0, 204);
        // ASVS 7.4.2: the deleted account's session ends at once.
        assert_eq!(call(&addr, &second, "GET", "/api/vms", None).await.0, 401);
        assert_eq!(call(&addr, &tab, "DELETE", &path, None).await.0, 404);
    }

    #[tokio::test]
    async fn a_password_change_needs_the_current_password_and_ends_other_sessions() {
        let (addr, tab) = serve_admin().await;
        let other_tab = log_in_tab(&addr, "admin", GOOD_PASSWORD).await;

        let wrong = serde_json::json!({
            "current_password": "not the current one",
            "new_password": OTHER_PASSWORD,
        });
        let (status, answer) =
            call(&addr, &tab, "POST", "/api/account/password", Some(wrong)).await;
        assert_eq!(status, 403, "{answer}");
        // Nothing changed: both sessions still work.
        assert_eq!(
            call(&addr, &other_tab, "GET", "/api/vms", None).await.0,
            200
        );

        let right = serde_json::json!({
            "current_password": GOOD_PASSWORD,
            "new_password": OTHER_PASSWORD,
        });
        let (status, answer) =
            call(&addr, &tab, "POST", "/api/account/password", Some(right)).await;
        assert_eq!(status, 200, "{answer}");
        assert_eq!(answer["ended_sessions"], 1);
        assert_eq!(call(&addr, &tab, "GET", "/api/vms", None).await.0, 200);
        assert_eq!(
            call(&addr, &other_tab, "GET", "/api/vms", None).await.0,
            401
        );
        assert_eq!(login(&addr, "admin", GOOD_PASSWORD).await.0, 401);
        assert_eq!(login(&addr, "admin", OTHER_PASSWORD).await.0, 200);
    }

    /// Every audit row as one line: event, account, client IP, target,
    /// result, and detail, with `-` for NULL.
    async fn audit_lines(state: &AppState) -> Vec<String> {
        state
            .db
            .call(|c| {
                c.prepare(
                    "SELECT event, account_name, client_ip, target_name, result, detail_json
                     FROM audit_log ORDER BY id",
                )?
                .query_map([], |r| {
                    let cols: Vec<String> = (0..6)
                        .map(|i| {
                            r.get::<_, Option<String>>(i)
                                .map(|v| v.unwrap_or("-".into()))
                        })
                        .collect::<Result<_, _>>()?;
                    Ok(cols.join(" "))
                })?
                .collect()
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn logins_setup_and_account_changes_write_audit_rows_without_secrets() {
        let (addr, token, state) = serve_setup().await;
        let created = post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        assert_eq!(created.0, 201);
        const WRONG: &str = "not the right passphrase";
        // A password typed into the name field.
        const TYPED_AS_NAME: &str = "my secret passphrase";
        assert_eq!(login(&addr, "admin", WRONG).await.0, 401);
        assert_eq!(login(&addr, TYPED_AS_NAME, WRONG).await.0, 401);
        let tab = log_in_tab(&addr, "Admin", GOOD_PASSWORD).await;

        let new = serde_json::json!({"username": "second", "password": OTHER_PASSWORD});
        let (_, second) = call(&addr, &tab, "POST", "/api/accounts", Some(new)).await;
        let again = serde_json::json!({"username": "SECOND", "password": OTHER_PASSWORD});
        assert_eq!(
            call(&addr, &tab, "POST", "/api/accounts", Some(again))
                .await
                .0,
            409
        );
        let path = "/api/account/password";
        let wrong = serde_json::json!({"current_password": WRONG, "new_password": OTHER_PASSWORD});
        assert_eq!(call(&addr, &tab, "POST", path, Some(wrong)).await.0, 403);
        let right =
            serde_json::json!({"current_password": GOOD_PASSWORD, "new_password": OTHER_PASSWORD});
        assert_eq!(call(&addr, &tab, "POST", path, Some(right)).await.0, 200);
        let second = format!("/api/accounts/{}", second["id"]);
        assert_eq!(call(&addr, &tab, "DELETE", &second, None).await.0, 204);
        let (_, list) = call(&addr, &tab, "GET", "/api/accounts", None).await;
        let admin = format!("/api/accounts/{}", list[0]["id"]);
        assert_eq!(call(&addr, &tab, "DELETE", &admin, None).await.0, 409);

        let lines = audit_lines(&state).await;
        let ip = "127.0.0.1";
        assert_eq!(
            lines,
            [
                format!("setup.completed admin {ip} - ok -"),
                format!(r#"login.failed admin {ip} - failed {{"reason":"wrong_password"}}"#),
                format!(r#"login.failed - {ip} - failed {{"reason":"no_such_account"}}"#),
                format!("login.succeeded admin {ip} - ok -"),
                format!("account.create admin {ip} second ok -"),
                format!(r#"account.create admin {ip} SECOND failed {{"reason":"name_taken"}}"#),
                format!(
                    r#"account.change_password admin {ip} admin failed {{"reason":"wrong_password"}}"#
                ),
                format!("account.change_password admin {ip} admin ok -"),
                format!("account.delete admin {ip} second ok -"),
                format!(r#"account.delete admin {ip} admin failed {{"reason":"last_account"}}"#),
            ]
        );
        let all = lines.join("\n");
        for secret in [
            GOOD_PASSWORD,
            OTHER_PASSWORD,
            WRONG,
            TYPED_AS_NAME,
            token.as_str(),
            tab.cookie.rsplit('=').next().unwrap(),
            tab.csrf.as_str(),
        ] {
            assert!(!all.contains(secret), "{secret:?} is in the audit log");
        }
    }

    /// The state of VM `id` as the API reports it, polled until it is
    /// `want` or 5 seconds pass. Returns the last state.
    async fn wait_for_state(addr: &str, tab: &Tab, id: Uuid, want: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (_, vm) = call(addr, tab, "GET", &format!("/api/vms/{id}"), None).await;
            let state = vm["state"].as_str().unwrap_or("missing").to_owned();
            if state == want || Instant::now() > deadline {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn start_shut_down_and_force_off_change_the_state_and_are_audited() {
        let (addr, token, state) = serve_setup().await;
        post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        let tab = log_in_tab(&addr, "admin", GOOD_PASSWORD).await;
        let outside = Virt::open(TEST_URI).await.unwrap();
        let name = "lodger-spike-power-api";
        let id = outside
            .job(move |c| c.define_domain_xml(&domain_xml(name))?.uuid())
            .await
            .unwrap();
        assert_eq!(wait_for_state(&addr, &tab, id, "shutoff").await, "shutoff");
        let act = |action: &str| format!("/api/vms/{id}/actions/{action}");

        let start = Instant::now();
        assert_eq!(call(&addr, &tab, "POST", &act("start"), None).await.0, 204);
        assert_eq!(wait_for_state(&addr, &tab, id, "running").await, "running");
        assert!(start.elapsed() < Duration::from_secs(5));
        let (status, answer) = call(&addr, &tab, "POST", &act("start"), None).await;
        assert_eq!(
            (status, answer["error"].as_str()),
            (409, Some("the VM is running already"))
        );

        assert_eq!(
            call(&addr, &tab, "POST", &act("shutdown"), None).await.0,
            204
        );
        assert_eq!(wait_for_state(&addr, &tab, id, "shutoff").await, "shutoff");

        assert_eq!(call(&addr, &tab, "POST", &act("start"), None).await.0, 204);
        assert_eq!(wait_for_state(&addr, &tab, id, "running").await, "running");
        for body in [
            None,
            Some(serde_json::json!({"confirm": "LODGER-SPIKE-POWER-API"})),
        ] {
            let (status, answer) = call(&addr, &tab, "POST", &act("force-off"), body).await;
            assert_eq!(status, 422, "{answer}");
        }
        assert_eq!(wait_for_state(&addr, &tab, id, "running").await, "running");
        let confirm = serde_json::json!({ "confirm": name });
        assert_eq!(
            call(&addr, &tab, "POST", &act("force-off"), Some(confirm))
                .await
                .0,
            204
        );
        assert_eq!(wait_for_state(&addr, &tab, id, "shutoff").await, "shutoff");

        assert_eq!(
            call(&addr, &tab, "POST", &act("suspend"), None).await.0,
            404
        );
        let unknown = format!("/api/vms/{}/actions/start", Uuid::from_u128(0xdead));
        assert_eq!(call(&addr, &tab, "POST", &unknown, None).await.0, 404);
        let no_session = raw(&addr, "POST", &act("start"), &[SAME_ORIGIN.into()], "").await;
        assert_eq!(no_session.0, 401);

        let lines = audit_lines(&state).await;
        let lifecycle: Vec<&str> = lines
            .iter()
            .filter(|l| l.starts_with("vm.lifecycle"))
            .map(String::as_str)
            .collect();
        let row = |result: &str, detail: &str| {
            format!("vm.lifecycle admin 127.0.0.1 {name} {result} {detail}")
        };
        assert_eq!(
            lifecycle,
            [
                row("ok", r#"{"action":"start"}"#),
                row("failed", r#"{"action":"start","reason":"wrong_state"}"#),
                row("ok", r#"{"action":"shutdown"}"#),
                row("ok", r#"{"action":"start"}"#),
                row("ok", r#"{"action":"force-off"}"#),
            ]
        );
        outside
            .job(move |c| c.lookup_domain_by_uuid(id)?.undefine())
            .await
            .unwrap();
    }

    /// VM `id` as the API reports it, polled until `done` accepts it or 5
    /// seconds pass. Returns the last answer.
    async fn wait_for_vm(
        addr: &str,
        tab: &Tab,
        id: Uuid,
        done: impl Fn(u16, &Value) -> bool,
    ) -> (u16, Value) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (status, vm) = call(addr, tab, "GET", &format!("/api/vms/{id}"), None).await;
            if done(status, &vm) || Instant::now() > deadline {
                return (status, vm);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn pause_resume_reboot_autostart_and_delete_are_audited() {
        let (addr, token, state) = serve_setup().await;
        post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        let tab = log_in_tab(&addr, "admin", GOOD_PASSWORD).await;
        let outside = Virt::open(TEST_URI).await.unwrap();
        let name = "lodger-spike-life-api";
        // A disk outside every pool: the delete keeps it and says why. The
        // lodger-virt tests delete real pool volumes.
        let path = "/srv/lodger-spike-life-api.img";
        let xml = format!(
            "<domain type='test'><name>{name}</name><memory>65536</memory>\
             <os><type>hvm</type></os><devices><disk type='file' device='disk'>\
             <source file='{path}'/><target dev='vda'/></disk></devices></domain>"
        );
        let id = outside
            .job(move |c| c.define_domain_xml(&xml)?.uuid())
            .await
            .unwrap();
        assert_eq!(wait_for_state(&addr, &tab, id, "shutoff").await, "shutoff");
        let act = |action: &str| format!("/api/vms/{id}/actions/{action}");
        let vm = format!("/api/vms/{id}");

        assert_eq!(call(&addr, &tab, "POST", &act("pause"), None).await.0, 409);
        assert_eq!(call(&addr, &tab, "POST", &act("start"), None).await.0, 204);
        assert_eq!(wait_for_state(&addr, &tab, id, "running").await, "running");
        assert_eq!(call(&addr, &tab, "POST", &act("pause"), None).await.0, 204);
        assert_eq!(wait_for_state(&addr, &tab, id, "paused").await, "paused");
        let (status, answer) = call(&addr, &tab, "POST", &act("reboot"), None).await;
        assert_eq!(
            (status, answer["error"].as_str()),
            (409, Some("the VM is paused"))
        );
        assert_eq!(call(&addr, &tab, "POST", &act("resume"), None).await.0, 204);
        assert_eq!(wait_for_state(&addr, &tab, id, "running").await, "running");
        assert_eq!(call(&addr, &tab, "POST", &act("reboot"), None).await.0, 204);

        // Autostart: libvirt sends no event, so Lodger's own event must
        // refresh the inventory.
        let on = serde_json::json!({ "autostart": true });
        assert_eq!(call(&addr, &tab, "PATCH", &vm, Some(on)).await.0, 204);
        let (_, answer) = wait_for_vm(&addr, &tab, id, |_, v| v["autostart"] == true).await;
        assert_eq!(answer["autostart"], true);
        for bad in [
            serde_json::json!({ "autostart": "yes" }),
            serde_json::json!({ "autostart": false, "name": "x" }),
            serde_json::json!({}),
        ] {
            assert_eq!(call(&addr, &tab, "PATCH", &vm, Some(bad)).await.0, 400);
        }
        let off = serde_json::json!({ "autostart": false });
        assert_eq!(call(&addr, &tab, "PATCH", &vm, Some(off)).await.0, 204);

        // Delete needs the typed name, and the VM must be shut off.
        let remove =
            |confirm: &str| Some(serde_json::json!({ "confirm": confirm, "remove_volumes": true }));
        for body in [None, remove("LODGER-SPIKE-LIFE-API")] {
            let (status, answer) = call(&addr, &tab, "DELETE", &vm, body).await;
            assert_eq!(status, 422, "{answer}");
        }
        let (status, answer) = call(&addr, &tab, "DELETE", &vm, remove(name)).await;
        assert_eq!(
            (status, answer["error"].as_str()),
            (409, Some("shut the VM down first"))
        );
        let confirm = serde_json::json!({ "confirm": name });
        let forced = call(&addr, &tab, "POST", &act("force-off"), Some(confirm)).await;
        assert_eq!(forced.0, 204);
        assert_eq!(wait_for_state(&addr, &tab, id, "shutoff").await, "shutoff");
        let (status, answer) = call(&addr, &tab, "DELETE", &vm, remove(name)).await;
        assert_eq!(status, 200, "{answer}");
        assert_eq!(
            answer,
            serde_json::json!({
                "removed": [],
                "skipped": [{ "path": path, "reason": "not_in_pool" }],
            })
        );
        let (status, _) = wait_for_vm(&addr, &tab, id, |s, _| s == 404).await;
        assert_eq!(status, 404);
        assert_eq!(call(&addr, &tab, "DELETE", &vm, remove(name)).await.0, 404);
        let no_session = raw(&addr, "DELETE", &vm, &[SAME_ORIGIN.into()], "").await;
        assert_eq!(no_session.0, 401);

        let lines = audit_lines(&state).await;
        let rows: Vec<&str> = lines
            .iter()
            .filter(|l| l.starts_with("vm."))
            .map(String::as_str)
            .collect();
        let row = |event: &str, result: &str, detail: &str| {
            format!("{event} admin 127.0.0.1 {name} {result} {detail}")
        };
        let lifecycle = |result: &str, detail: &str| row("vm.lifecycle", result, detail);
        assert_eq!(
            rows,
            [
                lifecycle("failed", r#"{"action":"pause","reason":"wrong_state"}"#),
                lifecycle("ok", r#"{"action":"start"}"#),
                lifecycle("ok", r#"{"action":"pause"}"#),
                lifecycle("failed", r#"{"action":"reboot","reason":"wrong_state"}"#),
                lifecycle("ok", r#"{"action":"resume"}"#),
                lifecycle("ok", r#"{"action":"reboot"}"#),
                row("vm.edited", "ok", r#"{"action":"autostart-on"}"#),
                row("vm.edited", "ok", r#"{"action":"autostart-off"}"#),
                lifecycle("failed", r#"{"action":"delete","reason":"wrong_state"}"#),
                lifecycle("ok", r#"{"action":"force-off"}"#),
                lifecycle("ok", r#"{"action":"delete"}"#),
            ]
        );
    }

    /// Pool `id` as the API reports it, polled until `done` accepts it or 5
    /// seconds pass. Returns the last answer.
    async fn wait_for_pool(
        addr: &str,
        tab: &Tab,
        id: &str,
        done: impl Fn(u16, &Value) -> bool,
    ) -> (u16, Value) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (status, pool) = call(addr, tab, "GET", &format!("/api/pools/{id}"), None).await;
            if done(status, &pool) || Instant::now() > deadline {
                return (status, pool);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn pools_are_created_changed_removed_and_audited() {
        let (addr, token, state) = serve_setup().await;
        post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        let tab = log_in_tab(&addr, "admin", GOOD_PASSWORD).await;
        let name = "lodger-spike-pool-api";
        let path = "/srv/lodger-spike-pool-api";
        let create = serde_json::json!({ "name": name, "kind": "dir", "path": path });
        let (status, answer) = call(&addr, &tab, "POST", "/api/pools", Some(create.clone())).await;
        assert_eq!(status, 201, "{answer}");
        let id = answer["uuid"].as_str().unwrap().to_owned();
        let (status, pool) = wait_for_pool(&addr, &tab, &id, |s, _| s == 200).await;
        assert_eq!(status, 200, "{pool}");
        assert_eq!(pool["name"], name);
        assert_eq!(pool["state"], "running");
        assert_eq!(pool["autostart"], true);
        assert_eq!(pool["kind"], "dir");
        assert_eq!(pool["path"], path);
        assert_eq!(pool["used_by"], serde_json::json!([]));
        let (_, list) = call(&addr, &tab, "GET", "/api/pools", None).await;
        assert!(list.as_array().unwrap().iter().any(|p| p["name"] == name));

        // Rejected before any change.
        let rejects = [
            (create.clone(), 409, "exists already"),
            (
                serde_json::json!({ "name": "lodger-spike-pool-api2", "kind": "dir", "path": "/srv/lodger-spike-pool-api/" }),
                422,
                "Pool folder is the folder of pool \"lodger-spike-pool-api\" already",
            ),
            (
                serde_json::json!({ "name": "p2", "kind": "dir", "path": "/etc/vm" }),
                422,
                "system folder",
            ),
            (
                serde_json::json!({ "name": "p2", "kind": "dir" }),
                422,
                "Pool folder is empty",
            ),
            (
                serde_json::json!({ "name": "p2", "kind": "nfs", "export": "/x" }),
                422,
                "NFS server is empty",
            ),
            (
                serde_json::json!({ "name": "bad name", "kind": "dir", "path": "/srv/x" }),
                422,
                "Pool name",
            ),
            (
                serde_json::json!({ "name": "p2", "kind": "lvm", "path": "/srv/x" }),
                400,
                "bad JSON",
            ),
            (
                serde_json::json!({ "name": "p2", "kind": "dir", "path": "/srv/x", "x": 1 }),
                400,
                "bad JSON",
            ),
        ];
        for (body, want, text) in rejects {
            let (status, answer) =
                call(&addr, &tab, "POST", "/api/pools", Some(body.clone())).await;
            assert_eq!(status, want, "{body}: {answer}");
            let message = answer["error"].as_str().unwrap_or_default();
            assert!(message.contains(text), "{body}: {message}");
        }

        // An NFS pool without a folder mounts below /var/lib/libvirt/pools.
        let nfs = serde_json::json!({
            "name": "lodger-spike-pool-nfs", "kind": "nfs", "host": "nas.lan",
            "export": "/volume1/vm", "autostart": false,
        });
        let (status, answer) = call(&addr, &tab, "POST", "/api/pools", Some(nfs)).await;
        assert_eq!(status, 201, "{answer}");
        let nfs_id = answer["uuid"].as_str().unwrap().to_owned();
        let (_, pool) = wait_for_pool(&addr, &tab, &nfs_id, |s, _| s == 200).await;
        assert_eq!(pool["kind"], "netfs");
        assert_eq!(pool["path"], "/var/lib/libvirt/pools/lodger-spike-pool-nfs");
        assert_eq!(
            pool["nfs"],
            serde_json::json!({ "host": "nas.lan", "export": "/volume1/vm" })
        );
        assert_eq!(pool["autostart"], false);

        // A VM with a disk in the pool shows as a user.
        let outside = Virt::open(TEST_URI).await.unwrap();
        outside
            .job(move |c| {
                let xml = format!(
                    "<domain type='test'><name>lodger-spike-pool-user</name><memory>1024</memory>\
                     <os><type>hvm</type></os><devices><disk type='file' device='disk'>\
                     <source file='{path}/a.img'/><target dev='vda'/></disk></devices></domain>"
                );
                c.define_domain_xml(&xml).map(drop)
            })
            .await
            .unwrap();
        let (_, pool) = call(&addr, &tab, "GET", &format!("/api/pools/{id}"), None).await;
        assert_eq!(
            pool["used_by"],
            serde_json::json!(["lodger-spike-pool-user"])
        );

        // Stop, autostart off, and the wrong state.
        let url = format!("/api/pools/{id}");
        let stop = serde_json::json!({ "active": false, "autostart": false });
        assert_eq!(call(&addr, &tab, "PATCH", &url, Some(stop)).await.0, 204);
        let (_, pool) = wait_for_pool(&addr, &tab, &id, |_, p| {
            p["state"] == "inactive" && p["autostart"] == false
        })
        .await;
        assert_eq!(
            (&pool["state"], &pool["autostart"]),
            (&serde_json::json!("inactive"), &serde_json::json!(false))
        );
        let again = serde_json::json!({ "active": false });
        let (status, answer) = call(&addr, &tab, "PATCH", &url, Some(again)).await;
        assert_eq!(
            (status, answer["error"].as_str()),
            (409, Some("the pool is not running"))
        );
        assert_eq!(
            call(&addr, &tab, "PATCH", &url, Some(serde_json::json!({})))
                .await
                .0,
            400
        );

        // Removal needs the typed name.
        for body in [
            None,
            Some(serde_json::json!({ "confirm": "LODGER-SPIKE-POOL-API" })),
        ] {
            assert_eq!(call(&addr, &tab, "DELETE", &url, body).await.0, 422);
        }
        let confirm = serde_json::json!({ "confirm": name });
        assert_eq!(
            call(&addr, &tab, "DELETE", &url, Some(confirm.clone()))
                .await
                .0,
            204
        );
        let (status, _) = wait_for_pool(&addr, &tab, &id, |s, _| s == 404).await;
        assert_eq!(status, 404);
        assert_eq!(
            call(&addr, &tab, "DELETE", &url, Some(confirm)).await.0,
            404
        );
        let nfs_url = format!("/api/pools/{nfs_id}");
        let confirm =
            serde_json::json!({ "confirm": "lodger-spike-pool-nfs", "delete_files": true });
        assert_eq!(
            call(&addr, &tab, "DELETE", &nfs_url, Some(confirm)).await.0,
            204
        );
        let no_session = raw(&addr, "GET", "/api/pools", &[SAME_ORIGIN.into()], "").await;
        assert_eq!(no_session.0, 401);

        let lines = audit_lines(&state).await;
        let rows: Vec<&str> = lines
            .iter()
            .filter(|l| l.starts_with("pool."))
            .map(String::as_str)
            .collect();
        let row = |event: &str, target: &str, result: &str, detail: &str| {
            format!("{event} admin 127.0.0.1 {target} {result} {detail}")
        };
        assert_eq!(
            rows,
            [
                row("pool.created", name, "ok", "-"),
                row("pool.created", "lodger-spike-pool-nfs", "ok", "-"),
                row("pool.stopped", name, "ok", "-"),
                row("pool.edited", name, "ok", r#"{"action":"autostart-off"}"#),
                row(
                    "pool.stopped",
                    name,
                    "failed",
                    r#"{"reason":"wrong_state"}"#
                ),
                row("pool.deleted", name, "ok", r#"{"action":"keep-files"}"#),
                row(
                    "pool.deleted",
                    "lodger-spike-pool-nfs",
                    "ok",
                    r#"{"action":"delete-files"}"#
                ),
            ]
        );
    }

    #[tokio::test]
    async fn volumes_are_created_listed_kept_while_used_deleted_and_audited() {
        let (addr, token, state) = serve_setup().await;
        post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        let tab = log_in_tab(&addr, "admin", GOOD_PASSWORD).await;
        let pool_name = "lodger-spike-vol-api";
        let path = "/srv/lodger-spike-vol-api";
        let create = serde_json::json!({ "name": pool_name, "kind": "dir", "path": path });
        let (status, answer) = call(&addr, &tab, "POST", "/api/pools", Some(create)).await;
        assert_eq!(status, 201, "{answer}");
        let id = answer["uuid"].as_str().unwrap().to_owned();
        wait_for_pool(&addr, &tab, &id, |s, _| s == 200).await;
        let url = format!("/api/pools/{id}/volumes");

        // A 20 GiB qcow2 volume appears with that size and format.
        let disk = serde_json::json!({
            "name": "disk1.qcow2", "format": "qcow2", "capacity_bytes": 21_474_836_480_u64,
        });
        let (status, answer) = call(&addr, &tab, "POST", &url, Some(disk.clone())).await;
        assert_eq!(status, 201, "{answer}");
        assert_eq!(answer["name"], "disk1.qcow2");
        let (status, list) = call(&addr, &tab, "GET", &url, None).await;
        assert_eq!(status, 200, "{list}");
        assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
        assert_eq!(list[0]["name"], "disk1.qcow2");
        assert_eq!(list[0]["capacity_bytes"], 21_474_836_480_u64);
        assert_eq!(list[0]["format"], "qcow2");
        assert_eq!(list[0]["path"], format!("{path}/disk1.qcow2"));
        assert_eq!(list[0]["used_by"], serde_json::json!([]));

        // Rejected before any change.
        let rejects = [
            (disk.clone(), 409, "has a volume \"disk1.qcow2\" already"),
            (
                serde_json::json!({ "name": "bad name", "format": "raw", "capacity_bytes": 1_048_576 }),
                422,
                "Volume name",
            ),
            (
                serde_json::json!({ "name": "d.img", "format": "raw", "capacity_bytes": 0 }),
                422,
                "size must be between",
            ),
            (
                serde_json::json!({ "name": "d.img", "format": "vmdk", "capacity_bytes": 1_048_576 }),
                400,
                "bad JSON",
            ),
        ];
        for (body, want, text) in rejects {
            let (status, answer) = call(&addr, &tab, "POST", &url, Some(body.clone())).await;
            assert_eq!(status, want, "{body}: {answer}");
            let message = answer["error"].as_str().unwrap_or_default();
            assert!(message.contains(text), "{body}: {message}");
        }
        let (_, list) = call(&addr, &tab, "GET", &url, None).await;
        assert_eq!(list.as_array().unwrap().len(), 1, "nothing changed: {list}");
        let unknown = format!("/api/pools/{}/volumes", Uuid::nil());
        assert_eq!(call(&addr, &tab, "GET", &unknown, None).await.0, 404);
        // A NUL byte in the URL's name is refused before libvirt sees it.
        let (status, answer) = call(&addr, &tab, "DELETE", &format!("{url}/a%00b"), None).await;
        assert_eq!(status, 422, "{answer}");
        assert!(
            answer["error"].as_str().unwrap().contains("NUL"),
            "{answer}"
        );

        // A volume that a VM uses stays, and the answer names the VM.
        let outside = Virt::open(TEST_URI).await.unwrap();
        outside
            .job(move |c| {
                let xml = format!(
                    "<domain type='test'><name>lodger-spike-vol-user</name><memory>1024</memory>\
                     <os><type>hvm</type></os><devices><disk type='file' device='disk'>\
                     <source file='{path}/disk1.qcow2'/><target dev='vda'/></disk></devices></domain>"
                );
                c.define_domain_xml(&xml).map(drop)
            })
            .await
            .unwrap();
        let (_, list) = call(&addr, &tab, "GET", &url, None).await;
        assert_eq!(
            list[0]["used_by"],
            serde_json::json!(["lodger-spike-vol-user"])
        );
        let one = format!("{url}/disk1.qcow2");
        let (status, answer) = call(&addr, &tab, "DELETE", &one, None).await;
        assert_eq!(status, 409, "{answer}");
        assert_eq!(answer["error"], "in use by lodger-spike-vol-user");
        let (_, list) = call(&addr, &tab, "GET", &url, None).await;
        assert_eq!(list.as_array().unwrap().len(), 1, "the volume stays");

        outside
            .job(|c| c.lookup_domain_by_name("lodger-spike-vol-user")?.undefine())
            .await
            .unwrap();
        assert_eq!(call(&addr, &tab, "DELETE", &one, None).await.0, 204);
        let (status, answer) = call(&addr, &tab, "DELETE", &one, None).await;
        assert_eq!(status, 404, "{answer}");
        assert_eq!(answer["error"], "no such pool or volume");

        // A stopped pool lists no volumes.
        let stop = serde_json::json!({ "active": false });
        let pool_url = format!("/api/pools/{id}");
        assert_eq!(
            call(&addr, &tab, "PATCH", &pool_url, Some(stop)).await.0,
            204
        );
        let (status, answer) = call(&addr, &tab, "GET", &url, None).await;
        assert_eq!(status, 409, "{answer}");
        assert!(
            answer["error"].as_str().unwrap().contains("not running"),
            "{answer}"
        );
        let no_session = raw(&addr, "GET", &url, &[SAME_ORIGIN.into()], "").await;
        assert_eq!(no_session.0, 401);
        let confirm = serde_json::json!({ "confirm": pool_name });
        assert_eq!(
            call(&addr, &tab, "DELETE", &pool_url, Some(confirm))
                .await
                .0,
            204
        );

        let lines = audit_lines(&state).await;
        let rows: Vec<&str> = lines
            .iter()
            .filter(|l| l.starts_with("volume."))
            .map(String::as_str)
            .collect();
        let target = format!("{pool_name}/disk1.qcow2");
        let row = |event: &str, result: &str, detail: &str| {
            format!("{event} admin 127.0.0.1 {target} {result} {detail}")
        };
        assert_eq!(
            rows,
            [
                row("volume.created", "ok", "-"),
                row("volume.deleted", "failed", r#"{"reason":"in_use"}"#),
                row("volume.deleted", "ok", "-"),
                row("volume.deleted", "failed", r#"{"reason":"no_such_volume"}"#),
            ]
        );
    }

    /// Network `id` as the API reports it, polled until `done` accepts it or
    /// 5 seconds pass. Returns the last answer.
    async fn wait_for_network(
        addr: &str,
        tab: &Tab,
        id: &str,
        done: impl Fn(u16, &Value) -> bool,
    ) -> (u16, Value) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let url = format!("/api/networks/{id}");
            let (status, network) = call(addr, tab, "GET", &url, None).await;
            if done(status, &network) || Instant::now() > deadline {
                return (status, network);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn networks_are_created_changed_deleted_and_audited() {
        let (addr, token, state) = serve_setup().await;
        post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        let tab = log_in_tab(&addr, "admin", GOOD_PASSWORD).await;
        let name = "lodger-spike-net-api";
        let create = serde_json::json!({ "name": name, "mode": "nat", "subnet": "10.211.0.0/24" });
        let (status, answer) =
            call(&addr, &tab, "POST", "/api/networks", Some(create.clone())).await;
        assert_eq!(status, 201, "{answer}");
        let id = answer["uuid"].as_str().unwrap().to_owned();
        let (status, network) = wait_for_network(&addr, &tab, &id, |s, _| s == 200).await;
        assert_eq!(status, 200, "{network}");
        assert_eq!(network["name"], name);
        assert_eq!(network["active"], true);
        assert_eq!(network["autostart"], true);
        assert_eq!(network["mode"], "nat");
        assert_eq!(network["subnets"], serde_json::json!(["10.211.0.0/24"]));
        assert_eq!(network["used_by"], serde_json::json!([]));
        let (_, list) = call(&addr, &tab, "GET", "/api/networks", None).await;
        assert!(list.as_array().unwrap().iter().any(|n| n["name"] == name));
        let (status, bridges) = call(&addr, &tab, "GET", "/api/host-bridges", None).await;
        assert_eq!(status, 200);
        assert!(bridges.is_array(), "{bridges}");

        // Rejected before any change.
        let rejects = [
            (create.clone(), 409, "exists already"),
            (
                serde_json::json!({ "name": "lodger-spike-net-api2", "mode": "isolated", "subnet": "10.211.0.128/25" }),
                422,
                "which network \"lodger-spike-net-api\" uses",
            ),
            (
                serde_json::json!({ "name": "n2", "mode": "bridge", "bridge": "lodgernobr0" }),
                422,
                "A host bridge must exist first",
            ),
            (
                serde_json::json!({ "name": "n2", "mode": "nat" }),
                422,
                "Subnet is empty",
            ),
            (
                serde_json::json!({ "name": "n2", "mode": "bridge" }),
                422,
                "Host bridge is empty",
            ),
            (
                serde_json::json!({ "name": "n2", "mode": "nat", "subnet": "8.8.8.0/24" }),
                422,
                "10.0.0.0/8",
            ),
            (
                serde_json::json!({ "name": "n 2", "mode": "nat", "subnet": "10.212.0.0/24" }),
                422,
                "Network name",
            ),
            (
                serde_json::json!({ "name": "n2", "mode": "macvtap" }),
                400,
                "bad JSON",
            ),
        ];
        for (body, want, text) in rejects {
            let (status, answer) =
                call(&addr, &tab, "POST", "/api/networks", Some(body.clone())).await;
            assert_eq!(status, want, "{body}: {answer}");
            let message = answer["error"].as_str().unwrap_or_default();
            assert!(message.contains(text), "{body}: {message}");
        }

        // A VM with a NIC on the network shows as a user.
        let outside = Virt::open(TEST_URI).await.unwrap();
        outside
            .job(move |c| {
                let xml = format!(
                    "<domain type='test'><name>lodger-spike-net-user</name><memory>1024</memory>\
                     <os><type>hvm</type></os><devices><interface type='network'>\
                     <source network='{name}'/></interface></devices></domain>"
                );
                c.define_domain_xml(&xml).map(drop)
            })
            .await
            .unwrap();
        let url = format!("/api/networks/{id}");
        let (_, network) = call(&addr, &tab, "GET", &url, None).await;
        assert_eq!(
            network["used_by"],
            serde_json::json!(["lodger-spike-net-user"])
        );

        // Stop, autostart off, and the wrong state.
        let stop = serde_json::json!({ "active": false, "autostart": false });
        assert_eq!(call(&addr, &tab, "PATCH", &url, Some(stop)).await.0, 204);
        let (_, network) = wait_for_network(&addr, &tab, &id, |_, n| {
            n["active"] == false && n["autostart"] == false
        })
        .await;
        assert_eq!(network["active"], false);
        assert_eq!(network["autostart"], false);
        let again = serde_json::json!({ "active": false });
        let (status, answer) = call(&addr, &tab, "PATCH", &url, Some(again)).await;
        assert_eq!(
            (status, answer["error"].as_str()),
            (409, Some("the network is not running"))
        );
        assert_eq!(
            call(&addr, &tab, "PATCH", &url, Some(serde_json::json!({})))
                .await
                .0,
            400
        );

        // Delete needs the typed name.
        for body in [
            None,
            Some(serde_json::json!({ "confirm": "LODGER-SPIKE-NET-API" })),
        ] {
            assert_eq!(call(&addr, &tab, "DELETE", &url, body).await.0, 422);
        }
        let confirm = serde_json::json!({ "confirm": name });
        assert_eq!(
            call(&addr, &tab, "DELETE", &url, Some(confirm.clone()))
                .await
                .0,
            204
        );
        let (status, _) = wait_for_network(&addr, &tab, &id, |s, _| s == 404).await;
        assert_eq!(status, 404);
        assert_eq!(
            call(&addr, &tab, "DELETE", &url, Some(confirm)).await.0,
            404
        );
        let no_session = raw(&addr, "GET", "/api/networks", &[SAME_ORIGIN.into()], "").await;
        assert_eq!(no_session.0, 401);

        let lines = audit_lines(&state).await;
        let rows: Vec<&str> = lines
            .iter()
            .filter(|l| l.starts_with("network."))
            .map(String::as_str)
            .collect();
        let row = |event: &str, result: &str, detail: &str| {
            format!("{event} admin 127.0.0.1 {name} {result} {detail}")
        };
        assert_eq!(
            rows,
            [
                row("network.created", "ok", "-"),
                row("network.stopped", "ok", "-"),
                row("network.edited", "ok", r#"{"action":"autostart-off"}"#),
                row("network.stopped", "failed", r#"{"reason":"wrong_state"}"#),
                row("network.deleted", "ok", "-"),
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_sixth_failed_login_waits() {
        let (addr, token, state) = serve_setup().await;
        post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        for _ in 0..5 {
            assert_eq!(
                login(&addr, "admin", "wrong wrong wrong wrong").await.0,
                401
            );
        }
        let start = Instant::now();
        assert_eq!(
            login(&addr, "admin", "wrong wrong wrong wrong").await.0,
            401
        );
        let sixth = start.elapsed();
        assert!(
            sixth >= Duration::from_secs(1),
            "the sixth attempt took {sixth:?}"
        );
        // The time above includes argon2, which the other tests slow down,
        // because they share the limit of 2 runs. The throttle helper alone
        // runs no argon2, so its time shows the wait itself.
        let start = Instant::now();
        let ip = "127.0.0.1".parse().unwrap();
        let attempt = crate::auth::throttled(&state, "admin", ip).await.unwrap();
        assert_eq!(attempt.wait, Duration::from_secs(2));
        assert!(
            start.elapsed() >= Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn unknown_api_and_ws_paths_stay_404() {
        let (addr, _host) = serve().await;
        assert_eq!(get(&addr, "/api/missing").await.0, 404);
        assert_eq!(get(&addr, "/ws/missing").await.0, 404);
        assert_eq!(get(&addr, "/api").await.0, 404);
    }

    type Ws = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;
    type WsRequest = tokio_tungstenite::tungstenite::handshake::client::Request;

    /// A ticket for `cookie`'s session, whose CSRF token is `csrf`, asked
    /// for by a script that sends no `Origin`.
    async fn ticket_for(addr: &str, cookie: &str, csrf: &str) -> String {
        ticket_from(addr, cookie, csrf, None).await
    }

    /// [`ticket_for`], asked for by a page on `origin`.
    async fn ticket_from(addr: &str, cookie: &str, csrf: &str, origin: Option<&str>) -> String {
        let mut headers = vec![
            format!("Cookie: {cookie}"),
            SAME_ORIGIN.into(),
            format!("X-CSRF-Token: {csrf}"),
        ];
        headers.extend(origin.map(|o| format!("Origin: {o}")));
        let (status, out) = raw(addr, "POST", "/api/ws-tickets", &headers, "").await;
        assert_eq!(status, 201, "{out}");
        let body: Value = serde_json::from_str(&body_of(&out)).unwrap();
        body["ticket"].as_str().unwrap().to_owned()
    }

    /// An upgrade request for `path` with `cookie` and `ticket` if given. It
    /// sends no `Origin` and, like a browser, no `Sec-Fetch-Site`.
    fn socket_request(
        addr: &str,
        path: &str,
        cookie: Option<&str>,
        ticket: Option<&str>,
    ) -> WsRequest {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let query = ticket.map(|t| format!("?ticket={t}")).unwrap_or_default();
        let mut req = format!("ws://{addr}{path}{query}")
            .into_client_request()
            .unwrap();
        if let Some(cookie) = cookie {
            req.headers_mut().insert("cookie", cookie.parse().unwrap());
        }
        req
    }

    /// Opens `path` with the server's test session and a fresh ticket.
    async fn open_socket(addr: &str, path: &str) -> Ws {
        let cookie = COOKIES.lock().unwrap().get(addr).cloned().unwrap();
        let ticket = ticket_for(addr, &cookie, "csrf").await;
        let req = socket_request(addr, path, Some(&cookie), Some(&ticket));
        tokio_tungstenite::connect_async(req).await.unwrap().0
    }

    /// The HTTP status with which the server refuses `req`.
    async fn refusal(req: WsRequest) -> u16 {
        match tokio_tungstenite::connect_async(req).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => resp.status().as_u16(),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_lifecycle_event_reaches_a_websocket_client() {
        let (addr, _host) = serve().await;
        let mut ws = open_socket(&addr, "/ws/events").await;

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
    async fn a_socket_needs_a_session_a_fresh_ticket_and_the_origin_that_asked() {
        let (addr, _host) = serve().await;
        let cookie = COOKIES.lock().unwrap().get(&addr).cloned().unwrap();
        let path = "/ws/events";
        let page = "https://lodger.lan";
        let with_origin = |ticket: &str, origin: &str| {
            let mut req = socket_request(&addr, path, Some(&cookie), Some(ticket));
            req.headers_mut().insert("origin", origin.parse().unwrap());
            req
        };

        // A page on another origin, and one that claims to be Host (which
        // DNS rebinding can forge): 403.
        let host_origin = format!("http://{addr}");
        for origin in [
            "http://evil.example",
            "https://evil.lodger.lan",
            &host_origin,
        ] {
            let ticket = ticket_from(&addr, &cookie, "csrf", Some(page)).await;
            assert_eq!(refusal(with_origin(&ticket, origin)).await, 403, "{origin}");
        }
        // The page that asked for the ticket gets in, once.
        let ticket = ticket_from(&addr, &cookie, "csrf", Some(page)).await;
        assert!(
            tokio_tungstenite::connect_async(with_origin(&ticket, page))
                .await
                .is_ok()
        );
        assert_eq!(refusal(with_origin(&ticket, page)).await, 403);

        // No session: 401. No ticket, or a made-up one: 403.
        let ticket = ticket_for(&addr, &cookie, "csrf").await;
        assert_eq!(
            refusal(socket_request(&addr, path, None, Some(&ticket))).await,
            401
        );
        assert_eq!(
            refusal(socket_request(&addr, path, Some(&cookie), None)).await,
            403
        );
        let made_up = "0".repeat(64);
        assert_eq!(
            refusal(socket_request(&addr, path, Some(&cookie), Some(&made_up))).await,
            403
        );
        // A 401 does not spend the ticket, and it still works once.
        let ok = socket_request(&addr, path, Some(&cookie), Some(&ticket));
        assert!(tokio_tungstenite::connect_async(ok).await.is_ok());
        let again = socket_request(&addr, path, Some(&cookie), Some(&ticket));
        assert_eq!(refusal(again).await, 403);
    }

    #[tokio::test]
    async fn a_ticket_works_only_for_the_session_that_asked_for_it() {
        let (addr, token, _state) = serve_setup().await;
        post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        let (_, alice, alice_body) = login(&addr, "admin", GOOD_PASSWORD).await;
        let (_, bob, _) = login(&addr, "admin", GOOD_PASSWORD).await;
        let alice_csrf: Value = serde_json::from_str(&alice_body).unwrap();
        let ticket = ticket_for(
            &addr,
            alice.as_deref().unwrap(),
            alice_csrf["csrf_token"].as_str().unwrap(),
        )
        .await;
        let req = socket_request(&addr, "/ws/events", bob.as_deref(), Some(&ticket));
        assert_eq!(refusal(req).await, 403);
        // The failed try used the ticket up.
        let req = socket_request(&addr, "/ws/events", alice.as_deref(), Some(&ticket));
        assert_eq!(refusal(req).await, 403);
    }

    #[tokio::test]
    async fn a_ticket_needs_a_session_and_its_csrf_token() {
        let (addr, _host) = serve().await;
        let cookie = COOKIES.lock().unwrap().get(&addr).cloned().unwrap();
        let (status, _) = raw(&addr, "POST", "/api/ws-tickets", &[SAME_ORIGIN.into()], "").await;
        assert_eq!(status, 401);
        let headers = [format!("Cookie: {cookie}"), SAME_ORIGIN.into()];
        let (status, _) = raw(&addr, "POST", "/api/ws-tickets", &headers, "").await;
        assert_eq!(status, 403);
    }

    #[tokio::test]
    async fn logging_out_closes_the_session_sockets_within_5_seconds() {
        let (addr, _host) = serve().await;
        let cookie = COOKIES.lock().unwrap().get(&addr).cloned().unwrap();
        let mut ws = open_socket(&addr, "/ws/events").await;
        let headers = [
            format!("Cookie: {cookie}"),
            SAME_ORIGIN.into(),
            "X-CSRF-Token: csrf".into(),
        ];
        let (status, _) = raw(&addr, "DELETE", "/api/session", &headers, "").await;
        assert_eq!(status, 204);
        let start = Instant::now();
        // Events from the other tests, which share the test driver, may
        // arrive before the close.
        let close = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    Some(Ok(Message::Close(frame))) => return frame,
                    Some(Ok(_)) => {}
                    other => panic!("the socket ended without a close: {other:?}"),
                }
            }
        })
        .await
        .expect("the socket stayed open for 5 seconds after the logout");
        assert!(start.elapsed() < Duration::from_secs(5));
        let frame = close.expect("a close frame with a reason");
        assert_eq!(u16::from(frame.code), 1008);
        assert_eq!(frame.reason.as_str(), "the session ended");
    }

    /// Opens `/ws/vms/{id}/vnc` and returns the HTTP status of a refusal.
    async fn vnc_refusal(addr: &str, id: &str) -> u16 {
        let cookie = COOKIES.lock().unwrap().get(addr).cloned().unwrap();
        let ticket = ticket_for(addr, &cookie, "csrf").await;
        let path = format!("/ws/vms/{id}/vnc");
        refusal(socket_request(addr, &path, Some(&cookie), Some(&ticket))).await
    }

    #[tokio::test]
    async fn the_vnc_socket_refuses_what_it_cannot_open() {
        let (addr, host) = serve().await;
        let unknown = "00000000-0000-0000-0000-000000000000";
        let cookie = COOKIES.lock().unwrap().get(&addr).cloned().unwrap();
        let path = format!("/ws/vms/{unknown}/vnc");
        assert_eq!(
            refusal(socket_request(&addr, &path, Some(&cookie), None)).await,
            403
        );
        assert_eq!(vnc_refusal(&addr, unknown).await, 404);
        // The test driver has no display to open, so libvirt refuses.
        let test = host
            .inventory()
            .vms
            .into_values()
            .find(|vm| vm.name == "test")
            .unwrap();
        assert_eq!(vnc_refusal(&addr, &test.uuid.to_string()).await, 409);
    }

    #[tokio::test]
    async fn the_vnc_socket_answers_503_while_libvirt_is_down() {
        let missing = std::env::temp_dir().join("lodger-no-such-driver.xml");
        let host = Arc::new(Host::start(&format!("test://{}", missing.display())).unwrap());
        let (addr, _host) = serve_host(host).await;
        let any = "00000000-0000-0000-0000-000000000000";
        assert_eq!(vnc_refusal(&addr, any).await, 503);
    }

    #[tokio::test]
    async fn the_server_ignores_client_text_and_ends_on_close() {
        use futures_util::SinkExt;
        let (addr, _host) = serve().await;
        let mut ws = open_socket(&addr, "/ws/events").await;
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

    /// The next text message of `kind` on `ws`, within `within`. Other
    /// messages are skipped: events from the other tests share the driver.
    async fn next_of_type(ws: &mut Ws, kind: &str, within: Duration) -> Option<Value> {
        tokio::time::timeout(within, async {
            while let Some(Ok(msg)) = ws.next().await {
                if let Message::Text(text) = msg {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["type"] == kind {
                        return Some(value);
                    }
                }
            }
            None
        })
        .await
        .ok()
        .flatten()
    }

    #[tokio::test]
    async fn a_socket_gets_stats_only_while_it_subscribes() {
        use futures_util::SinkExt;
        let host = Arc::new(Host::start(TEST_URI).unwrap());
        let mut conn = host.watch_state();
        tokio::time::timeout(
            Duration::from_secs(5),
            conn.wait_for(|s| *s == ConnState::Connected),
        )
        .await
        .unwrap()
        .unwrap();
        let mut state = AppState::new(
            Arc::clone(&host),
            crate::db::Db::in_memory().await,
            None,
            vec![],
            None,
        );
        let fast = Duration::from_millis(50);
        state.stats = crate::stats::Stats::new(Arc::clone(&host), fast);
        let cookie = log_in_directly(&state).await;
        let addr = start(state.clone()).await;
        COOKIES.lock().unwrap().insert(addr.clone(), cookie);
        let mut ws = open_socket(&addr, "/ws/events").await;

        // Unknown requests start nothing.
        for text in [
            r#"{"subscribe":"cpu"}"#,
            "subscribe stats",
            r#"{"subscribe":"stats","x":1}"#,
        ] {
            ws.send(Message::Text(text.into())).await.unwrap();
        }
        assert!(next_of_type(&mut ws, "stats", fast * 4).await.is_none());
        assert_eq!(state.stats.calls(), 0);

        ws.send(Message::Text(r#"{"subscribe":"stats"}"#.into()))
            .await
            .unwrap();
        let first = next_of_type(&mut ws, "stats", Duration::from_secs(5))
            .await
            .expect("stats within 5 seconds");
        assert!(first["vms"].is_array(), "{first}");
        assert!(
            next_of_type(&mut ws, "stats", Duration::from_secs(5))
                .await
                .is_some()
        );

        ws.send(Message::Text(r#"{"unsubscribe":"stats"}"#.into()))
            .await
            .unwrap();
        // A snapshot that was on its way may still arrive. After that, none.
        let _ = next_of_type(&mut ws, "stats", fast * 2).await;
        assert!(next_of_type(&mut ws, "stats", fast * 6).await.is_none());
        let calls = state.stats.calls();
        tokio::time::sleep(fast * 4).await;
        assert_eq!(state.stats.calls(), calls, "stats calls went on");
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
        let _stuck = open_socket(&addr, "/ws/events").await;
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
