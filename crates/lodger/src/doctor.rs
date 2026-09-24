//! `lodger doctor`: checks the host for what Lodger needs (TAD 4.3, risk T6).
//!
//! Each check prints PASS, FAIL, or SKIP with one line of reason. A failed
//! check also prints the commands that fix it. A check that does not apply to
//! the host, such as `SELinux` on Debian, is skipped. Doctor changes nothing.

use std::fmt::Write as _;
use std::path::Path;

use crate::config::{Config, Overrides};

/// The socket of the monolithic `libvirtd` or of `virtproxyd`, and the socket
/// of the modular `virtqemud`.
const LIBVIRT_SOCKETS: [&str; 2] = ["run/libvirt/libvirt-sock", "run/libvirt/virtqemud-sock"];
const PASSWD_FILE: &str = "etc/passwd";
const GROUP_FILE: &str = "etc/group";
const APPARMOR_ENABLED: &str = "sys/module/apparmor/parameters/enabled";
const APPARMOR_ABSTRACTION: &str = "etc/apparmor.d/abstractions/libvirt-qemu";
const APPARMOR_LOCAL: &str = "etc/apparmor.d/local/abstractions/libvirt-qemu";
/// The rule that a virtio NIC hot-plug needs on Debian 13 (PRD R10).
const VHOST_RULE: &str = "/dev/vhost-net rw,";
const SELINUX_ENFORCE: &str = "sys/fs/selinux/enforce";
const SELINUX_NFS: &str = "sys/fs/selinux/booleans/virt_use_nfs";
/// The first libvirt that reverts external snapshots (PRD R5).
const REVERT_VERSION: (u32, u32, u32) = (9, 9, 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Pass,
    Fail,
    Skip,
}

/// The result of one check.
#[derive(Debug)]
struct Check {
    name: &'static str,
    outcome: Outcome,
    reason: String,
    /// Commands that fix a failed check.
    fix: Vec<String>,
}

impl Check {
    fn pass(name: &'static str, reason: impl Into<String>) -> Self {
        Self::new(name, Outcome::Pass, reason, &[])
    }

    fn skip(name: &'static str, reason: impl Into<String>) -> Self {
        Self::new(name, Outcome::Skip, reason, &[])
    }

    fn fail(name: &'static str, reason: impl Into<String>, fix: &[&str]) -> Self {
        Self::new(name, Outcome::Fail, reason, fix)
    }

    fn new(name: &'static str, outcome: Outcome, reason: impl Into<String>, fix: &[&str]) -> Self {
        Self {
            name,
            outcome,
            reason: reason.into(),
            fix: fix.iter().map(|&f| f.to_owned()).collect(),
        }
    }
}

/// `lodger doctor`: prints every check. Fails if a check fails.
pub fn run(overrides: Overrides) -> Result<String, String> {
    let config = Config::load(overrides, None)?;
    let mut checks = host_checks(Path::new("/"));
    checks.extend(
        tokio::runtime::Builder::new_current_thread()
            .build()
            .map_err(|e| format!("cannot start the async runtime: {e}"))?
            .block_on(libvirt_checks(&config.uri)),
    );
    print!("{}", report(&checks));
    let failed = checks.iter().filter(|c| c.outcome == Outcome::Fail).count();
    let summary = summary(&checks);
    if failed == 0 {
        Ok(summary)
    } else {
        Err(summary)
    }
}

/// The checks that read only files below `root`.
fn host_checks(root: &Path) -> Vec<Check> {
    vec![socket(root), group(root), apparmor(root), selinux(root)]
}

fn socket(root: &Path) -> Check {
    const NAME: &str = "libvirt socket";
    match LIBVIRT_SOCKETS.iter().find(|s| root.join(s).exists()) {
        Some(found) => Check::pass(NAME, format!("/{found} exists")),
        None => Check::fail(
            NAME,
            "there is no libvirt socket in /run/libvirt, so libvirt does not run",
            &[
                "sudo systemctl enable --now libvirtd.socket  # or virtqemud.socket with the modular daemons",
            ],
        ),
    }
}

