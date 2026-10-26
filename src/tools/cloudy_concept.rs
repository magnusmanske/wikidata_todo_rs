//! `cloudy_concept.php`: the concept cloud of an item. The pages its
//! Wikipedia articles link to, by item, and which of these items lack a
//! label in a language; with a way to add one (via Widar).

use crate::mediawiki;
use crate::state::AppState;
use crate::terms::{self, TermType};
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, is_language_code};
use askama::Template;
use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::Html;
use futures::{StreamExt, stream};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;

/// Wikis asked at a time.
const CONCURRENCY: usize = 6;
/// API requests per wiki, of 500 links each.
const MAX_REQUESTS: usize = 10;
/// A label for all languages; an item with one has a label in every language.
const MUL: &str = "mul";

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    q: String,
    lang: Option<String>,
    #[serde(default)]
    fallback: String,
}

/// An item linked to from the articles.
struct Row {
    item: String,
    /// The label in the first language that has one, and that language
    /// unless it is the main one.
    label: Option<(String, Option<String>)>,
    /// Titles of the linking pages, most frequent first.
    titles: Vec<String>,
    wikis: usize,
}

struct Cloud {
    item: String,
    label: Option<String>,
    wikis: usize,
    /// Wikis that did not answer.
    failed: usize,
    rows: Vec<Row>,
}

#[derive(Template)]
#[template(path = "tools/cloudy_concept.html")]
struct Page {
    title: &'static str,
    q: String,
    lang: String,
    fallback: String,
    cloud: Option<Cloud>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let lang = match request.lang.as_deref().map(str::trim) {
        Some(lang) if !lang.is_empty() => lang.to_lowercase(),
        _ => browser_language(&headers),
    };
    if !is_language_code(&lang) {
        return Err(AppError::BadRequest("invalid language code".into()));
    }
    let fallbacks = fallbacks(&request.fallback, &lang);
    let q = crate::wikidata::parse_id(&request.q, 'Q');
    let mut page = Page {
        title: "Concept cloud",
        q: q.clone().unwrap_or_else(|| request.q.trim().to_string()),
        lang,
        fallback: fallbacks.join(","),
        cloud: None,
    };
    if !request.q.trim().is_empty() {
        let q = q.ok_or_else(|| AppError::BadRequest("invalid item".into()))?;
        let mut languages: Vec<&str> = vec![&page.lang];
        languages.extend(fallbacks.iter().map(String::as_str));
        languages.push(MUL);
        page.cloud = Some(cloud(&state, &q, &languages).await?);
    }
    render(&page)
}

/// The first language the browser asks for, or English.
fn browser_language(headers: &HeaderMap) -> String {
    let accept = headers.get(header::ACCEPT_LANGUAGE).and_then(|v| v.to_str().ok()).unwrap_or_default();
    let first = accept.split([',', ';', '-']).next().unwrap_or_default().trim().to_lowercase();
    if is_language_code(&first) { first } else { "en".to_string() }
}

/// Valid codes from a comma-separated list, each once, without `lang`.
fn fallbacks(list: &str, lang: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for code in list.split(',').map(|l| l.trim().to_lowercase()) {
        if is_language_code(&code) && code != lang && !out.contains(&code) {
            out.push(code);
        }
    }
    out
}

