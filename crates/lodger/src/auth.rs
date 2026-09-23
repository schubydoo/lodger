//! Login, sessions, and logout (TAD sections 7.1 and 7.2).
//!
//! - `POST /api/session` checks a username and password and sets the
//!   `__Host-lodger_sid` cookie: 256 random bits, `Secure`, `HttpOnly`,
//!   `SameSite=Strict`, `Path=/`. The database stores only the SHA-256.
//! - `GET /api/session` returns the logged-in user and the CSRF token.
//! - `DELETE /api/session` ends the session.
//!
//! - `POST /api/ws-tickets` gives a single-use ticket for a WebSocket
//!   upgrade. [`open_socket`] checks the upgrade, and [`session_ended`]
//!   closes the socket within 5 seconds after its session ends.
//!
//! [`require_session`] guards every other API route: without a live session
//! it answers 401 and runs nothing. On a request that is not GET or HEAD, it
//! also needs the session's token in `X-CSRF-Token`, or it answers 403. A
//! session ends after 60 minutes without use or 24 hours after its start.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::db::{NewSession, Session};
use crate::server::AppState;

/// The session cookie. The `__Host-` prefix makes browsers require `Secure`
/// and `Path=/` and refuse a `Domain`, so no sibling host can set it.
pub const COOKIE: &str = "__Host-lodger_sid";

fn error(code: StatusCode, message: &str) -> Response {
    (code, Json(serde_json::json!({ "error": message }))).into_response()
}

fn unauthorized() -> Response {
    error(StatusCode::UNAUTHORIZED, "log in first")
}

/// 32 random bytes as 64 hex characters.
fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("cannot make a token: {e}"))?;
    Ok(hex::encode(bytes))
}

/// How often an open WebSocket checks that its session is still live.
pub const SOCKET_SESSION_CHECK: Duration = Duration::from_secs(2);

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// The session token from the `Cookie` header, if there is one.
fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.to_owned())
}

fn set_cookie(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE}={token}; Path=/; Secure; HttpOnly; SameSite=Strict"
    ))
    .expect("a hex token is a valid header value")
}

fn clear_cookie() -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE}=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0"
    ))
    .expect("a fixed string is a valid header value")
}

/// Looks up the live session for a request's cookie. A database error is
/// logged here; the caller answers 500.
async fn lookup(state: &AppState, headers: &HeaderMap) -> Result<Option<Session>, ()> {
    let Some(token) = cookie_token(headers) else {
        return Ok(None);
    };
    state.db.session(token_hash(&token)).await.map_err(|e| {
        eprintln!("lodger: session lookup: the database failed: {e}");
    })
}

fn lookup_failed() -> Response {
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "cannot check the session",
    )
}

/// The header that carries the session's CSRF token.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// Whether `headers` carry the session's CSRF token, compared in constant
/// time.
fn csrf_matches(headers: &HeaderMap, session: &Session) -> bool {
    headers
        .get(CSRF_HEADER)
        .is_some_and(|sent| sent.as_bytes().ct_eq(session.csrf_token.as_bytes()).into())
}

/// Middleware for the protected routes: 401 without a live session, and 403
/// for a state-changing request without the session's CSRF token. The handler can
/// read the [`Session`] from the request extensions.
pub async fn require_session(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let session = match lookup(&state, req.headers()).await {
        Ok(Some(session)) => session,
        Ok(None) => return unauthorized(),
        Err(()) => return lookup_failed(),
    };
    if !crate::security::is_safe(req.method()) && !csrf_matches(req.headers(), &session) {
        return error(StatusCode::FORBIDDEN, "missing or wrong X-CSRF-Token");
    }
    req.extensions_mut().insert(session);
    next.run(req).await
}

#[derive(Deserialize)]
pub struct Login {
    pub username: String,
    pub password: String,
}

