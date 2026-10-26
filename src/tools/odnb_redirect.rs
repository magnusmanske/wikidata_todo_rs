//! `odnb_redirect.php`: redirects an Oxford Dictionary of National Biography
//! ID to its page there: index entries (`101…`, nine digits) and articles.

use crate::web::{AppError, Params};
use axum::response::Redirect;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    id: String,
}

pub async fn handler(Params(request): Params<Request>) -> Result<Redirect, AppError> {
    let url = odnb_url(&request.id).ok_or_else(|| AppError::BadRequest("The ID must be a number.".to_string()))?;
    Ok(Redirect::to(&url))
}

fn odnb_url(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(match id.len() == 9 && id.starts_with("101") {
        true => format!("https://www.oxforddnb.com/index/{id}/"),
        false => format!("https://www.oxforddnb.com/view/article/{id}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn urls() {
        assert_eq!(odnb_url(" 1234 ").as_deref(), Some("https://www.oxforddnb.com/view/article/1234"));
        assert_eq!(odnb_url("101001234").as_deref(), Some("https://www.oxforddnb.com/index/101001234/"));
        assert_eq!(odnb_url("1010012345").as_deref(), Some("https://www.oxforddnb.com/view/article/1010012345"));
        assert_eq!(odnb_url("12a"), None);
        assert_eq!(odnb_url("1/../x"), None);
        assert_eq!(odnb_url(""), None);
    }

    #[tokio::test]
    async fn redirects() {
        let dir = tempfile::tempdir().unwrap();
        let response = get(dir.path(), "/odnb_redirect.php?id=1234").await;
        assert_eq!(response.status.as_u16(), 303);
        assert_eq!(response.headers["location"], "https://www.oxforddnb.com/view/article/1234");
        assert_eq!(get(dir.path(), "/odnb_redirect.php?id=x").await.status.as_u16(), 400);
    }
}
