//! `best_cats.php`: the categories most used by a wiki's articles whose
//! Wikidata items have neither "instance of" (P31) nor "subclass of" (P279);
//! where to start adding those. Large wikis are sampled.

use crate::replicas::{display_title, linktarget_ids, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render, thousands};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use futures::{StreamExt, TryStreamExt, stream};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;

/// Articles looked at, at most: in windows of consecutive page IDs at random places.
const SAMPLE: usize = 100_000;
const WINDOW: usize = 5_000;
/// Items per query on Wikidata, and queries at a time.
const CHUNK: usize = 5_000;
const CONCURRENCY: usize = 4;
/// Categories looked up, to find the shown ones among them.
const CANDIDATES: usize = 1000;
const SHOWN: usize = 100;
const NS_PROPERTY: i64 = 120;

#[derive(Debug, Deserialize)]
pub struct Request {
    wiki: Option<String>,
}

struct Results {
    wiki: Wiki,
    sampled: bool,
    articles: String,
    lacking: String,
    categories: Vec<(String, String)>,
}

#[derive(Template)]
#[template(path = "tools/best_cats.html")]
struct Page {
    title: &'static str,
    wiki: String,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mut page = Page { title: "Best categories", wiki: "dewiki".to_string(), results: None };
    let Some(dbname) = request.wiki.map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()) else {
        return render(&page);
    };
    let wiki = Wiki::from_dbname(&dbname)?;
    page.wiki = dbname;
    let (articles, sampled) = articles(&state, &wiki).await?;
    let items: Vec<String> = articles.keys().cloned().collect();
    let lacking = lacking(&state, &items).await?;
    let pages: Vec<u64> = articles.iter().filter(|(item, _)| lacking.contains(*item)).map(|(_, id)| *id).collect();
    let categories = categories(&state, &wiki, &pages).await?;
    page.results = Some(Results {
        wiki,
        sampled,
        articles: thousands(articles.len() as u64),
        lacking: thousands(pages.len() as u64),
        categories: categories.into_iter().map(|(title, n)| (title, thousands(n))).collect(),
    });
    render(&page)
}

/// Articles (page IDs) by item: all, or a sample of large wikis.
async fn articles(state: &AppState, wiki: &Wiki) -> anyhow::Result<(HashMap<String, u64>, bool)> {
    let mut conn = state.replicas.conn(wiki, &["page", "page_props"]).await?;
    let count: Option<u64> =
        conn.query_first("SELECT COUNT(*) FROM page_props WHERE pp_propname='wikibase_item'").await?;
    let sql = "SELECT pp_value, pp_page FROM page_props JOIN page ON page_id=pp_page
        WHERE pp_propname='wikibase_item' AND page_namespace=0";
    if count.unwrap_or_default() <= SAMPLE as u64 {
        let rows: Vec<(String, u64)> = conn.query(sql).await?;
        return Ok((rows.into_iter().collect(), false));
    }
    let max: Option<u64> = conn.query_first("SELECT MAX(page_id) FROM page").await?;
    let mut articles = HashMap::new();
    for _ in 0..SAMPLE / WINDOW {
        let start = rand::random_range(0..max.unwrap_or(1).max(1));
        let window = format!("{sql} AND pp_page>=? ORDER BY pp_page LIMIT {WINDOW}");
        let rows: Vec<(String, u64)> = conn.exec(window, (start,)).await?;
        articles.extend(rows);
    }
    Ok((articles, true))
}

/// Those of the items that have neither P31 nor P279.
async fn lacking(state: &AppState, items: &[String]) -> anyhow::Result<HashSet<String>> {
    let wikidata = Wiki::wikidata();
    let tables = ["page", "pagelinks", "linktarget"];
    let mut conn = state.replicas.conn(&wikidata, &tables).await?;
    let targets = linktarget_ids(&mut conn, NS_PROPERTY, &["P31", "P279"]).await?;
    drop(conn);
    let sql = format!(
        "SELECT page_title FROM page WHERE page_namespace=0
         AND NOT EXISTS (SELECT 1 FROM pagelinks WHERE pl_from=page_id AND pl_target_id IN ({}))
         AND page_title IN",
        crate::replicas::placeholders(targets.len())
    );
    let params: Vec<_> = targets.into_iter().map(Into::into).collect();
    let (sql, params, wikidata, tables) = (&sql, &params, &wikidata, &tables);
    let chunks: Vec<Vec<String>> = items.chunks(CHUNK).map(<[String]>::to_vec).collect();
    let found: Vec<Vec<String>> = stream::iter(chunks)
        .map(|chunk| async move {
            let mut conn = state.replicas.conn(wikidata, tables).await?;
            select_in(&mut conn, sql, "", params, &chunk).await
        })
        .buffer_unordered(CONCURRENCY)
        .try_collect()
        .await?;
    Ok(found.into_iter().flatten().collect())
}

/// The categories of the pages, by how many of them are in each, most first;
/// without hidden and maintenance categories.
async fn categories(state: &AppState, wiki: &Wiki, pages: &[u64]) -> anyhow::Result<Vec<(String, u64)>> {
    let mut conn = state.replicas.conn(wiki, &["categorylinks", "linktarget", "page", "page_props"]).await?;
    let sql = "SELECT cl_target_id, COUNT(*) FROM categorylinks WHERE cl_from IN";
    let rows: Vec<(u64, u64)> = select_in(&mut conn, sql, "GROUP BY cl_target_id", &[], pages).await?;
    let mut counts: HashMap<u64, u64> = HashMap::new();
    for (target, n) in rows {
        *counts.entry(target).or_default() += n;
    }
    let mut counts: Vec<(u64, u64)> = counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    counts.truncate(CANDIDATES);
    let ids: Vec<u64> = counts.iter().map(|(id, _)| *id).collect();
    let sql = "SELECT lt_id, lt_title FROM linktarget WHERE lt_namespace=14 AND lt_id IN";
    let titles: HashMap<u64, String> = select_in(&mut conn, sql, "", &[], &ids).await?.into_iter().collect();
    let names: Vec<&String> = titles.values().collect();
    let sql = "SELECT page_title FROM page JOIN page_props ON pp_page=page_id
        WHERE pp_propname='hiddencat' AND page_namespace=14 AND page_title IN";
    let hidden: HashSet<String> = select_in(&mut conn, sql, "", &[], &names).await?.into_iter().collect();
    Ok(counts
        .into_iter()
        .filter_map(|(id, n)| Some((titles.get(&id)?.clone(), n)))
        .filter(|(title, _)| !hidden.contains(title) && !is_maintenance(title))
        .take(SHOWN)
        .map(|(title, n)| (display_title(&title), n))
        .collect())
}

/// Maintenance categories that are not hidden, by name (as the PHP tool had it).
fn is_maintenance(title: &str) -> bool {
    ["rticles", "dmy", "Commons", "Coordinates", "Wikipedia"].iter().any(|s| title.contains(s))
        || ["Pages", "All"].iter().any(|s| title.starts_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn maintenance() {
        assert!(is_maintenance("Articles_with_short_description"));
        assert!(is_maintenance("Pages_using_x"));
        assert!(!is_maintenance("Mann"));
    }

    #[tokio::test]
    async fn form_and_unknown_wiki() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/best_cats/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("value=\"dewiki\""));
        assert_eq!(get(dir.path(), "/best_cats/?wiki=nonsense").await.status.as_u16(), 400);
    }
}