/// Shows only the username: the password must never reach a log line.
impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub username: String,
    /// The value for the `X-CSRF-Token` header (Task 2.4).
    pub csrf_token: String,
}

/// `POST /api/session`: log in.
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<Login>, JsonRejection>,
) -> Response {
    // Login has no session yet, so no CSRF token. The Origin rule in
    // `security.rs` and the JSON body stand in for it (TAD 7.4).
    let login = match body {
        Ok(Json(login)) => login,
        Err(rejection) => return rejection.into_response(),
    };
    let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);

    // After 5 failures in 15 minutes, for this account or this IP, wait. The
    // attempt counts as a failure from here on, so parallel attempts see
    // it, and one that ends early (a closed connection, a database error)
    // stays a failure.
    let attempt = match state.throttle.begin(&login.username, ip, Instant::now()) {
        Ok(attempt) => attempt,
        Err(retry) => {
            let mut response = error(
                StatusCode::TOO_MANY_REQUESTS,
                "too many login attempts. Try again later",
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(retry.as_secs()));
            return response;
        }
    };
    if !attempt.wait.is_zero() {
        // {:?} escapes control characters, so a crafted name cannot forge
        // log lines. The name is cut, so it cannot flood the log either.
        let shown: String = login.username.chars().take(64).collect();
        eprintln!(
            "lodger: WARNING: {} failed logins in 15 minutes for account {shown:?} \
             or from {ip}; this attempt waits {} s",
            attempt.failures,
            attempt.wait.as_secs()
        );
        tokio::time::sleep(attempt.wait).await;
    }

    let account = match state.db.find_account(login.username.clone()).await {
        Ok(account) => account,
        Err(e) => {
            eprintln!("lodger: login: the database failed: {e}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "cannot check the login");
        }
    };
    let password = login.password;
    let stored = account.as_ref().map(|a| a.password_hash.clone());
    // argon2 runs for a missing account too, so the time does not tell
    // which usernames exist.
    let verified = crate::passwords::run(move || match stored {
        Some(stored) => crate::passwords::verify(&password, &stored),
        None => crate::passwords::verify_nobody(&password),
    })
    .await
    .unwrap_or(false);
    let Some(account) = account.filter(|_| verified) else {
        // The throttle counted the failure already. One message for both
        // cases: no hint which part was wrong.
        return error(StatusCode::UNAUTHORIZED, "wrong username or password");
    };
    state.throttle.succeed(&login.username, ip, &attempt);

    let (token, csrf_token) = match (random_token(), random_token()) {
        (Ok(t), Ok(c)) => (t, c),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("lodger: login: {e}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "cannot start a session");
        }
    };
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(256).collect());
    let new = NewSession {
        token_sha256: token_hash(&token),
        account_id: account.id,
        csrf_token: csrf_token.clone(),
        client_ip: ip.to_string(),
        user_agent,
    };
    if let Err(e) = state.db.create_session(new).await {
        eprintln!("lodger: login: the database failed: {e}");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "cannot start a session");
    }
    let mut response = Json(SessionInfo {
        username: account.username,
        csrf_token,
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, set_cookie(&token));
    response
}

/// `GET /api/session`: who is logged in, with the CSRF token.
pub async fn current(Extension(session): Extension<Session>) -> Json<SessionInfo> {
    Json(SessionInfo {
        username: session.username,
        csrf_token: session.csrf_token,
    })
}

/// `DELETE /api/session`: log out. The session stops working at once on
/// every endpoint, because each request looks it up in the database.
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let token = cookie_token(&headers).expect("require_session found the cookie");
    if let Err(e) = state.db.delete_session(token_hash(&token)).await {
        eprintln!("lodger: logout: the database failed: {e}");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "cannot end the session");
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, clear_cookie());
    response
}

/// The SHA-256 of the request's session token, which names its session.
fn session_key(headers: &HeaderMap) -> Option<[u8; 32]> {
    cookie_token(headers).map(|token| token_hash(&token))
}

