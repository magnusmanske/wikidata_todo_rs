//! PagePile: lists of pages, kept by <https://pagepile.toolforge.org/>.

use crate::replicas::{Replicas, select_in};
use crate::wiki::Wiki;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;

const API: &str = "https://pagepile.toolforge.org/api.php";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pile {
    pub id: u64,
    /// The database name, e.g. `enwiki`.
    pub wiki: String,
    /// Titles with namespace prefix, spaces as underscores or spaces, as PagePile gives them.
    pub pages: Vec<String>,
}

#[derive(Deserialize)]
struct Data {
    #[serde(default)]
    pages: Vec<String>,
    #[serde(default)]
    wiki: String,
    #[serde(default)]
    status: Option<String>,
}

pub async fn get(http: &reqwest::Client, id: u64) -> Result<Pile> {
    let id_s = id.to_string();
    let data: Data = http
        .get(API)
        .query(&[("id", id_s.as_str()), ("action", "get_data"), ("format", "json"), ("doit", "1")])
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .context("PagePile did not answer")?
        .json()
        .await
        .context("PagePile's answer is not JSON")?;
    if data.wiki.is_empty() {
        bail!("PagePile #{id}: {}", data.status.unwrap_or_else(|| "no such pile".to_string()));
    }
    Ok(Pile { id, wiki: data.wiki, pages: data.pages })
}

/// The Wikidata items of a pile: its `Q…` pages for a Wikidata pile, else the
/// items of its namespace-0 pages (pages without an item are left out).
pub async fn items(http: &reqwest::Client, replicas: &Replicas, id: u64) -> Result<Vec<String>> {
    let pile = get(http, id).await?;
    if pile.wiki == "wikidatawiki" {
        return Ok(pile.pages.into_iter().filter(|p| p.starts_with('Q')).collect());
    }
    let titles: Vec<String> = pile.pages.iter().filter(|p| !p.contains(':')).map(|p| p.replace('_', " ")).collect();
    let by_title = items_for_titles(replicas, &pile.wiki, &titles).await?;
    Ok(titles.iter().filter_map(|t| by_title.get(t).cloned()).collect())
}

/// The items of pages (titles with spaces) on `wiki` (a database name), by title.
pub async fn items_for_titles(replicas: &Replicas, wiki: &str, titles: &[String]) -> Result<HashMap<String, String>> {
    let mut conn = replicas.conn(&Wiki::wikidata(), &["wb_items_per_site"]).await?;
    let sql = "SELECT ips_site_page, ips_item_id FROM wb_items_per_site WHERE ips_site_id=? AND ips_site_page IN";
    let rows: Vec<(String, u64)> = select_in(&mut conn, sql, "", &[wiki.into()], titles).await?;
    Ok(rows.into_iter().map(|(title, item)| (title, format!("Q{item}"))).collect())
}

/// Makes a new pile of `pages` (titles; for Wikidata, item IDs) on `wiki`; returns its ID.
pub async fn create(http: &reqwest::Client, wiki: &str, pages: &[String]) -> Result<u64> {
    let data = pages.join("\n");
    let json: serde_json::Value = http
        .post(API)
        .form(&[("action", "create_pile_with_data"), ("wiki", wiki), ("data", data.as_str())])
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .context("PagePile did not answer")?
        .json()
        .await
        .context("PagePile's answer is not JSON")?;
    json["pile"]["id"].as_u64().with_context(|| format!("PagePile made no pile: {}", json["status"]))
}

/// Where a pile can be seen.
pub fn url(id: u64) -> String {
    format!("{API}?id={id}&action=get_data&format=html&doit1")
}
