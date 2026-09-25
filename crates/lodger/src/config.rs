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
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
    #[serde(default)]
    allow_plain_http: bool,
}

/// The PEM files for built-in TLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsFiles {
    /// The certificate, followed by any intermediate certificates.
    pub cert: PathBuf,
    /// The private key of the first certificate.
    pub key: PathBuf,
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
    /// `https://lodger.lan`, for the Origin check (TAD section 7.4). A browser
    /// without `Sec-Fetch-Site` (Safari before 16.4, Firefox before 90) can
    /// log in only when this is set.
    pub public_url: Option<String>,
    /// Proxies whose `X-Forwarded-For` Lodger trusts (TAD section 7.4).
    pub trusted_proxies: Vec<IpNet>,
    /// Built-in TLS, when the file names both a certificate and a key.
    pub tls: Option<TlsFiles>,
    /// Serve plain HTTP on an address that is not loopback, for a TLS proxy
    /// that is not in `trusted_proxies`. The session cookie is `Secure`, so a
    /// browser must still reach Lodger over HTTPS to log in.
    pub allow_plain_http: bool,
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
            tls: match (file.tls_cert, file.tls_key) {
                (Some(cert), Some(key)) => Some(TlsFiles { cert, key }),
                (None, None) => None,
                // Half a pair would silently serve plain HTTP.
                _ => return Err("set both tls_cert and tls_key, or neither".to_owned()),
            },
            allow_plain_http: file.allow_plain_http,
        })
    }

    /// ASVS 12.2.1: a browser on the network must reach Lodger over TLS.
    /// Plain HTTP on an address that is not loopback needs a reverse proxy
    /// with TLS, named in `trusted_proxies`, or the explicit opt-in.
    pub fn check_plain_http(&self) -> Result<(), String> {
        if self.listen.ip().is_loopback()
            || self.tls.is_some()
            || !self.trusted_proxies.is_empty()
            || self.allow_plain_http
        {
            return Ok(());
        }
        Err(format!(
            "listen = {} is not loopback, and TLS is off, so passwords would cross the network \
             in clear text. Set tls_cert and tls_key (sudo lodger install --self-signed <ip> \
             makes a pair), or set trusted_proxies for a reverse proxy with TLS, or set \
             allow_plain_http = true if a TLS proxy that Lodger does not trust for client \
             addresses sits in front of it",
            self.listen
        ))
    }

    /// Whether Lodger serves plain HTTP on the network by the opt-in alone.
    pub fn plain_http_by_opt_in(&self) -> bool {
        self.allow_plain_http
            && !self.listen.ip().is_loopback()
            && self.tls.is_none()
            && self.trusted_proxies.is_empty()
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

    use super::{Config, DEFAULT_STATE_DIR, Overrides, TlsFiles};

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
        assert!(c.tls.is_none());
        assert!(!c.allow_plain_http);
    }

    #[test]
    fn plain_http_on_the_network_needs_tls_a_proxy_or_the_opt_in() {
        let load = |text: &str| {
            let f = file(text);
            Config::load(with_config(f.path().into()), None).unwrap()
        };
        let tls = "tls_cert = \"/c.pem\"\ntls_key = \"/k.pem\"";
        for (text, allowed, by_opt_in) in [
            ("", true, false),
            ("listen = \"[::1]:8460\"", true, false),
            ("listen = \"0.0.0.0:8460\"", false, false),
            ("listen = \"192.168.1.10:8460\"", false, false),
            (&format!("listen = \"0.0.0.0:8460\"\n{tls}"), true, false),
            (
                "listen = \"0.0.0.0:8460\"\ntrusted_proxies = [\"172.17.0.0/16\"]",
                true,
                false,
            ),
            (
                "listen = \"0.0.0.0:8460\"\nallow_plain_http = true",
                true,
                true,
            ),
            ("allow_plain_http = true", true, false),
        ] {
            let c = load(text);
            assert_eq!(c.check_plain_http().is_ok(), allowed, "{text}");
            assert_eq!(c.plain_http_by_opt_in(), by_opt_in, "{text}");
        }
        let err = load("listen = \"0.0.0.0:8460\"")
            .check_plain_http()
            .unwrap_err();
        assert!(err.contains("allow_plain_http = true"), "{err}");
        assert!(err.contains("--self-signed"), "{err}");
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
            tls_cert = "/etc/lodger/tls/cert.pem"
            tls_key = "/etc/lodger/tls/key.pem"
            "#,
        );
        let c = Config::load(with_config(f.path().into()), None).unwrap();
        assert_eq!(c.listen.to_string(), "0.0.0.0:9000");
        assert_eq!(c.uri, "test:///default");
        assert_eq!(c.state_dir, PathBuf::from("/srv/lodger"));
        assert_eq!(c.public_url.as_deref(), Some("https://lodger.lan"));
        assert_eq!(c.trusted_proxies.len(), 2);
        assert_eq!(
            c.tls,
            Some(TlsFiles {
                cert: "/etc/lodger/tls/cert.pem".into(),
                key: "/etc/lodger/tls/key.pem".into(),
            })
        );
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
    fn half_a_tls_pair_is_an_error() {
        for text in ["tls_cert = \"/c.pem\"", "tls_key = \"/k.pem\""] {
            let f = file(text);
            let err = Config::load(with_config(f.path().into()), None).unwrap_err();
            assert_eq!(err, "set both tls_cert and tls_key, or neither", "{text}");
        }
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
