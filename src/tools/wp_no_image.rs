//! `wp_no_image/api.php`: the lists of Wikipedia articles without an image
//! (`wp_no_image/<wiki>.html`) mark an article as having one after all; it
//! is added to `<wiki>.has_image`, which the lists leave out.

use crate::state::AppState;
use crate::web::{AppError, Params};
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Page titles are at most 255 bytes.
const MAX_TITLE: usize = 255;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Request {
    wiki: String,
    page: String,
}

pub async fn api(State(state): State<Arc<AppState>>, Params(request): Params<Request>) -> Result<Response, AppError> {
    let page = title(&request.page);
    if request.wiki.is_empty() || page.is_empty() {
        // As the PHP: nothing to do, nothing to say.
        return Ok(([(header::CONTENT_TYPE, "text/plain; charset=UTF-8")], "").into_response());
    }
    let list = has_image_file(&state.config.public_html.join("wp_no_image"), &request.wiki)
        .await
        .ok_or_else(|| AppError::BadRequest(format!("No list for wiki '{}'", request.wiki)))?;
    crate::append::line_once(&list, &page).await?;
    Ok(Json(json!({"status": "OK"})).into_response())
}

/// `<wiki>.has_image` in `dir`, for the wikis that have a list there (the
/// PHP took any `wiki`, even `../../somewhere`).
async fn has_image_file(dir: &Path, wiki: &str) -> Option<PathBuf> {
    let valid = wiki
        .strip_suffix("wiki")
        .is_some_and(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
    if !valid {
        return None;
    }
    let file = |extension: &str| dir.join(format!("{wiki}.{extension}"));
    for extension in ["html", "has_image"] {
        if tokio::fs::try_exists(file(extension)).await.unwrap_or(false) {
            return Some(file("has_image"));
        }
    }
    None
}

/// The page title on one line, cut to the longest a title can be.
fn title(page: &str) -> String {
    let mut title: String = page.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string();
    while title.len() > MAX_TITLE {
        title.pop();
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn titles() {
        assert_eq!(title(" A_b\r\nc "), "A_bc");
        assert_eq!(title(&"ä".repeat(200)).len(), 254);
    }

    #[tokio::test]
    async fn marks_pages() {
        let dir = tempfile::tempdir().unwrap();
        let lists = dir.path().join("wp_no_image");
        std::fs::create_dir(&lists).unwrap();
        std::fs::write(lists.join("cawiki.html"), "").unwrap();
        std::fs::write(dir.path().join("secretwiki.html"), "").unwrap();
        let done = get(dir.path(), "/wp_no_image/api.php?wiki=cawiki&page=Barcelona%0Aevil").await;
        assert_eq!(done.body, r#"{"status":"OK"}"#);
        get(dir.path(), "/wp_no_image/api.php?wiki=cawiki&page=Girona").await;
        assert_eq!(std::fs::read_to_string(lists.join("cawiki.has_image")).unwrap(), "Barcelonaevil\nGirona\n");
        for wiki in ["dewiki", "../secretwiki", "x/cawiki", "wiki"] {
            let refused = get(dir.path(), &format!("/wp_no_image/api.php?wiki={wiki}&page=A")).await;
            assert_eq!(refused.status.as_u16(), 400, "{wiki}");
        }
        assert_eq!(get(dir.path(), "/wp_no_image/api.php?wiki=cawiki").await.body, "");
    }
}
