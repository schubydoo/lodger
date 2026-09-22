//! Command-line interface.

use std::net::SocketAddr;

use clap::{Parser, Subcommand};

/// The address Lodger listens on by default. Loopback only: remote access goes
/// through a reverse proxy with TLS.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8460";

/// The libvirt connection that Lodger manages by default.
pub const DEFAULT_URI: &str = "qemu:///system";

/// Lodger: a web UI for the KVM/QEMU VMs on this host.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Serve the web UI and the API.
    Serve {
        /// Address and port to listen on.
        #[arg(long, default_value = DEFAULT_LISTEN)]
        listen: SocketAddr,
        /// libvirt connection URI. `test:///default` uses libvirt's built-in
        /// test driver, which needs no libvirtd.
        #[arg(long, default_value = DEFAULT_URI)]
        uri: String,
    },
    /// Print the Lodger version and the libvirt client library version.
    Version,
}

/// The `version` output: `lodger <version> (libvirt client <x.y.z>)`.
pub fn version_line() -> Result<String, String> {
    let (major, minor, micro) = lodger_virt::client_library_version()
        .map_err(|e| format!("cannot read the libvirt client version: {e}"))?;
    Ok(format!(
        "lodger {} (libvirt client {major}.{minor}.{micro})",
        env!("CARGO_PKG_VERSION")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_defaults_to_loopback_port_8460() {
        let cli = Cli::try_parse_from(["lodger", "serve"]).unwrap();
        match cli.command {
            Command::Serve { listen, uri } => {
                assert!(listen.ip().is_loopback());
                assert_eq!(listen.port(), 8460);
                assert_eq!(uri, "qemu:///system");
            }
            Command::Version => panic!("expected serve"),
        }
    }

    #[test]
    fn serve_accepts_a_listen_address() {
        let cli = Cli::try_parse_from(["lodger", "serve", "--listen", "127.0.0.1:0"]).unwrap();
        assert!(matches!(cli.command, Command::Serve { listen, .. } if listen.port() == 0));
    }

    #[test]
    fn serve_accepts_a_libvirt_uri() {
        let cli = Cli::try_parse_from(["lodger", "serve", "--uri", "test:///default"]).unwrap();
        assert!(matches!(cli.command, Command::Serve { uri, .. } if uri == "test:///default"));
    }

    #[test]
    fn a_subcommand_is_required() {
        assert!(Cli::try_parse_from(["lodger"]).is_err());
    }

    #[test]
    fn version_line_names_both_versions() {
        let line = version_line().unwrap();
        assert!(line.starts_with(&format!("lodger {} ", env!("CARGO_PKG_VERSION"))));
        assert!(line.contains("(libvirt client "));
    }
}
