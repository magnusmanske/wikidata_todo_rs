//! `canned_queries.php`: the API behind `canned_queries.html`. Of the
//! given items (numbers, comma-separated), those that exist and, with
//! `params={"skip_if_proplink":[21]}`, do not link to any of those properties.

use crate::replicas::{linktarget_ids, placeholders, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, api_json};
use crate::wiki::Wiki;
use crate::wikidata::parse_id;
use axum::extract::State;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;
use wikimisc::mysql_async::Value as SqlValue;

/// The page asks for 201 at a time.
const MAX_ITEMS: usize = 10_000;
const NS_PROPERTY: i64 = 120;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    items: String,
    #[serde(default)]
    params: String,
    callback: Option<String>,
}

/// The `params` JSON.
#[derive(Debug, Default, Deserialize)]
struct Options {
    /// Property numbers (or IDs).
    skip_if_proplink: Option<Vec<Value>>,
}

#[derive(Debug, Serialize)]
struct Answer {
    items: Vec<u64>,
    status: Value,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    // As before, unreadable params mean no filter, and no items.
    let options: Options = serde_json::from_str(&request.params).unwrap_or_default();
    let items = match options.skip_if_proplink {
        Some(properties) => {
            let items = item_numbers(&request.items);
            let properties: Vec<String> = properties.iter().filter_map(|p| parse_id(&id_text(p), 'P')).collect();
            not_linking(&state, &items, &properties).await?
        }
        None => vec![],
    };
    Ok(api_json(Answer { items, status: json!({"error": "OK"}) }, request.callback))
}

/// `21`, `"21"` and `"P21"` alike.
fn id_text(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_string)
}

/// The item numbers in a comma-separated list, each once, in order.
fn item_numbers(items: &str) -> Vec<u64> {
    let mut seen = HashSet::new();
    items
        .split(',')
        .filter_map(|i| parse_id(i, 'Q')?.get(1..)?.parse().ok())
        .filter(|n| seen.insert(*n))
        .take(MAX_ITEMS)
        .collect()
}

/// Those of `items` that exist and link to none of `properties`.
async fn not_linking(state: &AppState, items: &[u64], properties: &[String]) -> anyhow::Result<Vec<u64>> {
    if items.is_empty() {
        return Ok(vec![]);
    }
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page", "pagelinks", "linktarget"]).await?;
    let titles: Vec<String> = items.iter().map(|n| format!("Q{n}")).collect();
    let sql = "SELECT page_id, page_title FROM page WHERE page_namespace=0 AND page_title IN";
    let pages: Vec<(u64, String)> = select_in(&mut conn, sql, "", &[], &titles).await?;
    let properties: Vec<&str> = properties.iter().map(String::as_str).collect();
    let targets = linktarget_ids(&mut conn, NS_PROPERTY, &properties).await?;
    let linking: HashSet<u64> = match targets.is_empty() {
        true => HashSet::new(),
        false => {
            let sql = format!(
                "SELECT DISTINCT pl_from FROM pagelinks WHERE pl_target_id IN ({}) AND pl_from IN",
                placeholders(targets.len())
            );
            let params: Vec<SqlValue> = targets.into_iter().map(SqlValue::from).collect();
            let page_ids: Vec<u64> = pages.iter().map(|(id, _)| *id).collect();
            select_in(&mut conn, &sql, "", &params, &page_ids).await?.into_iter().collect()
        }
    };
    let wanted: HashSet<String> =
        pages.into_iter().filter(|(id, _)| !linking.contains(id)).map(|(_, title)| title).collect();
    Ok(items.iter().copied().filter(|n| wanted.contains(&format!("Q{n}"))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn items_and_properties() {
        assert_eq!(item_numbers("1,Q2, 3,x,,1"), [1, 2, 3]);
        assert!(item_numbers("").is_empty());
        assert_eq!(id_text(&json!(21)), "21");
        assert_eq!(id_text(&json!("P21")), "P21");
    }

    #[tokio::test]
    async fn without_filter_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let response = get(dir.path(), "/canned_queries.php?items=1,2&params=nonsense").await;
        assert_eq!(response.status.as_u16(), 200);
        assert_eq!(response.body, r#"{"items":[],"status":{"error":"OK"}}"#);
    }
}
