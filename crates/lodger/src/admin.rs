//! `lodger admin`: account recovery without the web UI (PRD F2, TAD 4.3).
//!
//! - `reset-password <username>` sets a new password and ends every session
//!   of the account.
//! - `create <username>` adds an account.
//!
//! Both need root, because the database is private to the Lodger user. They
//! open only a database that exists: a new one that root created would be
//! useless to the service. The running service sees each change at once,
//! because it reads sessions and accounts from the database on every
//! request. Each command writes an audit row. The new password must pass
//! the same policy as in the web UI.

use std::io::IsTerminal;
use std::path::Path;

use lodger_core::validate::Name;

use crate::audit::{self, Entry, Journal};
use crate::cli::AdminAction;
use crate::config::{Config, Overrides};
use crate::db::{self, Db};

/// The effective user ID from the text of `/proc/<pid>/status`: the second
/// field of the `Uid:` line.
fn effective_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|ids| ids.split_whitespace().nth(1))
        .and_then(|uid| uid.parse().ok())
}

/// Fails unless the process runs as root. An unreadable status also fails.
fn require_root(status: std::io::Result<String>) -> Result<(), String> {
    match status.ok().as_deref().and_then(effective_uid) {
        Some(0) => Ok(()),
        _ => Err("lodger admin must run as root. Use sudo lodger admin ...".to_owned()),
    }
}

/// Runs an admin command and returns the line to print.
pub fn run(action: AdminAction, overrides: Overrides) -> Result<String, String> {
    require_root(std::fs::read_to_string("/proc/self/status"))?;
    let config = Config::load(overrides, std::env::var("STATE_DIRECTORY").ok().as_deref())?;
    let path = config.state_dir.join(db::FILE);
    if !path.exists() {
        return Err(format!(
            "there is no database at {}. Start the Lodger service once, or name the state directory with --state-dir",
            path.display()
        ));
    }
    let ctx = Context {
        journal: Journal::Socket(Path::new(audit::JOURNAL_SOCKET)),
        sudo_user: std::env::var("SUDO_USER").ok(),
    };
    tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|e| format!("cannot start the async runtime: {e}"))?
        .block_on(async {
            let db = Db::open(&config.state_dir).await?;
            match action {
                AdminAction::ResetPassword { username } => {
                    reset_password(&db, &username, read_password, &ctx).await
                }
                AdminAction::Create { username } => {
                    create(&db, &username, read_password, &ctx).await
                }
            }
        })
}

/// Where an admin command sends the audit copy, and who ran it.
struct Context<'a> {
    journal: Journal<'a>,
    /// The user who ran `sudo`.
    sudo_user: Option<String>,
}

impl Context<'_> {
    /// Writes the audit row of an admin command, marked as a CLI change.
    async fn audit(&self, db: &Db, mut entry: Entry) -> Result<(), String> {
        entry.detail.via = Some("cli");
        entry.detail.sudo_user.clone_from(&self.sudo_user);
        audit::record(db, self.journal, entry).await
    }
}

/// Reads the new password: twice from the terminal without echo, or one line
/// from stdin when stdin is not a terminal, for scripts.
fn read_password(username: &str) -> Result<String, String> {
    if std::io::stdin().is_terminal() {
        let first = rpassword::prompt_password(format!("New password for {username}: "))
            .map_err(|e| format!("cannot read the password: {e}"))?;
        let second = rpassword::prompt_password("Repeat the password: ")
            .map_err(|e| format!("cannot read the password: {e}"))?;
        if first != second {
            return Err("the two passwords differ".to_owned());
        }
        Ok(first)
    } else {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("cannot read the password: {e}"))?;
        let end = line.trim_end_matches(['\n', '\r']).len();
        line.truncate(end);
        Ok(line)
    }
}

/// Reads the password with `read` and returns its hash, if it passes the
/// policy.
fn new_hash(
    username: &str,
    read: impl FnOnce(&str) -> Result<String, String>,
) -> Result<String, String> {
    let password = read(username)?;
    lodger_core::password::check(&password).map_err(|e| e.to_string())?;
    crate::passwords::hash(&password).map_err(|e| format!("cannot hash the password: {e}"))
}

const RESET: &str = "account.reset_password";
const CREATE: &str = "account.create";