fn group(root: &Path) -> Check {
    const NAME: &str = "libvirt group";
    let read = |file| std::fs::read_to_string(root.join(file)).unwrap_or_default();
    let (users, groups) = (read(PASSWD_FILE), read(GROUP_FILE));
    let Some(libvirt) = entry(&groups, "libvirt") else {
        return Check::fail(
            NAME,
            "the libvirt group does not exist. The libvirt daemon package creates it",
            &[],
        );
    };
    let Some(lodger) = entry(&users, "lodger") else {
        return Check::fail(
            NAME,
            "the lodger user does not exist, so the service cannot run. `lodger install` creates it",
            &["sudo lodger install"],
        );
    };
    // polkit reads real membership: the primary group or the member list.
    let primary = lodger.get(3) == libvirt.get(2);
    let listed = libvirt
        .get(3)
        .is_some_and(|members| members.split(',').any(|m| m == "lodger"));
    if primary || listed {
        Check::pass(NAME, "the lodger user is a member of the libvirt group")
    } else {
        Check::fail(
            NAME,
            "the lodger user is not a member of the libvirt group, so it cannot reach libvirt",
            &[
                "sudo usermod -aG libvirt lodger",
                "sudo systemctl restart lodger",
            ],
        )
    }
}

fn apparmor(root: &Path) -> Check {
    const NAME: &str = "AppArmor vhost rule";
    let enabled = std::fs::read_to_string(root.join(APPARMOR_ENABLED)).unwrap_or_default();
    if enabled.trim() != "Y" {
        return Check::skip(NAME, "AppArmor is not active");
    }
    let Ok(abstraction) = std::fs::read_to_string(root.join(APPARMOR_ABSTRACTION)) else {
        return Check::skip(NAME, "libvirt uses no AppArmor abstraction on this host");
    };
    let local = std::fs::read_to_string(root.join(APPARMOR_LOCAL)).unwrap_or_default();
    if has_vhost_rule(&abstraction) || has_vhost_rule(&local) {
        Check::pass(
            NAME,
            "AppArmor lets QEMU open /dev/vhost-net, so NIC hot-plug works",
        )
    } else {
        Check::fail(
            NAME,
            format!(
                "AppArmor blocks /dev/vhost-net, so a NIC hot-plug fails. Add the rule `{VHOST_RULE}` to /{APPARMOR_LOCAL}; a VM gets it at its next start"
            ),
            &[
                "sudo mkdir -p /etc/apparmor.d/local/abstractions",
                // The leading newline keeps the rule off a last line that
                // has no newline of its own.
                "printf '\\n/dev/vhost-net rw,\\n' | sudo tee -a /etc/apparmor.d/local/abstractions/libvirt-qemu",
            ],
        )
    }
}

/// True if a line of the `AppArmor` text allows reading and writing
/// `/dev/vhost-net`. Spaces and a trailing comment do not matter.
fn has_vhost_rule(text: &str) -> bool {
    text.lines().any(|line| {
        let rule = line.split('#').next().unwrap_or_default().trim();
        let Some(rule) = rule.strip_suffix(',') else {
            return false;
        };
        let mut parts = rule.split_whitespace();
        parts.next() == Some("/dev/vhost-net")
            && parts
                .next()
                .is_some_and(|perms| perms.contains('r') && perms.contains('w'))
    })
}

fn selinux(root: &Path) -> Check {
    const NAME: &str = "SELinux virt_use_nfs";
    let Ok(enforce) = std::fs::read_to_string(root.join(SELINUX_ENFORCE)) else {
        return Check::skip(NAME, "SELinux is not active");
    };
    if enforce.trim() == "0" {
        return Check::pass(NAME, "SELinux is permissive, so it blocks no NFS disk");
    }
    let Ok(value) = std::fs::read_to_string(root.join(SELINUX_NFS)) else {
        return Check::skip(NAME, "the SELinux policy has no virt_use_nfs boolean");
    };
    // The file holds the current and the pending value, for example "0 0".
    if value.split_whitespace().next() == Some("1") {
        Check::pass(NAME, "virt_use_nfs is on, so VMs can use disks on NFS")
    } else {
        Check::fail(
            NAME,
            "virt_use_nfs is off, so VMs cannot use disks on NFS. Ignore this if no storage pool is on NFS",
            &["sudo setsebool -P virt_use_nfs 1"],
        )
    }
}

