//! Serves the embedded SvelteKit build (`web/build`).
//!
//! Rules, in order:
//! 1. Only GET and HEAD. Any other method gets 405.
//! 2. A path with a `..` segment or a backslash gets 404 before any lookup.
//! 3. An existing file is served with its MIME type and an `ETag`. Files under
//!    `_app/immutable/` carry a content hash in their name, so they are cached
//!    for a year. Everything else is `no-cache` and revalidated by `ETag`.
//! 4. A missing path whose last segment has a `.` (for example `/missing.js`)
//!    gets 404, so a missing script never comes back as HTML.
//! 5. Any other path is an app route: it gets the `200.html` fallback.
//! 6. If the binary was built without `web/build`, the fallback is a built-in
//!    stub page that says how to build the UI.

use std::borrow::Cow;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;

/// The fallback page that adapter-static writes for the single-page app.
pub const FALLBACK: &str = "200.html";
const IMMUTABLE_PREFIX: &str = "_app/immutable/";
const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";

/// Shown when the binary was built without the web UI.
pub const STUB_PAGE: &str = "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>Lodger</title></head>\n<body><h1>Lodger</h1><p>This binary was built without the web UI. Build it with <code>just build</code>, which runs <code>pnpm run build</code> in <code>web/</code> before <code>cargo build --release</code>.</p></body></html>\n";

/// One file from the web build.
pub struct Asset {
    pub bytes: Cow<'static, [u8]>,
    pub sha256: [u8; 32],
    pub mime: String,
}

/// Where the web files come from. The binary uses [`Embedded`]; tests use a fake.
pub trait AssetSource {
    fn get(&self, path: &str) -> Option<Asset>;
}

#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/build/"]
#[allow_missing = true]
struct WebBuild;

/// The files embedded from `web/build` at compile time. Debug builds read the
/// folder from disk at run time instead, so a rebuilt UI shows up without
/// recompiling.
pub struct Embedded;

impl AssetSource for Embedded {
    fn get(&self, path: &str) -> Option<Asset> {
        WebBuild::get(path).map(|f| Asset {
            sha256: f.metadata.sha256_hash(),
            mime: f.metadata.mimetype().to_string(),
            bytes: f.data,
        })
    }
}

/// Answers one request for `path` (the URL path, with or without a leading `/`).
pub fn respond(
    source: &dyn AssetSource,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        // RFC 9110 section 15.5.6: a 405 response MUST carry an Allow header.
        return Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .header(header::ALLOW, "GET, HEAD")
            .body(Body::empty())
            .expect("valid response");
    }
    let path = path.trim_start_matches('/');
    if path.contains('\\') || path.split('/').any(|seg| seg == "..") {
        return status(StatusCode::NOT_FOUND);
    }
    if !path.is_empty() {
        if let Some(asset) = source.get(path) {
            let cache = if path.starts_with(IMMUTABLE_PREFIX) {
                IMMUTABLE_CACHE
            } else {
                REVALIDATE
            };
            return file(method, headers, asset, cache);
        }
        let last = path.rsplit('/').next().unwrap_or_default();
        if last.contains('.') {
            return status(StatusCode::NOT_FOUND);
        }
    }
    match source.get(FALLBACK) {
        Some(asset) => file(method, headers, asset, REVALIDATE),
        None => html(method, STUB_PAGE),
    }
}

fn file(method: &Method, headers: &HeaderMap, asset: Asset, cache: &'static str) -> Response {
    let etag = etag(&asset.sha256);
    let not_modified = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|tag| tag.trim() == etag));
    let mut builder = Response::builder()
        .header(header::ETAG, &etag)
        .header(header::CACHE_CONTROL, cache);
    if not_modified {
        builder = builder.status(StatusCode::NOT_MODIFIED);
        return builder.body(Body::empty()).expect("valid response");
    }
    let mime = HeaderValue::from_str(&with_charset(&asset.mime))
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    builder = builder
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, asset.bytes.len());
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(asset.bytes)
    };
    builder.body(body).expect("valid response")
}

/// The media type with `charset=utf-8` for text, which the build writes as
/// UTF-8 (ASVS 4.1.1). A type that names a charset already stays as it is.
fn with_charset(mime: &str) -> String {
    let text = mime.starts_with("text/")
        || matches!(
            mime,
            "application/javascript" | "application/json" | "application/xml" | "image/svg+xml"
        )
        || mime.ends_with("+xml")
        || mime.ends_with("+json");
    if text && !mime.contains("charset=") {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_owned()
    }
}

fn html(method: &Method, page: &'static str) -> Response {
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(page)
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, REVALIDATE)
        .body(body)
        .expect("valid response")
}

fn status(code: StatusCode) -> Response {
    Response::builder()
        .status(code)
        .body(Body::empty())
        .expect("valid response")
}

