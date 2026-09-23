//! The SQLite database in the state directory (TAD sections 5.1 and 5.2).
//!
//! One connection, which `tokio-rusqlite` runs on its own thread, serves
//! every query. The file and its directory are private to the Lodger user:
//! the directory has mode 0700 and the database 0600. SQLite gives the WAL
//! and shared-memory files the mode of the database file.

use std::fs::{DirBuilder, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;
use rusqlite_migration::{M, Migrations};

/// The database file in the state directory.
pub const FILE: &str = "lodger.db";

/// Every migration, in order. Never edit or remove a released one: add a new
/// one instead. `user_version` counts how many have run.
const MIGRATIONS: &[M<'static>] = &[M::up(include_str!("migrations/0001_initial.sql"))];

/// The current time as SQL, in the ISO 8601 form that the tables store.
/// ISO 8601 strings in UTC compare in time order.
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";
/// A session ends after 60 minutes without use (TAD 7.2).
const IDLE_CUTOFF: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-60 minutes')";
/// A session ends 24 hours after it started, whatever its use (TAD 7.2).
const ABSOLUTE_HOURS: u32 = 24;

/// An account row, for the login check.
#[derive(Debug, Clone)]
pub struct Account {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
}

/// An account as the account page shows it: no password hash.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AccountInfo {
    pub id: i64,
    pub username: String,
    pub created_at: String,
    pub password_changed_at: String,
}

/// What [`Db::delete_account`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deleted {
    Yes,
    NoSuchAccount,
    /// Refused: it is the last account, and Lodger would be locked.
    LastAccount,
}

/// One row of the audit log. `detail_json` holds only allowlisted fields:
/// never a password, a token, or cloud-init user-data (TAD 5.1).
#[derive(Debug, Clone)]
pub struct AuditRow {
    pub event: &'static str,
    pub target_kind: &'static str,
    pub target_name: String,
    pub result: &'static str,
    pub detail_json: String,
}

/// The data of a new session.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub token_sha256: [u8; 32],
    pub account_id: i64,
    pub csrf_token: String,
    pub client_ip: String,
    pub user_agent: Option<String>,
}

/// A live session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub account_id: i64,
    pub username: String,
    /// For the `X-CSRF-Token` check (Task 2.4).
    pub csrf_token: String,
}

