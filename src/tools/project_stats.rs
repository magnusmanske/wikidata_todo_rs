//! `project_stats.php`: how many items link to each project, and how many
//! have a label in each language. Both take the database minutes, so they
//! are computed in the background and kept for a day.
//!
//! Counting the labels of all items would take the term store hours, so
//! that count is estimated from a random sample of item IDs.

use crate::cache::TimedCache;
use crate::replicas::select_in;
use crate::state::AppState;
use crate::terms;
use crate::web::{AppError, Params, render, thousands};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;

const KEEP_FOR: Duration = Duration::from_secs(24 * 3600);
/// How long a request waits for the numbers before it says to come back.
const WAIT: Duration = Duration::from_secs(20);
/// Random item IDs whose labels are counted.
const SAMPLE: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Langlinks,
    Labels,
}

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    mode: String,
}

/// Counts per project or language, most first.
struct Stats {
    items: u64,
    counts: Vec<(String, u64)>,
    /// When the numbers were computed.
    at: String,
}

struct Row {
    key: String,
    count: String,
    with: String,
    without: String,
}

#[derive(Template)]
#[template(path = "tools/project_stats.html")]
struct Page {
    title: &'static str,
    mode: Mode,
    items: Option<String>,
    rows: Vec<Row>,
    at: String,
    sample: String,
}

static LANGLINKS: TimedCache<Stats> = TimedCache::new(KEEP_FOR);
static LABELS: TimedCache<Stats> = TimedCache::new(KEEP_FOR);

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mode = match request.mode.as_str() {
        "labels" => Mode::Labels,
        _ => Mode::Langlinks,
    };
    // In a task of its own, so the numbers are done even if nobody waits for them.
    let task = tokio::spawn(async move {
        match mode {
            Mode::Langlinks => LANGLINKS.get_or_fetch(async || langlinks(&state).await).await,
            Mode::Labels => LABELS.get_or_fetch(async || labels(&state).await).await,
        }
    });
    let stats = match tokio::time::timeout(WAIT, task).await {
        Ok(joined) => Some(joined.map_err(|e| AppError::Internal(e.to_string()))??),
        Err(_) => None,
    };
    let page = Page {
        title: "Project stats",
        mode,
        items: stats.as_ref().map(|s| thousands(s.items)),
        rows: stats.as_ref().map(|s| rows(s)).unwrap_or_default(),
        at: stats.as_ref().map(|s| s.at.clone()).unwrap_or_default(),
        sample: thousands(SAMPLE as u64),
    };
    render(&page)
}

fn rows(stats: &Stats) -> Vec<Row> {
    let total = stats.items.max(1) as f64;
    stats
        .counts
        .iter()
        .map(|(key, count)| {
            let share = (*count as f64 * 100.0 / total).min(100.0);
            Row {
                key: key.clone(),
                count: thousands(*count),
                with: format!("{share:.1}"),
                without: format!("{:.1}", 100.0 - share),
            }
        })
        .collect()
}

/// Items, that is, Wikidata pages in the main namespace that are not redirects.
async fn item_count(state: &AppState) -> anyhow::Result<u64> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page"]).await?;
    let n: Option<u64> =
        conn.query_first("SELECT COUNT(*) FROM page WHERE page_namespace=0 AND page_is_redirect=0").await?;
    Ok(n.unwrap_or_default())
}

async fn langlinks(state: &AppState) -> anyhow::Result<Stats> {
    let items = item_count(state).await?;
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["wb_items_per_site"]).await?;
    let sql = "SELECT ips_site_id, COUNT(*) AS n FROM wb_items_per_site GROUP BY ips_site_id ORDER BY n DESC";
    let counts: Vec<(String, u64)> = conn.query(sql).await?;
    Ok(Stats { items, counts, at: now() })
}

/// The number of items with a label in each language, estimated from the
/// labels of [`SAMPLE`] random item IDs (of which some are deleted or redirects).
async fn labels(state: &AppState) -> anyhow::Result<Stats> {
    let items = item_count(state).await?;
    let mut conn = terms::conn(&state.replicas).await?;
    let max: Option<u64> = conn.query_first("SELECT MAX(wbit_item_id) FROM wbt_item_terms").await?;
    let max = max.unwrap_or(1).max(1);
    let sample: Vec<u64> =
        (0..SAMPLE).map(|_| rand::random_range(1..=max)).collect::<HashSet<_>>().into_iter().collect();
    let sql = "SELECT wbxl_language, COUNT(*) FROM wbt_item_terms
        JOIN wbt_term_in_lang ON wbit_term_in_lang_id=wbtl_id
        JOIN wbt_text_in_lang ON wbtl_text_in_lang_id=wbxl_id
        WHERE wbtl_type_id=1 AND wbit_item_id IN";
    let rows: Vec<(Vec<u8>, u64)> = select_in(&mut conn, sql, "GROUP BY wbxl_language", &[], &sample).await?;
    let mut hits: HashMap<String, u64> = HashMap::new();
    for (language, n) in rows {
        *hits.entry(String::from_utf8_lossy(&language).into_owned()).or_default() += n;
    }
    Ok(Stats { items, counts: scaled(hits, sample.len(), max), at: now() })
}

/// Sample counts scaled up to `population`, to the nearest thousand (more
/// would be false precision), most first.
fn scaled(hits: HashMap<String, u64>, sample: usize, population: u64) -> Vec<(String, u64)> {
    let factor = population as f64 / sample.max(1) as f64;
    let mut counts: Vec<(String, u64)> =
        hits.into_iter().map(|(k, n)| (k, (n as f64 * factor / 1000.0).round() as u64 * 1000)).collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts
}

fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_and_rows() {
        let hits = HashMap::from([("de".to_string(), 1), ("en".to_string(), 3)]);
        assert_eq!(scaled(hits, 10, 1_000_100), [("en".to_string(), 300_000), ("de".to_string(), 100_000)]);
        let stats = Stats { items: 1000, counts: vec![("enwiki".into(), 1234)], at: String::new() };
        let row = &rows(&stats)[0];
        assert_eq!((row.count.as_str(), row.with.as_str(), row.without.as_str()), ("1,234", "100.0", "0.0"));
    }
}
