//! `lodger install` and `lodger uninstall` (TAD sections 6.4 and 6.5).
//!
//! Install checks the host, copies the binary to `/usr/local/bin`, creates the
//! `lodger` system user in the `libvirt` group, writes the configuration and
//! the hardened unit, and starts the service. Uninstall stops and removes the
//! service and the binary. It keeps the configuration, the state directory,
//! and the user unless `--purge` is given. Neither command touches a VM, a
//! pool, or a network: libvirt keeps all of them.
//!
//! These are the only commands that run other programs (TAD 6.4):
//! `systemd-sysusers` creates the user, `userdel` removes it, and `systemctl`
//! manages the unit. Each call passes a fixed argument list, never a shell.

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::cli::DEFAULT_LISTEN;
use crate::config::{Config, Overrides, TlsFiles};

/// The hardened unit from TAD section 6.5.
const UNIT: &str = include_str!("../dist/lodger.service");
/// The `systemd-sysusers` entry for the service user.
const SYSUSERS: &str = include_str!("../dist/sysusers.conf");
/// The configuration that a first install writes.
const CONFIG: &str = include_str!("../dist/config.toml");

// Paths below the root of the host, without the leading slash.
const BIN: &str = "usr/local/bin/lodger";
const CONFIG_DIR: &str = "etc/lodger";
const CONFIG_FILE: &str = "etc/lodger/config.toml";
/// The pair that `--self-signed` writes.
const TLS_CERT: &str = "etc/lodger/tls/cert.pem";
const TLS_KEY: &str = "etc/lodger/tls/key.pem";
const UNIT_FILE: &str = "etc/systemd/system/lodger.service";
const SYSUSERS_FILE: &str = "etc/sysusers.d/lodger.conf";
const STATE_DIR: &str = "var/lib/lodger";
const GROUP_FILE: &str = "etc/group";
const PASSWD_FILE: &str = "etc/passwd";
/// systemd creates this directory at boot (`sd_booted`).
const SYSTEMD_RUNNING: &str = "run/systemd/system";
/// The socket of the monolithic `libvirtd` or of `virtproxyd`, and the socket
/// of the modular `virtqemud`.
const LIBVIRT_SOCKETS: [&str; 2] = ["run/libvirt/libvirt-sock", "run/libvirt/virtqemud-sock"];

const SERVICE: &str = "lodger.service";

/// How long `install` waits for the service to listen.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// What the commands do outside the file system. The tests replace it.
pub trait Run {
    /// Runs one program with a fixed argument list.
    fn run(&mut self, program: &str, args: &[&str]) -> Result<(), String>;
    /// True if something accepts a connection on the address.
    fn accepts(&mut self, addr: SocketAddr) -> bool;
}

/// Runs each program for real, with no shell.
pub struct System;

impl Run for System {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<(), String> {
        let status = std::process::Command::new(program)
            .args(args)
            .status()
            .map_err(|e| format!("cannot run {program}: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{program} {} failed: {status}", args.join(" ")))
        }
    }

    fn accepts(&mut self, addr: SocketAddr) -> bool {
        TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
    }
}

/// The file system that the commands change: `/` on a host, and a temporary
/// directory in the tests.
pub struct Host {
    root: PathBuf,
}

impl Host {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// The path as a program argument.
    fn arg(&self, relative: &str) -> String {
        self.path(relative).display().to_string()
    }
}

/// `sudo lodger install [--self-signed <name>]`: installs and starts the
/// service, then waits until it listens.
pub fn run_install(self_signed: Option<&str>) -> Result<String, String> {
    crate::admin::require_root(std::fs::read_to_string("/proc/self/status"), "install")?;
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this binary: {e}"))?;
    let done = install(
        &Host::new("/"),
        &exe,
        self_signed,
        SystemTime::now(),
        &mut System,
    )?;
    wait_listening(done.listen, START_TIMEOUT)?;
    Ok(done.message())
}

/// What an install did, for the lines that it prints.
#[derive(Debug)]
struct Installed {
    listen: SocketAddr,
    /// The configuration turns on built-in TLS.
    tls: bool,
    /// The pair that `--self-signed` made.
    certificate: Option<Certificate>,
}

/// The facts about a new self-signed pair that the operator needs.
#[derive(Debug)]
struct Certificate {
    name: String,
    fingerprint: String,
    not_after: String,
}

impl Installed {
    fn message(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        let mut out = format!(
            "Lodger runs at {scheme}://{}. Read the setup token with: sudo journalctl -u lodger",
            self.listen
        );
        let Some(cert) = &self.certificate else {
            return out;
        };
        out.push_str(&format!(
            "\nMade a self-signed certificate for {}, valid until {}.\nSHA-256 fingerprint: {}\n\
             Your browser warns about this certificate. Accept it only if the browser shows the \
             same fingerprint.",
            cert.name, cert.not_after, cert.fingerprint
        ));
        if self.listen.ip().is_loopback() {
            let example = match cert.name.parse::<std::net::IpAddr>() {
                Ok(ip) => SocketAddr::new(ip, self.listen.port()).to_string(),
                Err(_) => format!("<this host's LAN address>:{}", self.listen.port()),
            };
            out.push_str(&format!(
                "\nLodger still listens only on {}. To serve the LAN, set listen = \"{example}\" \
                 in /etc/lodger/config.toml, then run: sudo systemctl restart lodger",
                self.listen
            ));
        }
        out
    }
}

