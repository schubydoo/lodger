//! Browser security: the Origin rule for state-changing requests, and the response
//! headers (TAD section 7.4).
//!
//! `SameSite=Strict` does not stop a request from a sibling subdomain, which
//! is the same site. So every request that is not GET or HEAD must send
//! `Sec-Fetch-Site: same-origin`, or an `Origin` equal to `public_url`.
//! Anything else gets 403 before a handler runs. Lodger never compares
//! `Origin` with `Host`, because DNS rebinding can forge `Host`. A script
//! such as `curl` must send `Sec-Fetch-Site: same-origin` itself.
//!
//! The per-session `X-CSRF-Token` check is in [`crate::auth::require_session`],
//! because it needs the session.
//!
//! Every response gets the Content Security Policy, `Referrer-Policy`, and
//! `X-Content-Type-Options`. The CSP allows the one inline script of the
//! fallback page by its SHA-256 hash, which Lodger computes from the embedded
//! page at start. So the hash always matches the page that Lodger serves.

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::server::AppState;

/// The origin of `url`: `scheme://host[:port]` in lowercase, without a
/// default port, as a browser writes it in `Origin`.
pub fn origin_of(url: &str) -> Result<String, String> {
    let bad = |why: &str| format!("public_url {url:?} {why}");
    let uri: Uri = url.parse().map_err(|_| bad("is not a URL"))?;
    let scheme = uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| bad("has no scheme"))?;
    let default_port = match scheme.as_str() {
        "https" => 443,
        "http" => 80,
        _ => return Err(bad("must start with https:// or http://")),
    };
    let authority = uri.authority().ok_or_else(|| bad("has no host"))?;
    if authority.as_str().contains('@') {
        return Err(bad("must not contain a user name"));
    }
    let host = authority.host().to_ascii_lowercase();
    Ok(match authority.port_u16() {
        Some(port) if port != default_port => format!("{scheme}://{host}:{port}"),
        _ => format!("{scheme}://{host}"),
    })
}

/// GET and HEAD change nothing. The Origin rule and the CSRF check both use
/// this, so they always agree on which requests they guard.
pub fn is_safe(method: &Method) -> bool {
    method == Method::GET || method == Method::HEAD
}

/// Whether a state-changing request comes from a Lodger page.
pub fn from_lodger(headers: &HeaderMap, public_origin: Option<&str>) -> bool {
    let value = |name| headers.get(name).and_then(|v| v.to_str().ok());
    if value("sec-fetch-site") == Some("same-origin") {
        return true;
    }
    match (value(header::ORIGIN.as_str()), public_origin) {
        (Some(origin), Some(public)) => origin.eq_ignore_ascii_case(public),
        _ => false,
    }
}

/// The 403 message. A browser that sends no `Sec-Fetch-Site` (Safari before
/// 16.4, Firefox before 90) passes only with `public_url`, so it says so.
const FOREIGN: &str = "this request must come from a Lodger page. If it does, your browser \
                       sends no Sec-Fetch-Site header: set public_url in the configuration file";

/// Middleware: 403 for a state-changing request that does not come from
/// Lodger.
pub async fn require_same_origin(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if is_safe(req.method()) || from_lodger(req.headers(), state.public_origin.as_deref()) {
        return next.run(req).await;
    }
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": FOREIGN })),
    )
        .into_response()
}

/// The SHA-256 of each inline script in `page`, as CSP sources.
fn script_hashes(page: &str) -> Vec<String> {
    let mut hashes = Vec::new();
    let mut rest = page;
    while let Some(start) = rest.find("<script") {
        rest = &rest[start..];
        let Some(open_end) = rest.find('>') else {
            break;
        };
        let Some(close) = rest.find("</script>") else {
            break;
        };
        let (tag, body) = (&rest[..open_end], &rest[open_end + 1..close]);
        // A script with `src` loads from 'self' and needs no hash.
        if !tag.contains(" src=") {
            let digest = Sha256::digest(body.as_bytes());
            let b64 = base64::engine::general_purpose::STANDARD.encode(digest);
            hashes.push(format!("'sha256-{b64}'"));
        }
        rest = &rest[close + "</script>".len()..];
    }
    hashes
}

