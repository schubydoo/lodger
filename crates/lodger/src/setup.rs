//! First-run setup: a one-time token claims a new install (TAD section 7.1,
//! PRD F2).
//!
//! At a start with no accounts, Lodger makes a 128-bit token and writes it to
//! the log, which systemd sends to the journal. Only the token's SHA-256 hash
//! stays in memory. The token works once and for 60 minutes. A restart before
//! the first account writes a new token, and the old one stops working.
//!
//! `POST /api/setup` checks the token before anything else, so a guess costs
//! the server no password hashing. The account is created in one database
//! transaction that fails when any account exists, so two parallel claims
//! create exactly one account. After that, `/api/setup` answers 404.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::password_hash::PasswordHasher;
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use lodger_core::validate::Name;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::server::AppState;

/// How long a setup token works (PRD section 5.3).
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// The hash of the active setup token and when it stops working.
#[derive(Debug)]
pub struct SetupToken {
    hash: [u8; 32],
    expires: Instant,
}

impl SetupToken {
    /// Makes a new token. Returns the token text, which only the log gets,
    /// and the value to keep.
    pub fn generate(now: Instant) -> Result<(String, Self), String> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| format!("cannot make a setup token: {e}"))?;
        let text = hex::encode(bytes);
        let token = Self {
            hash: Sha256::digest(text.as_bytes()).into(),
            expires: now + TOKEN_LIFETIME,
        };
        Ok((text, token))
    }

    /// Compares `submitted` with the token in constant time.
    fn matches(&self, submitted: &str) -> bool {
        let hash: [u8; 32] = Sha256::digest(submitted.trim().as_bytes()).into();
        self.hash.ct_eq(&hash).into()
    }
}

/// The setup state: `Some` while a token can claim the install.
pub type Setup = Mutex<Option<SetupToken>>;

#[derive(Debug, Serialize)]
pub struct Open {
    /// Always `true`: the endpoint answers 404 when setup is closed.
    pub required: bool,
}

/// `GET /api/setup`: 200 while setup is open, 404 after it.
pub async fn status(State(state): State<AppState>) -> Response {
    let open = state
        .setup
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|t| Instant::now() < t.expires);
    if open {
        Json(Open { required: true }).into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

#[derive(Deserialize)]
pub struct Claim {
    pub token: String,
    pub username: String,
    pub password: String,
}

/// Shows only the username: the token and the password must never reach a
/// log line, not even through a `{claim:?}` added later.
impl std::fmt::Debug for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claim")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Serialize)]
pub struct Created {
    pub username: String,
}

fn error(code: StatusCode, message: impl Into<String>) -> Response {
    (code, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

/// `POST /api/setup`: creates the first account.
///
/// The body is read only after the Origin and the setup state are checked,
/// so a closed setup answers 404 whatever the request carries.
pub async fn claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Claim>, JsonRejection>,
) -> Response {
    // 1. Only a page from this server may claim the install. Setup needs no
    //    session, but it changes state (review rule 7). Task 2.4 adds the
    //    general Origin and CSRF checks.
    if !crate::ws::same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // 2. Setup must be open, and the token must match. Nothing else runs for
    //    a wrong token.
    if state
        .setup
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_none()
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let claim = match body {
        Ok(Json(claim)) => claim,
        Err(rejection) => return rejection.into_response(),
    };
    {
        let setup = state
            .setup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(token) = setup.as_ref() else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if !token.matches(&claim.token) {
            return error(StatusCode::FORBIDDEN, "wrong setup token");
        }
        if Instant::now() >= token.expires {
            return error(
                StatusCode::FORBIDDEN,
                "the setup token expired. Restart Lodger to write a new one to the log",
            );
        }
    }

    // 3. The account's own rules.
    let username = match Name::parse("username", &claim.username) {
        Ok(name) => name,
        Err(e) => return error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
    };
    if let Err(e) = lodger_core::password::check(&claim.password) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string());
    }

    // 4. argon2id takes about 19 MiB and tens of milliseconds: off the
    //    async threads.
    let password = claim.password;
    let hash = match tokio::task::spawn_blocking(move || hash_password(&password)).await {
        Ok(Ok(hash)) => hash,
        Ok(Err(e)) => {
            eprintln!("lodger: setup: cannot hash the password: {e}");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot hash the password",
            );
        }
        Err(e) => {
            eprintln!("lodger: setup: the hashing task failed: {e}");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot hash the password",
            );
        }
    };

    // 5. One transaction: only a database with no accounts takes the first.
    let name = username.as_str().to_owned();
    let created = state.db.create_first_account(name.clone(), hash).await;
    match created {
        Ok(true) => {
            *state
                .setup
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            eprintln!("lodger: setup done: created the first account, {name}");
            (StatusCode::CREATED, Json(Created { username: name })).into_response()
        }
        // Another claim won the race.
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            eprintln!("lodger: setup: the database failed: {e}");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot create the account",
            )
        }
    }
}

/// Hashes a password with argon2id at the OWASP minimum parameters, which
/// are the `argon2` defaults (TAD section 7.1).
pub fn hash_password(password: &str) -> Result<String, String> {
    argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{SetupToken, TOKEN_LIFETIME, hash_password};

    #[test]
    fn a_token_is_128_random_bits_in_hex() {
        let (a, _) = SetupToken::generate(Instant::now()).unwrap();
        let (b, _) = SetupToken::generate(Instant::now()).unwrap();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn only_the_token_matches() {
        let (text, token) = SetupToken::generate(Instant::now()).unwrap();
        assert!(token.matches(&text));
        // Copied from a terminal, with a newline.
        assert!(token.matches(&format!(" {text}\n")));
        assert!(!token.matches(&text.to_uppercase()));
        assert!(!token.matches(""));
        assert!(!token.matches(&text[..31]));
    }

    #[test]
    fn a_token_lives_60_minutes() {
        let now = Instant::now();
        let (_, token) = SetupToken::generate(now).unwrap();
        assert_eq!(token.expires - now, TOKEN_LIFETIME);
        assert_eq!(TOKEN_LIFETIME, Duration::from_secs(3600));
    }

    #[test]
    fn a_claim_never_shows_its_secrets_in_debug_output() {
        let claim = super::Claim {
            token: "0123456789abcdef0123456789abcdef".into(),
            username: "admin".into(),
            password: "correct horse battery staple".into(),
        };
        let shown = format!("{claim:?}");
        assert!(shown.contains("admin"), "{shown}");
        assert!(!shown.contains("0123456789abcdef"), "{shown}");
        assert!(!shown.contains("correct horse"), "{shown}");
    }

    #[test]
    fn the_hash_is_argon2id_with_the_owasp_minimum() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(
            hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "{hash}"
        );
        // A fresh salt each time.
        assert_ne!(hash, hash_password("correct horse battery staple").unwrap());
    }
}
