//! `dupe_finder.php`: potential duplicate items. Items whose only sitelink
//! is to one site may duplicate items with the page title (without a
//! trailing “(…)”) as label or alias, but with no sitelink to that site.
//! Scans the site's pages in title order, a batch at a time, and streams
//! the results; a run stops after `limit` candidates, with a link to go on.

use crate::replicas::{linktarget_ids, select_in, titles_linking_to};
use crate::state::AppState;
use crate::terms::{self, TermType};
use crate::web::{AppError, Params, render, thousands};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use wikimisc::escape::html;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

const DEFAULT_LIMIT: usize = 5000;
const MAX_LIMIT: usize = 50_000;
/// Sitelinks read per batch.
const BATCH: usize = 5000;
/// Sitelinks read per run, at most, however few candidates they hold.
const MAX_SCANNED: usize = 500_000;
/// Disambiguation pages, categories, templates, lists: not duplicates.
const META_TYPES: &[&str] = &["Q4167410", "Q13406463", "Q4167836", "Q11266439"];
const TABLES: &[&str] = &["wb_items_per_site", "page", "pagelinks", "linktarget"];
/// Where the streamed results go in the page.
const RESULTS: &str = "<!-- results -->";

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_site")]
    site: String,
    #[serde(default)]
    limit: String,
    #[serde(default)]
    instance_of: String,
    /// The page title to continue after.
    #[serde(default)]
    from: String,
    run: Option<String>,
}

fn default_site() -> String {
    "enwiki".to_string()
}

#[derive(Template)]
#[template(path = "tools/dupe_finder.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    limit: usize,
    max_limit: usize,
    running: bool,
}

/// What to look for, parsed from the request.
#[derive(Debug, Clone, PartialEq)]
struct Search {
    site: String,
    instance_of: Vec<String>,
    limit: usize,
    from: String,
}

impl Search {
    fn parse(request: &Request) -> Result<Self, AppError> {
        let site = request.site.trim().to_lowercase();
        Wiki::from_dbname(&site)?;
        let limit = match request.limit.trim() {
            "" | "0" => DEFAULT_LIMIT,
            limit => limit.parse().map_err(|_| AppError::BadRequest("invalid limit".into()))?,
        };
        let instance_of =
            request.instance_of.split(',').filter_map(|q| crate::wikidata::parse_id(q, 'Q')).collect::<Vec<_>>();
        Ok(Self { site, instance_of, limit: limit.min(MAX_LIMIT), from: request.from.trim().replace('_', " ") })
    }

    /// The URL of the next run, after `title`.
    fn next_url(&self, title: &str) -> String {
        format!(
            "/dupe_finder/?site={}&limit={}&instance_of={}&from={}&run=1",
            self.site,
            self.limit,
            self.instance_of.join(","),
            encode_component(title)
        )
    }
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let search = Search::parse(&request)?;
    let page = Page {
        title: "Duplicate item finder",
        request: &request,
        limit: search.limit,
        max_limit: MAX_LIMIT,
        running: request.run.is_some(),
    };
    let html = render(&page)?.0;
    if request.run.is_none() {
        return Ok(axum::response::Html(html).into_response());
    }
    let (head, tail) = html.split_once(RESULTS).unwrap_or((&html, ""));
    let (head, tail) = (head.to_string(), tail.to_string());
    let (mut tx, rx) = mpsc::channel::<String>(4);
    tokio::spawn(async move {
        let _ = tx.send(head).await;
        if let Err(e) = run(&state, &search, &mut tx).await {
            tracing::warn!("dupe_finder: {e:#}");
            let _ = tx.send("<p class=\"error\">The search failed; please try again later.</p>".into()).await;
        }
        let _ = tx.send(tail).await;
    });
    let body = Body::from_stream(rx.map(|s| Ok::<_, std::convert::Infallible>(Bytes::from(s))));
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response())
}

/// A candidate: an item with a sitelink to the site only.
struct Candidate {
    item: u64,
    page: String,
    label: String,
}

async fn run(state: &AppState, search: &Search, out: &mut mpsc::Sender<String>) -> anyhow::Result<()> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), TABLES).await?;
    let meta = linktarget_ids(&mut conn, 0, META_TYPES).await?;
    let types: Vec<&str> = search.instance_of.iter().map(String::as_str).collect();
    let types = linktarget_ids(&mut conn, 0, &types).await?;
    if !search.instance_of.is_empty() && types.is_empty() {
        out.send("<p>No item is an instance of these types.</p>".into()).await?;
        return Ok(());
    }
    let (mut after, mut scanned, mut candidates, mut found) = (search.from.clone(), 0, 0, 0);
    let mut had_label = HashSet::new();
    out.send("<ol>".into()).await?;
    loop {
        let sql = "SELECT ips_item_id, ips_site_page FROM wb_items_per_site
                   WHERE ips_site_id=? AND ips_site_page>? ORDER BY ips_site_page LIMIT ?";
        let rows: Vec<(u64, String)> = conn.exec(sql, (&search.site, &after, BATCH)).await?;
        scanned += rows.len();
        let Some((_, last)) = rows.last() else { break };
        after = last.clone();
        let mut batch = unique_to_site(&mut conn, &search.site, rows, &meta, &types).await?;
        batch.retain(|c| had_label.insert(c.label.clone()));
        candidates += batch.len();
        let lines = duplicates(state, &mut conn, &search.site, &batch, &meta).await?;
        found += lines.len();
        if !lines.is_empty() {
            out.send(lines.concat()).await?;
        }
        if candidates >= search.limit || scanned >= MAX_SCANNED {
            out.send(format!(
                "</ol><p>{} candidates checked, {} with potential duplicates, up to “{}”. <a href=\"{}\">Go on</a></p>",
                thousands(candidates as u64),
                thousands(found as u64),
                html(&after),
                html(&search.next_url(&after))
            ))
            .await?;
            return Ok(());
        }
    }
    out.send(format!(
        "</ol><p>Done: {} candidates checked, {} with potential duplicates.</p>",
        thousands(candidates as u64),
        thousands(found as u64)
    ))
    .await?;
    Ok(())
}

