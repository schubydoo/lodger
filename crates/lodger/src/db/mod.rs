//! The SQLite database in the state directory (TAD sections 5.1 and 5.2).
//!
//! One connection, which `tokio-rusqlite` runs on its own thread, serves
//! every query. The file and its directory are private to the Lodger user:
//! the directory has mode 0700 and the database 0600. SQLite gives the WAL
//! and shared-memory files the mode of the database file.

use std::fs::{DirBuilder, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use rusqlite_migration::{M, Migrations};

/// The database file in the state directory.
pub const FILE: &str = "lodger.db";

/// Every migration, in order. Never edit or remove a released one: add a new
/// one instead. `user_version` counts how many have run.
const MIGRATIONS: &[M<'static>] = &[M::up(include_str!("migrations/0001_initial.sql"))];

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
