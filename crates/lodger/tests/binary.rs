//! Runs the real `lodger` binary, so the process entry point and the server are
//! exercised in CI.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

/// libvirt's built-in test driver: it runs inside the process, so CI needs
/// no libvirtd.
const TEST_URI: &str = "test:///default";

fn lodger() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lodger"))
}

#[test]
fn version_prints_both_versions() {
    let out = lodger().arg("version").output().expect("lodger runs");
    assert!(out.status.success(), "exit status: {:?}", out.status);
    let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    assert!(
        stdout.starts_with(&format!("lodger {} ", env!("CARGO_PKG_VERSION"))),
        "unexpected output: {stdout}"
    );
    assert!(
        stdout.contains("(libvirt client "),
        "unexpected output: {stdout}"
    );
}

#[test]
fn no_subcommand_is_a_usage_error() {
    let out = lodger().output().expect("lodger runs");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn serve_fails_cleanly_when_the_address_is_taken() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap();
    let state = tempfile::tempdir().unwrap();
    let out = lodger()
        .args(["serve", "--listen", &addr.to_string(), "--uri", TEST_URI])
        .arg("--state-dir")
        .arg(state.path())
        .output()
        .expect("lodger runs");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("cannot serve on"), "stderr: {stderr}");
}

#[test]
fn serve_fails_cleanly_when_the_state_directory_is_unusable() {
    // /proc does not allow new directories, not even for root.
    let out = lodger()
        .args(["serve", "--listen", "127.0.0.1:0", "--uri", TEST_URI])
        .args(["--state-dir", "/proc/lodger-test"])
        .output()
        .expect("lodger runs");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("cannot create /proc/lodger-test"),
        "stderr: {stderr}"
    );
}

