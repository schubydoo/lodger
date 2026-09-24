//! The audit log: who changed what, from where, and with what result (PRD
//! 7.2, TAD 5.1 and 6.7).
//!
//! [`record`] writes a row to `audit_log` and a copy to journald. The copy is
//! one line, `lodger: audit: ` and the row as JSON. JSON escapes control
//! characters, so a crafted name cannot forge log lines. The service writes
//! the copy to stderr, which systemd sends to the journal. The admin CLI runs
//! in a terminal, so it sends the copy to the journal socket instead.
//!
//! Only allowlisted fields reach a row: the event, the account, the client
//! IP, the target, the result, and the [`Detail`] fields. None of them can
//! hold a password, a token, or cloud-init user-data. A failed login for an
//! unknown name stores no name, because users sometimes type their password
//! into the name field.
//!
//! [`delete_old_rows`] deletes rows older than 365 days once a day (TAD 5.2).

use std::net::IpAddr;
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::db::{AuditRow, Db};

/// The journal's socket for the native protocol.
pub const JOURNAL_SOCKET: &str = "/run/systemd/journal/socket";

/// How long Lodger keeps audit rows (TAD 5.2).
pub const KEEP_DAYS: u32 = 365;

/// The allowlisted detail fields. Add a field here only if it can never
/// hold a secret.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Detail {
    /// `cli` for a change through `lodger admin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<&'static str>,
    /// The user who ran `sudo lodger admin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sudo_user: Option<String>,
    /// The action on the target, such as `start`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<&'static str>,
    /// Why a change failed, as a fixed code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

/// One audit event.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub event: &'static str,
    /// The Lodger account that acted, or that tried to log in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ip: Option<IpAddr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// `ok` or `failed`.
    pub result: &'static str,
    #[serde(skip_serializing_if = "is_empty")]
    pub detail: Detail,
}

fn is_empty(detail: &Detail) -> bool {
    *detail == Detail::default()
}

impl Entry {
    /// An event with the result `ok` and nothing else set.
    pub fn ok(event: &'static str) -> Self {
        Self {
            event,
            account: None,
            client_ip: None,
            target_kind: None,
            target: None,
            result: "ok",
            detail: Detail::default(),
        }
    }

    /// An event with the result `failed` and a fixed reason code.
    pub fn failed(event: &'static str, reason: &'static str) -> Self {
        let mut entry = Self::ok(event);
        entry.result = "failed";
        entry.detail.reason = Some(reason);
        entry
    }

    pub fn account(mut self, name: impl Into<String>) -> Self {
        self.account = Some(name.into());
        self
    }

    pub fn client_ip(mut self, ip: IpAddr) -> Self {
        self.client_ip = Some(ip);
        self
    }

    /// The account that the change touched.
    pub fn target_account(mut self, name: impl Into<String>) -> Self {
        self.target_kind = Some("account");
        self.target = Some(name.into());
        self
    }

    /// The VM that the change touched.
    pub fn target_vm(mut self, name: impl Into<String>) -> Self {
        self.target_kind = Some("vm");
        self.target = Some(name.into());
        self
    }

    /// The storage pool that the change touched.
    pub fn target_pool(mut self, name: impl Into<String>) -> Self {
        self.target_kind = Some("pool");
        self.target = Some(name.into());
        self
    }

    /// The journald line.
    fn line(&self) -> String {
        let json = serde_json::to_string(self).expect("an entry always serializes");
        format!("lodger: audit: {json}")
    }

    fn row(self) -> AuditRow {
        let detail_json = (!is_empty(&self.detail))
            .then(|| serde_json::to_string(&self.detail).expect("a detail always serializes"));
        AuditRow {
            event: self.event,
            account_name: self.account,
            client_ip: self.client_ip.map(|ip| ip.to_string()),
            target_kind: self.target_kind,
            target_name: self.target,
            result: self.result,
            detail_json,
        }
    }
}

/// Where the journald copy goes.
#[derive(Debug, Clone, Copy)]
pub enum Journal<'a> {
    /// stderr, which systemd sends to the journal for the service.
    Stderr,
    /// The journal socket at this path. If the send fails, stderr.
    Socket(&'a Path),
}

/// Where [`Journal::write`] put a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wrote {
    Socket,
    Stderr,
}

impl Journal<'_> {
    fn write(self, line: &str) -> Wrote {
        if let Journal::Socket(path) = self {
            // The native protocol: one field per line. JSON has no raw
            // newline, so the line is one field.
            let message = format!("SYSLOG_IDENTIFIER=lodger\nPRIORITY=5\nMESSAGE={line}\n");
            let sent = UnixDatagram::unbound().and_then(|s| s.send_to(message.as_bytes(), path));
            if sent.is_ok() {
                return Wrote::Socket;
            }
        }
        eprintln!("{line}");
        Wrote::Stderr
    }
}

/// Writes the journald copy, then the row. The copy comes first, so a
/// database failure still leaves a trace.
pub async fn record(db: &Db, journal: Journal<'_>, entry: Entry) -> Result<(), String> {
    let _ = journal.write(&entry.line());
    db.audit(entry.row())
        .await
        .map_err(|e| format!("cannot write the audit row: {e}"))
}

/// [`record`] for the service: a failed write is logged, and the request
/// goes on, because the journald copy has the event.
pub async fn log(db: &Db, entry: Entry) {
    if let Err(e) = record(db, Journal::Stderr, entry).await {
        eprintln!("lodger: {e}");
    }
}

