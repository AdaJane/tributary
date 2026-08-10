//! The embedded web console — compiled only with `--features embed-ui`, so
//! plain builds never need `web/dist` to exist. Debug builds read the dist
//! from disk at runtime; release builds bake it into the binary.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

const INDEX: &str = "index.html";
/// Vite content-hashes everything under assets/, so those bytes never
/// change under their name; the shell itself must revalidate.
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
const CACHE_REVALIDATE: &str = "no-cache";

#[derive(rust_embed::Embed)]
#[folder = "../../web/dist/"]
struct Assets;

/// SPA fallback: exact assets stream from the binary; extensionless paths
/// (client-side routes) get the shell; missing files with extensions 404.
pub async fn serve(uri: Uri) -> Response {
    let Some(name) = resolve(uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match Assets::get(name) {
        Some(file) => asset_response(name, file),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The resolve table. `/` and client routes map to the shell; anything
/// with an extension is only itself.
fn resolve(path: &str) -> Option<&str> {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { INDEX } else { path };
    if Assets::get(path).is_some() {
        Some(path)
    } else if !path.contains('.') {
        Some(INDEX)
    } else {
        None
    }
}

/// MIME comes from the *resolved* name, never the request path — a deep
/// link like `/tracks` must come back as text/html.
fn asset_response(name: &str, file: rust_embed::EmbeddedFile) -> Response {
    let cache = if name.starts_with("assets/") {
        CACHE_IMMUTABLE
    } else {
        CACHE_REVALIDATE
    };
    (
        [
            (header::CONTENT_TYPE, file.metadata.mimetype().to_string()),
            (header::CACHE_CONTROL, cache.to_string()),
        ],
        file.data,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    // These run against the real web/dist (build the web app first — the
    // `dist` recipe and the release workflow both do).

    #[test]
    fn root_and_client_routes_resolve_to_the_shell() {
        assert_eq!(resolve("/"), Some(INDEX));
        assert_eq!(resolve("/tracks"), Some(INDEX));
        assert_eq!(resolve("/setup/deep/link"), Some(INDEX));
    }

    #[test]
    fn missing_files_with_extensions_do_not_resolve() {
        assert_eq!(resolve("/assets/nope.js"), None);
        assert_eq!(resolve("/favicon.nope"), None);
    }

    #[tokio::test]
    async fn the_shell_is_html_and_revalidates() {
        let res = serve(Uri::from_static("/")).await;
        assert_eq!(res.status(), StatusCode::OK);
        let content_type = res.headers()[header::CONTENT_TYPE].to_str().unwrap();
        assert!(content_type.starts_with("text/html"), "{content_type}");
        assert_eq!(res.headers()[header::CACHE_CONTROL], CACHE_REVALIDATE);
    }

    #[tokio::test]
    async fn hashed_assets_serve_immutable_with_their_own_mime() {
        let name = Assets::iter()
            .find(|p| p.starts_with("assets/") && p.ends_with(".js"))
            .expect("vite emitted hashed js");
        let uri: Uri = format!("/{name}").parse().unwrap();
        let res = serve(uri).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CACHE_CONTROL], CACHE_IMMUTABLE);
        let content_type = res.headers()[header::CONTENT_TYPE].to_str().unwrap();
        assert!(content_type.contains("javascript"), "{content_type}");
    }

    #[tokio::test]
    async fn missing_assets_are_404() {
        let res = serve(Uri::from_static("/assets/nope.js")).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