#[test]
fn admin_refuses_to_run_without_root() {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    assert!(
        !status
            .lines()
            .any(|l| l.starts_with("Uid:") && l.split_whitespace().nth(2) == Some("0")),
        "run this test as a user other than root"
    );
    let state = tempfile::tempdir().unwrap();
    for action in ["reset-password", "create"] {
        let mut child = lodger()
            .args(["admin", action, "alice", "--state-dir"])
            .arg(state.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("lodger runs");
        // lodger can exit before it reads stdin, so a failed write is fine.
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(b"a long enough password 42\n");
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{action}");
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(
            stderr.contains("lodger admin must run as root"),
            "{action}: {stderr}"
        );
        assert!(out.stdout.is_empty(), "{action}");
    }
    // The check comes first: nothing was created in the state directory.
    assert_eq!(std::fs::read_dir(state.path()).unwrap().count(), 0);
}

/// Stops the server when the test ends, even on a failed assertion. `stop` sends
/// SIGTERM, like systemd does, and the server shuts down gracefully. SIGKILL is
/// only the fallback: a killed process writes no coverage data. The server's
/// temporary state directory goes away with it.
struct Server {
    child: Child,
    state: tempfile::TempDir,
}

impl Server {
    fn stop(&mut self) -> std::process::ExitStatus {
        let pid = self.child.id().to_string();
        let sent = Command::new("kill").args(["-TERM", &pid]).status();
        assert!(sent.is_ok_and(|s| s.success()), "could not send SIGTERM");
        self.child.wait().expect("lodger serve exits")
    }

    /// The state directory: a directory that did not exist before the start.
    fn state_dir(&self) -> std::path::PathBuf {
        self.state.path().join("state")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn start() -> (Server, String) {
    start_with(&[])
}

/// Starts `lodger serve` on the test driver, with a fresh state directory and
/// `extra` arguments.
fn start_with(extra: &[&str]) -> (Server, String) {
    let state = tempfile::tempdir().unwrap();
    let mut child = lodger()
        .args(["serve", "--listen", "127.0.0.1:0", "--uri", TEST_URI])
        .arg("--state-dir")
        .arg(state.path().join("state"))
        .args(extra)
        .stdout(Stdio::piped())
        .spawn()
        .expect("lodger serve starts");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let addr = line
        .trim()
        .strip_prefix("lodger listening on http://")
        .unwrap_or_else(|| panic!("unexpected first line: {line}"))
        .to_string();
    (Server { child, state }, addr)
}

fn http_get(addr: &str, path: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn serve_answers_app_routes_and_reserves_api_and_ws() {
    let (mut server, addr) = start();
    let root = http_get(&addr, "/");
    assert!(root.starts_with("HTTP/1.1 200"), "{root}");
    assert!(
        root.to_lowercase().contains("content-type: text/html"),
        "{root}"
    );
    assert!(http_get(&addr, "/vms/web1").starts_with("HTTP/1.1 200"));
    assert!(http_get(&addr, "/missing.js").starts_with("HTTP/1.1 404"));
    assert!(http_get(&addr, "/api/missing").starts_with("HTTP/1.1 404"));
    assert!(http_get(&addr, "/ws/missing").starts_with("HTTP/1.1 404"));
    server.stop();
}

#[test]
fn serve_lists_the_test_driver_vms() {
    let dir = tempfile::tempdir().unwrap();
    let (child, addr, token) = serve_on(&dir.path().join("state"));
    let mut server = Server {
        child,
        state: tempfile::tempdir().unwrap(),
    };
    assert!(post_setup(&addr, &token.unwrap(), "admin").starts_with("HTTP/1.1 201"));
    // Without a session, the list is refused.
    assert!(http_get(&addr, "/api/vms").starts_with("HTTP/1.1 401"));
    let cookie = login(&addr, "admin", "correct horse battery staple").expect("the login works");

    // The supervisor connects in the background. Wait for the first load.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let body = loop {
        let reply = http_get_with(&addr, "/api/vms", &cookie);
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        if reply.contains(r#""name":"test""#) {
            break reply;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no VMs after 5 s: {reply}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(body.contains(r#""state":"running""#), "{body}");
    server.stop();
}

#[test]
fn serve_exits_cleanly_on_sigterm() {
    let (mut server, addr) = start();
    assert!(http_get(&addr, "/").starts_with("HTTP/1.1 200"));
    let status = server.stop();
    assert!(status.success(), "exit status after SIGTERM: {status:?}");
}

fn mode(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_fresh_start_creates_a_private_database() {
    let (mut server, addr) = start();
    assert!(http_get(&addr, "/").starts_with("HTTP/1.1 200"));
    let dir = server.state_dir();
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("lodger.db")), 0o600);
    server.stop();
}

#[test]
fn health_reports_the_database_and_libvirt() {
    let (mut server, addr) = start();
    let reply = http_get(&addr, "/api/health");
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.contains(r#""database":"ok""#), "{reply}");
    server.stop();
}

#[test]
fn serve_reads_the_configuration_file() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("from-config");
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "uri = \"{TEST_URI}\"\nstate_dir = \"{}\"\n",
            state.display()
        ),
    )
    .unwrap();
    // --state-dir from start_with would override the file, so run by hand.
    let mut child = lodger()
        .args(["serve", "--listen", "127.0.0.1:0", "--config"])
        .arg(&config)
        .stdout(Stdio::piped())
        .spawn()
        .expect("lodger serve starts");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let mut server = Server {
        child,
        state: tempfile::tempdir().unwrap(),
    };
    assert!(line.starts_with("lodger listening on "), "{line}");
    assert!(state.join("lodger.db").exists());
    server.stop();
}

/// The server's log lines after the start, collected while it runs.
type Log = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

/// Starts `lodger serve` on `state` and returns the process, the address, and
/// the setup token from the log, if one was written.
fn serve_on(state: &std::path::Path) -> (Child, String, Option<String>) {
    let (child, addr, token, _) = serve_logged(state);
    (child, addr, token)
}

fn serve_logged(state: &std::path::Path) -> (Child, String, Option<String>, Log) {
    let mut child = lodger()
        .args(["serve", "--listen", "127.0.0.1:0", "--uri", TEST_URI])
        .arg("--state-dir")
        .arg(state)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lodger serve starts");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let addr = line
        .trim()
        .strip_prefix("lodger listening on http://")
        .unwrap_or_else(|| panic!("unexpected first line: {line}"))
        .to_string();
    // The token line comes before the settings summary.
    let mut token = None;
    let mut stderr = BufReader::new(child.stderr.take().unwrap()).lines();
    for line in stderr.by_ref() {
        let line = line.unwrap();
        if let Some(rest) = line.strip_prefix("lodger: setup token: ") {
            token = Some(rest.split_whitespace().next().unwrap().to_string());
        }
        if line.starts_with("lodger: libvirt ") {
            break;
        }
    }
    // Keep reading, as the journal does. A closed pipe would make the
    // server's next log line fail.
    let log = Log::default();
    let sink = std::sync::Arc::clone(&log);
    std::thread::spawn(move || {
        for line in stderr.map_while(Result::ok) {
            sink.lock().unwrap().push(line);
        }
    });
    (child, addr, token, log)
}

fn post_setup(addr: &str, token: &str, username: &str) -> String {
    let body = format!(
        r#"{{"token":"{token}","username":"{username}","password":"correct horse battery staple"}}"#
    );
    let mut s = TcpStream::connect(addr).unwrap();
    write!(
        s,
        "POST /api/setup HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nSec-Fetch-Site: same-origin\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn a_start_deletes_audit_rows_older_than_365_days() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let (child, _, _) = serve_on(&state);
    Server {
        child,
        state: tempfile::tempdir().unwrap(),
    }
    .stop();

    let db = rusqlite::Connection::open(state.join("lodger.db")).unwrap();
    for (age, event) in [("-400 days", "old"), ("-10 days", "recent")] {
        db.execute(
            "INSERT INTO audit_log (ts, event, result)
             VALUES (strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1), ?2, 'ok')",
            [age, event],
        )
        .unwrap();
    }
    let events = || -> Vec<String> {
        db.prepare("SELECT event FROM audit_log ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(events(), ["old", "recent"]);

    // The daily task runs once at the start.
    let (child, _, _) = serve_on(&state);
    let mut server = Server {
        child,
        state: tempfile::tempdir().unwrap(),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while events().len() > 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(events(), ["recent"]);
    server.stop();
}

#[test]
fn a_restart_before_the_first_account_writes_a_new_token() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");

    let (child, _, first) = serve_on(&state);
    let first = first.expect("the first start writes a setup token");
    Server {
        child,
        state: tempfile::tempdir().unwrap(),
    }
    .stop();

    let (child, addr, second) = serve_on(&state);
    let mut server = Server {
        child,
        state: tempfile::tempdir().unwrap(),
    };
    let second = second.expect("a restart with no account writes a new token");
    assert_ne!(first, second);
    // The old token stopped working; the new one claims the install.
    assert!(post_setup(&addr, &first, "admin").starts_with("HTTP/1.1 403"));
    assert!(post_setup(&addr, &second, "admin").starts_with("HTTP/1.1 201"));
    assert!(post_setup(&addr, &second, "other").starts_with("HTTP/1.1 404"));
    server.stop();

    // With an account, a start writes no token, and setup stays closed.
    let (child, addr, third) = serve_on(&state);
    let mut server = Server {
        child,
        state: tempfile::tempdir().unwrap(),
    };
    assert_eq!(third, None);
    assert!(http_get(&addr, "/api/setup").starts_with("HTTP/1.1 404"));
    server.stop();
}

fn http_get_with(addr: &str, path: &str, cookie: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nCookie: {cookie}\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

/// Logs in. Returns the session cookie, or `None` for a refused login.
fn login(addr: &str, username: &str, password: &str) -> Option<String> {
    let body = format!(r#"{{"username":"{username}","password":"{password}"}}"#);
    let mut s = TcpStream::connect(addr).unwrap();
    write!(
        s,
        "POST /api/session HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nSec-Fetch-Site: same-origin\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out.lines()
        .find_map(|l| l.strip_prefix("set-cookie: "))
        .and_then(|v| v.split(';').next())
        .map(str::to_owned)
}

#[test]
fn the_sixth_failed_login_waits_and_every_failure_is_logged() {
    let dir = tempfile::tempdir().unwrap();
    let (child, addr, token, log) = serve_logged(&dir.path().join("state"));
    let mut server = Server {
        child,
        state: tempfile::tempdir().unwrap(),
    };
    assert!(post_setup(&addr, &token.unwrap(), "admin").starts_with("HTTP/1.1 201"));
    for _ in 0..5 {
        assert_eq!(login(&addr, "admin", "wrong wrong wrong wrong"), None);
    }
    assert!(
        !log.lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("WARNING: 5 failed logins"))
    );
    let start = std::time::Instant::now();
    assert_eq!(login(&addr, "admin", "wrong wrong wrong wrong"), None);
    assert!(
        start.elapsed() >= std::time::Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    let warned = log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.contains(r#"WARNING: 5 failed logins in 15 minutes for account "admin""#));
    assert!(warned, "{:?}", log.lock().unwrap());
    // Each failure has its journald copy on stderr, where systemd collects it.
    let copy = r#"lodger: audit: {"event":"login.failed","account":"admin","client_ip":"127.0.0.1","result":"failed","detail":{"reason":"wrong_password"}}"#;
    let copies = || log.lock().unwrap().iter().filter(|l| *l == copy).count();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while copies() < 6 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(copies(), 6, "{:?}", log.lock().unwrap());
    server.stop();
}