/// `sudo lodger uninstall [--purge]`.
pub fn run_uninstall(purge: bool) -> Result<String, String> {
    crate::admin::require_root(std::fs::read_to_string("/proc/self/status"), "uninstall")?;
    uninstall(&Host::new("/"), purge, &mut System)
}

/// Installs the service. With `self_signed`, it also makes a self-signed
/// pair for that name and turns on TLS in the configuration.
///
/// The checks run first, and a failed check changes nothing. An existing
/// configuration file stays, because the operator may have changed it: the
/// pair only sets its `tls_cert` and `tls_key`, and keeps its comments.
fn install(
    host: &Host,
    exe: &Path,
    self_signed: Option<&str>,
    now: SystemTime,
    run: &mut impl Run,
) -> Result<Installed, String> {
    let listen = check(host, self_signed, run)?;
    copy_binary(exe, &host.path(BIN))?;
    write(&host.path(SYSUSERS_FILE), SYSUSERS.as_bytes(), 0o644)?;
    run.run("systemd-sysusers", &[&host.arg(SYSUSERS_FILE)])?;
    if !host.path(CONFIG_FILE).exists() {
        write(&host.path(CONFIG_FILE), CONFIG.as_bytes(), 0o644)?;
    }
    // After systemd-sysusers, which creates the user that owns the key.
    let certificate = self_signed
        .map(|name| write_self_signed(host, name, now))
        .transpose()?;
    write(&host.path(UNIT_FILE), UNIT.as_bytes(), 0o644)?;
    run.run("systemctl", &["daemon-reload"])?;
    run.run("systemctl", &["enable", SERVICE])?;
    // A restart also starts a stopped service, and an upgrade needs it to run
    // the new binary.
    run.run("systemctl", &["restart", SERVICE])?;
    let tls = Config::load(
        Overrides {
            config: Some(host.path(CONFIG_FILE)),
            ..Overrides::default()
        },
        None,
    )?
    .tls
    .is_some();
    Ok(Installed {
        listen,
        tls,
        certificate,
    })
}

/// The files that `--self-signed` writes, as the configuration names them.
fn self_signed_files() -> TlsFiles {
    TlsFiles {
        cert: Path::new("/").join(TLS_CERT),
        key: Path::new("/").join(TLS_KEY),
    }
}

/// Makes the pair, writes it, and names it in the configuration. The key is
/// readable only by root and the lodger user.
fn write_self_signed(host: &Host, name: &str, now: SystemTime) -> Result<Certificate, String> {
    let pair = crate::tls::self_signed(name, now)?;
    let users = std::fs::read_to_string(host.path(PASSWD_FILE)).unwrap_or_default();
    let owner = user_ids(&users, "lodger").ok_or(
        "the lodger user does not exist after systemd-sysusers, so the TLS key has no owner",
    )?;
    write_as(
        &host.path(TLS_KEY),
        pair.key_pem.as_bytes(),
        0o600,
        Some(owner),
    )?;
    write(&host.path(TLS_CERT), pair.cert_pem.as_bytes(), 0o644)?;
    set_tls_paths(&host.path(CONFIG_FILE))?;
    Ok(Certificate {
        name: name.to_owned(),
        fingerprint: pair.fingerprint,
        not_after: pair.not_after.to_string(),
    })
}

/// Sets `tls_cert` and `tls_key` in the configuration file. `toml_edit` keeps
/// the operator's comments and the order of the other keys.
fn set_tls_paths(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let files = self_signed_files();
    doc["tls_cert"] = toml_edit::value(files.cert.display().to_string());
    doc["tls_key"] = toml_edit::value(files.key.display().to_string());
    write(path, doc.to_string().as_bytes(), 0o644)
}

/// The user and group IDs of `name` in the text of `/etc/passwd`.
fn user_ids(passwd: &str, name: &str) -> Option<(u32, u32)> {
    let line = passwd
        .lines()
        .find(|line| line.split(':').next() == Some(name))?;
    let mut fields = line.split(':').skip(2);
    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
}

/// Checks the host and returns the address that the service will listen on.
/// A failure lists every problem, each with the step that fixes it.
fn check(host: &Host, self_signed: Option<&str>, run: &mut impl Run) -> Result<SocketAddr, String> {
    let mut problems = Vec::new();
    if let Some(name) = self_signed
        && !crate::tls::valid_name(name)
    {
        problems.push(format!(
            "{name:?} is not an IP address or a host name, so it cannot go into a certificate."
        ));
    }
    if self_signed.is_some()
        && let Some(problem) = foreign_pair(host)
    {
        problems.push(problem);
    }
    if !host.path(SYSTEMD_RUNNING).is_dir() {
        problems.push(
            "systemd does not run on this host. Lodger installs only as a systemd service."
                .to_owned(),
        );
    }
    if !LIBVIRT_SOCKETS.iter().any(|s| host.path(s).exists()) {
        problems.push(
            "there is no libvirt socket in /run/libvirt. Install libvirt, then start its socket: sudo systemctl enable --now libvirtd.socket, or virtqemud.socket on a host with the modular daemons"
                .to_owned(),
        );
    }
    let groups = std::fs::read_to_string(host.path(GROUP_FILE)).unwrap_or_default();
    if !has_entry(&groups, "libvirt") {
        problems.push(
            "the libvirt group does not exist. Install the libvirt daemon package, which creates it."
                .to_owned(),
        );
    }
    // Without a file, the service reads the template, which holds the
    // defaults (a test proves it).
    let listen = if host.path(CONFIG_FILE).exists() {
        let overrides = Overrides {
            config: Some(host.path(CONFIG_FILE)),
            ..Overrides::default()
        };
        match Config::load(overrides, None) {
            Ok(config) => {
                // Never replace a certificate that the operator chose.
                if self_signed.is_some()
                    && let Some(tls) = config.tls
                    && tls != self_signed_files()
                {
                    problems.push(format!(
                        "the configuration already names the TLS certificate {}. To replace it \
                         with a self-signed one, remove tls_cert and tls_key from \
                         /etc/lodger/config.toml first.",
                        tls.cert.display()
                    ));
                }
                Some(config.listen)
            }
            Err(e) => {
                problems.push(format!("the existing configuration is not valid: {e}"));
                None
            }
        }
    } else {
        Some(
            DEFAULT_LISTEN
                .parse()
                .expect("the default listen address parses"),
        )
    };
    // Before the first install, a listener on the address is another program.
    // The service could not bind, but the wait after the start would connect
    // to that program and report success.
    if let Some(listen) = listen
        && !host.path(UNIT_FILE).exists()
        && run.accepts(listen)
    {
        problems.push(format!(
            "another program listens on {listen}. Stop it, or write another `listen` address in /etc/lodger/config.toml."
        ));
    }
    match listen {
        Some(listen) if problems.is_empty() => Ok(listen),
        _ => Err(format!(
            "cannot install, and nothing changed:\n- {}",
            problems.join("\n- ")
        )),
    }
}