/// `lodger admin reset-password`.
async fn reset_password(
    db: &Db,
    username: &str,
    read: impl FnOnce(&str) -> Result<String, String>,
    ctx: &Context<'_>,
) -> Result<String, String> {
    let username = parse_name(username)?;
    let username = username.as_str();
    let Some(account) = db.find_account(username.to_owned()).await? else {
        ctx.audit(
            db,
            Entry::failed(RESET, "no_such_account").target_account(username),
        )
        .await?;
        return Err(format!("there is no account called {username}"));
    };
    let hash = new_hash(&account.username, read)?;
    match db.reset_password(account.username.clone(), hash).await? {
        Some(ended) => {
            ctx.audit(db, Entry::ok(RESET).target_account(&account.username))
                .await?;
            Ok(format!(
                "Set a new password for {} and ended {ended} session(s).",
                account.username
            ))
        }
        None => {
            ctx.audit(
                db,
                Entry::failed(RESET, "no_such_account").target_account(username),
            )
            .await?;
            Err(format!("there is no account called {username}"))
        }
    }
}

/// `lodger admin create`.
async fn create(
    db: &Db,
    username: &str,
    read: impl FnOnce(&str) -> Result<String, String>,
    ctx: &Context<'_>,
) -> Result<String, String> {
    let username = parse_name(username)?.as_str().to_owned();
    if db.find_account(username.clone()).await?.is_some() {
        ctx.audit(
            db,
            Entry::failed(CREATE, "name_taken").target_account(&username),
        )
        .await?;
        return Err(format!("an account called {username} exists already"));
    }
    let hash = new_hash(&username, read)?;
    match db.create_account(username.clone(), hash).await? {
        Some(_) => {
            ctx.audit(db, Entry::ok(CREATE).target_account(&username))
                .await?;
            Ok(format!("Created the account {username}."))
        }
        None => {
            ctx.audit(
                db,
                Entry::failed(CREATE, "name_taken").target_account(&username),
            )
            .await?;
            Err(format!("an account called {username} exists already"))
        }
    }
}