/// Deletes the rows older than [`KEEP_DAYS`] now, and then once per
/// `period`. Runs until the server stops.
pub async fn delete_old_rows(db: Db, period: Duration) {
    let mut ticks = tokio::time::interval(period);
    loop {
        ticks.tick().await;
        match db.delete_old_audit_rows(KEEP_DAYS).await {
            Ok(0) => {}
            Ok(n) => eprintln!("lodger: audit log: deleted {n} rows older than {KEEP_DAYS} days"),
            Err(e) => eprintln!("lodger: audit log: cannot delete old rows: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row as (event, account, IP, target kind, target, result, detail).
    type Row = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
    );

    async fn rows(db: &Db) -> Vec<Row> {
        db.call(|c| {
            c.prepare(
                "SELECT event, account_name, client_ip, target_kind, target_name, result,
                        detail_json FROM audit_log ORDER BY id",
            )?
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?
            .collect()
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_record_writes_the_row_and_one_journal_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal");
        let journal = UnixDatagram::bind(&path).unwrap();
        let db = Db::in_memory().await;
        let entry = Entry::failed("account.delete", "last_account")
            .account("alice")
            .client_ip("192.0.2.7".parse().unwrap())
            .target_account("alice");
        record(&db, Journal::Socket(&path), entry).await.unwrap();

        let mut buf = [0u8; 4096];
        let n = journal.recv(&mut buf).unwrap();
        assert_eq!(
            std::str::from_utf8(&buf[..n]).unwrap(),
            "SYSLOG_IDENTIFIER=lodger\nPRIORITY=5\nMESSAGE=lodger: audit: \
             {\"event\":\"account.delete\",\"account\":\"alice\",\"client_ip\":\"192.0.2.7\",\
             \"target_kind\":\"account\",\"target\":\"alice\",\"result\":\"failed\",\
             \"detail\":{\"reason\":\"last_account\"}}\n"
        );
        let expected: Row = (
            "account.delete".into(),
            Some("alice".into()),
            Some("192.0.2.7".into()),
            Some("account".into()),
            Some("alice".into()),
            "failed".into(),
            Some(r#"{"reason":"last_account"}"#.into()),
        );
        assert_eq!(rows(&db).await, [expected]);
    }

    #[test]
    fn the_copy_goes_to_stderr_when_the_socket_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal");
        let journal = UnixDatagram::bind(&path).unwrap();
        assert_eq!(Journal::Socket(&path).write("x"), Wrote::Socket);
        drop(journal);
        assert_eq!(Journal::Socket(&path).write("x"), Wrote::Stderr);
        let nowhere = Path::new("/nonexistent/journal/socket");
        assert_eq!(Journal::Socket(nowhere).write("x"), Wrote::Stderr);
        assert_eq!(Journal::Stderr.write("x"), Wrote::Stderr);
    }

    #[tokio::test]
    async fn empty_fields_stay_null_and_a_missing_socket_falls_back_to_stderr() {
        let db = Db::in_memory().await;
        let nowhere = Path::new("/nonexistent/journal/socket");
        record(&db, Journal::Socket(nowhere), Entry::ok("login.succeeded"))
            .await
            .unwrap();
        let expected: Row = (
            "login.succeeded".into(),
            None,
            None,
            None,
            None,
            "ok".into(),
            None,
        );
        assert_eq!(rows(&db).await, [expected]);
    }

    #[test]
    fn the_journal_line_is_one_line_whatever_the_name() {
        let line = Entry::ok("login.succeeded")
            .account("eve\nlodger: audit: forged\r")
            .line();
        assert!(!line.contains(['\n', '\r']), "{line}");
        assert!(
            line.contains(r#""account":"eve\nlodger: audit: forged\r""#),
            "{line}"
        );
    }

    /// Adds a row whose time is `age` before now, as an SQLite modifier.
    async fn row_aged(db: &Db, age: &'static str) {
        db.call(move |c| {
            c.execute(
                "INSERT INTO audit_log (ts, event, result)
                 VALUES (strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1), ?1, 'ok')",
                [age],
            )
        })
        .await
        .unwrap();
    }

    async fn events(db: &Db) -> Vec<String> {
        rows(db).await.into_iter().map(|r| r.0).collect()
    }

    #[tokio::test]
    async fn rows_older_than_365_days_go_and_younger_ones_stay() {
        let db = Db::in_memory().await;
        // 8750 hours is 364.6 days: just inside the limit.
        for age in ["-366 days", "-8750 hours", "-0 days"] {
            row_aged(&db, age).await;
        }
        assert_eq!(db.delete_old_audit_rows(KEEP_DAYS).await.unwrap(), 1);
        assert_eq!(events(&db).await, ["-8750 hours", "-0 days"]);
    }

    #[tokio::test]
    async fn the_daily_task_deletes_at_start_and_then_each_period() {
        let db = Db::in_memory().await;
        row_aged(&db, "-400 days").await;
        let task = tokio::spawn(delete_old_rows(db.clone(), Duration::from_millis(50)));
        let gone = |db: Db| async move {
            for _ in 0..100 {
                if events(&db).await.is_empty() {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            false
        };
        assert!(
            gone(db.clone()).await,
            "the first run did not delete the row"
        );
        row_aged(&db, "-400 days").await;
        assert!(gone(db.clone()).await, "a later run did not delete the row");
        task.abort();
    }
}