/// Why `--self-signed` must not write to `/etc/lodger/tls/`, if a pair that
/// Lodger did not make is there. The template suggests the same paths for an
/// operator's own certificate, and its private key cannot be made again.
fn foreign_pair(host: &Host) -> Option<String> {
    let (cert, key) = (host.path(TLS_CERT), host.path(TLS_KEY));
    let move_away = "To use a self-signed pair instead, move it away first.";
    match std::fs::read(&cert) {
        Ok(pem) if crate::tls::made_by_lodger(&pem) => None,
        Ok(_) => Some(format!(
            "/{TLS_CERT} holds a certificate that Lodger did not make. {move_away}"
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => key
            .exists()
            .then(|| format!("/{TLS_KEY} exists without a certificate. {move_away}")),
        Err(e) => Some(format!("cannot read /{TLS_CERT}: {e}")),
    }
}

/// True if a `name:...` line exists in the text of `/etc/passwd` or
/// `/etc/group`.
fn has_entry(text: &str, name: &str) -> bool {
    text.lines()
        .any(|line| line.split(':').next() == Some(name))
}

/// Uninstalls the service and returns the line to print.
fn uninstall(host: &Host, purge: bool, run: &mut impl Run) -> Result<String, String> {
    let unit = host.path(UNIT_FILE);
    if unit.exists() {
        run.run("systemctl", &["disable", "--now", SERVICE])?;
        remove_file(&unit)?;
        run.run("systemctl", &["daemon-reload"])?;
    }
    remove_file(&host.path(BIN))?;
    if !purge {
        return Ok("Lodger is uninstalled. /etc/lodger, /var/lib/lodger, and the lodger user stay. To remove them too, run: sudo lodger uninstall --purge".to_owned());
    }
    remove_dir(&host.path(CONFIG_DIR))?;
    remove_dir(&host.path(STATE_DIR))?;
    remove_file(&host.path(SYSUSERS_FILE))?;
    let users = std::fs::read_to_string(host.path(PASSWD_FILE)).unwrap_or_default();
    if has_entry(&users, "lodger") {
        run.run("userdel", &["lodger"])?;
    }
    Ok("Lodger and its data are removed. libvirt keeps every VM, pool, and network.".to_owned())
}

/// Copies the binary. The copy goes to a new file in the destination
/// directory, so it gets the label of that directory under `SELinux`, which a
/// move can lose (TAD 6.4). The binary is read whole first, so the installed
/// binary can install itself again.
fn copy_binary(exe: &Path, dest: &Path) -> Result<(), String> {
    let bytes = std::fs::read(exe).map_err(|e| format!("cannot read {}: {e}", exe.display()))?;
    write(dest, &bytes, 0o755)
}

/// Writes a file with the given mode through a temporary file in the same
/// directory, so a reader never sees half of it.
fn write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    write_as(path, bytes, mode, None)
}

/// Like [`write()`], and with an owner `(uid, gid)`, set before the file gets
/// its name, so no other user can open it in between.
fn write_as(path: &Path, bytes: &[u8], mode: u32, owner: Option<(u32, u32)>) -> Result<(), String> {
    let dir = path.parent().expect("every target has a parent");
    create_dirs(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let name = path.file_name().expect("every target has a name");
    let temp = dir.join(format!(".{}.new", name.to_string_lossy()));
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&temp)
        .and_then(|mut f| {
            if let Some((uid, gid)) = owner {
                std::os::unix::fs::fchown(&f, Some(uid), Some(gid))?;
            }
            f.write_all(bytes)?;
            // The mode of `open` passes through the umask.
            f.set_permissions(std::fs::Permissions::from_mode(mode))?;
            f.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp, path));
    result.map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("cannot write {}: {e}", path.display())
    })
}

/// Creates each missing directory with mode 0755. The mode ignores the umask,
/// because the service user must read the configuration below /etc/lodger.
fn create_dirs(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        create_dirs(parent)?;
    }
    match std::fs::create_dir(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
        Ok(()) => std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)),
    }
}