/// The username, under the same rules as in the web UI. An invalid name
/// never reaches the audit log.
fn parse_name(username: &str) -> Result<Name, String> {
    Name::parse("username", username).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewSession;

    const GOOD: &str = "a long enough password 42";
    const BETTER: &str = "another long password 43";

    fn given(password: &'static str) -> impl FnOnce(&str) -> Result<String, String> {
        move |_| Ok(password.to_owned())
    }

    fn ctx() -> Context<'static> {
        Context {
            journal: Journal::Stderr,
            sudo_user: Some("schuby".to_owned()),
        }
    }

    /// Every audit row as (`event`, `target_name`, `result`, `detail_json`).
    async fn audit_rows(db: &Db) -> Vec<(String, String, String, String)> {
        db.call(|c| {
            c.prepare("SELECT event, target_name, result, detail_json FROM audit_log ORDER BY id")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect()
        })
        .await
        .unwrap()
    }

    async fn session_count(db: &Db) -> i64 {
        db.call(|c| c.query_row("SELECT count(*) FROM sessions", [], |r| r.get(0)))
            .await
            .unwrap()
    }

    /// A database with the account `alice` (password GOOD) and 2 sessions.
    async fn with_alice() -> Db {
        let db = Db::in_memory().await;
        let id = db
            .create_account("alice".into(), crate::passwords::hash(GOOD).unwrap())
            .await
            .unwrap()
            .unwrap();
        for n in 0..2u8 {
            db.create_session(NewSession {
                token_sha256: [n; 32],
                account_id: id,
                csrf_token: "csrf".into(),
                client_ip: "192.0.2.1".into(),
                user_agent: None,
            })
            .await
            .unwrap();
        }
        db
    }

    #[test]
    fn the_effective_uid_is_the_second_field() {
        let status = "Name:\tlodger\nUid:\t1000\t0\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(effective_uid(status), Some(0));
        assert_eq!(effective_uid("Uid:\t0\t1000\t0\t0\n"), Some(1000));
        assert_eq!(effective_uid("Name:\tlodger\n"), None);
        assert_eq!(effective_uid("Uid:\t0\n"), None);
    }

    #[test]
    fn only_root_passes() {
        assert!(require_root(Ok("Uid:\t0\t0\t0\t0\n".into())).is_ok());
        let e = require_root(Ok("Uid:\t1000\t1000\t1000\t1000\n".into())).unwrap_err();
        assert!(e.contains("must run as root"), "{e}");
        assert!(require_root(Err(std::io::ErrorKind::NotFound.into())).is_err());
    }

    #[test]
    fn this_process_reads_its_own_uid() {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        assert!(effective_uid(&status).is_some(), "{status}");
    }

    #[tokio::test]
    async fn reset_sets_the_password_ends_every_session_and_audits() {
        let db = with_alice().await;
        let line = reset_password(&db, "ALICE", given(BETTER), &ctx())
            .await
            .unwrap();
        assert_eq!(line, "Set a new password for alice and ended 2 session(s).");
        let stored = db.find_account("alice".into()).await.unwrap().unwrap();
        assert!(crate::passwords::verify(BETTER, &stored.password_hash));
        assert!(!crate::passwords::verify(GOOD, &stored.password_hash));
        assert_eq!(session_count(&db).await, 0);
        let rows = audit_rows(&db).await;
        assert_eq!(rows.len(), 1);
        let (event, target, result, detail_json) = &rows[0];
        assert_eq!(
            (event.as_str(), target.as_str(), result.as_str()),
            (RESET, "alice", "ok")
        );
        assert_eq!(detail_json, r#"{"via":"cli","sudo_user":"schuby"}"#);
        assert!(!detail_json.contains(BETTER));
    }

    #[tokio::test]
    async fn reset_of_a_missing_account_asks_nothing_and_audits_the_failure() {
        let db = with_alice().await;
        let e = reset_password(&db, "bob", |_| panic!("no prompt"), &ctx())
            .await
            .unwrap_err();
        assert_eq!(e, "there is no account called bob");
        assert_eq!(session_count(&db).await, 2);
        let rows = audit_rows(&db).await;
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].0.as_str(), rows[0].2.as_str()), (RESET, "failed"));
        assert!(
            rows[0].3.contains(r#""reason":"no_such_account""#),
            "{}",
            rows[0].3
        );
    }

    #[tokio::test]
    async fn a_weak_password_changes_nothing() {
        let db = with_alice().await;
        let e = reset_password(&db, "alice", given("password"), &ctx())
            .await
            .unwrap_err();
        assert!(e.contains("15"), "{e}");
        let stored = db.find_account("alice".into()).await.unwrap().unwrap();
        assert!(crate::passwords::verify(GOOD, &stored.password_hash));
        assert_eq!(session_count(&db).await, 2);
        assert!(audit_rows(&db).await.is_empty());
    }

    #[tokio::test]
    async fn the_prompt_names_the_stored_account() {
        let db = with_alice().await;
        reset_password(
            &db,
            "Alice",
            |name| {
                assert_eq!(name, "alice");
                Ok(BETTER.to_owned())
            },
            &ctx(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_adds_an_account_that_can_log_in_and_audits() {
        let db = with_alice().await;
        let line = create(&db, "bob", given(BETTER), &ctx()).await.unwrap();
        assert_eq!(line, "Created the account bob.");
        let bob = db.find_account("bob".into()).await.unwrap().unwrap();
        assert!(crate::passwords::verify(BETTER, &bob.password_hash));
        let rows = audit_rows(&db).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].0.as_str(), rows[0].1.as_str(), rows[0].2.as_str()),
            (CREATE, "bob", "ok")
        );
    }

    #[tokio::test]
    async fn create_refuses_a_taken_name_and_audits_the_failure() {
        let db = with_alice().await;
        let e = create(&db, "Alice", |_| panic!("no prompt"), &ctx())
            .await
            .unwrap_err();
        assert_eq!(e, "an account called Alice exists already");
        let rows = audit_rows(&db).await;
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].0.as_str(), rows[0].2.as_str()), (CREATE, "failed"));
        assert!(
            rows[0].3.contains(r#""reason":"name_taken""#),
            "{}",
            rows[0].3
        );
    }

    #[tokio::test]
    async fn an_invalid_name_or_weak_password_creates_nothing() {
        let db = Db::in_memory().await;
        assert!(
            create(&db, "", |_| panic!("no prompt"), &ctx())
                .await
                .is_err()
        );
        assert!(create(&db, "bob", given("short"), &ctx()).await.is_err());
        assert_eq!(db.account_count().await.unwrap(), 0);
        assert!(audit_rows(&db).await.is_empty());
    }
}
