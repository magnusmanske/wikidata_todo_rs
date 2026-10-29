//! PagePile: lists of pages, kept by <https://pagepile.toolforge.org/>.

use crate::replicas::{Replicas, select_in};
use crate::wiki::Wiki;
use anyhow::{Context, Result};
use pagepile::client::{Client, DEFAULT_API, NewPage, Page, Pile, Query};
use std::collections::HashMap;

fn client(http: &reqwest::Client) -> Client {
    Client::with_http(http.clone(), DEFAULT_API)
}

/// A pile; at most its first `limit` pages, as piles can be huge.
pub async fn get(http: &reqwest::Client, id: u64, limit: usize) -> Result<Pile> {
    let query = Query { limit: Some(limit as u64), ..Default::default() };
    let mut pile = client(http).pile_with(id, &query).await.with_context(|| format!("PagePile #{id}"))?;
    // In case PagePile ignores the limit.
    pile.pages.truncate(limit);
    Ok(pile)
}

/// The Wikidata items of a pile: its items for a Wikidata pile, else the
/// items of its namespace-0 pages (pages without an item are left out); of
/// at most its first `limit` pages.
pub async fn items(http: &reqwest::Client, replicas: &Replicas, id: u64, limit: usize) -> Result<Vec<String>> {
    let pile = get(http, id, limit).await?;
    if pile.wiki == pagepile::wiki::Wiki::WIKIDATA {
        return Ok(pile.items().map(str::to_string).collect());
    }
    let titles: Vec<String> = pile.pages.iter().filter(|p| p.namespace == 0).map(Page::text).collect();
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
    let pages: Vec<NewPage> = pages.iter().map(NewPage::title).collect();
    client(http).create(wiki, &pages).await.context("PagePile made no pile")
}

/// Where a pile can be seen.
pub fn url(http: &reqwest::Client, id: u64) -> String {
    client(http).view_url(id)
}
