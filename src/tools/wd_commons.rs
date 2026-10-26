//! `wd_commons.php`: Commons files and categories that link to Wikidata
//! items (`[[d:Q…]]`), for the items of a Wikidata search, in any language,
//! or of a query.

use crate::mediawiki;
use crate::replicas::{display_title, select_in};
use crate::state::AppState;
use crate::terms;
use crate::wdq;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

const MAX_ITEMS: usize = 500;
const MAX_FILES_PER_ITEM: usize = 50;
const SEARCH_LIMIT: &str = "50";
const NS_FILE: i64 = 6;
const NS_CATEGORY: i64 = 14;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Request {
    query: String,
    wdq: String,
}

/// What links to an item on Commons.
#[derive(Debug, Default)]
struct Links {
    files: Vec<String>,
    categories: Vec<String>,
}

struct Hit {
    item: String,
    text: String,
    links: Links,
}

struct Results {
    heading: &'static str,
    /// The same search on Commons.
    commons_search: Option<String>,
    items: usize,
    hits: Vec<Hit>,
}

#[derive(Template)]
#[template(path = "tools/wd_commons.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    supported: &'static str,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let (query, wdq) = (request.query.trim(), request.wdq.trim());
    let results = match (query.is_empty(), wdq.is_empty()) {
        (false, _) => Some(search(&state, query).await?),
        (true, false) => Some(from_query(&state, wdq).await?),
        (true, true) => None,
    };
    render(&Page { title: "Wikidata Commons search", request: &request, supported: wdq::SUPPORTED, results })
}

async fn search(state: &AppState, query: &str) -> Result<Results, AppError> {
    let params =
        [("action", "query"), ("list", "search"), ("srnamespace", "0"), ("srlimit", SEARCH_LIMIT), ("srsearch", query)];
    let json = mediawiki::api(&state.http, &Wiki::wikidata(), &params).await?;
    let found: Vec<(String, String)> = json["query"]["search"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|hit| {
            let snippet = mediawiki::html_to_text(hit["snippet"].as_str().unwrap_or_default());
            Some((hit["title"].as_str()?.to_string(), snippet))
        })
        .collect();
    let commons_search = format!(
        "https://commons.wikimedia.org/w/index.php?title=Special:MediaSearch&type=image&search={}",
        encode_component(query)
    );
    let mut results = hits(state, "Search results", found).await?;
    results.commons_search = Some(commons_search);
    Ok(results)
}

async fn from_query(state: &AppState, query: &str) -> Result<Results, AppError> {
    let found = wdq::entities(&state.sparql, query, MAX_ITEMS).await?;
    let items: Vec<String> = found.ids.into_iter().filter(|i| i.starts_with('Q')).collect();
    let labels = terms::labels(&state.replicas, &items, terms::DEFAULT_LANGUAGES).await?;
    let found = items
        .into_iter()
        .map(|item| {
            let label = labels.get(&item).cloned().unwrap_or_default();
            (item, label)
        })
        .collect();
    hits(state, "Query results", found).await
}

/// The items, with their text, that have links on Commons.
async fn hits(state: &AppState, heading: &'static str, found: Vec<(String, String)>) -> Result<Results, AppError> {
    let items: Vec<String> = found.iter().map(|(item, _)| item.clone()).collect();
    let mut links = commons_links(state, &items).await?;
    let hits =
        found.into_iter().filter_map(|(item, text)| Some(Hit { links: links.remove(&item)?, item, text })).collect();
    Ok(Results { heading, commons_search: None, items: items.len(), hits })
}

/// The Commons files and categories that link to each item.
async fn commons_links(state: &AppState, items: &[String]) -> anyhow::Result<HashMap<String, Links>> {
    let mut conn = state.replicas.conn(&Wiki::commons(), &["iwlinks", "page"]).await?;
    let sql = "SELECT iwl_title, page_namespace, page_title FROM iwlinks JOIN page ON page_id=iwl_from
        WHERE iwl_prefix='wikidata' AND iwl_title IN";
    let rows: Vec<(String, i64, String)> = select_in(&mut conn, sql, "ORDER BY page_title", &[], items).await?;
    let mut links: HashMap<String, Links> = HashMap::new();
    for (item, namespace, title) in rows {
        let entry = links.entry(item).or_default();
        match namespace {
            NS_FILE if entry.files.len() < MAX_FILES_PER_ITEM => entry.files.push(display_title(&title)),
            NS_CATEGORY => entry.categories.push(display_title(&title)),
            _ => {}
        }
    }
    links.retain(|_, l| !l.files.is_empty() || !l.categories.is_empty());
    Ok(links)
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

    #[tokio::test]
    async fn form_and_bad_wdq() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/wd_commons/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"query\""));
        assert_eq!(get(dir.path(), "/wd_commons/?wdq=claim%5B").await.status.as_u16(), 400);
    }
}
