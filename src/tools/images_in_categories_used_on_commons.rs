//! `images_in_categories_used_on_commons.php`: for each Commons category in
//! a tree, how many of its files there are, and how many of them Wikidata
//! items use; or just the categories none of whose files are used.

use crate::replicas::{display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render, thousands};
use crate::wiki::{Wiki, encode_title};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

const MAX_DEPTH: u32 = 10;
/// Categories of details and views of a subject are left out.
const IGNORED: &[&str] = &["detail", "view"];

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    category: String,
    depth: Option<u32>,
    zero_only: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self { category: String::new(), depth: Some(5), zero_only: None }
    }
}

struct Row {
    name: String,
    url: String,
    used: String,
    total: String,
}

#[derive(Template)]
#[template(path = "tools/images_in_categories_used_on_commons.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    depth: u32,
    zero_only: bool,
    rows: Option<Vec<Row>>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let depth = request.depth.unwrap_or(5).min(MAX_DEPTH);
    let zero_only = request.zero_only.as_deref().is_some_and(|z| z != "0");
    let rows = match request.category.trim() {
        "" => None,
        category => Some(rows(&state, category, depth, zero_only).await?),
    };
    render(&Page { title: "Images in categories used on Wikidata", request: &request, depth, zero_only, rows })
}

async fn rows(state: &AppState, category: &str, depth: u32, zero_only: bool) -> anyhow::Result<Vec<Row>> {
    let commons = Wiki::commons();
    let category = crate::mediawiki::strip_namespace(category, &["category"]).unwrap_or(category);
    let tree = state.replicas.category_tree(&commons, category, depth).await?;
    let tree: Vec<String> = tree.into_iter().filter(|c| !IGNORED.iter().any(|i| c.contains(i))).collect();
    let mut conn = state.replicas.conn(&commons, &["page", "categorylinks", "linktarget", "globalimagelinks"]).await?;
    let sql = "SELECT lt_id, lt_title FROM linktarget WHERE lt_namespace=14 AND lt_title IN";
    let targets: HashMap<u64, String> = select_in(&mut conn, sql, "", &[], &tree).await?.into_iter().collect();
    let ids: Vec<u64> = targets.keys().copied().collect();
    let sql = "SELECT cl_target_id, COUNT(*) FROM categorylinks WHERE cl_type='file' AND cl_target_id IN";
    let totals: Vec<(u64, u64)> = select_in(&mut conn, sql, "GROUP BY cl_target_id", &[], &ids).await?;
    let with_files: Vec<u64> = totals.iter().map(|(id, _)| *id).collect();
    let sql = "SELECT cl_target_id, COUNT(*) FROM categorylinks JOIN page ON page_id=cl_from
        WHERE cl_type='file' AND EXISTS (SELECT 1 FROM globalimagelinks
            WHERE gil_to=page_title AND gil_wiki='wikidatawiki' AND gil_page_namespace_id=0)
        AND cl_target_id IN";
    let used: HashMap<u64, u64> =
        select_in(&mut conn, sql, "GROUP BY cl_target_id", &[], &with_files).await?.into_iter().collect();
    let mut counts: Vec<(&str, u64, u64)> = totals
        .iter()
        .filter_map(|(id, total)| Some((targets.get(id)?.as_str(), used.get(id).copied().unwrap_or(0), *total)))
        .filter(|(_, used, _)| !zero_only || *used == 0)
        .collect();
    counts.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(b.0)));
    Ok(counts
        .into_iter()
        .map(|(name, used, total)| Row {
            url: format!("https://commons.wikimedia.org/wiki/Category:{}", encode_title(name)),
            name: display_title(name),
            used: thousands(used),
            total: thousands(total),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/images_in_categories_used_on_commons/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("value=\"5\""));
        let bad = get(dir.path(), "/images_in_categories_used_on_commons/?category=X&depth=deep").await;
        assert_eq!(bad.status.as_u16(), 400);
    }
}