/// The Content-Security-Policy for the fallback `page`.
pub fn csp(page: &str, public_origin: Option<&str>) -> HeaderValue {
    let mut script_src = String::from("'self'");
    for hash in script_hashes(page) {
        script_src.push(' ');
        script_src.push_str(&hash);
    }
    // Some browsers do not count wss: as 'self', so the host is named.
    let socket = public_origin
        .and_then(|o| {
            o.strip_prefix("https://")
                .map(|h| format!(" wss://{h}"))
                .or_else(|| o.strip_prefix("http://").map(|h| format!(" ws://{h}")))
        })
        .unwrap_or_default();
    let policy = format!(
        "default-src 'self'; script-src {script_src}; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; connect-src 'self'{socket}; object-src 'none'; base-uri 'none'; \
         form-action 'self'; frame-ancestors 'none'"
    );
    HeaderValue::from_str(&policy).expect("the policy is ASCII")
}

/// Middleware: the security headers on every response.
pub async fn add_headers(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, state.csp.as_ref().clone());
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{csp, from_lodger, origin_of, script_hashes};

    #[test]
    fn a_public_url_becomes_the_origin_a_browser_sends() {
        assert_eq!(
            origin_of("https://Lodger.LAN/").unwrap(),
            "https://lodger.lan"
        );
        assert_eq!(
            origin_of("https://lodger.lan:443").unwrap(),
            "https://lodger.lan"
        );
        assert_eq!(
            origin_of("https://lodger.lan:8443/x").unwrap(),
            "https://lodger.lan:8443"
        );
        assert_eq!(origin_of("http://10.0.0.5:80").unwrap(), "http://10.0.0.5");
        assert_eq!(origin_of("http://[::1]:8460").unwrap(), "http://[::1]:8460");
        for bad in [
            "lodger.lan",
            "ftp://lodger.lan",
            "https://a@lodger.lan",
            "not a url",
        ] {
            assert!(origin_of(bad).is_err(), "{bad}");
        }
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.insert(*name, HeaderValue::from_static(value));
        }
        h
    }

    #[test]
    fn only_same_origin_or_the_public_url_passes() {
        let public = Some("https://lodger.lan");
        assert!(from_lodger(
            &headers(&[("sec-fetch-site", "same-origin")]),
            None
        ));
        assert!(from_lodger(
            &headers(&[("origin", "https://lodger.lan")]),
            public
        ));
        // A sibling subdomain is the same site, but not the same origin.
        let sibling = headers(&[
            ("sec-fetch-site", "same-site"),
            ("origin", "https://evil.lodger.lan"),
        ]);
        assert!(!from_lodger(&sibling, public));
        assert!(!from_lodger(
            &headers(&[("origin", "http://lodger.lan")]),
            public
        ));
        assert!(!from_lodger(
            &headers(&[("sec-fetch-site", "cross-site")]),
            public
        ));
        // No header at all, and an Origin without a public_url to match.
        assert!(!from_lodger(&HeaderMap::new(), public));
        assert!(!from_lodger(
            &headers(&[("origin", "https://lodger.lan")]),
            None
        ));
        // Host is never the reference, even when it matches Origin.
        let rebound = headers(&[
            ("origin", "http://attacker.example"),
            ("host", "attacker.example"),
        ]);
        assert!(!from_lodger(&rebound, public));
    }

    #[test]
    fn the_csp_allows_the_inline_script_by_its_hash_only() {
        // The hash of "alert(1)", from `printf 'alert(1)' | openssl sha256 -binary | base64`.
        let page =
            "<html><script>alert(1)</script><script type=\"module\" src=\"/a.js\"></script></html>";
        assert_eq!(
            script_hashes(page),
            ["'sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI='"]
        );
        let policy = csp(page, Some("https://lodger.lan:8443"));
        let policy = policy.to_str().unwrap();
        assert!(
            policy.contains(
                "script-src 'self' 'sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI=';"
            ),
            "{policy}"
        );
        assert!(
            policy.contains("connect-src 'self' wss://lodger.lan:8443;"),
            "{policy}"
        );
        for part in [
            "frame-ancestors 'none'",
            "object-src 'none'",
            "base-uri 'none'",
        ] {
            assert!(policy.contains(part), "{policy} lacks {part}");
        }
        assert!(!policy.contains("'unsafe-eval'"), "{policy}");
        let plain = csp("<p>no script</p>", Some("http://10.0.0.5"));
        let plain = plain.to_str().unwrap();
        assert!(plain.contains("script-src 'self';"), "{plain}");
        assert!(
            plain.contains("connect-src 'self' ws://10.0.0.5;"),
            "{plain}"
        );
        let unknown = csp("", None);
        assert!(unknown.to_str().unwrap().contains("connect-src 'self';"));
    }
}
