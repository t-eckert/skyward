//! The web client, compiled into the binary.
//!
//! # Why embedded rather than served from a directory
//!
//! For the same reason there is one binary rather than a decoder and an API:
//! two artifacts means two ways to deploy the wrong version. A `--web-root`
//! pointing at a directory on the Pi would let a six-week-old client sit in
//! front of a freshly deployed server, showing fields the API no longer sends
//! and silently omitting the ones it does — with nothing anywhere saying the
//! two disagree. Baking the assets in makes that impossible: `skyward
//! --version` describes the interface as well as the decoder.
//!
//! It also means deployment stays `scp skyward pi:` and nothing else.
//!
//! # Caching
//!
//! SvelteKit content-hashes everything under `_app/immutable/`, so those may be
//! cached forever — the filename changes when the bytes do. `index.html` must
//! never be cached, because it is what points at the current hashed filenames;
//! a stale copy of it references assets that no longer exist and the app fails
//! to boot after an upgrade.

use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "$CARGO_MANIFEST_DIR/../../client/build"]
struct Assets;

/// Assets under this prefix carry a content hash in the filename.
const IMMUTABLE_PREFIX: &str = "_app/immutable/";

const ONE_YEAR: &str = "public, max-age=31536000, immutable";
const NO_CACHE: &str = "no-cache";

/// Paths that belong to the server, and must never be answered with the app.
///
/// A router fallback catches *every* unmatched path, including misspelled and
/// removed API routes. Without this list `GET /api/v1/aircaft` returned `200`
/// and a page of HTML, so `fetch` resolved successfully and the client fell
/// over parsing `<!doctype html>` as JSON — which is a long way from the URL
/// that was actually wrong.
const RESERVED: [&str; 3] = ["api/", "healthz", "readyz"];

/// Serve an embedded asset, falling back to the SPA entry point.
///
/// The fallback is what makes a deep link work: `/aircraft/A0A41F` is a client
/// route with no file behind it, so a reload has to be answered with
/// `index.html` and resolved in the browser.
pub async fn handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if RESERVED.iter().any(|p| path.starts_with(p)) {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": format!("no such endpoint: /{path}") })),
        )
            .into_response();
    }

    match serve(path) {
        Some(response) => response,
        // Not a file we hold: hand back the app and let the client router
        // decide whether the URL means anything.
        None => serve("index.html").unwrap_or_else(|| {
            (StatusCode::NOT_FOUND, "client not built into this binary").into_response()
        }),
    }
}

fn serve(path: &str) -> Option<Response> {
    let asset = Assets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();

    let cache = if path.starts_with(IMMUTABLE_PREFIX) {
        ONE_YEAR
    } else {
        NO_CACHE
    };

    Some(
        Response::builder()
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(header::CACHE_CONTROL, cache)
            .body(Body::from(asset.data.into_owned()))
            .expect("static response is always well formed")
            .into_response(),
    )
}

/// Whether a real client was compiled in, for `doctor` to report.
///
/// The build script writes a placeholder page when `client/build` is missing,
/// so the presence of `index.html` proves nothing. The hashed asset directory
/// only exists if the client was actually built.
pub fn is_built() -> bool {
    Assets::iter().any(|f| f.starts_with(IMMUTABLE_PREFIX))
}

/// How many files, and how many bytes, are embedded.
pub fn footprint() -> (usize, usize) {
    Assets::iter().fold((0, 0), |(n, bytes), f| {
        let size = Assets::get(&f).map(|a| a.data.len()).unwrap_or(0);
        (n + 1, bytes + size)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_point_is_always_present() {
        // Either the real client or the build script's placeholder, but never
        // nothing -- a binary that serves no page at all is a broken deploy
        // that only shows up in a browser.
        assert!(Assets::get("index.html").is_some());
    }

    #[tokio::test]
    async fn an_unknown_path_returns_the_app_not_a_404() {
        // Deep links like /aircraft/A0A41F have no file behind them.
        let response = handler("/aircraft/A0A41F".parse::<Uri>().unwrap()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/html")
        );
    }

    /// The bug this guards against shipped once and was found by curl, not by
    /// a test: a typo'd endpoint answered `200 text/html`, so the client's
    /// `fetch` succeeded and then failed parsing HTML as JSON.
    #[tokio::test]
    async fn an_unknown_api_path_is_a_json_404_not_the_app() {
        for path in ["/api/v1/aircaft", "/api/", "/healthz/extra", "/readyz2"] {
            let response = handler(path.parse::<Uri>().unwrap()).await;
            let kind = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if path == "/readyz2" {
                // Not reserved -- `readyz2` is a plausible client route.
                continue;
            }
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert!(kind.starts_with("application/json"), "{path} gave {kind}");
        }
    }

    #[tokio::test]
    async fn the_entry_point_is_never_cached() {
        let response = handler("/".parse::<Uri>().unwrap()).await;
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some(NO_CACHE),
            "a cached index.html points at asset names that no longer exist \
             after an upgrade, and the app fails to boot"
        );
    }

    #[test]
    fn hashed_assets_are_cached_forever() {
        let Some(hashed) = Assets::iter().find(|f| f.starts_with(IMMUTABLE_PREFIX)) else {
            // The client has not been built in this checkout; nothing to check.
            return;
        };
        let response = serve(&hashed).expect("asset exists");
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some(ONE_YEAR)
        );
    }
}
