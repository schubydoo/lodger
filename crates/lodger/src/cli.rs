//! Command-line interface.

use std::net::SocketAddr;
use std::path::PathBuf;

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
    ///
    /// Settings come from the configuration file; each flag overrides its key.
    Serve {
        /// Configuration file [default: /etc/lodger/config.toml, if it exists]
        #[arg(long)]
        config: Option<PathBuf>,
        /// Address and port to listen on [default: 127.0.0.1:8460]
        #[arg(long)]
        listen: Option<SocketAddr>,
        /// libvirt connection URI [default: `qemu:///system`]. `test:///default`
        /// uses libvirt's built-in test driver, which needs no libvirtd.
        #[arg(long)]
        uri: Option<String>,
        /// Directory for the database [default: `$STATE_DIRECTORY` from systemd,
        /// or /var/lib/lodger]
        #[arg(long)]
        state_dir: Option<PathBuf>,
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
    fn serve_leaves_unset_flags_to_the_configuration() {
        let cli = Cli::try_parse_from(["lodger", "serve"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Serve {
                config: None,
                listen: None,
                uri: None,
                state_dir: None
            }
        ));
    }

    #[test]
    fn serve_accepts_every_flag() {
        let cli = Cli::try_parse_from([
            "lodger",
            "serve",
            "--config",
            "/c.toml",
            "--listen",
            "127.0.0.1:0",
            "--uri",
            "test:///default",
            "--state-dir",
            "/tmp/s",
        ])
        .unwrap();
        let Command::Serve {
            config,
            listen,
            uri,
            state_dir,
        } = cli.command
        else {
            panic!("expected serve");
        };
        assert_eq!(config.unwrap().to_str(), Some("/c.toml"));
        assert_eq!(listen.unwrap().port(), 0);
        assert_eq!(uri.as_deref(), Some("test:///default"));
        assert_eq!(state_dir.unwrap().to_str(), Some("/tmp/s"));
    }

    #[test]
    fn the_defaults_parse() {
        let listen: SocketAddr = DEFAULT_LISTEN.parse().unwrap();
        assert!(listen.ip().is_loopback());
        assert_eq!(listen.port(), 8460);
        assert_eq!(DEFAULT_URI, "qemu:///system");
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
