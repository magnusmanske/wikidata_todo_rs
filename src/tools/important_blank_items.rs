//! `important_blank_items.php`: items with many sitelinks but no statements,
//! page by page, from a list a job writes (`no_items.50K.tab` in the tool's
//! home: a header line, then `<item number>\t<sitelinks>`, most sitelinks first).

use crate::replicas::select_in;
use crate::state::AppState;
use crate::terms;
use crate::web::{AppError, Paging, Params, render, thousands};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

const FILE: &str = "no_items.50K.tab";
const MAX_SIZE: usize = 500;
/// "mul" holds the labels common to many languages.
const LANGUAGES: &[&str] = &["en", "mul", "de", "es", "fr", "it"];

#[derive(Debug, Deserialize)]
pub struct Request {
    start: Option<usize>,
    size: Option<usize>,
}

struct Row {
    number: usize,
    item: String,
    label: String,
    sitelinks: u64,
    /// Links to items the item has gained since the list was made.
    item_links: Option<u64>,
}

#[derive(Template)]
#[template(path = "tools/important_blank_items.html")]
struct Page {
    title: &'static str,
    rows: Vec<Row>,
    nav: Paging,
    total: String,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let size = request.size.unwrap_or(100).clamp(1, MAX_SIZE);
    // A missing file is an empty list: the job has not run yet.
    let text = tokio::fs::read_to_string(state.config.data_dir.join(FILE)).await.unwrap_or_default();
    let list = parse(&text);
    let total = list.len();
    let start = request.start.unwrap_or(0).min(total);
    let slice = list.get(start..).unwrap_or_default();
    let slice = slice.get(..size.min(slice.len())).unwrap_or_default();
    let rows = rows(&state, start, slice).await?;
    let nav = Paging::new(start, size, total, "");
    render(&Page { title: "Important blank items", rows, nav, total: thousands(total as u64) })
}

/// `(item number, sitelinks)` per line; the header and anything malformed are skipped.
fn parse(text: &str) -> Vec<(u64, u64)> {
    text.lines()
        .filter_map(|line| {
            let (item, sitelinks) = line.split_once('\t')?;
            Some((item.trim().parse().ok()?, sitelinks.trim().parse().ok()?))
        })
        .collect()
}

async fn rows(state: &AppState, start: usize, slice: &[(u64, u64)]) -> anyhow::Result<Vec<Row>> {
    if slice.is_empty() {
        return Ok(vec![]);
    }
    let ids: Vec<String> = slice.iter().map(|(q, _)| format!("Q{q}")).collect();
    let labels = terms::labels(&state.replicas, &ids, LANGUAGES).await?;
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page", "pagelinks", "linktarget"]).await?;
    let sql = "SELECT page_title, COUNT(*) FROM page JOIN pagelinks ON pl_from=page_id
        JOIN linktarget ON pl_target_id=lt_id WHERE lt_namespace=0 AND page_namespace=0 AND page_title IN";
    let links: HashMap<String, u64> =
        select_in(&mut conn, sql, "GROUP BY page_title", &[], &ids).await?.into_iter().collect();
    Ok(ids
        .into_iter()
        .zip(slice)
        .enumerate()
        .map(|(i, (item, (_, sitelinks)))| Row {
            number: start + i + 1,
            label: labels.get(&item).cloned().unwrap_or_else(|| item.clone()),
            item_links: links.get(&item).copied(),
            sitelinks: *sitelinks,
            item,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn parses_the_list() {
        assert_eq!(parse("item\tsitelinks\n25216209\t18\n\nbad\n42\t7\n"), [(25216209, 18), (42, 7)]);
    }

    #[tokio::test]
    async fn without_a_list() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/important_blank_items/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("not available"));
        assert_eq!(get(dir.path(), "/important_blank_items/?start=x").await.status.as_u16(), 400);
    }
}
