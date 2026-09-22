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
    let out = lodger()
        .args(["serve", "--listen", &addr.to_string(), "--uri", TEST_URI])
        .output()
        .expect("lodger runs");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("cannot serve on"), "stderr: {stderr}");
}

/// Stops the server when the test ends, even on a failed assertion. `stop` sends
/// SIGTERM, like systemd does, and the server shuts down gracefully. SIGKILL is
/// only the fallback: a killed process writes no coverage data.
struct Server(Child);

impl Server {
    fn stop(&mut self) -> std::process::ExitStatus {
        let pid = self.0.id().to_string();
        let sent = Command::new("kill").args(["-TERM", &pid]).status();
        assert!(sent.is_ok_and(|s| s.success()), "could not send SIGTERM");
        self.0.wait().expect("lodger serve exits")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn start() -> (Server, String) {
    let mut child = lodger()
        .args(["serve", "--listen", "127.0.0.1:0", "--uri", TEST_URI])
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
    (Server(child), addr)
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
    let (mut server, addr) = start();
    // The supervisor connects in the background. Wait for the first load.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let body = loop {
        let reply = http_get(&addr, "/api/vms");
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
