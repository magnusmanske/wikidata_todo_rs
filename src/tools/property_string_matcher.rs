//! `property_string_matcher.php`: the items that have given strings as
//! values of a property (an external ID, say).

use crate::sparql;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wikidata::parse_id;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use futures::{StreamExt, TryStreamExt, stream};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

const MAX_VALUES: usize = 10_000;
/// Values per query.
const CHUNK: usize = 500;
const CONCURRENCY: usize = 4;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    prop: String,
    #[serde(default)]
    rows: String,
    doit: Option<String>,
}

struct Results {
    property: String,
    values: usize,
    matches: Vec<(String, String)>,
}

#[derive(Template)]
#[template(path = "tools/property_string_matcher.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    max_values: usize,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mut page = Page { title: "Property string matcher", request: &request, max_values: MAX_VALUES, results: None };
    if request.doit.is_none() {
        return render(&page);
    }
    let property = parse_id(&request.prop, 'P')
        .ok_or_else(|| AppError::BadRequest("Please give a property, such as P214.".to_string()))?;
    let values = values(&request.rows);
    let found = matches(&state, &property, &values).await?;
    // In the order of the input.
    let matches = values
        .iter()
        .flat_map(|value| found.get(*value).into_iter().flatten().map(|item| (item.clone(), value.to_string())))
        .collect();
    page.results = Some(Results { property, values: values.len(), matches });
    render(&page)
}

/// The non-empty lines, each once, at most [`MAX_VALUES`].
fn values(rows: &str) -> Vec<&str> {
    let mut seen = std::collections::HashSet::new();
    rows.lines().map(str::trim).filter(|v| !v.is_empty() && seen.insert(*v)).take(MAX_VALUES).collect()
}

/// The items for each value.
async fn matches(state: &AppState, property: &str, values: &[&str]) -> anyhow::Result<HashMap<String, Vec<String>>> {
    let queries: Vec<String> = values.chunks(CHUNK).map(|chunk| query(property, chunk)).collect();
    let results: Vec<serde_json::Value> = stream::iter(queries)
        .map(|q| async move { state.sparql.json(&q).await })
        .buffered(CONCURRENCY)
        .try_collect()
        .await?;
    let mut found: HashMap<String, Vec<String>> = HashMap::new();
    for json in &results {
        for row in sparql::bindings(json) {
            let (Some(item), Some(value)) =
                (row["item"]["value"].as_str().and_then(sparql::entity_id), row["value"]["value"].as_str())
            else {
                continue;
            };
            let items = found.entry(value.to_string()).or_default();
            if !items.iter().any(|i| i == item) {
                items.push(item.to_string());
            }
        }
    }
    Ok(found)
}

/// Statements of any rank count.
fn query(property: &str, values: &[&str]) -> String {
    let values: Vec<String> = values.iter().map(|v| sparql::literal(v)).collect();
    format!(
        "SELECT ?item ?value {{ VALUES ?value {{ {} }} ?item p:{property}/ps:{property} ?value }}",
        values.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn values_and_query() {
        assert_eq!(values(" a\n\nb \na\n"), ["a", "b"]);
        assert_eq!(
            query("P214", &["1", "x\"y"]),
            r#"SELECT ?item ?value { VALUES ?value { "1" "x\"y" } ?item p:P214/ps:P214 ?value }"#
        );
    }

    #[tokio::test]
    async fn form_and_bad_property() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/property_string_matcher/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"rows\""));
        assert_eq!(get(dir.path(), "/property_string_matcher/?prop=x&doit=1").await.status.as_u16(), 400);
    }
}
