//! `linked_items.php`: the Wikidata items of the articles that a page, some
//! pages, or a piece of wikitext link to (redirects followed); or of the
//! pages in a PagePile. On screen, or as a new PagePile.

use crate::mediawiki;
use crate::pagepile;
use crate::replicas::{db_title, display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use anyhow::Context;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use futures::{StreamExt, TryStreamExt, stream};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

const MAX_PAGES: usize = 500;
const CONCURRENCY: usize = 4;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    project: String,
    page: String,
    pages: String,
    pagepile: String,
    wikitext: String,
    format: String,
    ignore_templates: Option<String>,
    doit: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            language: "en".to_string(),
            project: "wikipedia".to_string(),
            page: String::new(),
            pages: String::new(),
            pagepile: String::new(),
            wikitext: String::new(),
            format: "screen".to_string(),
            ignore_templates: None,
            doit: None,
        }
    }
}

#[derive(Default)]
struct Results {
    items: Vec<String>,
    /// `Q…|page` lines.
    pairs: Vec<String>,
    /// Input pages that do not exist.
    missing: Vec<String>,
    /// The new PagePile, `(id, url)`.
    pile: Option<(u64, String)>,
}

#[derive(Template)]
#[template(path = "tools/linked_items.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    max_pages: usize,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let results = match request.doit.is_some() {
        true => Some(results(&state, &request).await?),
        false => None,
    };
    render(&Page { title: "Linked items", request: &request, max_pages: MAX_PAGES, results })
}

async fn results(state: &AppState, request: &Request) -> Result<Results, AppError> {
    let (wiki, titles, missing) = match request.pagepile.trim() {
        "" => {
            let wiki = Wiki::new(&request.language, &request.project)?;
            let (titles, missing) = linked_titles(state, &wiki, request).await?;
            (wiki, titles, missing)
        }
        pile => {
            let id = pile.parse().map_err(|_| AppError::BadRequest("The PagePile ID must be a number.".into()))?;
            let pile = pagepile::get(&state.http, id).await?;
            let wiki = Wiki::from_dbname(&pile.wiki)?;
            (wiki, pile.pages.iter().map(|p| db_title(p)).collect(), vec![])
        }
    };
    let titles = follow_redirects(state, &wiki, titles).await?;
    let mut results = Results { missing, ..Default::default() };
    if wiki == Wiki::wikidata() {
        results.items =
            titles.into_iter().filter(|t| crate::wikidata::parse_id(t, 'Q').as_deref() == Some(t)).collect();
        results.pairs = results.items.clone();
    } else {
        let titles: Vec<String> = titles.iter().map(|t| display_title(t)).collect();
        let items = pagepile::items_for_titles(&state.replicas, wiki.dbname(), &titles).await?;
        let mut seen = HashSet::new();
        for title in titles {
            let Some(item) = items.get(&title) else { continue };
            results.pairs.push(format!("{item}|{title}"));
            if seen.insert(item.clone()) {
                results.items.push(item.clone());
            }
        }
    }
    if request.format == "pagepile" && !results.items.is_empty() {
        let id = pagepile::create(&state.http, "wikidatawiki", &results.items).await?;
        results.pile = Some((id, pagepile::url(id)));
    }
    Ok(results)
}

/// The existing articles linked from the input, as database titles; and the
/// input pages that do not exist.
async fn linked_titles(
    state: &AppState,
    wiki: &Wiki,
    request: &Request,
) -> Result<(Vec<String>, Vec<String>), AppError> {
    let strip = request.ignore_templates.is_some();
    let pages: Vec<String> = match request.page.trim() {
        "" => request.pages.lines().map(str::trim).filter(|p| !p.is_empty()).map(String::from).collect(),
        page => vec![page.to_string()],
    };
    if pages.len() > MAX_PAGES {
        return Err(AppError::BadRequest(format!("At most {MAX_PAGES} pages, please.")));
    }
    let mut links = BTreeSet::new();
    let mut missing = vec![];
    if pages.is_empty() {
        let text = request.wikitext.trim();
        if text.is_empty() {
            return Err(AppError::BadRequest("Please give a page, some pages, wikitext, or a PagePile.".into()));
        }
        let text = if strip { strip_templates(text) } else { text.to_string() };
        links.extend(
            parse_links(&state.http, wiki, &[("text", &text), ("contentmodel", "wikitext")]).await?.unwrap_or_default(),
        );
    } else {
        let http = &state.http;
        let found: Vec<(String, Option<Vec<String>>)> = stream::iter(pages)
            .map(|page| async move {
                let links = page_links(http, wiki, &page, strip).await?;
                Ok::<_, anyhow::Error>((page, links))
            })
            .buffered(CONCURRENCY)
            .try_collect()
            .await?;
        for (page, page_links) in found {
            match page_links {
                Some(page_links) => links.extend(page_links),
                None => missing.push(page),
            }
        }
    }
    Ok((links.into_iter().collect(), missing))
}

