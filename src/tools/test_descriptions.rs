//! `test_descriptions.php`: random items, with their automatic description
//! (from AutoDesc) next to their manual one, and some statistics on both.

use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, is_language_code};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use futures::{StreamExt, stream};
use serde::Deserialize;
use std::sync::Arc;

const AUTODESC: &str = "https://autodesc.toolforge.org/";
/// AutoDesc's languages; it knows all but English and German partially.
const LANGUAGES: &[&str] = &["en", "de", "vi", "nl", "pl", "fr", "sv", "fi", "el"];
const MAX_ITEMS: usize = 100;
const CONCURRENCY: usize = 8;

#[derive(Debug, Deserialize)]
pub struct Request {
    noi: Option<usize>,
    lang: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AutoDesc {
    label: String,
    result: String,
    manual_description: String,
}

struct Row {
    item: String,
    label: String,
    automatic: String,
    manual: String,
}

/// Totals over all rows, `(automatic, manual)`.
#[derive(Debug, Default, PartialEq)]
struct Stats {
    present: (usize, usize),
    words: (usize, usize),
    chars: (usize, usize),
    identical: usize,
}

#[derive(Template)]
#[template(path = "tools/test_descriptions.html")]
struct Page {
    title: &'static str,
    language: String,
    languages: &'static [&'static str],
    items: usize,
    max_items: usize,
    rows: Vec<Row>,
    stats: Stats,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let items = request.noi.unwrap_or(10).clamp(1, MAX_ITEMS);
    let language = request.lang.unwrap_or_else(|| "en".to_string()).trim().to_string();
    if !is_language_code(&language) {
        return Err(AppError::BadRequest("unknown language".to_string()));
    }
    let random = random_items(&state.http, items).await?;
    let rows: Vec<Row> = stream::iter(random)
        .map(|item| {
            let (http, language) = (&state.http, language.as_str());
            async move {
                let desc = autodesc(http, &item, language).await.unwrap_or_else(|e| {
                    tracing::warn!("AutoDesc failed for {item}: {e:#}");
                    let error = "CONNECTION ERROR".to_string();
                    AutoDesc { label: item.clone(), result: error.clone(), manual_description: error }
                });
                Row { label: desc.label, automatic: desc.result, manual: desc.manual_description, item }
            }
        })
        .buffered(CONCURRENCY)
        .collect()
        .await;
    let stats = stats(&rows);
    render(&Page {
        title: "Description test",
        language,
        languages: LANGUAGES,
        items,
        max_items: MAX_ITEMS,
        rows,
        stats,
    })
}

async fn random_items(http: &reqwest::Client, n: usize) -> anyhow::Result<Vec<String>> {
    let limit = n.to_string();
    let params = [
        ("action", "query"),
        ("list", "random"),
        ("rnnamespace", "0"),
        ("rnfilterredir", "nonredirects"),
        ("rnlimit", limit.as_str()),
    ];
    let json = mediawiki::api(http, &Wiki::wikidata(), &params).await?;
    Ok(json["query"]["random"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["title"].as_str())
        .map(String::from)
        .collect())
}

/// The descriptions, as plain text.
async fn autodesc(http: &reqwest::Client, item: &str, language: &str) -> anyhow::Result<AutoDesc> {
    let params = [("q", item), ("lang", language), ("mode", "long"), ("links", "text"), ("format", "json")];
    Ok(http.get(AUTODESC).query(&params).send().await?.error_for_status()?.json().await?)
}

/// The text of a description; AutoDesc marks the lack of one in italics.
fn text(description: &str) -> &str {
    match description.trim_start().starts_with("<i>") {
        true => "",
        false => description.trim(),
    }
}

fn stats(rows: &[Row]) -> Stats {
    let mut stats = Stats::default();
    for row in rows {
        let (automatic, manual) = (text(&row.automatic), text(&row.manual));
        stats.identical += usize::from(automatic == manual);
        for (s, present, words, chars) in [
            (automatic, &mut stats.present.0, &mut stats.words.0, &mut stats.chars.0),
            (manual, &mut stats.present.1, &mut stats.words.1, &mut stats.chars.1),
        ] {
            let n = s.split_whitespace().count();
            *present += usize::from(n > 0);
            *words += n;
            *chars += s.chars().filter(|c| !c.is_whitespace()).count();
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    fn row(automatic: &str, manual: &str) -> Row {
        Row { item: "Q1".into(), label: "x".into(), automatic: automatic.into(), manual: manual.into() }
    }

    #[test]
    fn statistics() {
        let rows = [row("British writer", "British writer"), row("<i>no description</i>", "a b c")];
        let stats = stats(&rows);
        assert_eq!(stats, Stats { present: (1, 2), words: (2, 5), chars: (13, 16), identical: 1 });
    }

    #[tokio::test]
    async fn bad_language() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(get(dir.path(), "/test_descriptions/?lang=%3Cx%3E").await.status.as_u16(), 400);
    }
}
