//! Operator settings from `/etc/lodger/config.toml` (TAD section 5.1).
//!
//! These settings decide how Lodger is exposed, so they live in a root-owned
//! file and never in the database, where a Lodger admin could change them.
//! Command-line flags override the file. Unknown keys are an error, so a
//! typo cannot silently keep a default.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ipnet::IpNet;
use serde::Deserialize;

use crate::cli::{DEFAULT_LISTEN, DEFAULT_URI};

/// The file that `lodger serve` reads when `--config` is not given.
pub const DEFAULT_CONFIG: &str = "/etc/lodger/config.toml";

/// The state directory when neither the file nor systemd names one.
pub const DEFAULT_STATE_DIR: &str = "/var/lib/lodger";

/// The settings as the file writes them. Every key is optional.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    listen: Option<SocketAddr>,
    uri: Option<String>,
    state_dir: Option<PathBuf>,
    public_url: Option<String>,
    #[serde(default)]
    trusted_proxies: Vec<IpNet>,
    // No `tls` key yet: Lodger has no built-in TLS. Accepting certificate
    // paths and ignoring them would mislead, so the key is unknown, and an
    // unknown key is an error.
}

/// The settings that `lodger serve` runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub listen: SocketAddr,
    /// The libvirt connection URI.
    pub uri: String,
    /// Holds the database. systemd creates it as `StateDirectory=lodger`.
    pub state_dir: PathBuf,
    /// The origin of the URL that browsers use, such as
    /// `https://lodger.lan`, for the Origin check (TAD section 7.4).
    pub public_url: Option<String>,
    /// Proxies whose `X-Forwarded-For` Lodger trusts (TAD section 7.4).
    pub trusted_proxies: Vec<IpNet>,
}

/// Command-line values that override the file.
#[derive(Debug, Default)]
pub struct Overrides {
    pub config: Option<PathBuf>,
    pub listen: Option<SocketAddr>,
    pub uri: Option<String>,
    pub state_dir: Option<PathBuf>,
}

impl Config {
    /// Reads the configuration.
    ///
    /// The file named by `--config` must exist. The default file may be
    /// missing, and then the defaults apply. The state directory comes from
    /// `--state-dir`, the file, systemd's `STATE_DIRECTORY`, or the default,
    /// in that order.
    pub fn load(overrides: Overrides, state_directory_env: Option<&str>) -> Result<Self, String> {
        Self::load_with_default(overrides, state_directory_env, Path::new(DEFAULT_CONFIG))
    }

    fn load_with_default(
        overrides: Overrides,
        state_directory_env: Option<&str>,
        default_config: &Path,
    ) -> Result<Self, String> {
        let file = match &overrides.config {
            Some(path) => read(path)?,
            // Only a missing default file means "use the defaults". Any
            // other error, such as no permission to read /etc/lodger, stops
            // the start: silently ignoring the file would drop its settings.
            None => match std::fs::read_to_string(default_config) {
                Ok(text) => parse(default_config, &text)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
                Err(e) => return Err(format!("cannot read {}: {e}", default_config.display())),
            },
        };
        let state_dir = overrides
            .state_dir
            .or(file.state_dir)
            // systemd may pass several directories, separated by colons.
            .or_else(|| {
                state_directory_env
                    .and_then(|v| v.split(':').next())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIR));
        let listen = match overrides.listen.or(file.listen) {
            Some(listen) => listen,
            None => DEFAULT_LISTEN
                .parse()
                .expect("the default listen address parses"),
        };
        Ok(Self {
            listen,
            uri: overrides
                .uri
                .or(file.uri)
                .unwrap_or_else(|| DEFAULT_URI.to_owned()),
            state_dir,
            public_url: file
                .public_url
                .as_deref()
                .map(crate::security::origin_of)
                .transpose()?,
            trusted_proxies: file.trusted_proxies,
        })
    }
}

fn read(path: &Path) -> Result<File, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse(path, &text)
}

