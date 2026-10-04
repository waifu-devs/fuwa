//! The web client from `web/`, embedded in the binary when it's built with the
//! `web` feature. Files are served as built; any other path gets `index.html`,
//! so addresses like `/fuwa.waifu.dev/<server>/<channel>` open the app.

use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use tower_http::compression::predicate::{NotForContentType, Predicate};
use tower_http::compression::{CompressionLayer, DefaultPredicate};

use crate::app::HasSettings;

#[cfg(feature = "web")]
mod embedded {
    use axum::body::Body;
    use axum::response::{IntoResponse, Response};
    use http::{HeaderMap, HeaderValue, StatusCode, Uri, header};

    #[derive(rust_embed::Embed)]
    #[folder = "../web/dist"]
    struct Assets;

    /// The client talks to any fuwa server and shows avatars from anywhere, but
    /// runs only its own scripts. 'wasm-unsafe-eval' lets it compile its own
    /// WebAssembly (the encryption direct messages use), and nothing else.
    /// Forms only ever submit here.
    const CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; \
        style-src 'self' 'unsafe-inline'; img-src * data: blob:; media-src * blob:; connect-src *; font-src 'self' data:; \
        object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";

    pub async fn serve(uri: Uri, headers: HeaderMap) -> Response {
        let path = uri.path().trim_start_matches('/');
        let (path, file) = match Assets::get(path).filter(|_| !path.is_empty()) {
            Some(file) => (path, file),
            // Anything that looks like a file but isn't one is a real 404.
            None if path.rsplit('/').next().is_some_and(|last| last.contains('.') && path.starts_with("assets/")) => {
                return (StatusCode::NOT_FOUND, "not found\n").into_response();
            }
            None => match Assets::get("index.html") {
                Some(file) => ("index.html", file),
                None => {
                    return (StatusCode::NOT_FOUND, "the web client wasn't built into this binary\n").into_response();
                }
            },
        };

        let etag = format!("\"{}\"", hex(&file.metadata.sha256_hash()[..16]));
        let fresh = headers.get(header::IF_NONE_MATCH).is_some_and(|value| value.as_bytes() == etag.as_bytes());
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        // Built assets have content hashes in their names, so they never change.
        let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };

        let mut response = if fresh {
            StatusCode::NOT_MODIFIED.into_response()
        } else {
            Response::new(Body::from(file.data.into_owned()))
        };
        let h = response.headers_mut();
        h.insert(header::ETAG, HeaderValue::from_str(&etag).expect("hex is a valid header"));
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
        h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
        if !fresh {
            h.insert(header::CONTENT_TYPE, HeaderValue::from_str(mime.as_ref()).expect("mime types are valid headers"));
        }
        if path == "index.html" {
            h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
        }
        response
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// The handler for every path the API doesn't answer: the web app when it's
/// built in and switched on (checked per request, so admins can switch it
/// live), otherwise a short note at `/` and 404 elsewhere. Scripts and styles
/// go out gzipped; fonts and images are compressed already. Served over
/// https (the public URL says so), browsers are told to keep to it.
pub fn handler(source: Arc<impl HasSettings>) -> MethodRouter {
    let compress = DefaultPredicate::new().and(NotForContentType::const_new("font/"));
    axum::routing::any(move |method: Method, uri: Uri, headers: HeaderMap| {
        let source = source.clone();
        async move {
            let https = source.settings().public_url.starts_with("https://");
            let mut response = page(source, method, uri, headers).await;
            if https {
                response.headers_mut().insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static(HSTS));
            }
            response
        }
    })
    .layer(CompressionLayer::new().compress_when(compress))
}

/// Middleware for every public answer: one that doesn't say how long it may
/// be kept (health checks, SSO pages, API calls, 404s) is kept by nobody, so
/// a shared cache in front of the instance (Railway's CDN on fuwa.chat) never
/// stores it. Built assets and pictures say so themselves.
pub async fn no_store_by_default(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-store"));
    response
}

/// Two years, as browsers' preload lists ask.
const HSTS: &str = "max-age=63072000";

/// What a path gets: the app, the sign-in callback, a note, or a 404.
async fn page(source: Arc<impl HasSettings>, method: Method, uri: Uri, headers: HeaderMap) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return (StatusCode::NOT_FOUND, "not found\n").into_response();
    }
    let settings = source.settings();
    // Signing in with waifu.dev or single sign-on comes back to its page (and
    // the scripts it runs) even with the app off, since a fuwa app on another
    // address may have started the sign-in.
    let sign_in_page = uri.path() == crate::linked::CALLBACK || uri.path() == crate::sso::DONE;
    #[cfg(feature = "web")]
    if settings.web || sign_in_page || uri.path().starts_with("/assets/") {
        return embedded::serve(uri, headers).await;
    }
    let _ = &headers;
    if sign_in_page {
        return (
            StatusCode::NOT_FOUND,
            "this fuwa server was built without its web app, so it can't finish signing in here\n",
        )
            .into_response();
    }
    if uri.path() == "/" {
        let info = crate::app::node_info(&settings, None);
        return format!(
            "{} is a fuwa instance (fuwa {}).\nConnect to it from a fuwa client with {}\n",
            info.name, info.version, info.public_url
        )
        .into_response();
    }
    (StatusCode::NOT_FOUND, "not found\n").into_response()
}

/// Whether this binary carries the web client.
pub const BUILT_IN: bool = cfg!(feature = "web");