/// A handle to the database. Clone it freely; every clone uses the same
/// connection.
#[derive(Clone)]
pub struct Db {
    conn: tokio_rusqlite::Connection,
    path: PathBuf,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Db {
    /// Creates the state directory and the database if needed, sets their
    /// modes, and runs the migrations that have not run yet.
    pub async fn open(state_dir: &Path) -> Result<Self, String> {
        let path = state_dir.join(FILE);
        prepare_files(state_dir, &path)?;
        let conn = tokio_rusqlite::Connection::open(&path)
            .await
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        conn.call(|c| -> Result<(), String> {
            let pragma = |name: &str, value: &dyn rusqlite::ToSql| {
                c.pragma_update(None, name, value)
                    .map_err(|e| format!("PRAGMA {name}: {e}"))
            };
            // WAL keeps the file consistent after a power cut. FULL makes
            // each commit durable, which the audit log wants; Lodger writes
            // little, so the cost does not matter.
            pragma("journal_mode", &"WAL")?;
            pragma("synchronous", &"FULL")?;
            // SQLite enforces foreign keys, and so ON DELETE CASCADE, only
            // when a connection asks for it.
            pragma("foreign_keys", &true)?;
            pragma("busy_timeout", &5000)?;
            Migrations::from_slice(MIGRATIONS)
                .to_latest(c)
                .map_err(|e| format!("migrations: {e}"))
        })
        .await
        .map_err(|e| format!("cannot prepare {}: {e}", path.display()))?;
        Ok(Self { conn, path })
    }

    /// An in-memory database with every migration, for tests.
    #[cfg(test)]
    pub async fn in_memory() -> Self {
        let conn = tokio_rusqlite::Connection::open_in_memory().await.unwrap();
        conn.call(|c| -> Result<(), String> {
            c.pragma_update(None, "foreign_keys", true)
                .map_err(|e| e.to_string())?;
            Migrations::from_slice(MIGRATIONS)
                .to_latest(c)
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap();
        Self {
            conn,
            path: PathBuf::from(":memory:"),
        }
    }

    /// The number of accounts. Setup is open only while it is 0.
    pub async fn account_count(&self) -> Result<i64, String> {
        self.conn
            .call(|c| c.query_row("SELECT count(*) FROM accounts", [], |r| r.get(0)))
            .await
            .map_err(|e| e.to_string())
    }

    /// Creates the first account, in one transaction that sees no other
    /// account. Returns `false` when an account already exists, for example
    /// because a parallel setup claim won.
    pub async fn create_first_account(
        &self,
        username: String,
        password_hash: String,
    ) -> Result<bool, String> {
        self.conn
            .call(move |c| {
                // IMMEDIATE takes the write lock at BEGIN, so the count and
                // the insert see the same database.
                let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let count: i64 = tx.query_row("SELECT count(*) FROM accounts", [], |r| r.get(0))?;
                if count > 0 {
                    return Ok(false);
                }
                tx.execute(
                    "INSERT INTO accounts (username, password_hash, created_at, password_changed_at)
                     VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                             strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                    [&username, &password_hash],
                )?;
                tx.commit()?;
                Ok(true)
            })
            .await
            .map_err(|e: tokio_rusqlite::Error<rusqlite::Error>| e.to_string())
    }

    /// Every account, by name.
    pub async fn list_accounts(&self) -> Result<Vec<AccountInfo>, String> {
        self.conn
            .call(|c| {
                let mut stmt = c.prepare(
                    "SELECT id, username, created_at, password_changed_at
                     FROM accounts ORDER BY username",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok(AccountInfo {
                        id: r.get(0)?,
                        username: r.get(1)?,
                        created_at: r.get(2)?,
                        password_changed_at: r.get(3)?,
                    })
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// Adds an account. Returns its id, or `None` when the name is taken,
    /// compared without regard to case.
    pub async fn create_account(
        &self,
        username: String,
        password_hash: String,
    ) -> Result<Option<i64>, String> {
        self.conn
            .call(move |c| {
                let inserted = c.execute(
                    &format!(
                        "INSERT INTO accounts (username, password_hash, created_at, password_changed_at)
                         VALUES (?1, ?2, {NOW}, {NOW})"
                    ),
                    [&username, &password_hash],
                );
                match inserted {
                    Ok(_) => Ok(Some(c.last_insert_rowid())),
                    Err(rusqlite::Error::SqliteFailure(e, _))
                        if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
                    {
                        Ok(None)
                    }
                    Err(e) => Err(e),
                }
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// Deletes an account and, through the foreign key, all its sessions
    /// (ASVS 7.4.2). The count and the delete run in one transaction, so two
    /// parallel deletes cannot remove the last two accounts.
    pub async fn delete_account(&self, id: i64) -> Result<Deleted, String> {
        self.conn
            .call(move |c| {
                let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let count: i64 = tx.query_row("SELECT count(*) FROM accounts", [], |r| r.get(0))?;
                let exists: bool = tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = ?1)",
                    [id],
                    |r| r.get(0),
                )?;
                let outcome = if !exists {
                    Deleted::NoSuchAccount
                } else if count <= 1 {
                    Deleted::LastAccount
                } else {
                    tx.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
                    Deleted::Yes
                };
                tx.commit()?;
                Ok(outcome)
            })
            .await
            .map_err(|e: tokio_rusqlite::Error<rusqlite::Error>| e.to_string())
    }

    /// The account with this id.
    pub async fn account_by_id(&self, id: i64) -> Result<Option<Account>, String> {
        self.conn
            .call(move |c| {
                c.query_row(
                    "SELECT id, username, password_hash FROM accounts WHERE id = ?1",
                    [id],
                    |r| {
                        Ok(Account {
                            id: r.get(0)?,
                            username: r.get(1)?,
                            password_hash: r.get(2)?,
                        })
                    },
                )
                .optional()
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// Stores a new password hash and ends every session of the account
    /// except `keep` (ASVS 7.4.3), in one transaction. Returns how many
    /// sessions ended.
    pub async fn change_password(
        &self,
        account_id: i64,
        password_hash: String,
        keep: [u8; 32],
    ) -> Result<usize, String> {
        self.conn
            .call(move |c| {
                let tx = c.transaction()?;
                tx.execute(
                    &format!(
                        "UPDATE accounts SET password_hash = ?1, password_changed_at = {NOW}
                         WHERE id = ?2"
                    ),
                    rusqlite::params![password_hash, account_id],
                )?;
                let ended = tx.execute(
                    "DELETE FROM sessions WHERE account_id = ?1 AND token_sha256 != ?2",
                    rusqlite::params![account_id, keep.as_slice()],
                )?;
                tx.commit()?;
                Ok(ended)
            })
            .await
            .map_err(|e: tokio_rusqlite::Error<rusqlite::Error>| e.to_string())
    }

    /// Stores a new password hash for the account with this username and
    /// ends all of its sessions, in one transaction. Returns how many
    /// sessions ended, or `None` when no account has the name.
    pub async fn reset_password(
        &self,
        username: String,
        password_hash: String,
    ) -> Result<Option<usize>, String> {
        self.conn
            .call(move |c| {
                // IMMEDIATE takes the write lock before the read, so a write by
                // the running service cannot fail the upgrade with SQLITE_BUSY.
                let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let id: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM accounts WHERE username = ?1",
                        [&username],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(id) = id else {
                    return Ok(None);
                };
                tx.execute(
                    &format!(
                        "UPDATE accounts SET password_hash = ?1, password_changed_at = {NOW}
                         WHERE id = ?2"
                    ),
                    rusqlite::params![password_hash, id],
                )?;
                let ended = tx.execute("DELETE FROM sessions WHERE account_id = ?1", [id])?;
                tx.commit()?;
                Ok(Some(ended))
            })
            .await
            .map_err(|e: tokio_rusqlite::Error<rusqlite::Error>| e.to_string())
    }

    /// Adds a row to the audit log, with the current time.
    pub async fn audit(&self, row: AuditRow) -> Result<(), String> {
        self.conn
            .call(move |c| {
                c.execute(
                    &format!(
                        "INSERT INTO audit_log (ts, event, target_kind, target_name, result, detail_json)
                         VALUES ({NOW}, ?1, ?2, ?3, ?4, ?5)"
                    ),
                    rusqlite::params![
                        row.event,
                        row.target_kind,
                        row.target_name,
                        row.result,
                        row.detail_json
                    ],
                )
                .map(drop)
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// The account with this username, compared without regard to case.
    pub async fn find_account(&self, username: String) -> Result<Option<Account>, String> {
        self.conn
            .call(move |c| {
                c.query_row(
                    "SELECT id, username, password_hash FROM accounts WHERE username = ?1",
                    [&username],
                    |r| {
                        Ok(Account {
                            id: r.get(0)?,
                            username: r.get(1)?,
                            password_hash: r.get(2)?,
                        })
                    },
                )
                .optional()
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// Stores a new session. The token itself never reaches the database:
    /// only its SHA-256. It ends 24 hours after its start at the latest.
    pub async fn create_session(&self, new: NewSession) -> Result<(), String> {
        self.conn
            .call(move |c| {
                // A login is a good moment to drop sessions that ended.
                c.execute(
                    &format!(
                        "DELETE FROM sessions WHERE expires_at <= {NOW} OR last_seen_at <= {IDLE_CUTOFF}"
                    ),
                    [],
                )?;
                c.execute(
                    &format!(
                        "INSERT INTO sessions (token_sha256, account_id, csrf_token, created_at,
                             last_seen_at, expires_at, client_ip, user_agent)
                         VALUES (?1, ?2, ?3, {NOW}, {NOW},
                                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+{ABSOLUTE_HOURS} hours'), ?4, ?5)"
                    ),
                    rusqlite::params![
                        new.token_sha256.as_slice(),
                        new.account_id,
                        new.csrf_token,
                        new.client_ip,
                        new.user_agent
                    ],
                )?;
                Ok(())
            })
            .await
            .map_err(|e: tokio_rusqlite::Error<rusqlite::Error>| e.to_string())
    }

    /// The live session with this token hash, or `None`. A live session is
    /// younger than 24 hours and was used in the last 60 minutes (TAD 7.2).
    /// Using it counts as activity, recorded at most once a minute.
    pub async fn session(&self, token_sha256: [u8; 32]) -> Result<Option<Session>, String> {
        self.conn
            .call(move |c| {
                let found = c
                    .query_row(
                        &format!(
                            "SELECT s.account_id, a.username, s.csrf_token
                             FROM sessions s JOIN accounts a ON a.id = s.account_id
                             WHERE s.token_sha256 = ?1
                               AND s.expires_at > {NOW} AND s.last_seen_at > {IDLE_CUTOFF}"
                        ),
                        [token_sha256.as_slice()],
                        |r| {
                            Ok(Session {
                                account_id: r.get(0)?,
                                username: r.get(1)?,
                                csrf_token: r.get(2)?,
                            })
                        },
                    )
                    .optional()?;
                if found.is_some() {
                    c.execute(
                        &format!(
                            "UPDATE sessions SET last_seen_at = {NOW} WHERE token_sha256 = ?1
                               AND last_seen_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 minutes')"
                        ),
                        [token_sha256.as_slice()],
                    )?;
                }
                Ok(found)
            })
            .await
            .map_err(|e: tokio_rusqlite::Error<rusqlite::Error>| e.to_string())
    }

    /// Whether the session is still live. Unlike [`Db::session`], it does
    /// not count as use, so an open WebSocket does not keep a session alive.
    pub async fn session_alive(&self, token_sha256: [u8; 32]) -> Result<bool, String> {
        self.conn
            .call(move |c| {
                c.query_row(
                    &format!(
                        "SELECT EXISTS (SELECT 1 FROM sessions WHERE token_sha256 = ?1
                           AND expires_at > {NOW} AND last_seen_at > {IDLE_CUTOFF})"
                    ),
                    [token_sha256.as_slice()],
                    |r| r.get(0),
                )
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// Ends the session with this token hash, if it exists.
    pub async fn delete_session(&self, token_sha256: [u8; 32]) -> Result<(), String> {
        self.conn
            .call(move |c| {
                c.execute(
                    "DELETE FROM sessions WHERE token_sha256 = ?1",
                    [token_sha256.as_slice()],
                )
                .map(drop)
            })
            .await
            .map_err(|e| e.to_string())
    }

    /// Runs a trivial query, for the health check.
    pub async fn ping(&self) -> Result<(), String> {
        self.conn
            .call(|c| c.query_row("SELECT 1", [], |_| Ok(())))
            .await
            .map_err(|e| e.to_string())
    }

    /// Runs `f` on the connection's thread.
    #[cfg(test)]
    pub async fn call<R, F>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut rusqlite::Connection) -> rusqlite::Result<R> + Send + 'static,
        R: Send + 'static,
    {
        self.conn.call(f).await.map_err(|e| e.to_string())
    }
}

/// Gives the state directory mode 0700 and the database file mode 0600,
/// before SQLite opens it, so the file is never readable by others, not even
/// for a moment. Both are created if they are missing, and both get their
/// mode again if they exist.
fn prepare_files(state_dir: &Path, path: &Path) -> Result<(), String> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(state_dir)
        .map_err(|e| format!("cannot create {}: {e}", state_dir.display()))?;
    // DirBuilder sets the mode only on a directory that it creates. systemd
    // creates `StateDirectory=` itself, with mode 0755 unless the unit says
    // otherwise, so set 0700 every time.
    std::fs::set_permissions(state_dir, Permissions::from_mode(0o700))
        .map_err(|e| format!("cannot set the mode of {}: {e}", state_dir.display()))?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::set_permissions(path, Permissions::from_mode(0o600))
                .map_err(|e| format!("cannot set the mode of {}: {e}", path.display()))
        }
        Err(e) => Err(format!("cannot create {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests;
