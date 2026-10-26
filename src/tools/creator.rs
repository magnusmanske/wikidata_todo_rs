//! `creator.php`: the API of `creator.html`. `action=check_titles_aliases`
//! lists the items that have `title` as label or alias, in any language.

use crate::state::AppState;
use crate::terms::{self, TermType};
use crate::web::{AppError, Params, api_json};
use axum::extract::State;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    action: String,
    #[serde(default)]
    title: String,
    callback: Option<String>,
}

#[derive(Serialize)]
struct Answer {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Vec<String>>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let data = match request.action.as_str() {
        "check_titles_aliases" => Some(items_named(&state, &request.title).await?),
        _ => None,
    };
    Ok(api_json(Answer { status: "OK", data }, request.callback))
}

/// Items with `title` (spaces or underscores) as label or alias.
async fn items_named(state: &AppState, title: &str) -> Result<Vec<String>, AppError> {
    let title = title.replace('_', " ").trim().to_string();
    if title.is_empty() {
        return Ok(vec![]);
    }
    let types = [TermType::Label, TermType::Alias];
    let mut items: Vec<String> =
        terms::items_with_texts(&state.replicas, &[title], &types, None).await?.into_iter().map(|(q, _)| q).collect();
    items.sort_by_key(|q| q.get(1..).and_then(|n| n.parse::<u64>().ok()));
    items.dedup();
    Ok(items)
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

    #[tokio::test]
    async fn without_action_just_ok() {
        let dir = tempfile::tempdir().unwrap();
        let response = get(dir.path(), "/creator.php").await;
        assert_eq!(response.status.as_u16(), 200);
        assert_eq!(response.body, r#"{"status":"OK"}"#);
        let empty = get(dir.path(), "/creator.php?action=check_titles_aliases&title=_").await;
        assert_eq!(empty.body, r#"{"status":"OK","data":[]}"#);
    }
}