/// A strong `ETag` from the first 16 bytes of the file's SHA-256.
fn etag(sha256: &[u8; 32]) -> String {
    let hex: String = sha256[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("\"{hex}\"")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    struct Fake(HashMap<&'static str, (&'static str, &'static str)>);

    impl AssetSource for Fake {
        fn get(&self, path: &str) -> Option<Asset> {
            self.0.get(path).map(|(body, mime)| Asset {
                bytes: Cow::Borrowed(body.as_bytes()),
                sha256: [path.len() as u8; 32],
                mime: (*mime).to_string(),
            })
        }
    }

    fn app() -> Fake {
        Fake(HashMap::from([
            (FALLBACK, ("<html>app</html>", "text/html")),
            (
                "_app/immutable/entry/start.abc123.js",
                ("js", "text/javascript"),
            ),
            ("robots.txt", ("robots", "text/plain")),
        ]))
    }

    fn get(source: &dyn AssetSource, path: &str) -> Response {
        respond(source, &Method::GET, path, &HeaderMap::new())
    }

    fn header(r: &Response, name: header::HeaderName) -> &str {
        r.headers().get(name).unwrap().to_str().unwrap()
    }

    #[test]
    fn immutable_assets_are_cached_for_a_year() {
        let r = get(&app(), "/_app/immutable/entry/start.abc123.js");
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(header(&r, header::CACHE_CONTROL), IMMUTABLE_CACHE);
        assert_eq!(
            header(&r, header::CONTENT_TYPE),
            "text/javascript; charset=utf-8"
        );
    }

    #[test]
    fn other_files_revalidate() {
        let r = get(&app(), "/robots.txt");
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(header(&r, header::CACHE_CONTROL), REVALIDATE);
        assert!(r.headers().contains_key(header::ETAG));
    }

    #[test]
    fn app_routes_get_the_fallback_page() {
        for path in ["/", "", "/vms", "/vms/web1/console"] {
            let r = get(&app(), path);
            assert_eq!(r.status(), StatusCode::OK, "path {path:?}");
            assert_eq!(header(&r, header::CONTENT_TYPE), "text/html; charset=utf-8");
            assert_eq!(header(&r, header::CACHE_CONTROL), REVALIDATE);
        }
    }

    #[test]
    fn a_missing_file_with_an_extension_is_404() {
        assert_eq!(get(&app(), "/missing.js").status(), StatusCode::NOT_FOUND);
        assert_eq!(
            get(&app(), "/_app/immutable/gone.css").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn traversal_is_rejected_before_lookup() {
        for path in ["/../Cargo.toml", "/a/../../x", "/..", "/a\\b"] {
            assert_eq!(get(&app(), path).status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[test]
    fn a_matching_etag_gets_304_without_a_body() {
        let first = get(&app(), "/robots.txt");
        let tag = header(&first, header::ETAG).to_string();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_str(&format!("\"other\", {tag}")).unwrap(),
        );
        let r = respond(&app(), &Method::GET, "/robots.txt", &headers);
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
        assert!(!r.headers().contains_key(header::CONTENT_LENGTH));
    }

    #[test]
    fn head_has_headers_but_no_body() {
        let r = respond(&app(), &Method::HEAD, "/robots.txt", &HeaderMap::new());
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(header(&r, header::CONTENT_LENGTH), "6");
        let r = respond(&Fake(HashMap::new()), &Method::HEAD, "/", &HeaderMap::new());
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[test]
    fn other_methods_get_405() {
        let r = respond(&app(), &Method::POST, "/", &HeaderMap::new());
        assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(header(&r, header::ALLOW), "GET, HEAD");
    }

    #[test]
    fn without_a_web_build_the_stub_page_is_served() {
        let r = get(&Fake(HashMap::new()), "/");
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(header(&r, header::CONTENT_TYPE), "text/html; charset=utf-8");
    }

    #[test]
    fn a_bad_mime_type_falls_back_to_octet_stream() {
        let bad = Fake(HashMap::from([("x.bin", ("x", "bad\nmime"))]));
        let r = get(&bad, "/x.bin");
        assert_eq!(header(&r, header::CONTENT_TYPE), "application/octet-stream");
    }

    #[test]
    fn the_embedded_source_answers_lookups() {
        // Present or not, a lookup through the real embed must not panic.
        let _ = Embedded.get(FALLBACK);
        assert!(Embedded.get("no/such/file.xyz").is_none());
    }

    #[test]
    fn text_types_name_utf_8_and_binary_types_do_not() {
        for (mime, want) in [
            ("text/css", "text/css; charset=utf-8"),
            ("application/json", "application/json; charset=utf-8"),
            ("image/svg+xml", "image/svg+xml; charset=utf-8"),
            (
                "application/manifest+json",
                "application/manifest+json; charset=utf-8",
            ),
            ("text/plain; charset=utf-8", "text/plain; charset=utf-8"),
            ("image/png", "image/png"),
            ("font/woff2", "font/woff2"),
            ("application/octet-stream", "application/octet-stream"),
        ] {
            assert_eq!(super::with_charset(mime), want, "{mime}");
        }
    }
}
