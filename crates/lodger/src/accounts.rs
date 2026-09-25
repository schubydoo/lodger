//! Accounts (PRD F2, TAD 7.1 and 7.2). Every V1 account is an admin.
//!
//! - `GET /api/accounts` lists them. `you` marks the caller's own.
//! - `POST /api/accounts` adds one. The name follows the setup rules, and
//!   the password must pass the policy: 15 characters or more, and not on
//!   the list of the 3000 most common passwords.
//! - `DELETE /api/accounts/{id}` deletes one and ends all its sessions.
//!   Deleting the last account fails, so Lodger never locks everyone out.
//! - `POST /api/account/password` changes the caller's own password. It
//!   needs the current password, under the login throttle, and it ends every
//!   other session of the account (ASVS 6.2.3 and 7.4.3).
//!
//! All four sit behind the session guard, which also checks the CSRF token.

use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use lodger_core::validate::Name;
use serde::{Deserialize, Serialize};

use crate::audit::{self, Entry};
use crate::db::{AccountInfo, Deleted, Session};
use crate::server::AppState;

const CREATE: &str = "account.create";
const DELETE: &str = "account.delete";
const CHANGE_PASSWORD: &str = "account.change_password";

fn error(code: StatusCode, message: impl Into<String>) -> Response {
    (code, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

fn database_failed(what: &str, e: &str) -> Response {
    eprintln!("lodger: {what}: the database failed: {e}");
    error(StatusCode::INTERNAL_SERVER_ERROR, format!("cannot {what}"))
}

/// Hashes `password` under the argon2 limit.
async fn hash(password: String) -> Result<String, Box<Response>> {
    match crate::passwords::run(move || crate::passwords::hash(&password)).await {
        Some(Ok(hash)) => Ok(hash),
        Some(Err(e)) => {
            eprintln!("lodger: accounts: cannot hash the password: {e}");
            Err(Box::new(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot hash the password",
            )))
        }
        None => {
            eprintln!("lodger: accounts: the hashing task failed");
            Err(Box::new(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot hash the password",
            )))
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Listed {
    #[serde(flatten)]
    pub account: AccountInfo,
    /// The caller's own account.
    pub you: bool,
}

/// `GET /api/accounts`.
pub async fn list(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Response {
    match state.db.list_accounts().await {
        Ok(accounts) => Json(
            accounts
                .into_iter()
                .map(|account| Listed {
                    you: account.id == session.account_id,
                    account,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => database_failed("list the accounts", &e),
    }
}

#[derive(Deserialize)]
pub struct NewAccount {
    pub username: String,
    pub password: String,
}

/// Shows only the username: the password must never reach a log line.
impl std::fmt::Debug for NewAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewAccount")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Serialize)]
pub struct Created {
    pub id: i64,
    pub username: String,
}

/// `POST /api/accounts`.
pub async fn create(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    body: Result<Json<NewAccount>, JsonRejection>,
) -> Response {
    let new = match body {
        Ok(Json(new)) => new,
        Err(rejection) => return rejection.into_response(),
    };
    let username = match Name::parse("username", &new.username) {
        Ok(name) => name.as_str().to_owned(),
        Err(e) => return error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
    };
    if let Err(e) = lodger_core::password::check(&new.password) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string());
    }
    let hash = match hash(new.password).await {
        Ok(hash) => hash,
        Err(response) => return *response,
    };
    let by = |entry: Entry| {
        let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);
        entry
            .account(&session.username)
            .client_ip(ip)
            .target_account(&username)
    };
    match state.db.create_account(username.clone(), hash).await {
        Ok(Some(id)) => {
            eprintln!("lodger: accounts: created the account {username}");
            audit::log(&state.db, by(Entry::ok(CREATE))).await;
            (StatusCode::CREATED, Json(Created { id, username })).into_response()
        }
        Ok(None) => {
            audit::log(&state.db, by(Entry::failed(CREATE, "name_taken"))).await;
            error(
                StatusCode::CONFLICT,
                format!("an account called {username} exists already"),
            )
        }
        Err(e) => database_failed("create the account", &e),
    }
}

/// `DELETE /api/accounts/{id}`.
pub async fn delete(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
) -> Response {
    let by = |entry: Entry| {
        let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);
        entry.account(&session.username).client_ip(ip)
    };
    match state.db.delete_account(id).await {
        Ok(Deleted::Yes(name)) => {
            eprintln!("lodger: accounts: deleted the account with id {id}");
            audit::log(&state.db, by(Entry::ok(DELETE)).target_account(name)).await;
            StatusCode::NO_CONTENT.into_response()
        }
        // No name to record: the account never existed.
        Ok(Deleted::NoSuchAccount) => error(StatusCode::NOT_FOUND, "no such account"),
        // The last account is the caller's own, since the caller has a session.
        Ok(Deleted::LastAccount) => {
            audit::log(
                &state.db,
                by(Entry::failed(DELETE, "last_account")).target_account(&session.username),
            )
            .await;
            error(
                StatusCode::CONFLICT,
                "this is the last account. Add another account before you delete this one",
            )
        }
        Err(e) => database_failed("delete the account", &e),
    }
}

#[derive(Deserialize)]
pub struct PasswordChange {
    pub current_password: String,
    pub new_password: String,
}

/// Shows nothing: both fields are secrets.
impl std::fmt::Debug for PasswordChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordChange").finish_non_exhaustive()
    }
}

#[derive(Serialize)]
pub struct Changed {
    /// How many other sessions of the account ended.
    pub ended_sessions: usize,
    /// The CSRF token of the new session that replaces the caller's.
    pub csrf_token: String,
}

/// `POST /api/account/password`: the caller's own password.
pub async fn change_password(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(session): Extension<Session>,
    body: Result<Json<PasswordChange>, JsonRejection>,
) -> Response {
    let change = match body {
        Ok(Json(change)) => change,
        Err(rejection) => return rejection.into_response(),
    };
    if let Err(e) = lodger_core::password::check(&change.new_password) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string());
    }
    let account = match state.db.account_by_id(session.account_id).await {
        Ok(Some(account)) => account,
        // The account went away after the session check.
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "log in first"),
        Err(e) => return database_failed("change the password", &e),
    };

    // A stolen session must not become a way to guess the password, so the
    // check runs under the login throttle.
    let ip = crate::client_ip::client_ip(peer.ip(), &headers, &state.trusted_proxies);
    let attempt = match crate::auth::throttled(&state, &account.username, ip).await {
        Ok(attempt) => attempt,
        Err(response) => return *response,
    };
    let current = change.current_password;
    let stored = account.password_hash;
    let verified = crate::passwords::run(move || crate::passwords::verify(&current, &stored))
        .await
        .unwrap_or(false);
    let by = |entry: Entry| {
        entry
            .account(&account.username)
            .client_ip(ip)
            .target_account(&account.username)
    };
    if !verified {
        audit::log(
            &state.db,
            by(Entry::failed(CHANGE_PASSWORD, "wrong_password")),
        )
        .await;
        // 403, not 401: the session is fine, and the page must not log out.
        return error(StatusCode::FORBIDDEN, "the current password is wrong");
    }
    state.throttle.succeed(&account.username, ip, &attempt);

    let hash = match hash(change.new_password).await {
        Ok(hash) => hash,
        Err(response) => return *response,
    };
    let current = crate::auth::session_key(&headers).expect("require_session found the cookie");
    // ASVS 7.2.4: the change is a new authentication, so the caller gets a
    // new session too.
    let (token, new) = match crate::auth::new_session(account.id, ip, &headers) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("lodger: change the password: {e}");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot change the password",
            );
        }
    };
    let csrf_token = new.csrf_token.clone();
    match state
        .db
        .change_password(account.id, hash, current, new)
        .await
    {
        Ok(ended_sessions) => {
            audit::log(&state.db, by(Entry::ok(CHANGE_PASSWORD))).await;
            eprintln!(
                "lodger: accounts: {} changed the password; {ended_sessions} other sessions ended",
                account.username
            );
            let mut response = Json(Changed {
                ended_sessions,
                csrf_token,
            })
            .into_response();
            response.headers_mut().insert(
                axum::http::header::SET_COOKIE,
                crate::auth::set_cookie(&token),
            );
            response
        }
        Err(e) => database_failed("change the password", &e),
    }
}