/// The links of a page; without those in templates, if `strip`. `None` if
/// there is no such page.
async fn page_links(
    http: &reqwest::Client,
    wiki: &Wiki,
    page: &str,
    strip: bool,
) -> anyhow::Result<Option<Vec<String>>> {
    if !strip {
        return parse_links(http, wiki, &[("page", page), ("redirects", "1")]).await;
    }
    match mediawiki::page_wikitext(http, wiki, page).await? {
        Some(text) => parse_links(http, wiki, &[("text", &strip_templates(&text)), ("contentmodel", "wikitext")]).await,
        None => Ok(None),
    }
}

/// The existing articles that the wiki's parser finds linked, as database
/// titles. `None` if the page to parse does not exist.
async fn parse_links(
    http: &reqwest::Client,
    wiki: &Wiki,
    input: &[(&str, &str)],
) -> anyhow::Result<Option<Vec<String>>> {
    let mut params = vec![("action", "parse"), ("prop", "links"), ("format", "json"), ("formatversion", "2")];
    params.extend_from_slice(input);
    // Wikitext can be long: POST.
    let json: Value = http
        .post(wiki.api_url())
        .form(&params)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("cannot parse on {wiki}"))?
        .json()
        .await
        .with_context(|| format!("API response from {wiki} is not JSON"))?;
    if json["error"]["code"] == "missingtitle" {
        return Ok(None);
    }
    let links =
        json["parse"]["links"].as_array().with_context(|| format!("{wiki} did not parse: {}", json["error"]))?;
    Ok(Some(
        links
            .iter()
            .filter(|l| l["ns"] == 0 && l["exists"] == true)
            .filter_map(|l| l["title"].as_str().map(db_title))
            .collect(),
    ))
}

/// Wikitext without its templates, nested ones included; an unclosed
/// template stays as it is.
fn strip_templates(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // Where in `out` each open template began.
    let mut open: Vec<usize> = vec![];
    let mut rest = text;
    loop {
        if let Some(after) = rest.strip_prefix("{{") {
            open.push(out.len());
            out.push_str("{{");
            rest = after;
        } else if let Some(after) = rest.strip_prefix("}}")
            && let Some(start) = open.pop()
        {
            out.truncate(start);
            rest = after;
        } else {
            let mut chars = rest.chars();
            let Some(c) = chars.next() else { break };
            out.push(c);
            rest = chars.as_str();
        }
    }
    out
}

/// `titles` (database form, namespace 0), redirects replaced by their targets.
async fn follow_redirects(state: &AppState, wiki: &Wiki, titles: Vec<String>) -> anyhow::Result<Vec<String>> {
    let mut conn = state.replicas.conn(wiki, &["page", "redirect"]).await?;
    let sql = "SELECT page_title, rd_title FROM page JOIN redirect ON rd_from=page_id
        WHERE page_namespace=0 AND rd_namespace=0 AND page_title IN";
    let targets: HashMap<String, String> = select_in(&mut conn, sql, "", &[], &titles).await?.into_iter().collect();
    let mut seen = HashSet::new();
    Ok(titles.into_iter().map(|t| targets.get(&t).cloned().unwrap_or(t)).filter(|t| seen.insert(t.clone())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn templates_stripped() {
        assert_eq!(strip_templates("a {{b|[[c]] {{d}} }} [[e]] {{f}}"), "a  [[e]] ");
        assert_eq!(strip_templates("x }} {{y"), "x }} {{y");
        assert_eq!(strip_templates("ä{{ö}}ü"), "äü");
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/linked_items/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("name=\"wikitext\""));
        assert_eq!(get(dir.path(), "/linked_items/?doit=1").await.status.as_u16(), 400);
        assert_eq!(get(dir.path(), "/linked_items/?doit=1&pagepile=x").await.status.as_u16(), 400);
        assert_eq!(get(dir.path(), "/linked_items/?doit=1&page=X&project=evil").await.status.as_u16(), 400);
    }
}
