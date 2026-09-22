//! The `lodger` binary. The server and CLI arrive in later tasks.

fn main() {
    match lodger_virt::client_library_version() {
        Ok((major, minor, micro)) => println!(
            "lodger {} (libvirt client {major}.{minor}.{micro})",
            env!("CARGO_PKG_VERSION")
        ),
        Err(e) => {
            eprintln!("lodger: cannot read the libvirt client version: {e}");
            std::process::exit(1);
        }
    }
}
