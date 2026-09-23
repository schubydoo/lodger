//! The client's IP address, for throttling and the audit log (TAD 7.5).
//!
//! Lodger trusts a forwarded address only when the TCP peer is one of the
//! `trusted_proxies`. It then reads `X-Real-IP`, or else the rightmost entry
//! of `X-Forwarded-For`, which is the one that the trusted proxy added. Any
//! other peer is the client itself, and its headers are ignored, because a
//! client can send any header it likes.

use std::net::IpAddr;

use axum::http::HeaderMap;
use ipnet::IpNet;

pub fn client_ip(peer: IpAddr, headers: &HeaderMap, trusted: &[IpNet]) -> IpAddr {
    if !trusted.iter().any(|net| net.contains(&peer)) {
        return peer;
    }
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if let Some(ip) = header("x-real-ip").and_then(|v| v.trim().parse().ok()) {
        return ip;
    }
    header("x-forwarded-for")
        .and_then(|v| v.rsplit(',').next())
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(peer)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use axum::http::HeaderMap;
    use ipnet::IpNet;

    use super::client_ip;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(k, v)| (axum::http::HeaderName::from_static(k), v.parse().unwrap()))
            .collect()
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    const PROXY: &str = "172.17.0.2";

    fn trusted() -> Vec<IpNet> {
        vec!["172.17.0.0/16".parse().unwrap()]
    }

    #[test]
    fn an_untrusted_peer_is_the_client_whatever_it_sends() {
        let h = headers(&[("x-real-ip", "10.0.0.9"), ("x-forwarded-for", "10.0.0.8")]);
        assert_eq!(
            client_ip(ip("192.168.1.50"), &h, &trusted()),
            ip("192.168.1.50")
        );
        // No trusted proxies at all: the default.
        assert_eq!(client_ip(ip("172.17.0.2"), &h, &[]), ip("172.17.0.2"));
    }

    #[test]
    fn a_trusted_proxy_passes_x_real_ip() {
        let h = headers(&[("x-real-ip", "192.168.1.50")]);
        assert_eq!(client_ip(ip(PROXY), &h, &trusted()), ip("192.168.1.50"));
    }

    #[test]
    fn a_trusted_proxy_passes_the_rightmost_forwarded_entry() {
        // The client sent the first entry itself; the proxy appended the
        // real peer last.
        let h = headers(&[("x-forwarded-for", "6.6.6.6, 192.168.1.50")]);
        assert_eq!(client_ip(ip(PROXY), &h, &trusted()), ip("192.168.1.50"));
    }

    #[test]
    fn a_trusted_proxy_without_a_usable_header_is_the_client() {
        let h = headers(&[("x-forwarded-for", "not an address")]);
        assert_eq!(client_ip(ip(PROXY), &h, &trusted()), ip(PROXY));
        assert_eq!(
            client_ip(ip(PROXY), &HeaderMap::new(), &trusted()),
            ip(PROXY)
        );
    }
}