fn parse(path: &Path, text: &str) -> Result<File, String> {
    toml::from_str(text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;

    use super::{Config, DEFAULT_STATE_DIR, Overrides};

    fn file(text: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(text.as_bytes()).unwrap();
        f
    }

    fn with_config(path: PathBuf) -> Overrides {
        Overrides {
            config: Some(path),
            ..Overrides::default()
        }
    }

    #[test]
    fn an_empty_file_gives_the_defaults() {
        let f = file("");
        let c = Config::load(with_config(f.path().into()), None).unwrap();
        assert_eq!(c.listen.to_string(), "127.0.0.1:8460");
        assert_eq!(c.uri, "qemu:///system");
        assert_eq!(c.state_dir, PathBuf::from(DEFAULT_STATE_DIR));
        assert!(c.public_url.is_none() && c.trusted_proxies.is_empty());
    }

    #[test]
    fn the_file_sets_every_key() {
        let f = file(
            r#"
            listen = "0.0.0.0:9000"
            uri = "test:///default"
            state_dir = "/srv/lodger"
            public_url = "https://lodger.lan"
            trusted_proxies = ["172.17.0.0/16", "127.0.0.1/32"]
            "#,
        );
        let c = Config::load(with_config(f.path().into()), None).unwrap();
        assert_eq!(c.listen.to_string(), "0.0.0.0:9000");
        assert_eq!(c.uri, "test:///default");
        assert_eq!(c.state_dir, PathBuf::from("/srv/lodger"));
        assert_eq!(c.public_url.as_deref(), Some("https://lodger.lan"));
        assert_eq!(c.trusted_proxies.len(), 2);
    }

    #[test]
    fn flags_override_the_file() {
        let f = file("listen = \"0.0.0.0:9000\"\nuri = \"qemu:///system\"\nstate_dir = \"/srv\"");
        let c = Config::load(
            Overrides {
                config: Some(f.path().into()),
                listen: Some("127.0.0.1:1".parse().unwrap()),
                uri: Some("test:///default".into()),
                state_dir: Some("/tmp/x".into()),
            },
            Some("/var/lib/from-systemd"),
        )
        .unwrap();
        assert_eq!(c.listen.to_string(), "127.0.0.1:1");
        assert_eq!(c.uri, "test:///default");
        assert_eq!(c.state_dir, PathBuf::from("/tmp/x"));
    }

    #[test]
    fn systemd_names_the_state_directory_when_nothing_else_does() {
        let f = file("");
        let c = Config::load(with_config(f.path().into()), Some("/var/lib/lodger:/other")).unwrap();
        assert_eq!(c.state_dir, PathBuf::from("/var/lib/lodger"));
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        let f = file("listn = \"127.0.0.1:1\"");
        let err = Config::load(with_config(f.path().into()), None).unwrap_err();
        assert!(err.contains("listn"), "{err}");
    }

    #[test]
    fn tls_is_not_supported_yet() {
        let f = file("[tls]\ncert = \"/c.pem\"\nkey = \"/k.pem\"");
        let err = Config::load(with_config(f.path().into()), None).unwrap_err();
        assert!(err.contains("tls"), "{err}");
    }

    #[test]
    fn public_url_is_stored_as_its_origin_and_must_be_http() {
        let f = file("public_url = \"https://Lodger.lan:443/\"");
        let c = Config::load(with_config(f.path().into()), None).unwrap();
        assert_eq!(c.public_url.as_deref(), Some("https://lodger.lan"));
        let f = file("public_url = \"lodger.lan\"");
        let err = Config::load(with_config(f.path().into()), None).unwrap_err();
        assert!(err.contains("public_url"), "{err}");
    }

    #[test]
    fn a_bad_proxy_network_is_an_error() {
        let f = file("trusted_proxies = [\"not-a-net\"]");
        assert!(Config::load(with_config(f.path().into()), None).is_err());
    }

    #[test]
    fn a_missing_default_file_means_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let c =
            Config::load_with_default(Overrides::default(), None, &dir.path().join("none.toml"))
                .unwrap();
        assert_eq!(c.uri, "qemu:///system");
    }

    #[test]
    fn a_default_file_that_exists_is_read() {
        let f = file("uri = \"test:///default\"");
        let c = Config::load_with_default(Overrides::default(), None, f.path()).unwrap();
        assert_eq!(c.uri, "test:///default");
    }

    #[test]
    fn an_unreadable_default_file_is_an_error_not_the_defaults() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("config.toml"), "uri = \"test:///default\"").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // root reads through mode 0000, so the test proves nothing as root.
        let is_root = std::fs::read_dir(&locked).is_ok();
        let result =
            Config::load_with_default(Overrides::default(), None, &locked.join("config.toml"));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        if is_root {
            return;
        }
        let err = result.unwrap_err();
        assert!(err.contains("Permission denied"), "{err}");
    }

    #[test]
    fn a_named_file_must_exist() {
        let err = Config::load(with_config("/nonexistent/lodger.toml".into()), None).unwrap_err();
        assert!(
            err.starts_with("cannot read /nonexistent/lodger.toml"),
            "{err}"
        );
    }
}
