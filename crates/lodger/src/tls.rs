//! Built-in TLS (PRD F26, Task 3.17).
//!
//! The session cookie is `Secure` with the `__Host-` prefix, so a browser keeps
//! it only over HTTPS or on loopback. Without TLS, a LAN browser therefore
//! cannot log in, and the operator needs a reverse proxy. With `tls_cert` and
//! `tls_key` in the configuration, Lodger serves HTTPS itself.
//!
//! A certificate or key that cannot be used stops the start. Lodger never falls
//! back to plain HTTP, because that would send passwords in clear text to a
//! page that the operator expects to be encrypted.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::crypto::ring;
use tokio_rustls::server::TlsStream;
use x509_cert::Certificate;
use x509_cert::der::{DateTime, DecodePem};

use crate::config::TlsFiles;

/// How long a client may take to finish the TLS handshake. A client that
/// connects and then sends nothing must not hold a task forever.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How many finished handshakes may wait for the server to take them.
const QUEUE: usize = 64;

/// Reads the certificate chain and the key, and checks that they belong
/// together.
pub fn server_config(files: &TlsFiles) -> Result<Arc<ServerConfig>, String> {
    let chain = read_chain(&files.cert)?;
    let key = PrivateKeyDer::from_pem_file(&files.key)
        .map_err(|e| format!("cannot read the TLS key {}: {e}", files.key.display()))?;
    let mut config = ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("cannot set up TLS: {e}"))?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|e| {
            format!(
                "cannot use the TLS certificate {} with the key {}: {e}",
                files.cert.display(),
                files.key.display()
            )
        })?;
    // HTTP/1.1 only. Browsers open the WebSockets over HTTP/1.1, and one
    // protocol keeps the connection handling simple.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn read_chain(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let fail = |e: rustls_pki_types::pem::Error| {
        format!("cannot read the TLS certificate {}: {e}", path.display())
    };
    let chain = CertificateDer::pem_file_iter(path)
        .map_err(fail)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(fail)?;
    if chain.is_empty() {
        return Err(format!(
            "the TLS certificate {} holds no certificate",
            path.display()
        ));
    }
    Ok(chain)
}

/// The end of the validity period of the first certificate in `path`. It
/// prints as `2027-01-31T12:00:00Z`.
pub fn not_after(path: &Path) -> Result<DateTime, String> {
    let text = std::fs::read(path)
        .map_err(|e| format!("cannot read the TLS certificate {}: {e}", path.display()))?;
    let cert = Certificate::from_pem(&text)
        .map_err(|e| format!("cannot read the TLS certificate {}: {e}", path.display()))?;
    Ok(cert.tbs_certificate().validity().not_after.to_date_time())
}

/// How long a certificate from `lodger install --self-signed` lasts.
pub const SELF_SIGNED_DAYS: u64 = 730;

/// A new self-signed pair, as PEM text.
pub struct SelfSigned {
    pub cert_pem: String,
    pub key_pem: String,
    /// The SHA-256 fingerprint of the certificate, as `AB:CD:...`, which a
    /// person compares with the one that the browser shows.
    pub fingerprint: String,
    pub not_after: DateTime,
}

/// Makes a self-signed server certificate for `name`, an IP address or a
/// host name, valid from 1 hour before `now` for [`SELF_SIGNED_DAYS`]. The
/// hour allows for a client clock that is a little behind.
pub fn self_signed(name: &str, now: std::time::SystemTime) -> Result<SelfSigned, String> {
    use rcgen::{DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair};
    let fail = |e: rcgen::Error| format!("cannot make a certificate for {name}: {e}");
    let key = KeyPair::generate().map_err(fail)?;
    let mut params = rcgen::CertificateParams::new(vec![name.to_owned()]).map_err(fail)?;
    params.distinguished_name.push(DnType::CommonName, name);
    params.is_ca = IsCa::ExplicitNoCa;
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = (now - Duration::from_secs(60 * 60)).into();
    params.not_after = (now + Duration::from_secs(SELF_SIGNED_DAYS * 24 * 60 * 60)).into();
    let cert = params.self_signed(&key).map_err(fail)?;
    let digest = <sha2::Sha256 as sha2::Digest>::digest(cert.der());
    let fingerprint = digest
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    let cert_pem = cert.pem();
    let not_after = Certificate::from_pem(cert_pem.as_bytes())
        .map_err(|e| format!("cannot read the new certificate: {e}"))?
        .tbs_certificate()
        .validity()
        .not_after
        .to_date_time();
    Ok(SelfSigned {
        cert_pem,
        key_pem: key.serialize_pem(),
        fingerprint,
        not_after,
    })
}

