//! The apps with their own interface (Duplicity, Relator, …): their
//! HTML, JavaScript and CSS live in `public/` and are built into the binary.
//! Their APIs are tools of their own (`src/tools/`).

use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use include_dir::{Dir, include_dir};
use percent_encoding::percent_decode_str;

static PUBLIC: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/public");

/// Middleware in front of the PHP checkout's files: serves an app file if
/// there is one for the path.
pub async fn serve(request: Request, next: Next) -> Response {
    match lookup(request.uri().path()) {
        Some(Found::File(path, contents)) => file(path, contents),
        Some(Found::Directory(path)) => Redirect::permanent(&format!("/{path}/")).into_response(),
        None => next.run(request).await,
    }
}

enum Found {
    File(&'static str, &'static [u8]),
    /// A directory addressed without its trailing slash.
    Directory(String),
}

fn lookup(uri_path: &str) -> Option<Found> {
    let path = percent_decode_str(uri_path).decode_utf8().ok()?;
    let relative = path.trim_start_matches('/');
    if relative.split('/').any(|s| s.starts_with('.')) {
        return None;
    }
    let wanted = match relative.strip_suffix('/') {
        Some(dir) => format!("{dir}/index.html"),
        None if PUBLIC.get_dir(relative).is_some() => return Some(Found::Directory(relative.to_string())),
        None => relative.to_string(),
    };
    let file = PUBLIC.get_file(wanted)?;
    Some(Found::File(file.path().to_str()?, file.contents()))
}

fn file(path: &'static str, contents: &'static [u8]) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = (StatusCode::OK, contents).into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    // Changes come with deployments, which restart the service.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=600"));
    response
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

    #[tokio::test]
    async fn apps_from_the_binary() {
        // The PHP checkout is empty here: everything comes from `public/`.
        let dir = tempfile::tempdir().unwrap();
        let index = get(dir.path(), "/duplicity/").await;
        assert_eq!(index.status.as_u16(), 200);
        assert!(index.headers["content-type"].to_str().unwrap().starts_with("text/html"));
        let js = get(dir.path(), "/relator2/app.js").await;
        assert_eq!(js.status.as_u16(), 200);
        assert!(js.headers["content-type"].to_str().unwrap().contains("javascript"));
        assert_eq!(get(dir.path(), "/relator2").await.headers["location"], "/relator2/");
        assert_eq!(get(dir.path(), "/papertrail.html").await.status.as_u16(), 200);
        // Not there: on to the PHP checkout, which has nothing either.
        assert_eq!(get(dir.path(), "/relator2/no_such_file.js").await.status.as_u16(), 404);
        assert_eq!(get(dir.path(), "/relator2/../Cargo.toml").await.status.as_u16(), 404);
    }
}