/// Of the sitelinks `(item, page)` to `site`, those of items with no other
/// sitelink, not of a meta type and, if `types` are given, of one of them.
async fn unique_to_site(
    conn: &mut Conn,
    site: &str,
    rows: Vec<(u64, String)>,
    meta: &[u64],
    types: &[u64],
) -> anyhow::Result<Vec<Candidate>> {
    let rows: Vec<(u64, String)> = rows.into_iter().filter(|(_, page)| !page.contains(':')).collect();
    let items: Vec<u64> = rows.iter().map(|(q, _)| *q).collect();
    let sql = "SELECT DISTINCT ips_item_id FROM wb_items_per_site WHERE ips_site_id<>? AND ips_item_id IN";
    let elsewhere: HashSet<u64> = select_in(conn, sql, "", &[site.into()], &items).await?.into_iter().collect();
    let rows: Vec<(u64, String)> = rows.into_iter().filter(|(q, _)| !elsewhere.contains(q)).collect();
    let titles: Vec<String> = rows.iter().map(|(q, _)| format!("Q{q}")).collect();
    let excluded = titles_linking_to(conn, &titles, meta).await?;
    let kept = match types.is_empty() {
        true => None,
        false => Some(titles_linking_to(conn, &titles, types).await?),
    };
    Ok(rows
        .into_iter()
        .filter(|(q, _)| {
            let title = format!("Q{q}");
            !excluded.contains(&title) && kept.as_ref().is_none_or(|k| k.contains(&title))
        })
        .map(|(item, page)| Candidate { item, label: search_label(&page), page })
        .collect())
}

/// A page title without a trailing “ (…)”.
fn search_label(page: &str) -> String {
    let page = page.trim();
    match page.strip_suffix(')').and_then(|p| p.rfind(" (")) {
        Some(i) => page.get(..i).unwrap_or(page).trim_end().to_string(),
        None => page.to_string(),
    }
}

/// A list entry for each candidate with potential duplicates: other items
/// with its label as label or alias, not of a meta type, not linked to `site`.
async fn duplicates(
    state: &AppState,
    conn: &mut Conn,
    site: &str,
    candidates: &[Candidate],
    meta: &[u64],
) -> anyhow::Result<Vec<String>> {
    let labels: Vec<String> = candidates.iter().map(|c| c.label.clone()).collect();
    let types = [TermType::Label, TermType::Alias];
    let matches = terms::items_with_texts(&state.replicas, &labels, &types, None).await?;
    let mut by_label: HashMap<String, HashSet<String>> = HashMap::new();
    for (q, text) in matches {
        by_label.entry(text).or_default().insert(q);
    }
    let others: Vec<String> = by_label.values().flatten().cloned().collect::<HashSet<_>>().into_iter().collect();
    let mut excluded = titles_linking_to(conn, &others, meta).await?;
    let numbers: Vec<u64> = others.iter().filter_map(|q| q.get(1..)?.parse().ok()).collect();
    let sql = "SELECT DISTINCT ips_item_id FROM wb_items_per_site WHERE ips_site_id=? AND ips_item_id IN";
    let linked: Vec<u64> = select_in(conn, sql, "", &[site.into()], &numbers).await?;
    excluded.extend(linked.into_iter().map(|q| format!("Q{q}")));
    Ok(candidates
        .iter()
        .filter_map(|c| {
            let item = format!("Q{}", c.item);
            let mut dupes: Vec<&String> =
                by_label.get(&c.label)?.iter().filter(|q| **q != item && !excluded.contains(*q)).collect();
            if dupes.is_empty() {
                return None;
            }
            dupes.sort_by_key(|q| q.get(1..).and_then(|n| n.parse::<u64>().ok()));
            let links: Vec<String> = dupes.iter().map(|q| link(q)).collect();
            Some(format!("<li>{} (<i>{}</i>) may duplicate {}</li>\n", link(&item), html(&c.page), links.join(", ")))
        })
        .collect())
}

fn link(q: &str) -> String {
    format!("<a href=\"https://www.wikidata.org/wiki/{q}\">{q}</a>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn labels_and_search() {
        assert_eq!(search_label("Foo (band)"), "Foo");
        assert_eq!(search_label("Foo (a) b"), "Foo (a) b");
        assert_eq!(search_label("Foo"), "Foo");
        let request = Request {
            site: " DEwiki ".into(),
            limit: "".into(),
            instance_of: "Q5, x, 7".into(),
            from: "A b".into(),
            run: None,
        };
        let search = Search::parse(&request).unwrap();
        assert_eq!(search.site, "dewiki");
        assert_eq!(search.limit, DEFAULT_LIMIT);
        assert_eq!(search.instance_of, ["Q5", "Q7"]);
        assert_eq!(search.from, "A b");
        assert_eq!(search.next_url("A &"), "/dupe_finder/?site=dewiki&limit=5000&instance_of=Q5,Q7&from=A%20%26&run=1");
    }

    #[tokio::test]
    async fn form_and_bad_site() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/dupe_finder/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"site\"") && !form.body.contains(RESULTS));
        assert_eq!(get(dir.path(), "/dupe_finder/?site=evil&run=1").await.status.as_u16(), 400);
    }
}