/// True if `name` can go into a certificate: an IP address, or a host name
/// of letters, digits, and hyphens in dot-separated labels.
pub fn valid_name(name: &str) -> bool {
    if name.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// A listener that hands axum only connections whose TLS handshake finished.
///
/// A background task accepts TCP connections and runs each handshake in a task
/// of its own, so one slow client cannot delay the others.
pub struct TlsListener {
    ready: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    addr: SocketAddr,
}

impl TlsListener {
    pub fn new(tcp: TcpListener, config: Arc<ServerConfig>) -> std::io::Result<Self> {
        let addr = tcp.local_addr()?;
        let (tx, ready) = mpsc::channel(QUEUE);
        tokio::spawn(accept_loop(tcp, TlsAcceptor::from(config), tx));
        Ok(Self { ready, addr })
    }
}

async fn accept_loop(
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    ready: mpsc::Sender<(TlsStream<TcpStream>, SocketAddr)>,
) {
    loop {
        let (stream, peer) = match tcp.accept().await {
            Ok(accepted) => accepted,
            // For example too many open files. axum's own listener waits
            // 1 second too, so the loop does not spin.
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        // The server stopped and dropped the listener.
        if ready.is_closed() {
            return;
        }
        let acceptor = acceptor.clone();
        let ready = ready.clone();
        tokio::spawn(async move {
            // A failed or slow handshake just drops the connection: a plain
            // HTTP request or a port scan is not worth a log line.
            if let Ok(Ok(tls)) =
                tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await
            {
                let _ = ready.send((tls, peer)).await;
            }
        });
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(connection) => connection,
            // The accept loop never ends while the listener lives.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.addr)
    }
}

/// Self-signed pairs for the tests.
#[cfg(test)]
pub mod test_pair {
    use std::path::Path;

    use crate::config::TlsFiles;

    /// Writes `<name>.crt` and `<name>.key` into `dir`, for `127.0.0.1`, valid
    /// until the given date.
    pub fn write(dir: &Path, name: &str, not_after: (i32, u8, u8)) -> TlsFiles {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()]).unwrap();
        params.not_after = rcgen::date_time_ymd(not_after.0, not_after.1, not_after.2);
        let cert = params.self_signed(&key).unwrap();
        let files = TlsFiles {
            cert: dir.join(format!("{name}.crt")),
            key: dir.join(format!("{name}.key")),
        };
        std::fs::write(&files.cert, cert.pem()).unwrap();
        std::fs::write(&files.key, key.serialize_pem()).unwrap();
        files
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use rustls_pki_types::pem::PemObject;
    use rustls_pki_types::{CertificateDer, ServerName};
    use tokio_rustls::rustls::{self, ClientConnection, ServerConnection};

    use super::{not_after, self_signed, server_config, test_pair, valid_name};
    use crate::config::TlsFiles;

    /// Runs a full TLS handshake between this server pair and a client that
    /// trusts only the certificate and asks for `server_name`.
    fn handshake(pair: &super::SelfSigned, server_name: &str) -> Result<(), String> {
        let dir = tempfile::tempdir().unwrap();
        let files = TlsFiles {
            cert: dir.path().join("cert.pem"),
            key: dir.path().join("key.pem"),
        };
        std::fs::write(&files.cert, &pair.cert_pem).unwrap();
        std::fs::write(&files.key, &pair.key_pem).unwrap();
        let server = server_config(&files)?;
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from_pem_slice(pair.cert_pem.as_bytes()).unwrap())
            .unwrap();
        let client = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let (mut ours, mut theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let peer = std::thread::spawn(move || {
            let mut conn = ServerConnection::new(server).unwrap();
            while conn.is_handshaking() {
                if conn.complete_io(&mut theirs).is_err() {
                    break;
                }
            }
        });
        let name = ServerName::try_from(server_name.to_owned()).unwrap();
        let mut conn = ClientConnection::new(Arc::new(client), name).unwrap();
        let result = loop {
            if !conn.is_handshaking() {
                break Ok(());
            }
            if let Err(e) = conn.complete_io(&mut ours) {
                break Err(e.to_string());
            }
        };
        drop(ours);
        peer.join().unwrap();
        result
    }

    #[test]
    fn a_self_signed_pair_is_valid_only_for_its_name() {
        let now = SystemTime::now();
        let ip = self_signed("127.0.0.1", now).unwrap();
        handshake(&ip, "127.0.0.1").unwrap();
        let err = handshake(&ip, "127.0.0.2").unwrap_err();
        assert!(err.contains("certificate not valid for name"), "{err}");

        let host = self_signed("lodger.lan", now).unwrap();
        handshake(&host, "lodger.lan").unwrap();
        assert!(handshake(&host, "other.lan").is_err());
    }

    #[test]
    fn a_self_signed_pair_lasts_730_days_and_has_a_fingerprint() {
        let now = SystemTime::now();
        let pair = self_signed("192.168.1.10", now).unwrap();
        let end = pair.not_after.to_system_time();
        // A literal, so a change of the constant fails here.
        let want = now + Duration::from_secs(730 * 24 * 60 * 60);
        // X.509 time has whole seconds.
        let off = want.duration_since(end).unwrap_or_else(|e| e.duration());
        assert!(off < Duration::from_secs(2), "{off:?}");
        let parts: Vec<_> = pair.fingerprint.split(':').collect();
        assert_eq!(parts.len(), 32, "{}", pair.fingerprint);
        assert!(
            parts.iter().all(|p| p.len() == 2
                && p.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())),
            "{}",
            pair.fingerprint
        );
        // Two pairs never share a key.
        assert_ne!(
            pair.fingerprint,
            self_signed("192.168.1.10", now).unwrap().fingerprint
        );
    }

    #[test]
    fn a_name_is_an_ip_address_or_a_host_name() {
        for good in [
            "192.168.1.10",
            "::1",
            "fe80::1",
            "lodger",
            "lodger.lan",
            "a-b.c1.example",
        ] {
            assert!(valid_name(good), "{good}");
        }
        let long_label = "a".repeat(64);
        for bad in [
            "",
            "lodger lan",
            "-lodger.lan",
            "lodger-.lan",
            "lodger..lan",
            "*.lan",
            "lodger.lan/x",
            "http://lodger.lan",
            long_label.as_str(),
        ] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_matching_pair_loads_and_offers_only_http_1_1() {
        let dir = tempfile::tempdir().unwrap();
        let files = test_pair::write(dir.path(), "a", (2030, 1, 1));
        let config = server_config(&files).unwrap();
        assert_eq!(config.alpn_protocols, vec![b"http/1.1".to_vec()]);
    }

    #[test]
    fn a_missing_certificate_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let files = TlsFiles {
            cert: dir.path().join("none.crt"),
            key: dir.path().join("none.key"),
        };
        let err = server_config(&files).unwrap_err();
        assert!(err.starts_with("cannot read the TLS certificate "), "{err}");
        assert!(err.contains("none.crt"), "{err}");
    }

    #[test]
    fn a_key_from_another_pair_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let a = test_pair::write(dir.path(), "a", (2030, 1, 1));
        let b = test_pair::write(dir.path(), "b", (2030, 1, 1));
        let mixed = TlsFiles {
            cert: a.cert,
            key: b.key,
        };
        let err = server_config(&mixed).unwrap_err();
        assert!(err.starts_with("cannot use the TLS certificate "), "{err}");
    }

    #[test]
    fn a_file_without_a_certificate_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let a = test_pair::write(dir.path(), "a", (2030, 1, 1));
        // The key file holds a PEM block, but no certificate.
        let wrong = TlsFiles {
            cert: a.key.clone(),
            key: a.key,
        };
        let err = server_config(&wrong).unwrap_err();
        assert!(err.contains("holds no certificate"), "{err}");
    }

    #[test]
    fn not_after_reads_the_end_of_the_validity_period() {
        let dir = tempfile::tempdir().unwrap();
        let files = test_pair::write(dir.path(), "a", (2031, 3, 4));
        let end = not_after(&files.cert).unwrap();
        assert_eq!(end.to_string(), "2031-03-04T00:00:00Z");
    }
}
