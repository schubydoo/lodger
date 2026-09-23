use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use rusqlite_migration::Migrations;

use super::{Db, FILE, MIGRATIONS};

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Every table and its columns, in column order.
fn schema(c: &rusqlite::Connection) -> rusqlite::Result<BTreeMap<String, Vec<String>>> {
    let mut tables = c.prepare(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let names: Vec<String> = tables
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut out = BTreeMap::new();
    for name in names {
        let mut cols = c.prepare(&format!("SELECT name FROM pragma_table_info('{name}')"))?;
        let cols: Vec<String> = cols
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        out.insert(name, cols);
    }
    Ok(out)
}

/// The whole schema as SQL, to compare two states exactly.
fn schema_sql(c: &rusqlite::Connection) -> rusqlite::Result<Vec<String>> {
    let mut s = c.prepare("SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")?;
    s.query_map([], |r| r.get(0))?.collect()
}

#[tokio::test]
async fn a_fresh_start_creates_a_private_database() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    let db = Db::open(&state).await.unwrap();

    assert_eq!(mode(&state), 0o700);
    assert_eq!(mode(&state.join(FILE)), 0o600);
    // A write creates the WAL file, which gets the database file's mode.
    db.call(|c| c.execute("INSERT INTO settings VALUES ('k', '1', 'now')", []))
        .await
        .unwrap();
    assert_eq!(mode(&state.join(format!("{FILE}-wal"))), 0o600);
}

#[tokio::test]
async fn an_existing_database_gets_mode_0600_again() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join(FILE);
    std::fs::write(&file, b"").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    Db::open(tmp.path()).await.unwrap();
    assert_eq!(mode(&file), 0o600);
}

#[tokio::test]
async fn an_existing_state_directory_gets_mode_0700() {
    // As systemd creates StateDirectory= by default.
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
    Db::open(&state).await.unwrap();
    assert_eq!(mode(&state), 0o700);
}

#[tokio::test]
async fn migrations_run_once_and_a_second_run_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let first = Db::open(tmp.path()).await.unwrap();
    let (version, before) = first
        .call(|c| {
            Ok((
                c.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))?,
                schema_sql(c)?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(version, i64::try_from(MIGRATIONS.len()).unwrap());
    drop(first);

    let second = Db::open(tmp.path()).await.unwrap();
    let (again, after) = second
        .call(|c| {
            Ok((
                c.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))?,
                schema_sql(c)?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(again, version);
    assert_eq!(after, before);
}

#[test]
fn the_migrations_are_valid() {
    // rusqlite_migration runs them on a fresh in-memory database.
    Migrations::from_slice(MIGRATIONS).validate().unwrap();
}

/// PRD 5.6: libvirt is the source of truth, so SQLite holds no VM data. The
/// exact schema is fixed here: a new table or column must pass review.
#[tokio::test]
async fn the_database_holds_no_vm_data() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path()).await.unwrap();
    let schema = db.call(|c| schema(c)).await.unwrap();

    let expected: BTreeMap<String, Vec<String>> = [
        (
            "accounts",
            &[
                "id",
                "username",
                "password_hash",
                "totp_secret_enc",
                "totp_enabled",
                "created_at",
                "password_changed_at",
            ][..],
        ),
        (
            "audit_log",
            &[
                "id",
                "ts",
                "account_name",
                "client_ip",
                "event",
                "target_kind",
                "target_name",
                "job_id",
                "result",
                "detail_json",
            ],
        ),
        (
            "recovery_codes",
            &["id", "account_id", "code_hash", "used_at"],
        ),
        (
            "sessions",
            &[
                "token_sha256",
                "account_id",
                "csrf_token",
                "created_at",
                "last_seen_at",
                "expires_at",
                "client_ip",
                "user_agent",
            ],
        ),
        ("settings", &["key", "value_json", "updated_at"]),
    ]
    .into_iter()
    .map(|(t, cols)| (t.to_owned(), cols.iter().map(|c| (*c).to_owned()).collect()))
    .collect();
    assert_eq!(schema, expected);

    // A second guard, for when the list above changes: no column names a
    // libvirt object or its configuration.
    for (table, cols) in &schema {
        for col in cols {
            for word in [
                "vm", "domain", "uuid", "pool", "volume", "network", "snapshot", "xml", "disk",
            ] {
                assert!(
                    !col.split('_').any(|part| part == word),
                    "{table}.{col} looks like VM data"
                );
            }
        }
    }
}

#[tokio::test]
async fn deleting_an_account_deletes_its_sessions_and_codes() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path()).await.unwrap();
    let left = db
        .call(|c| {
            c.execute(
                "INSERT INTO accounts (username, password_hash, created_at, password_changed_at)
                 VALUES ('admin', 'x', 'now', 'now')",
                [],
            )?;
            let id = c.last_insert_rowid();
            c.execute(
                "INSERT INTO sessions VALUES (x'00', ?1, 'csrf', 'now', 'now', 'later', NULL, NULL)",
                [id],
            )?;
            c.execute("INSERT INTO recovery_codes (account_id, code_hash) VALUES (?1, 'h')", [id])?;
            c.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
            let sessions: i64 = c.query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))?;
            let codes: i64 = c.query_row("SELECT count(*) FROM recovery_codes", [], |r| r.get(0))?;
            Ok((sessions, codes))
        })
        .await
        .unwrap();
    assert_eq!(left, (0, 0));
}

