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
    use super::{not_after, server_config, test_pair};
    use crate::config::TlsFiles;

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
