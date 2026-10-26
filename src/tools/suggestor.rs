//! `suggestor.php`: statement suggestions for an article, as JSON(P): the
//! values that the items of the articles in its (small) categories share,
//! by property, with how many items have them; and their labels.

use crate::replicas::select_in;
use crate::sparql;
use crate::state::AppState;
use crate::terms;
use crate::web::{AppError, Params, api_json};
use crate::wiki::Wiki;
use axum::extract::State;
use axum::response::Response;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;

/// Larger categories say little about an article.
const MAX_CATEGORY_SIZE: u64 = 500;
/// Items sampled from the categories.
const MAX_ITEMS: usize = 1000;
/// A value counts if this share of the items has it…
const MIN_PERCENT: usize = 2;
/// …and at least this many.
const MIN_COUNT: usize = 5;
const DIRECT_CLAIM: &str = "http://www.wikidata.org/prop/direct/";

#[derive(Debug, Deserialize)]
pub struct Request {
    /// A database name, such as `enwiki`.
    #[serde(default)]
    wiki: String,
    #[serde(default)]
    article: String,
    callback: Option<String>,
}

#[derive(Debug, Serialize)]
struct Answer {
    /// Property → value → number of items.
    data: Map<String, Value>,
    labels: HashMap<String, String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let wiki = Wiki::from_dbname(&request.wiki)?;
    if request.article.trim().is_empty() {
        return Err(AppError::BadRequest("Missing parameter: article".to_string()));
    }
    let mut items = related_items(&state, &wiki, &request.article).await?;
    items.shuffle(&mut rand::rng());
    items.truncate(MAX_ITEMS);
    let cutoff = (items.len() * MIN_PERCENT / 100).max(MIN_COUNT);
    let counts = match items.is_empty() {
        true => vec![],
        false => value_counts(&state, &items, cutoff).await?,
    };
    let mut ids: Vec<String> = counts.iter().flat_map(|(p, v, _)| [p.clone(), v.clone()]).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut labels = terms::labels(&state.replicas, &ids, &["en", "mul"]).await?;
    for id in ids {
        labels.entry(id.clone()).or_insert(id);
    }
    Ok(api_json(Answer { data: group(counts), labels }, request.callback))
}

/// The items of the articles in the article's categories, but for large
/// categories and hidden ones (with `:` in their names, as before).
async fn related_items(state: &AppState, wiki: &Wiki, article: &str) -> anyhow::Result<Vec<String>> {
    let tables = ["page", "categorylinks", "linktarget", "category", "page_props"];
    let mut conn = state.replicas.conn(wiki, &tables).await?;
    let sql = "SELECT lt_id, lt_title FROM page
        JOIN categorylinks ON cl_from=page_id JOIN linktarget ON lt_id=cl_target_id
        JOIN category ON cat_title=lt_title
        WHERE page_namespace=0 AND page_title=? AND lt_namespace=14 AND cat_pages < cat_subcats + cat_files + ?";
    let categories: Vec<(u64, String)> =
        conn.exec(sql, (crate::replicas::db_title(article), MAX_CATEGORY_SIZE)).await?;
    let categories: Vec<u64> =
        categories.into_iter().filter(|(_, title)| !title.contains(':')).map(|(id, _)| id).collect();
    let sql = "SELECT DISTINCT pp_value FROM categorylinks
        JOIN page ON page_id=cl_from JOIN page_props ON pp_page=page_id
        WHERE page_namespace=0 AND page_is_redirect=0 AND pp_propname='wikibase_item' AND cl_target_id IN";
    let items: Vec<String> = select_in(&mut conn, sql, "", &[], &categories).await?;
    // They go into a SPARQL query.
    Ok(items.into_iter().filter(|q| crate::wikidata::parse_id(q, 'Q').as_ref() == Some(q)).collect())
}

/// `(property, value, items)` for item values that at least `cutoff` of `items` have.
async fn value_counts(state: &AppState, items: &[String], cutoff: usize) -> anyhow::Result<Vec<(String, String, u64)>> {
    let values: Vec<String> = items.iter().map(|q| format!("wd:{q}")).collect();
    let query = format!(
        "SELECT ?wdt ?value (COUNT(DISTINCT ?item) AS ?count) {{
            VALUES ?item {{ {} }}
            ?item ?wdt ?value .
            FILTER(STRSTARTS(STR(?wdt), \"{DIRECT_CLAIM}P\"))
            FILTER(STRSTARTS(STR(?value), \"http://www.wikidata.org/entity/Q\"))
        }} GROUP BY ?wdt ?value HAVING(COUNT(DISTINCT ?item) >= {cutoff})",
        values.join(" ")
    );
    let json = state.sparql.json(&query).await?;
    Ok(sparql::bindings(&json)
        .filter_map(|row| {
            let property = row["wdt"]["value"].as_str()?.strip_prefix(DIRECT_CLAIM)?;
            let value = sparql::entity_id(row["value"]["value"].as_str()?)?;
            let count = row["count"]["value"].as_str()?.parse().ok()?;
            Some((property.to_string(), value.to_string(), count))
        })
        .collect())
}

/// Counts by property, most common values first; properties with the most
/// common values first.
fn group(mut counts: Vec<(String, String, u64)>) -> Map<String, Value> {
    let top: HashMap<String, u64> = counts.iter().fold(HashMap::new(), |mut top, (p, _, n)| {
        let max = top.entry(p.clone()).or_default();
        *max = (*max).max(*n);
        top
    });
    counts.sort_by(|a, b| {
        let top = |p: &String| top.get(p).copied().unwrap_or_default();
        top(&b.0).cmp(&top(&a.0)).then_with(|| a.0.cmp(&b.0)).then_with(|| b.2.cmp(&a.2)).then_with(|| a.1.cmp(&b.1))
    });
    let mut data = Map::new();
    for (property, value, count) in counts {
        if let Value::Object(values) = data.entry(property).or_insert_with(|| Value::Object(Map::new())) {
            values.insert(value, count.into());
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn grouped_by_property() {
        let row = |p: &str, v: &str, n| (p.to_string(), v.to_string(), n);
        let data = group(vec![row("P27", "Q1", 10), row("P31", "Q5", 90), row("P27", "Q2", 30)]);
        assert_eq!(serde_json::to_string(&data).unwrap(), r#"{"P31":{"Q5":90},"P27":{"Q2":30,"Q1":10}}"#);
    }

    #[tokio::test]
    async fn bad_input() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(get(dir.path(), "/suggestor.php?wiki=nonsense&article=X").await.status.as_u16(), 400);
        assert_eq!(get(dir.path(), "/suggestor.php?wiki=enwiki").await.status.as_u16(), 400);
    }
}