#[tokio::test]
async fn usernames_are_unique_without_regard_to_case() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path()).await.unwrap();
    let second = db
        .call(|c| {
            let add =
                "INSERT INTO accounts (username, password_hash, created_at, password_changed_at)
                       VALUES (?1, 'x', 'now', 'now')";
            c.execute(add, ["admin"])?;
            c.execute(add, ["Admin"])
        })
        .await;
    assert!(second.unwrap_err().contains("UNIQUE"));
}

#[tokio::test]
async fn the_database_uses_wal_and_strict_tables() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path()).await.unwrap();
    let (journal, wrong_type) = db
        .call(|c| {
            let journal: String = c.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
            // STRICT rejects text in an INTEGER column.
            let wrong = c.execute(
                "INSERT INTO accounts (username, password_hash, totp_enabled, created_at, password_changed_at)
                 VALUES ('a', 'x', 'yes', 'now', 'now')",
                [],
            );
            Ok((journal, wrong.is_err()))
        })
        .await
        .unwrap();
    assert_eq!(journal, "wal");
    assert!(wrong_type);
}

#[tokio::test]
async fn ping_works() {
    let tmp = tempfile::tempdir().unwrap();
    Db::open(tmp.path()).await.unwrap().ping().await.unwrap();
}

#[tokio::test]
async fn a_state_directory_that_is_a_file_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let not_a_dir = tmp.path().join("file");
    std::fs::write(&not_a_dir, b"x").unwrap();
    let err = Db::open(&not_a_dir).await.unwrap_err();
    assert!(err.starts_with("cannot create"), "{err}");
}

#[tokio::test]
async fn a_file_that_is_not_a_database_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join(FILE),
        b"this is not an SQLite database, only text",
    )
    .unwrap();
    let err = Db::open(tmp.path()).await.unwrap_err();
    assert!(err.starts_with("cannot prepare"), "{err}");
}

/// The guard behind "two parallel claims create exactly one account": claims
/// that pass the setup token check at the same time all reach this call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_one_first_account_can_be_created() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path()).await.unwrap();
    let calls = (0..8).map(|n| {
        let db = db.clone();
        tokio::spawn(async move {
            db.create_first_account(format!("admin{n}"), "hash".into())
                .await
        })
    });
    let mut created = 0;
    for call in calls {
        if call.await.unwrap().unwrap() {
            created += 1;
        }
    }
    assert_eq!(created, 1);
    assert_eq!(db.account_count().await.unwrap(), 1);
}