/// The fields of the `name:...` line of `/etc/passwd` or `/etc/group`.
fn entry<'a>(text: &'a str, name: &str) -> Option<Vec<&'a str>> {
    text.lines()
        .map(|line| line.split(':').collect::<Vec<_>>())
        .find(|fields| fields.first() == Some(&name))
}

/// The checks that need a libvirt connection.
async fn libvirt_checks(uri: &str) -> Vec<Check> {
    const NAME: &str = "libvirt connection";
    let info = match lodger_virt::Virt::open(uri).await {
        Ok(virt) => virt.host_info().await,
        Err(e) => Err(e),
    };
    match info {
        Ok(info) => vec![
            Check::pass(
                NAME,
                format!("{uri} answers with libvirt {}", info.libvirt_version),
            ),
            revert(&info.libvirt_version),
        ],
        Err(e) => vec![
            Check::fail(
                NAME,
                format!("cannot connect to {uri}: {e}"),
                &["sudo lodger doctor  # root, or a member of the libvirt group, can connect"],
            ),
            Check::skip("snapshot revert", "the libvirt version needs a connection"),
        ],
    }
}

fn revert(version: &str) -> Check {
    const NAME: &str = "snapshot revert";
    let (a, b, c) = REVERT_VERSION;
    match parse_version(version) {
        Some(v) if v >= REVERT_VERSION => Check::pass(
            NAME,
            format!("libvirt {version} reverts external snapshots"),
        ),
        Some(_) => Check::fail(
            NAME,
            format!(
                "libvirt {version} cannot revert external snapshots, so Lodger disables Revert. It needs libvirt {a}.{b}.{c} or newer"
            ),
            &[],
        ),
        None => Check::fail(
            NAME,
            format!("cannot read the libvirt version {version:?}"),
            &[],
        ),
    }
}

fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.split('.').map(str::parse::<u32>);
    let v = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    parts.next().is_none().then_some(v)
}

/// One line per check, then one `fix:` line per command.
fn report(checks: &[Check]) -> String {
    let mut out = String::new();
    for check in checks {
        let label = match check.outcome {
            Outcome::Pass => "PASS",
            Outcome::Fail => "FAIL",
            Outcome::Skip => "SKIP",
        };
        let _ = writeln!(out, "{label}  {}: {}", check.name, check.reason);
        for fix in &check.fix {
            let _ = writeln!(out, "      fix: {fix}");
        }
    }
    out
}

