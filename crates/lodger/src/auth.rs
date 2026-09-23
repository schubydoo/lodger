//! Login, sessions, and logout (TAD sections 7.1 and 7.2).
//!
//! - `POST /api/session` checks a username and password and sets the
//!   `__Host-lodger_sid` cookie: 256 random bits, `Secure`, `HttpOnly`,
//!   `SameSite=Strict`, `Path=/`. The database stores only the SHA-256.
//! - `GET /api/session` returns the logged-in user and the CSRF token.
//! - `DELETE /api/session` ends the session.
//!
//! [`require_session`] guards every other API route: without a live session
//! it answers 401 and runs nothing. A session ends after 60 minutes without
//! use or 24 hours after its start.

use std::net::SocketAddr;
use std::time::Instant;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

/// Middleware for the protected routes: 401 without a live session. The
/// handler can read the [`Session`] from the request extensions.
pub async fn require_session(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    match lookup(&state, req.headers()).await {
        Ok(Some(session)) => {
            req.extensions_mut().insert(session);
            next.run(req).await
        }
        Ok(None) => unauthorized(),
        Err(()) => lookup_failed(),
    }
}

/// An extractor for handlers outside [`require_session`]: it rejects the
/// request with 401 without a live session.
pub struct CurrentSession(pub Session);

impl FromRequestParts<AppState> for CurrentSession {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        match lookup(state, &parts.headers).await {
            Ok(Some(session)) => Ok(Self(session)),
            Ok(None) => Err(unauthorized()),
            Err(()) => Err(lookup_failed()),
        }
    }
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
    // Login needs no session, but it changes state. Task 2.4 adds the
    // general Origin and CSRF checks.
    if !crate::ws::same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let login = match body {
        Ok(Json(login)) => login,
        Err(rejection) => return rejection.into_response(),
    };
    let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);

    // After 5 failures in 15 minutes, for this account or this IP, wait.
    let (delay, failures) = state.throttle.delay(&login.username, ip, Instant::now());
    if !delay.is_zero() {
        // {:?} escapes control characters, so a crafted name cannot forge
        // log lines. The name is cut, so it cannot flood the log either.
        let shown: String = login.username.chars().take(64).collect();
        eprintln!(
            "lodger: WARNING: {failures} failed logins in 15 minutes for account {shown:?} \
             or from {ip}; this attempt waits {} s",
            delay.as_secs()
        );
        tokio::time::sleep(delay).await;
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
    let verified = tokio::task::spawn_blocking(move || match stored {
        Some(stored) => crate::passwords::verify(&password, &stored),
        None => crate::passwords::verify_nobody(&password),
    })
    .await
    .unwrap_or(false);
    let Some(account) = account.filter(|_| verified) else {
        state.throttle.fail(&login.username, ip, Instant::now());
        // One message for both cases: no hint which part was wrong.
        return error(StatusCode::UNAUTHORIZED, "wrong username or password");
    };
    state.throttle.succeed(&login.username);

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
pub async fn current(CurrentSession(session): CurrentSession) -> Json<SessionInfo> {
    Json(SessionInfo {
        username: session.username,
        csrf_token: session.csrf_token,
    })
}

/// `DELETE /api/session`: log out. The session stops working at once on
/// every endpoint, because each request looks it up in the database.
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    CurrentSession(_): CurrentSession,
) -> Response {
    if !crate::ws::same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let token = cookie_token(&headers).expect("CurrentSession found the cookie");
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
