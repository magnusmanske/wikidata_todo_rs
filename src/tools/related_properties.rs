//! `related_properties.php`: the properties that the items of a query (SPARQL,
//! or WDQ as of old) use, by how many of them do. With `botmode`, as JSON(P).

use crate::replicas::select_in;
use crate::state::AppState;
use crate::terms;
use crate::wdq;
use crate::web::{AppError, Params, api_json, render, thousands};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Of larger results, only the first are looked at.
const MAX_ITEMS: usize = 50_000;
const NS_PROPERTY: i64 = 120;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    q: String,
    botmode: Option<String>,
    callback: Option<String>,
}

/// As the PHP tool answered: the count as a string.
#[derive(Debug, Serialize)]
struct Usage {
    property: String,
    cnt: String,
}

struct Row {
    property: String,
    label: Option<String>,
    count: String,
    percent: String,
    items_url: Option<String>,
}

struct Results {
    items: String,
    sampled: bool,
    rows: Vec<Row>,
}

#[derive(Template)]
#[template(path = "tools/related_properties.html")]
struct Page {
    title: &'static str,
    q: String,
    supported: &'static str,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let q = request.q.trim();
    let mut page = Page { title: "Related properties", q: q.to_string(), supported: wdq::SUPPORTED, results: None };
    if q.is_empty() {
        return Ok(match request.botmode {
            Some(_) => api_json(Vec::<Usage>::new(), request.callback),
            None => render(&page)?.into_response(),
        });
    }
    let found = wdq::entities(&state.sparql, q, MAX_ITEMS).await?;
    let (var, sampled) = (found.var, found.total > MAX_ITEMS);
    let items: Vec<String> = found.ids.into_iter().filter(|id| id.starts_with('Q')).collect();
    let counts = property_counts(&state, &items).await?;
    if request.botmode.is_some() {
        let usages: Vec<Usage> =
            counts.into_iter().map(|(property, n)| Usage { property, cnt: n.to_string() }).collect();
        return Ok(api_json(usages, request.callback));
    }
    let ids: Vec<String> = counts.iter().map(|(p, _)| p.clone()).collect();
    let labels = terms::labels(&state.replicas, &ids, &["en"]).await?;
    let total = items.len().max(1) as f64;
    let rows = counts
        .into_iter()
        .map(|(property, n)| Row {
            label: labels.get(&property).cloned(),
            count: thousands(n),
            percent: format!("{:.1}", n as f64 * 100.0 / total),
            items_url: subgroup_query(q, &var, &property).map(|query| petscan_url(&query)),
            property,
        })
        .collect();
    page.results = Some(Results { items: thousands(items.len() as u64), sampled, rows });
    Ok(render(&page)?.into_response())
}

/// How many of the items use each property (in statements, qualifiers or
/// references), most used first.
async fn property_counts(state: &AppState, items: &[String]) -> anyhow::Result<Vec<(String, u64)>> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page", "pagelinks", "linktarget"]).await?;
    let sql = "SELECT page_id FROM page WHERE page_namespace=0 AND page_title IN";
    let page_ids: Vec<u64> = select_in(&mut conn, sql, "", &[], items).await?;
    // Without STRAIGHT_JOIN, the database may start from the properties, and their many links.
    let sql = "SELECT lt_title, COUNT(*) FROM pagelinks STRAIGHT_JOIN linktarget ON pl_target_id=lt_id
        WHERE lt_namespace=? AND pl_from IN";
    let rows: Vec<(String, u64)> =
        select_in(&mut conn, sql, "GROUP BY lt_title", &[NS_PROPERTY.into()], &page_ids).await?;
    let mut counts: HashMap<String, u64> = HashMap::new();
    for (property, n) in rows {
        *counts.entry(property).or_default() += n;
    }
    Ok(sorted(counts))
}

/// Most used first; ties by property number.
fn sorted(counts: HashMap<String, u64>) -> Vec<(String, u64)> {
    let mut counts: Vec<(String, u64)> = counts.into_iter().collect();
    let number = |p: &str| p.get(1..).and_then(|n| n.parse::<u64>().ok()).unwrap_or(u64::MAX);
    counts.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| number(a).cmp(&number(b))));
    counts
}

/// The query for those of the items that use `property`, if it can be
/// built: SPARQL with prefix declarations cannot be nested.
fn subgroup_query(q: &str, var: &str, property: &str) -> Option<String> {
    match q.contains('{') {
        false => wdq::to_sparql(&format!("{q} and claim[{property}]")).ok(),
        true if q.to_lowercase().contains("prefix") => None,
        true => Some(format!("SELECT ?{var} WHERE {{ {{ {q} }} ?{var} p:{property} [] }}")),
    }
}

fn petscan_url(query: &str) -> String {
    format!("https://petscan.toolforge.org/?doit=1&sparql={}", encode_component(query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn most_used_first() {
        let counts = HashMap::from([("P31".to_string(), 5), ("P2".to_string(), 5), ("P18".to_string(), 9)]);
        assert_eq!(sorted(counts), [("P18".to_string(), 9), ("P2".to_string(), 5), ("P31".to_string(), 5)]);
    }

    #[test]
    fn subgroups() {
        assert_eq!(
            subgroup_query("claim[31:5]", "item", "P18").as_deref(),
            Some("SELECT DISTINCT ?item { ?item p:P31/ps:P31 wd:Q5 . ?item p:P18 [] . }")
        );
        assert_eq!(
            subgroup_query("SELECT ?q { ?q wdt:P31 wd:Q5 }", "q", "P18").as_deref(),
            Some("SELECT ?q WHERE { { SELECT ?q { ?q wdt:P31 wd:Q5 } } ?q p:P18 [] }")
        );
        assert_eq!(subgroup_query("PREFIX x: <y> SELECT ?q { ?q x:a [] }", "q", "P18"), None);
    }

    #[tokio::test]
    async fn form_empty_bot_answer_and_bad_query() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/related_properties/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"q\""));
        let bot = get(dir.path(), "/related_properties.php?botmode=1&callback=cb").await;
        assert_eq!(bot.status.as_u16(), 200);
        assert!(bot.body.starts_with("cb("));
        assert_eq!(get(dir.path(), "/related_properties/?q=claim%5Bx%5D").await.status.as_u16(), 400);
    }
}