/// Removes a file. A missing file is not an error.
fn remove_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("cannot remove {}: {e}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Removes a directory and its content. A missing directory is not an error.
fn remove_dir(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("cannot remove {}: {e}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Waits until something accepts a connection on the address.
fn wait_listening(addr: SocketAddr, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the service did not listen on {addr} within {} seconds. Read the log with: sudo journalctl -u lodger",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records each call, and fails the call to `fail`, if given.
    #[derive(Default)]
    struct Recorder {
        calls: Vec<String>,
        fail: Option<&'static str>,
        /// The address where another program listens.
        taken: Option<SocketAddr>,
    }

    impl Run for Recorder {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<(), String> {
            let call = format!("{program} {}", args.join(" "));
            self.calls.push(call.clone());
            match self.fail {
                Some(fail) if call.starts_with(fail) => Err(format!("{call} failed")),
                _ => Ok(()),
            }
        }

        fn accepts(&mut self, addr: SocketAddr) -> bool {
            self.taken == Some(addr)
        }
    }

    /// A host that passes every check, with a binary to install.
    fn ready_host() -> (tempfile::TempDir, Host, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::new(dir.path().join("root"));
        std::fs::create_dir_all(host.path(SYSTEMD_RUNNING)).unwrap();
        put(&host, LIBVIRT_SOCKETS[0], "");
        put(&host, GROUP_FILE, "root:x:0:\nlibvirt:x:108:claude\n");
        let exe = dir.path().join("lodger-new");
        std::fs::write(&exe, b"\x7fELF lodger").unwrap();
        (dir, host, exe)
    }

    fn put(host: &Host, relative: &str, text: &str) {
        let path = host.path(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(host: &Host, relative: &str) -> String {
        std::fs::read_to_string(host.path(relative)).unwrap()
    }

    fn mode(host: &Host, relative: &str) -> u32 {
        std::fs::metadata(host.path(relative))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    }

    /// Every path below the root, so a test can show that nothing changed.
    fn tree(host: &Host) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                out.push(path.clone());
                if path.is_dir() {
                    walk(&path, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&host.root, &mut out);
        out.sort();
        out
    }

    #[test]
    fn install_writes_every_file_then_starts_the_service() {
        let (_dir, host, exe) = ready_host();
        let mut run = Recorder::default();
        let listen = install(&host, &exe, None, SystemTime::now(), &mut run)
            .unwrap()
            .listen;
        assert_eq!(listen.to_string(), DEFAULT_LISTEN);
        assert_eq!(std::fs::read(host.path(BIN)).unwrap(), b"\x7fELF lodger");
        assert_eq!(mode(&host, BIN), 0o755);
        assert_eq!(read(&host, SYSUSERS_FILE), SYSUSERS);
        assert_eq!(read(&host, CONFIG_FILE), CONFIG);
        assert_eq!(read(&host, UNIT_FILE), UNIT);
        for file in [SYSUSERS_FILE, CONFIG_FILE, UNIT_FILE] {
            assert_eq!(mode(&host, file), 0o644, "{file}");
        }
        assert_eq!(
            run.calls,
            [
                format!("systemd-sysusers {}", host.arg(SYSUSERS_FILE)),
                "systemctl daemon-reload".to_owned(),
                "systemctl enable lodger.service".to_owned(),
                "systemctl restart lodger.service".to_owned(),
            ]
        );
        // No temporary file stays behind.
        let names: Vec<_> = tree(&host)
            .iter()
            .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
            .filter(|n| n.ends_with(".new"))
            .collect();
        assert!(names.is_empty(), "{names:?}");
    }

    #[test]
    fn the_modular_virtqemud_socket_is_enough() {
        let (_dir, host, exe) = ready_host();
        std::fs::remove_file(host.path(LIBVIRT_SOCKETS[0])).unwrap();
        put(&host, LIBVIRT_SOCKETS[1], "");
        install(
            &host,
            &exe,
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
    }

    #[test]
    fn each_failed_check_stops_the_install_before_any_change() {
        type Break = (fn(&Host), &'static str);
        let breaks: [Break; 3] = [
            (
                |h| std::fs::remove_dir(h.path(SYSTEMD_RUNNING)).unwrap(),
                "systemd does not run",
            ),
            (
                |h| std::fs::remove_file(h.path(LIBVIRT_SOCKETS[0])).unwrap(),
                "no libvirt socket",
            ),
            (
                |h| put(h, GROUP_FILE, "root:x:0:\nlibvirtx:x:1:\nkvm:x:2:libvirt\n"),
                "the libvirt group does not exist",
            ),
        ];
        for (break_host, reason) in breaks {
            let (_dir, host, exe) = ready_host();
            break_host(&host);
            let before = tree(&host);
            let mut run = Recorder::default();
            let err = install(&host, &exe, None, SystemTime::now(), &mut run).unwrap_err();
            assert!(
                err.starts_with("cannot install, and nothing changed:"),
                "{err}"
            );
            assert!(err.contains(reason), "{err}");
            if reason == "no libvirt socket" {
                assert!(err.contains("libvirtd.socket") && err.contains("virtqemud.socket"));
            }
            assert_eq!(err.matches("\n- ").count(), 1, "{err}");
            assert!(run.calls.is_empty(), "{:?}", run.calls);
            assert_eq!(tree(&host), before);
        }
    }

    #[test]
    fn every_problem_is_listed_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::new(dir.path());
        let err = install(
            &host,
            Path::new("/nonexistent"),
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap_err();
        assert_eq!(err.matches("\n- ").count(), 3, "{err}");
    }

    #[test]
    fn another_program_on_the_port_stops_the_first_install() {
        let (_dir, host, exe) = ready_host();
        let before = tree(&host);
        let mut run = Recorder {
            taken: Some(DEFAULT_LISTEN.parse().unwrap()),
            ..Recorder::default()
        };
        let err = install(&host, &exe, None, SystemTime::now(), &mut run).unwrap_err();
        assert!(
            err.contains("another program listens on 127.0.0.1:8460"),
            "{err}"
        );
        assert!(run.calls.is_empty(), "{:?}", run.calls);
        assert_eq!(tree(&host), before);
    }

    #[test]
    fn a_reinstall_expects_the_old_service_on_the_port() {
        let (_dir, host, exe) = ready_host();
        install(
            &host,
            &exe,
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        let mut run = Recorder {
            taken: Some(DEFAULT_LISTEN.parse().unwrap()),
            ..Recorder::default()
        };
        install(&host, &exe, None, SystemTime::now(), &mut run).unwrap();
    }

    /// An address where nothing can listen: a connection to port 0 is always
    /// refused. A freed port is no such address, because a parallel test can
    /// bind it again at once.
    fn nobody_listens() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    #[test]
    fn the_system_runner_sees_a_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(System.accepts(listener.local_addr().unwrap()));
        assert!(!System.accepts(nobody_listens()));
    }

    #[test]
    fn an_existing_configuration_stays_and_names_the_address() {
        let (_dir, host, exe) = ready_host();
        put(&host, CONFIG_FILE, "listen = \"127.0.0.1:9123\"\n");
        let listen = install(
            &host,
            &exe,
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap()
        .listen;
        assert_eq!(listen.to_string(), "127.0.0.1:9123");
        assert_eq!(read(&host, CONFIG_FILE), "listen = \"127.0.0.1:9123\"\n");
    }

    #[test]
    fn an_invalid_existing_configuration_stops_the_install() {
        let (_dir, host, exe) = ready_host();
        put(&host, CONFIG_FILE, "listn = \"127.0.0.1:9123\"\n");
        let before = tree(&host);
        let mut run = Recorder::default();
        let err = install(&host, &exe, None, SystemTime::now(), &mut run).unwrap_err();
        assert!(
            err.contains("the existing configuration is not valid"),
            "{err}"
        );
        assert!(err.contains("listn"), "{err}");
        assert!(run.calls.is_empty());
        assert_eq!(tree(&host), before);
    }

    #[test]
    fn a_reinstall_replaces_the_unit_and_the_binary() {
        let (_dir, host, exe) = ready_host();
        put(&host, UNIT_FILE, "[Service]\nExecStart=/old\n");
        put(&host, BIN, "old binary");
        install(
            &host,
            &exe,
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        assert_eq!(read(&host, UNIT_FILE), UNIT);
        assert_eq!(std::fs::read(host.path(BIN)).unwrap(), b"\x7fELF lodger");
    }

    #[test]
    fn the_installed_binary_can_install_itself_again() {
        let (_dir, host, exe) = ready_host();
        install(
            &host,
            &exe,
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        std::fs::remove_file(&exe).unwrap();
        install(
            &host,
            &host.path(BIN),
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        assert_eq!(std::fs::read(host.path(BIN)).unwrap(), b"\x7fELF lodger");
    }

    #[test]
    fn a_failed_command_stops_the_install_with_its_error() {
        let (_dir, host, exe) = ready_host();
        let mut run = Recorder {
            fail: Some("systemd-sysusers"),
            ..Recorder::default()
        };
        let err = install(&host, &exe, None, SystemTime::now(), &mut run).unwrap_err();
        assert!(err.starts_with("systemd-sysusers "), "{err}");
        assert_eq!(run.calls.len(), 1);
        assert!(!host.path(UNIT_FILE).exists());
    }

    #[test]
    fn uninstall_removes_the_service_and_keeps_the_data() {
        let (_dir, host, exe) = ready_host();
        install(
            &host,
            &exe,
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        put(&host, &format!("{STATE_DIR}/lodger.db"), "data");
        put(
            &host,
            PASSWD_FILE,
            "lodger:x:990:990::/var/lib/lodger:/usr/sbin/nologin\n",
        );
        let mut run = Recorder::default();
        let line = uninstall(&host, false, &mut run).unwrap();
        assert!(line.contains("--purge"), "{line}");
        assert_eq!(
            run.calls,
            [
                "systemctl disable --now lodger.service",
                "systemctl daemon-reload"
            ]
        );
        assert!(!host.path(UNIT_FILE).exists());
        assert!(!host.path(BIN).exists());
        assert_eq!(read(&host, CONFIG_FILE), CONFIG);
        assert_eq!(read(&host, &format!("{STATE_DIR}/lodger.db")), "data");
        assert_eq!(read(&host, SYSUSERS_FILE), SYSUSERS);
    }

    #[test]
    fn purge_removes_everything_that_lodger_wrote_and_nothing_else() {
        let (_dir, host, exe) = ready_host();
        install(
            &host,
            &exe,
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        put(&host, &format!("{STATE_DIR}/lodger.db"), "data");
        put(
            &host,
            PASSWD_FILE,
            "root:x:0:0::/root:/bin/sh\nlodger:x:990:990::/:/x\n",
        );
        put(&host, "etc/libvirt/qemu/vm.xml", "<domain/>");
        let mut run = Recorder::default();
        uninstall(&host, true, &mut run).unwrap();
        assert_eq!(
            run.calls,
            [
                "systemctl disable --now lodger.service",
                "systemctl daemon-reload",
                "userdel lodger"
            ]
        );
        for gone in [UNIT_FILE, BIN, CONFIG_DIR, STATE_DIR, SYSUSERS_FILE] {
            assert!(!host.path(gone).exists(), "{gone}");
        }
        assert_eq!(read(&host, "etc/libvirt/qemu/vm.xml"), "<domain/>");
        assert!(read(&host, GROUP_FILE).contains("libvirt"));
    }

    #[test]
    fn uninstall_without_lodger_runs_nothing() {
        let (_dir, host, _exe) = ready_host();
        put(&host, PASSWD_FILE, "lodgerx:x:1:1::/:/x\n");
        let mut run = Recorder::default();
        uninstall(&host, true, &mut run).unwrap();
        assert!(run.calls.is_empty(), "{:?}", run.calls);
    }

    #[test]
    fn a_failed_stop_keeps_the_unit() {
        let (_dir, host, exe) = ready_host();
        install(
            &host,
            &exe,
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        let mut run = Recorder {
            fail: Some("systemctl disable"),
            ..Recorder::default()
        };
        assert!(uninstall(&host, true, &mut run).is_err());
        assert!(host.path(UNIT_FILE).exists());
        assert!(host.path(CONFIG_FILE).exists());
    }

    #[test]
    fn the_unit_has_every_directive_of_tad_6_5() {
        let lines: Vec<&str> = UNIT.lines().collect();
        for directive in [
            "User=lodger",
            "ExecStart=/usr/local/bin/lodger serve --config /etc/lodger/config.toml",
            "After=network-online.target libvirtd.socket virtqemud.socket virtproxyd.socket",
            "StateDirectory=lodger",
            "StateDirectoryMode=0700",
            "UMask=0077",
            "NoNewPrivileges=yes",
            "ProtectSystem=strict",
            "ProtectHome=yes",
            "PrivateTmp=yes",
            "PrivateDevices=yes",
            "PrivateIPC=yes",
            "ProtectKernelTunables=yes",
            "ProtectKernelModules=yes",
            "ProtectKernelLogs=yes",
            "ProtectControlGroups=yes",
            "ProtectClock=yes",
            "ProtectHostname=yes",
            "ProtectProc=invisible",
            "RestrictNamespaces=yes",
            "RestrictRealtime=yes",
            "RestrictSUIDSGID=yes",
            "LockPersonality=yes",
            "MemoryDenyWriteExecute=yes",
            "SystemCallArchitectures=native",
            "SystemCallFilter=@system-service",
            "SystemCallFilter=~@privileged @resources",
            "CapabilityBoundingSet=",
            // Without AF_UNIX, Lodger cannot reach the libvirt socket.
            "RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6",
            "Restart=on-failure",
            "WantedBy=multi-user.target",
        ] {
            assert!(lines.contains(&directive), "missing {directive}");
        }
        // Debian runs libvirtd, and Fedora runs virtqemud: a hard dependency
        // on either fails on the other. Nothing may hide /run/libvirt.
        for banned in [
            "Requires=",
            "DynamicUser",
            "InaccessiblePaths",
            "TemporaryFileSystem",
        ] {
            assert!(!UNIT.contains(banned), "{banned}");
        }
        assert!(UNIT.contains(&format!("/{BIN} ")) && UNIT.contains(&format!("/{CONFIG_FILE}")));
    }

    #[test]
    fn the_template_holds_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let load = |text: &str| {
            let path = dir.path().join("config.toml");
            std::fs::write(&path, text).unwrap();
            let overrides = Overrides {
                config: Some(path),
                ..Overrides::default()
            };
            Config::load(overrides, None).unwrap()
        };
        assert_eq!(load(CONFIG), load(""));
    }

    #[test]
    fn the_user_is_a_real_member_of_the_libvirt_group() {
        let lines: Vec<&str> = SYSUSERS.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(
            lines,
            [
                "u lodger - \"Lodger VM management\" /var/lib/lodger /usr/sbin/nologin",
                "m lodger libvirt"
            ]
        );
    }

    #[test]
    fn has_entry_matches_the_whole_first_field() {
        let text = "libvirt-qemu:x:64055:\nkvm:x:993:libvirt\nlibvirt:x:108:\n";
        assert!(has_entry(text, "libvirt"));
        assert!(!has_entry(
            "libvirt-qemu:x:1:\nkvm:x:2:libvirt\n",
            "libvirt"
        ));
        assert!(!has_entry("", "libvirt"));
    }

    #[test]
    fn wait_listening_returns_once_the_port_accepts() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        wait_listening(listener.local_addr().unwrap(), Duration::from_secs(5)).unwrap();
        let started = Instant::now();
        let err = wait_listening(nobody_listens(), Duration::from_millis(300)).unwrap_err();
        assert!(err.contains("journalctl -u lodger"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    /// The child half of the next test. It runs under umask 077.
    #[test]
    #[ignore = "the umask test runs it in a child process"]
    fn write_under_umask_077() {
        let root = std::env::var("LODGER_TEST_ROOT").unwrap();
        let host = Host::new(root);
        write(&host.path(CONFIG_FILE), b"x", 0o644).unwrap();
    }

    #[test]
    fn written_directories_and_files_ignore_the_umask() {
        // The umask belongs to the whole process, so the write runs in a
        // child process of this test binary.
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("sh")
            .args([
                "-c",
                "umask 077 && exec \"$0\" --exact install::tests::write_under_umask_077 --ignored",
            ])
            .arg(std::env::current_exe().unwrap())
            .env("LODGER_TEST_ROOT", dir.path())
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        let host = Host::new(dir.path());
        assert_eq!(read(&host, CONFIG_FILE), "x");
        assert_eq!(mode(&host, "etc"), 0o755);
        assert_eq!(mode(&host, CONFIG_DIR), 0o755);
        assert_eq!(mode(&host, CONFIG_FILE), 0o644);
    }

    #[test]
    fn the_system_runner_reports_the_exit_status() {
        System.run("true", &[]).unwrap();
        let err = System.run("false", &["x"]).unwrap_err();
        assert!(err.starts_with("false x failed: "), "{err}");
        let err = System.run("/nonexistent/lodger-test", &[]).unwrap_err();
        assert!(
            err.starts_with("cannot run /nonexistent/lodger-test"),
            "{err}"
        );
    }

    #[test]
    fn the_ready_line_follows_the_installed_configuration() {
        let (_dir, host, exe) = ready_host();
        let done = install(
            &host,
            &exe,
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        assert!(
            done.message()
                .starts_with("Lodger runs at http://127.0.0.1:8460. ")
        );
        // A later install keeps an operator's TLS settings, so it says https.
        put(
            &host,
            CONFIG_FILE,
            "tls_cert = \"/etc/ssl/l.pem\"\ntls_key = \"/etc/ssl/l.key\"\n",
        );
        let done = install(
            &host,
            &exe,
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        assert!(
            done.message()
                .starts_with("Lodger runs at https://127.0.0.1:8460. ")
        );
    }

    /// A ready host with a lodger user that has this test's own user ID, so
    /// the key's owner can be set without root. Its group is one of this
    /// process's other groups, when it has one: a user may give a file to any
    /// of its groups, and a group that differs from the default one proves
    /// that the owner was set.
    fn ready_host_with_user() -> (tempfile::TempDir, Host, PathBuf, (u32, u32)) {
        use std::os::unix::fs::MetadataExt;
        let (dir, host, exe) = ready_host();
        let meta = std::fs::metadata(dir.path()).unwrap();
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let other_group = status
            .lines()
            .find_map(|l| l.strip_prefix("Groups:"))
            .and_then(|groups| {
                groups
                    .split_whitespace()
                    .filter_map(|g| g.parse::<u32>().ok())
                    .find(|&g| g != meta.gid())
            });
        let ids = (meta.uid(), other_group.unwrap_or(meta.gid()));
        put(
            &host,
            PASSWD_FILE,
            &format!(
                "root:x:0:0::/root:/bin/sh\nlodger:x:{}:{}::/var/lib/lodger:/x\n",
                ids.0, ids.1
            ),
        );
        (dir, host, exe, ids)
    }

    fn fingerprint_of(pem: &str) -> String {
        use rustls_pki_types::pem::PemObject;
        let der = rustls_pki_types::CertificateDer::from_pem_slice(pem.as_bytes()).unwrap();
        <sha2::Sha256 as sha2::Digest>::digest(&der)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    fn installed_config(host: &Host) -> Config {
        Config::load(
            Overrides {
                config: Some(host.path(CONFIG_FILE)),
                ..Overrides::default()
            },
            None,
        )
        .unwrap()
    }

    #[test]
    fn self_signed_writes_the_pair_and_turns_on_tls() {
        use std::os::unix::fs::MetadataExt;
        let (_dir, host, exe, ids) = ready_host_with_user();
        let mut run = Recorder::default();
        let done = install(
            &host,
            &exe,
            Some("192.168.1.10"),
            SystemTime::now(),
            &mut run,
        )
        .unwrap();

        assert_eq!(mode(&host, TLS_KEY), 0o600);
        assert_eq!(mode(&host, TLS_CERT), 0o644);
        let key = std::fs::metadata(host.path(TLS_KEY)).unwrap();
        assert_eq!((key.uid(), key.gid()), ids);
        // The configuration names the host paths, and the pair on disk loads.
        let config = installed_config(&host);
        assert_eq!(config.tls, Some(self_signed_files()));
        crate::tls::server_config(&TlsFiles {
            cert: host.path(TLS_CERT),
            key: host.path(TLS_KEY),
        })
        .unwrap();
        // The template's comments stay.
        let text = read(&host, CONFIG_FILE);
        assert!(text.starts_with("# Lodger configuration."), "{text}");
        assert!(text.contains("# The libvirt connection."), "{text}");

        let cert = done.certificate.as_ref().unwrap();
        assert_eq!(cert.fingerprint, fingerprint_of(&read(&host, TLS_CERT)));
        assert!(done.tls);
        let message = done.message();
        assert!(
            message.starts_with("Lodger runs at https://127.0.0.1:8460."),
            "{message}"
        );
        assert!(
            message.contains(&format!("SHA-256 fingerprint: {}\n", cert.fingerprint)),
            "{message}"
        );
        assert!(
            message.contains("set listen = \"192.168.1.10:8460\""),
            "{message}"
        );
        // The service restarts after the pair exists.
        assert_eq!(
            run.calls.last().unwrap(),
            "systemctl restart lodger.service"
        );
    }

    #[test]
    fn a_second_self_signed_install_replaces_the_pair_and_keeps_one_setting() {
        let (_dir, host, exe, _) = ready_host_with_user();
        let first = install(
            &host,
            &exe,
            Some("lodger.lan"),
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        let config = read(&host, CONFIG_FILE);
        let second = install(
            &host,
            &exe,
            Some("lodger.lan"),
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        assert_ne!(
            first.certificate.unwrap().fingerprint,
            second.certificate.as_ref().unwrap().fingerprint
        );
        assert_eq!(
            read(&host, CONFIG_FILE),
            config,
            "the setting is written once"
        );
        assert_eq!(
            config.matches("tls_cert =").count(),
            2,
            "one comment, one setting"
        );
        assert!(second.message().contains("<this host's LAN address>:8460"));
    }

    #[test]
    fn self_signed_keeps_a_listen_address_and_says_nothing_about_it() {
        let (_dir, host, exe, _) = ready_host_with_user();
        put(&host, CONFIG_FILE, "listen = \"192.168.1.10:8460\"\n");
        let done = install(
            &host,
            &exe,
            Some("192.168.1.10"),
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        let message = done.message();
        assert!(
            message.starts_with("Lodger runs at https://192.168.1.10:8460."),
            "{message}"
        );
        assert!(!message.contains("set listen"), "{message}");
        assert_eq!(
            installed_config(&host).listen.to_string(),
            "192.168.1.10:8460"
        );
    }

    #[test]
    fn self_signed_never_replaces_the_operators_certificate() {
        let (_dir, host, exe, _) = ready_host_with_user();
        put(
            &host,
            CONFIG_FILE,
            "tls_cert = \"/etc/ssl/lodger.pem\"\ntls_key = \"/etc/ssl/lodger.key\"\n",
        );
        let before = tree(&host);
        let mut run = Recorder::default();
        let err = install(
            &host,
            &exe,
            Some("192.168.1.10"),
            SystemTime::now(),
            &mut run,
        )
        .unwrap_err();
        assert!(
            err.contains("already names the TLS certificate /etc/ssl/lodger.pem"),
            "{err}"
        );
        assert_eq!(tree(&host), before, "nothing changed");
        assert!(run.calls.is_empty());
    }

    #[test]
    fn self_signed_never_overwrites_a_foreign_pair_at_its_own_paths() {
        // The template suggests /etc/lodger/tls/ for an operator's own pair.
        let (dir, host, exe, _) = ready_host_with_user();
        put(
            &host,
            CONFIG_FILE,
            "tls_cert = \"/etc/lodger/tls/cert.pem\"\ntls_key = \"/etc/lodger/tls/key.pem\"\n",
        );
        let other = crate::tls::test_pair::write(dir.path(), "ca-signed", (2031, 1, 1));
        put(
            &host,
            TLS_CERT,
            &std::fs::read_to_string(&other.cert).unwrap(),
        );
        put(
            &host,
            TLS_KEY,
            &std::fs::read_to_string(&other.key).unwrap(),
        );
        let before = tree(&host);
        let key_before = read(&host, TLS_KEY);
        let mut run = Recorder::default();
        let err = install(
            &host,
            &exe,
            Some("192.168.1.10"),
            SystemTime::now(),
            &mut run,
        )
        .unwrap_err();
        assert!(
            err.contains("/etc/lodger/tls/cert.pem holds a certificate that Lodger did not make"),
            "{err}"
        );
        assert_eq!(tree(&host), before, "nothing changed");
        assert_eq!(read(&host, TLS_KEY), key_before);
        assert!(run.calls.is_empty());

        // A key alone is refused too: it may belong to a certificate elsewhere.
        std::fs::remove_file(host.path(TLS_CERT)).unwrap();
        let err = install(
            &host,
            &exe,
            Some("192.168.1.10"),
            SystemTime::now(),
            &mut run,
        )
        .unwrap_err();
        assert!(
            err.contains("/etc/lodger/tls/key.pem exists without a certificate"),
            "{err}"
        );
        assert_eq!(read(&host, TLS_KEY), key_before);
    }

    #[test]
    fn self_signed_refuses_a_bad_name_before_any_change() {
        let (_dir, host, exe, _) = ready_host_with_user();
        let before = tree(&host);
        let err = install(
            &host,
            &exe,
            Some("lodger lan"),
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap_err();
        assert!(
            err.contains("\"lodger lan\" is not an IP address or a host name"),
            "{err}"
        );
        assert_eq!(tree(&host), before, "nothing changed");
    }

    #[test]
    fn self_signed_needs_the_lodger_user_for_the_key() {
        // ready_host has no passwd file: systemd-sysusers only ran in the
        // recorder.
        let (_dir, host, exe) = ready_host();
        let err = install(
            &host,
            &exe,
            Some("192.168.1.10"),
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap_err();
        assert!(err.contains("the lodger user does not exist"), "{err}");
        assert!(!host.path(TLS_KEY).exists());
    }

    #[test]
    fn a_plain_install_keeps_plain_http() {
        let (_dir, host, exe) = ready_host();
        let done = install(
            &host,
            &exe,
            None,
            SystemTime::now(),
            &mut Recorder::default(),
        )
        .unwrap();
        assert!(!done.tls && done.certificate.is_none());
        assert_eq!(
            done.message(),
            "Lodger runs at http://127.0.0.1:8460. Read the setup token with: sudo journalctl -u lodger"
        );
        assert!(!host.path(TLS_CERT).exists());
    }
}
