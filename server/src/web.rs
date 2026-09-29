//! The web client from `web/`, embedded in the binary when it's built with the
//! `web` feature. Files are served as built; any other path gets `index.html`,
//! so addresses like `/fuwa.waifu.dev/<server>/<channel>` open the app.

use axum::routing::MethodRouter;

#[cfg(feature = "web")]
mod embedded {
    use axum::body::Body;
    use axum::response::{IntoResponse, Response};
    use http::{HeaderMap, HeaderValue, StatusCode, Uri, header};

    #[derive(rust_embed::Embed)]
    #[folder = "../web/dist"]
    struct Assets;

    /// The client talks to any fuwa server and shows avatars from anywhere, but
    /// runs only its own scripts.
    const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
        img-src * data: blob:; connect-src *; font-src 'self' data:; object-src 'none'; \
        base-uri 'none'; frame-ancestors 'none'";

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
                None => return (StatusCode::NOT_FOUND, "the web client wasn't built into this binary\n").into_response(),
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

/// The handler for every path the API doesn't answer, or `None` when the
/// client is off or wasn't built in.
pub fn fallback(enabled: bool) -> Option<MethodRouter> {
    #[cfg(feature = "web")]
    if enabled {
        return Some(axum::routing::get(embedded::serve));
    }
    let _ = enabled;
    None
}

/// Whether this binary carries the web client.
pub const BUILT_IN: bool = cfg!(feature = "web");
