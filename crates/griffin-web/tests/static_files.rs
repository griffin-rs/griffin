//! The static directory: what it serves, and that it never serves anything outside it.

use griffin_web::axum::Router;
use griffin_web::axum::body::{Body, to_bytes};
use griffin_web::axum::http::{Request, StatusCode, header};
use griffin_web::static_files;
use std::fs;
use std::path::{Path, PathBuf};
use tower::ServiceExt as _;

/// `<scratch>/static` holds the files to serve; `<scratch>/secret.txt` is outside it.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("griffin-static-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("static/assets")).unwrap();
    fs::write(dir.join("secret.txt"), "outside").unwrap();
    fs::write(dir.join("static/assets/app.css"), "body{}").unwrap();
    fs::write(dir.join("static/assets/app.js"), "1").unwrap();
    fs::write(dir.join("static/logo.svg"), "<svg/>").unwrap();
    fs::write(dir.join("static/blob.unknown"), "x").unwrap();
    dir
}

async fn get(dir: &Path, path: &str) -> (StatusCode, Option<String>, String) {
    let app: Router = static_files::serve(dir.join("static"));
    let response = app
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let kind = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned());
    let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, kind, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn files_are_served_with_their_content_type() {
    let dir = scratch("types");
    for (path, kind, body) in [
        ("/assets/app.css", "text/css; charset=utf-8", "body{}"),
        ("/assets/app.js", "text/javascript; charset=utf-8", "1"),
        ("/logo.svg", "image/svg+xml", "<svg/>"),
        ("/blob.unknown", "application/octet-stream", "x"),
    ] {
        let (status, got, text) = get(&dir, path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(got.as_deref(), Some(kind), "{path}");
        assert_eq!(text, body, "{path}");
    }
}

#[tokio::test]
async fn a_missing_file_a_directory_and_a_post_are_a_bare_404() {
    let dir = scratch("missing");
    for path in ["/nope.css", "/assets", "/assets/", "/"] {
        let (status, _, body) = get(&dir, path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(body, "", "{path}");
    }
    let app: Router = static_files::serve(dir.join("static"));
    let response = app
        .oneshot(Request::post("/logo.svg").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn nothing_outside_the_directory_is_served() {
    let dir = scratch("traversal");
    let outside = dir.join("secret.txt");
    let absolute = format!("/{}", outside.display());
    let double_encoded = "/%252e%252e/secret.txt";
    for path in [
        "/../secret.txt",
        "/assets/../../secret.txt",
        "/%2e%2e/secret.txt",
        "/%2E%2E/secret.txt",
        "/..%2fsecret.txt",
        "/assets/..%2F..%2Fsecret.txt",
        "/%2e%2e%2fsecret.txt",
        "/..%5csecret.txt",
        "/%+2e%2e/secret.txt",
        "/%+2",
        "/logo.svg%00.css",
        "//secret.txt",
        absolute.as_str(),
        double_encoded,
    ] {
        let (status, _, body) = get(&dir, path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert!(!body.contains("outside"), "{path}");
    }
}

#[test]
fn a_missing_directory_or_asset_is_an_error_that_says_what_to_run() {
    let dir = scratch("require");
    let statics = dir.join("static");
    let ok = static_files::check_built(&statics, &["assets/app.css"], "run X");
    assert_eq!(ok, Ok(()));
    let missing =
        static_files::check_built(&statics, &["assets/app.css", "assets/gone.js"], "run X");
    let message = missing.unwrap_err();
    assert!(
        message.contains("assets/gone.js") && message.contains("run X"),
        "{message}"
    );
    let none = static_files::check_built(&dir.join("nope"), &[], "run X").unwrap_err();
    assert!(none.contains("nope") && none.contains("run X"), "{none}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_that_points_out_of_the_directory_is_not_followed() {
    let dir = scratch("symlink");
    std::os::unix::fs::symlink(dir.join("secret.txt"), dir.join("static/link.txt")).unwrap();
    std::os::unix::fs::symlink(&dir, dir.join("static/up")).unwrap();
    for path in ["/link.txt", "/up/secret.txt"] {
        let (status, _, body) = get(&dir, path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert!(!body.contains("outside"), "{path}");
    }
    // A symlink that stays inside is fine.
    std::os::unix::fs::symlink(dir.join("static/logo.svg"), dir.join("static/same.svg")).unwrap();
    assert_eq!(get(&dir, "/same.svg").await.0, StatusCode::OK);
}