fn summary(checks: &[Check]) -> String {
    let count = |o| checks.iter().filter(|c| c.outcome == o).count();
    format!(
        "{} passed, {} failed, {} skipped",
        count(Outcome::Pass),
        count(Outcome::Fail),
        count(Outcome::Skip)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host root with the given files.
    fn root(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    const GROUPS: &str = "root:x:0:\nlibvirt:x:108:alice,lodger\n";
    const USERS: &str = "root:x:0:0::/root:/bin/sh\nlodger:x:988:988::/var/lib/lodger:/x\n";

    #[test]
    fn a_ready_debian_host_passes_every_file_check() {
        let dir = root(&[
            (LIBVIRT_SOCKETS[0], ""),
            (PASSWD_FILE, USERS),
            (GROUP_FILE, GROUPS),
            (APPARMOR_ENABLED, "Y\n"),
            (APPARMOR_ABSTRACTION, "  /dev/net/tun rw,\n"),
            (APPARMOR_LOCAL, "/dev/vhost-net rw,\n"),
        ]);
        let checks = host_checks(dir.path());
        let outcomes: Vec<_> = checks.iter().map(|c| (c.name, c.outcome)).collect();
        assert_eq!(
            outcomes,
            [
                ("libvirt socket", Outcome::Pass),
                ("libvirt group", Outcome::Pass),
                ("AppArmor vhost rule", Outcome::Pass),
                ("SELinux virt_use_nfs", Outcome::Skip),
            ]
        );
        assert_eq!(checks[0].reason, "/run/libvirt/libvirt-sock exists");
    }

    #[test]
    fn an_empty_host_fails_the_socket_and_the_group_and_skips_the_rest() {
        let dir = root(&[]);
        let checks = host_checks(dir.path());
        let outcomes: Vec<_> = checks.iter().map(|c| c.outcome).collect();
        assert_eq!(
            outcomes,
            [Outcome::Fail, Outcome::Fail, Outcome::Skip, Outcome::Skip]
        );
        assert!(checks[0].fix[0].contains("virtqemud.socket"));
    }

    #[test]
    fn the_modular_socket_passes() {
        let dir = root(&[(LIBVIRT_SOCKETS[1], "")]);
        let check = socket(dir.path());
        assert_eq!(check.outcome, Outcome::Pass);
        assert_eq!(check.reason, "/run/libvirt/virtqemud-sock exists");
    }

    #[test]
    fn the_missing_apparmor_rule_names_the_rule_and_the_two_commands() {
        let dir = root(&[
            (APPARMOR_ENABLED, "Y\n"),
            (
                APPARMOR_ABSTRACTION,
                "  /dev/net/tun rw,\n  /dev/vhost-net r,\n",
            ),
            (APPARMOR_LOCAL, "# /dev/vhost-net rw,\n"),
        ]);
        let check = apparmor(dir.path());
        assert_eq!(check.outcome, Outcome::Fail);
        assert!(
            check.reason.contains("`/dev/vhost-net rw,`"),
            "{}",
            check.reason
        );
        assert!(
            check
                .reason
                .contains("/etc/apparmor.d/local/abstractions/libvirt-qemu")
        );
        assert_eq!(
            check.fix,
            [
                "sudo mkdir -p /etc/apparmor.d/local/abstractions",
                "printf '\\n/dev/vhost-net rw,\\n' | sudo tee -a /etc/apparmor.d/local/abstractions/libvirt-qemu",
            ]
        );
    }

    #[test]
    fn a_rule_in_the_stock_abstraction_passes() {
        let dir = root(&[
            (APPARMOR_ENABLED, "Y\n"),
            (APPARMOR_ABSTRACTION, "  /dev/vhost-net rw,\n"),
        ]);
        assert_eq!(apparmor(dir.path()).outcome, Outcome::Pass);
    }

    #[test]
    fn the_vhost_rule_allows_spaces_and_a_comment() {
        assert!(has_vhost_rule("  /dev/vhost-net rw,\n"));
        assert!(has_vhost_rule("/dev/vhost-net  rw,  # NIC hot-plug\n"));
        assert!(has_vhost_rule("/dev/vhost-net rwk,\n"));
        assert!(!has_vhost_rule("# /dev/vhost-net rw,\n"));
        assert!(!has_vhost_rule("/dev/vhost-net r,\n"));
        assert!(!has_vhost_rule("/dev/vhost-net w,\n"));
        assert!(!has_vhost_rule("/dev/vhost-net rw\n"));
        assert!(!has_vhost_rule("/dev/vhost-net-x rw,\n"));
        assert!(!has_vhost_rule("deny /dev/vhost-net rw,\n"));
    }

    #[test]
    fn apparmor_is_skipped_when_off_or_unused_by_libvirt() {
        let off = root(&[(APPARMOR_ENABLED, "N\n"), (APPARMOR_ABSTRACTION, "")]);
        assert_eq!(apparmor(off.path()).reason, "AppArmor is not active");
        let unused = root(&[(APPARMOR_ENABLED, "Y\n")]);
        let check = apparmor(unused.path());
        assert_eq!(check.outcome, Outcome::Skip);
        assert!(check.reason.contains("no AppArmor abstraction"));
    }

    #[test]
    fn the_group_check_reads_real_membership() {
        let with = |groups: &str, users: &str| {
            let dir = root(&[(GROUP_FILE, groups), (PASSWD_FILE, users)]);
            group(dir.path())
        };
        // A member by name, and a member by primary group.
        assert_eq!(with(GROUPS, USERS).outcome, Outcome::Pass);
        let primary = with("libvirt:x:988:\n", USERS);
        assert_eq!(primary.outcome, Outcome::Pass);
        // "lodgerx" and "xlodger" are other users.
        let other = with("libvirt:x:108:lodgerx,xlodger\n", USERS);
        assert_eq!(other.outcome, Outcome::Fail);
        assert_eq!(other.fix[0], "sudo usermod -aG libvirt lodger");
        let no_user = with(GROUPS, "root:x:0:0::/root:/bin/sh\n");
        assert!(no_user.reason.contains("the lodger user does not exist"));
        assert_eq!(no_user.fix, ["sudo lodger install"]);
        let no_group = with("kvm:x:993:libvirt\n", USERS);
        assert!(no_group.reason.contains("the libvirt group does not exist"));
    }

    #[test]
    fn selinux_reads_the_current_value_of_the_boolean() {
        let with = |value: Option<&str>| {
            let mut files = vec![(SELINUX_ENFORCE, "1\n")];
            if let Some(v) = value {
                files.push((SELINUX_NFS, v));
            }
            let dir = root(&files);
            selinux(dir.path())
        };
        assert_eq!(with(Some("1 1")).outcome, Outcome::Pass);
        let off = with(Some("0 1"));
        assert_eq!(off.outcome, Outcome::Fail);
        assert_eq!(off.fix, ["sudo setsebool -P virt_use_nfs 1"]);
        assert_eq!(with(None).outcome, Outcome::Skip);
        let dir = root(&[]);
        assert_eq!(selinux(dir.path()).reason, "SELinux is not active");
        let permissive = root(&[(SELINUX_ENFORCE, "0\n"), (SELINUX_NFS, "0 0")]);
        let check = selinux(permissive.path());
        assert_eq!(check.outcome, Outcome::Pass);
        assert!(check.reason.contains("permissive"), "{}", check.reason);
    }

    #[test]
    fn revert_needs_libvirt_9_9_0() {
        assert_eq!(revert("9.9.0").outcome, Outcome::Pass);
        assert_eq!(revert("11.3.0").outcome, Outcome::Pass);
        assert_eq!(revert("10.0.0").outcome, Outcome::Pass);
        let old = revert("9.8.99");
        assert_eq!(old.outcome, Outcome::Fail);
        assert!(old.reason.contains("disables Revert"), "{}", old.reason);
        assert!(old.reason.contains("9.9.0 or newer"), "{}", old.reason);
        assert_eq!(revert("9.0.0").outcome, Outcome::Fail);
        assert_eq!(revert("9").outcome, Outcome::Fail);
        assert_eq!(revert("9.9.0.1").outcome, Outcome::Fail);
    }

    #[tokio::test]
    async fn the_test_driver_connects_and_reports_its_version() {
        let checks = libvirt_checks("test:///default").await;
        assert_eq!(checks[0].outcome, Outcome::Pass, "{}", checks[0].reason);
        assert!(
            checks[0]
                .reason
                .starts_with("test:///default answers with libvirt ")
        );
        assert_eq!(checks[1].name, "snapshot revert");
    }

    #[tokio::test]
    async fn a_failed_connection_fails_and_skips_revert() {
        let checks = libvirt_checks("test:///nonexistent/lodger-doctor.xml").await;
        assert_eq!(checks[0].outcome, Outcome::Fail);
        assert!(
            checks[0]
                .reason
                .starts_with("cannot connect to test:///nonexistent")
        );
        assert_eq!(checks[1].outcome, Outcome::Skip);
    }

    #[test]
    fn the_report_prints_one_line_per_check_and_each_fix() {
        let checks = [
            Check::pass("a", "fine"),
            Check::fail("b", "broken", &["cmd one", "cmd two"]),
            Check::skip("c", "not here"),
        ];
        assert_eq!(
            report(&checks),
            "PASS  a: fine\nFAIL  b: broken\n      fix: cmd one\n      fix: cmd two\nSKIP  c: not here\n"
        );
        assert_eq!(summary(&checks), "1 passed, 1 failed, 1 skipped");
    }
}
