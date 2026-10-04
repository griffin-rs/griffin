//! The static directory: files served from disk, as they are when the request comes, so
//! that a rebuilt stylesheet or script needs no restart.
//!
//! [`serve`] is a [`Router`] whose fallback answers `GET` and `HEAD` for a file under one
//! directory. Merge it last, so that every route of the table goes first. It answers
//! 404 with no body for everything else (a missing file, a directory, an escape), which
//! [`ErrorPages`](crate::error::ErrorPages) can dress.
//!
//! Nothing outside the directory is served. The path is percent-decoded once, split on
//! `/`, and refused if any segment is `..` or holds a NUL or a `\`; then the file is
//! resolved with symlinks and must still be under the resolved directory, so a link
//! that points out of it is not followed.
//!
//! Every response carries `x-content-type-options: nosniff` and `cache-control:
//! no-cache`. The files are read whole: serve large downloads some other way
//! (no ranges, no streaming).

use axum::Router;
use axum::body::Body;
use axum::http::{Method, StatusCode, Uri, header};
use axum::response::Response;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Fails, naming the first of `files` (paths under `dir`) that is not a file there, and
/// saying `how_to_build` it. For an application to call at boot, so that a missing
/// directory or an asset that was never built is an error at start and not a silent 404.
pub fn check_built(dir: &Path, files: &[&str], how_to_build: &str) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!(
            "the static directory {} does not exist: {how_to_build}",
            dir.display()
        ));
    }
    match files.iter().find(|file| !dir.join(file).is_file()) {
        Some(file) => Err(format!(
            "{} is missing from the static directory {}: {how_to_build}",
            file,
            dir.display()
        )),
        None => Ok(()),
    }
}

/// A router that serves the files of `dir` as its fallback.
pub fn serve<S: Clone + Send + Sync + 'static>(dir: impl Into<PathBuf>) -> Router<S> {
    let dir = Arc::new(dir.into());
    Router::new().fallback(move |method: Method, uri: Uri| {
        let dir = dir.clone();
        async move { respond(&dir, &method, uri.path()).await }
    })
}

fn not_found() -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

async fn respond(root: &Path, method: &Method, path: &str) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return not_found();
    }
    let Some(file) = resolve(root, path).await else {
        return not_found();
    };
    let Ok(bytes) = tokio::fs::read(&file).await else {
        return not_found();
    };
    let mut response = Response::new(if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(bytes)
    });
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        content_type(&file)
            .parse()
            .unwrap_or_else(|_| header::HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    response
}

/// The file `path` names under `root`, if it is a file that really is under `root`.
async fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    let mut file = root.to_path_buf();
    for segment in decoded.split('/') {
        if segment.contains(['\0', '\\']) || segment == ".." {
            return None;
        }
        if !segment.is_empty() && segment != "." {
            file.push(segment);
        }
    }
    let root = tokio::fs::canonicalize(root).await.ok()?;
    let file = tokio::fs::canonicalize(file).await.ok()?;
    (file.starts_with(&root) && tokio::fs::metadata(&file).await.ok()?.is_file()).then_some(file)
}

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}