#[derive(Debug, Serialize)]
pub struct TicketInfo {
    pub ticket: String,
}

/// `POST /api/ws-tickets`: a ticket for one WebSocket upgrade, for 30
/// seconds. [`require_session`] checked the session and the CSRF token.
pub async fn issue_ticket(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let key = session_key(&headers).expect("require_session found the cookie");
    let origin = header_text(&headers, header::ORIGIN).map(str::to_owned);
    match state.tickets.issue(key, origin, Instant::now()) {
        Ok(Some(ticket)) => (StatusCode::CREATED, Json(TicketInfo { ticket })).into_response(),
        Ok(None) => error(StatusCode::TOO_MANY_REQUESTS, "too many open tickets"),
        Err(e) => {
            eprintln!("lodger: ws ticket: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "cannot make a ticket")
        }
    }
}

/// The query of a WebSocket URL.
#[derive(Debug, Deserialize)]
pub struct SocketQuery {
    pub ticket: Option<String>,
}

fn header_text(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Checks a WebSocket upgrade (TAD 7.4) and returns its session key.
///
/// 1. The session must be live: 401.
/// 2. The ticket must be live, unused, and from this session, and the
///    upgrade's `Origin` must equal the `Origin` that asked for the ticket:
///    403. A browser applies no same-origin rule to a WebSocket and sends no
///    `Sec-Fetch-Site` on the upgrade, so this exact match keeps other pages
///    out.
pub async fn open_socket(
    state: &AppState,
    headers: &HeaderMap,
    query: &SocketQuery,
) -> Result<[u8; 32], Box<Response>> {
    let key = match lookup(state, headers).await {
        Ok(Some(_)) => session_key(headers).expect("lookup found the cookie"),
        Ok(None) => return Err(Box::new(unauthorized())),
        Err(()) => return Err(Box::new(lookup_failed())),
    };
    let ticket = query.ticket.as_deref().unwrap_or_default();
    let origin = header_text(headers, header::ORIGIN);
    if !state.tickets.redeem(ticket, key, origin, Instant::now()) {
        return Err(Box::new(error(
            StatusCode::FORBIDDEN,
            "missing, used, or expired ticket, or a page from another origin",
        )));
    }
    Ok(key)
}

/// Returns once the session `key` is no longer live: a logout, a timeout,
/// or a database that fails. A socket stops when this returns.
pub async fn session_ended(state: AppState, key: [u8; 32]) {
    loop {
        tokio::time::sleep(SOCKET_SESSION_CHECK).await;
        match state.db.session_alive(key).await {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                eprintln!("lodger: socket session check: the database failed: {e}");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{COOKIE, clear_cookie, cookie_token, random_token, set_cookie};

    #[test]
    fn a_token_is_256_random_bits_in_hex() {
        let (a, b) = (random_token().unwrap(), random_token().unwrap());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn the_cookie_has_every_protection() {
        let v = set_cookie("abc");
        let v = v.to_str().unwrap();
        assert!(v.starts_with("__Host-lodger_sid=abc;"), "{v}");
        for part in ["Path=/", "Secure", "HttpOnly", "SameSite=Strict"] {
            assert!(v.contains(part), "{v} lacks {part}");
        }
        assert!(!v.contains("Domain"), "{v}");
        assert!(clear_cookie().to_str().unwrap().contains("Max-Age=0"));
    }

    #[test]
    fn the_token_is_read_from_any_cookie_header() {
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, HeaderValue::from_static("theme=dark"));
        h.append(
            header::COOKIE,
            HeaderValue::from_str(&format!("a=1; {COOKIE}=tok; b=2")).unwrap(),
        );
        assert_eq!(cookie_token(&h).as_deref(), Some("tok"));
        // A cookie whose name only ends with the name does not count.
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("x__Host-lodger_sid=tok"),
        );
        assert_eq!(cookie_token(&h), None);
    }
}
