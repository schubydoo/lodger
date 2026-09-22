//! Runs the real `lodger` binary, so the process entry point is exercised in CI.

use std::process::Command;

#[test]
fn prints_its_version_and_the_libvirt_client_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_lodger"))
        .output()
        .expect("the lodger binary runs");
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
