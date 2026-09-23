//! The HTTP server: the API, the WebSocket, reserved prefixes, then the
//! embedded web UI.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware;
use axum::response::Response;
use axum::routing::{any, get, post};
use ipnet::IpNet;
use lodger_virt::Host;

use crate::assets::{self, AssetSource, Embedded};
use crate::config::Config;
use crate::db::Db;
use crate::setup::{self, Setup, SetupToken};
use crate::throttle::Throttle;
use crate::{api, auth, console, security, ws};

/// What every handler can reach.
#[derive(Debug, Clone)]
pub struct AppState {
    pub host: Arc<Host>,
    pub db: Db,
    pub setup: Arc<Setup>,
    pub throttle: Arc<Throttle>,
    /// Proxies whose forwarded client address Lodger believes (TAD 7.5).
    pub trusted_proxies: Arc<Vec<IpNet>>,
    /// The origin of `public_url`, for the Origin check (TAD 7.4).
    pub public_origin: Option<Arc<str>>,
    /// The Content-Security-Policy for every response.
    pub csp: Arc<HeaderValue>,
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
            host,
            db,
            setup: Arc::new(std::sync::Mutex::new(setup)),
            throttle: Arc::default(),
            trusted_proxies: Arc::new(trusted_proxies),
        }
    }
}

/// Builds the router. Paths under `/api` and `/ws` without a handler answer
/// 404, and they never fall through to the web UI.
///
/// Every API route needs a live session (TAD 7.2), except health, setup,
/// and login. New routes go into `protected`, so the guard is the default.
/// The guard also checks `X-CSRF-Token` on state-changing requests. The WebSocket
/// routes get their session check in Task 2.5.
///
/// Around everything: state-changing requests must come from a Lodger page, and
/// every response gets the security headers (`security.rs`).
pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/host", get(api::host))
        .route("/api/vms", get(api::vms))
        .route("/api/vms/{id}", get(api::vm))
        .route("/api/session", get(auth::current).delete(auth::logout))
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
    let db = Db::open(&config.state_dir).await?;
    tokio::task::spawn_blocking(crate::passwords::prepare)
        .await
        .map_err(|e| format!("cannot prepare the password check: {e}"))?;
    let setup = open_setup(&db).await?;
    let host = Host::start(&config.uri).map_err(|e| format!("cannot use {:?}: {e}", config.uri))?;
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
    // stdout carries only the address line, which scripts and tests read.
    // Everything else goes to stderr, which systemd sends to the journal.
    println!(
        "lodger listening on http://{}",
        listener.local_addr().map_err(fail)?
    );
    eprintln!("{}", summary(&config));
    if !listen.ip().is_loopback() {
        // TAD section 7.4: Lodger has no built-in TLS, so passwords and
        // session cookies would cross the network in clear text. Browsers
        // also keep the Secure session cookie only over HTTPS or loopback.
        eprintln!(
            "lodger: WARNING: listening on {listen}, which is not loopback. Put Lodger behind a \
             reverse proxy with TLS and listen on 127.0.0.1 instead."
        );
    }
    // The TCP peer's address, which the login's client-IP rule needs.
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
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
        "lodger: libvirt {}, state directory {}, public URL {}, {} trusted proxies",
        config.uri,
        config.state_dir.display(),
        config.public_url.as_deref().unwrap_or("not set"),
        config.trusted_proxies.len()
    )
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_sixth_failed_login_waits() {
        let (addr, token, _state) = serve_setup().await;
        post_json(&addr, "/api/setup", &claim(&token, "admin", GOOD_PASSWORD)).await;
        let mut slowest = Duration::ZERO;
        for _ in 0..5 {
            let start = Instant::now();
            assert_eq!(
                login(&addr, "admin", "wrong wrong wrong wrong").await.0,
                401
            );
            slowest = slowest.max(start.elapsed());
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
        assert!(
            sixth > slowest,
            "sixth {sixth:?}, slowest before {slowest:?}"
        );
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

    /// Opens `/ws/vms/{id}/vnc` and returns the HTTP status of a refusal.
    async fn vnc_refusal(addr: &str, id: &str, origin: Option<&str>) -> u16 {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://{addr}/ws/vms/{id}/vnc")
            .into_client_request()
            .unwrap();
        if let Some(origin) = origin {
            req.headers_mut().insert("origin", origin.parse().unwrap());
        }
        match tokio_tungstenite::connect_async(req).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => resp.status().as_u16(),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_vnc_socket_refuses_what_it_cannot_open() {
        let (addr, host) = serve().await;
        let unknown = "00000000-0000-0000-0000-000000000000";
        assert_eq!(
            vnc_refusal(&addr, unknown, Some("http://evil.example")).await,
            403
        );
        assert_eq!(vnc_refusal(&addr, unknown, None).await, 404);
        // The test driver has no display to open, so libvirt refuses.
        let test = host
            .inventory()
            .vms
            .into_values()
            .find(|vm| vm.name == "test")
            .unwrap();
        assert_eq!(vnc_refusal(&addr, &test.uuid.to_string(), None).await, 409);
    }

    #[tokio::test]
    async fn the_vnc_socket_answers_503_while_libvirt_is_down() {
        let missing = std::env::temp_dir().join("lodger-no-such-driver.xml");
        let host = Arc::new(Host::start(&format!("test://{}", missing.display())).unwrap());
        let (addr, _host) = serve_host(host).await;
        let any = "00000000-0000-0000-0000-000000000000";
        assert_eq!(vnc_refusal(&addr, any, None).await, 503);
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