async fn cloud(state: &AppState, q: &str, languages: &[&str]) -> Result<Cloud, AppError> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["wb_items_per_site"]).await?;
    let number = q.get(1..).and_then(|n| n.parse::<u64>().ok()).unwrap_or_default();
    let sitelinks: Vec<(String, String)> = conn
        .exec("SELECT ips_site_id, ips_site_page FROM wb_items_per_site WHERE ips_item_id=?", (number,))
        .await
        .map_err(anyhow::Error::from)?;
    drop(conn);
    let articles: Vec<(Wiki, String)> = sitelinks
        .into_iter()
        .filter_map(|(site, title)| {
            let wiki = Wiki::from_dbname(&site).ok().filter(|w| w.host().ends_with(".wikipedia.org"))?;
            Some((wiki, title))
        })
        .collect();
    let wikis = articles.len();
    let http = &state.http;
    let results: Vec<anyhow::Result<Vec<(String, String)>>> = stream::iter(articles)
        .map(|(wiki, title)| async move {
            let links = linked_items(http, &wiki, &title).await;
            if let Err(e) = &links {
                tracing::warn!("cloudy_concept: links of {title} on {wiki}: {e:#}");
            }
            links
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let (links, failed): (Vec<_>, Vec<_>) = results.into_iter().partition(Result::is_ok);
    let mut rows = tally(links.into_iter().flatten().collect());
    let mut ids: Vec<String> = rows.iter().map(|r| r.item.clone()).collect();
    ids.push(q.to_string());
    let labels = best_labels(state, &ids, languages).await?;
    for row in &mut rows {
        row.label = labels.get(&row.item).map(|(language, label)| {
            (label.clone(), (*language != languages.first().copied().unwrap_or_default()).then(|| language.clone()))
        });
    }
    let label = labels.get(q).filter(|(l, _)| Some(l.as_str()) == languages.first().copied()).map(|(_, t)| t.clone());
    Ok(Cloud { item: q.to_string(), label, wikis, failed: failed.len(), rows })
}

/// The items (with title) of the existing articles a page links to.
async fn linked_items(http: &reqwest::Client, wiki: &Wiki, title: &str) -> anyhow::Result<Vec<(String, String)>> {
    let mut items = vec![];
    let mut cont: Vec<(String, String)> = vec![];
    for _ in 0..MAX_REQUESTS {
        let mut params = vec![
            ("action", "query"),
            ("titles", title),
            ("generator", "links"),
            ("gplnamespace", "0"),
            ("gpllimit", "max"),
            ("prop", "pageprops"),
            ("ppprop", "wikibase_item"),
            ("redirects", "1"),
        ];
        params.extend(cont.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let json = mediawiki::api(http, wiki, &params).await?;
        for page in json["query"]["pages"].as_array().into_iter().flatten() {
            if let (Some(q), Some(title)) = (page["pageprops"]["wikibase_item"].as_str(), page["title"].as_str()) {
                items.push((q.to_string(), title.to_string()));
            }
        }
        cont = json["continue"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect();
        if cont.is_empty() {
            break;
        }
    }
    Ok(items)
}

/// Per item, on how many wikis it is linked to, and as which titles;
/// most linked first.
fn tally(links: Vec<Vec<(String, String)>>) -> Vec<Row> {
    let mut by_item: HashMap<String, (usize, HashMap<String, usize>)> = HashMap::new();
    for wiki in links {
        let mut seen = std::collections::HashSet::new();
        for (item, title) in wiki {
            let (wikis, titles) = by_item.entry(item.clone()).or_default();
            if seen.insert(item) {
                *wikis += 1;
            }
            *titles.entry(title).or_default() += 1;
        }
    }
    let mut rows: Vec<Row> = by_item
        .into_iter()
        .map(|(item, (wikis, titles))| {
            let mut titles: Vec<(String, usize)> = titles.into_iter().collect();
            titles.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            Row { item, label: None, titles: titles.into_iter().map(|(t, _)| t).collect(), wikis }
        })
        .collect();
    let number = |q: &str| q.get(1..).and_then(|n| n.parse::<u64>().ok()).unwrap_or_default();
    rows.sort_by(|a, b| b.wikis.cmp(&a.wikis).then_with(|| number(&a.item).cmp(&number(&b.item))));
    rows
}

/// The label in the first of `languages` that has one, as `(language, label)`.
async fn best_labels(
    state: &AppState,
    ids: &[String],
    languages: &[&str],
) -> anyhow::Result<HashMap<String, (String, String)>> {
    let rank = |l: &str| languages.iter().position(|x| *x == l).unwrap_or(usize::MAX);
    let mut best: HashMap<String, (String, String)> = HashMap::new();
    for term in terms::terms(&state.replicas, ids, &[TermType::Label], languages).await? {
        if best.get(&term.entity).is_none_or(|(l, _)| rank(&term.language) < rank(l)) {
            best.insert(term.entity, (term.language, term.text));
        }
    }
    Ok(best)
}

impl Row {
    /// What to suggest as a new label: the most frequent title, without “(…)”.
    fn suggestion(&self) -> &str {
        let title = self.titles.first().map_or("", String::as_str);
        title.split(" (").next().unwrap_or(title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn languages_and_tally() {
        assert_eq!(fallbacks("zh-hant, zh-hans,de,x'y,zh-hans", "de"), ["zh-hant", "zh-hans"]);
        let mut headers = HeaderMap::new();
        assert_eq!(browser_language(&headers), "en");
        headers.insert(header::ACCEPT_LANGUAGE, "de-DE,de;q=0.9".parse().unwrap());
        assert_eq!(browser_language(&headers), "de");
        let pair = |q: &str, t: &str| (q.to_string(), t.to_string());
        let rows = tally(vec![
            vec![pair("Q2", "B (x)"), pair("Q1", "A")],
            vec![pair("Q2", "B (x)"), pair("Q2", "Bee")],
            vec![pair("Q2", "Bee"), pair("Q3", "C")],
        ]);
        let summary: Vec<(&str, usize)> = rows.iter().map(|r| (r.item.as_str(), r.wikis)).collect();
        assert_eq!(summary, [("Q2", 3), ("Q1", 1), ("Q3", 1)]);
        assert_eq!(rows[0].titles, ["B (x)", "Bee"]);
        assert_eq!(rows[0].suggestion(), "B");
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/cloudy_concept/?lang=de").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"q\""));
        assert_eq!(get(dir.path(), "/cloudy_concept/?q=x").await.status.as_u16(), 400);
        assert_eq!(get(dir.path(), "/cloudy_concept/?q=Q1&lang=%3Cb%3E").await.status.as_u16(), 400);
    }
}
