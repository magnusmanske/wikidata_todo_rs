//! `get_item_names.php`: the labels of items, from a list or a PagePile, as
//! a page, JSON(P), tab-separated text or a wiki list. Static pages here
//! (`tree.html`, `tabernacle.html`, …) call it for `{"status":"OK","i2n":{…}}`.

use crate::pagepile;
use crate::state::AppState;
use crate::terms;
use crate::web::{AppError, Params, api_json, render};
use crate::wiki::is_language_code;
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::Arc;

/// The fallback languages, after the preferred one. `mul` holds the label
/// of many items (names, mostly) for all languages.
const LANGUAGES: &[&str] = &["mul", "en", "de", "fr", "es", "it", "pl", "pt", "ja", "ru", "hu"];
const MAX_ITEMS: usize = 50_000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    items: String,
    #[serde(default = "default_format")]
    format: String,
    #[serde(default)]
    pagepile: String,
    #[serde(default)]
    lang: String,
    #[serde(default)]
    onlylang: String,
    #[serde(default)]
    callback: String,
    doit: Option<String>,
}

fn default_format() -> String {
    "json".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Html,
    Json,
    Tsv,
    Wiki,
}

impl Format {
    const ALL: [(Self, &'static str, &'static str); 4] = [
        (Self::Html, "html", "HTML"),
        (Self::Json, "json", "JSON"),
        (Self::Tsv, "tsv", "Tab-delimited text"),
        (Self::Wiki, "wiki", "Wikipedia list"),
    ];

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, key, _)| *key == s.trim()).map(|(f, ..)| *f)
    }
}

#[derive(Template)]
#[template(path = "tools/get_item_names.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    items: String,
    formats: [(Format, &'static str, &'static str); 4],
    format: Format,
    languages: String,
    names: Option<Vec<(String, String)>>,
}

#[derive(Serialize)]
struct Answer {
    status: &'static str,
    i2n: Map<String, Value>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let format = Format::parse(&request.format).ok_or_else(|| AppError::BadRequest("unknown format".into()))?;
    let mut items = parse_items(&request.items);
    let nothing_asked = (request.pagepile.trim().is_empty() || request.doit.is_none())
        && items.is_empty()
        && request.callback.is_empty()
        && request.lang.is_empty()
        && request.onlylang.is_empty();
    if nothing_asked && format != Format::Html {
        return Ok(render(&page(&request, format, &items, None))?.into_response());
    }
    if items.is_empty() && !request.pagepile.trim().is_empty() {
        let id = request.pagepile.trim().parse().map_err(|_| AppError::BadRequest("invalid PagePile ID".into()))?;
        items = pagepile::items(&state.http, &state.replicas, id, MAX_ITEMS).await?;
        items.truncate(MAX_ITEMS);
    }
    let languages = languages(&request.lang, &request.onlylang)?;
    let labels = terms::labels(&state.replicas, &items, &languages).await?;
    let names: Vec<(String, String)> = items
        .into_iter()
        .map(|q| {
            let label = labels.get(&q).map(|l| l.trim()).filter(|l| !l.is_empty()).unwrap_or(&q).to_string();
            (q, label)
        })
        .collect();
    let text = |lines: String| ([(header::CONTENT_TYPE, "text/plain; charset=UTF-8")], lines).into_response();
    Ok(match format {
        Format::Html => render(&page(&request, format, &[], Some(names)))?.into_response(),
        Format::Json => {
            // Callers put the names into their pages as HTML.
            let i2n = names.into_iter().map(|(q, l)| (q, Value::String(l.replace('<', "&lt;")))).collect();
            let callback = Some(request.callback).filter(|c| !c.is_empty());
            api_json(Answer { status: "OK", i2n }, callback)
        }
        Format::Tsv => text(names.iter().map(|(q, l)| format!("{q}\t{l}\n")).collect()),
        Format::Wiki => text(names.iter().map(|(q, l)| format!("# [[:d:{q}|]] [[{l}]]\n")).collect()),
    })
}

fn page<'a>(request: &'a Request, format: Format, items: &[String], names: Option<Vec<(String, String)>>) -> Page<'a> {
    Page {
        title: "Get item names",
        request,
        items: items.join("\n"),
        formats: Format::ALL,
        format,
        languages: LANGUAGES.join(", "),
        names,
    }
}

/// Item IDs from a comma- or line-separated list, each once; at most [`MAX_ITEMS`].
fn parse_items(input: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    input
        .split([',', '\n'])
        .filter_map(|i| crate::wikidata::parse_id(i, 'Q'))
        .filter(|q| seen.insert(q.clone()))
        .take(MAX_ITEMS)
        .collect()
}

/// `onlylang` alone, or `lang` (one or more codes) and the fallbacks.
fn languages<'a>(lang: &'a str, onlylang: &'a str) -> Result<Vec<&'a str>, AppError> {
    let onlylang = onlylang.trim();
    match onlylang {
        "" => Ok(terms::languages_with(lang, LANGUAGES)),
        "mul" => Ok(vec!["mul"]),
        code if is_language_code(code) => Ok(vec![code, "mul"]),
        _ => Err(AppError::BadRequest("invalid language code".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn items_and_languages() {
        assert_eq!(parse_items("Q1,q2\n3,,x,Q1"), ["Q1", "Q2", "Q3"]);
        assert_eq!(languages("de", "").unwrap()[..3], ["de", "mul", "en"]);
        assert_eq!(languages("de", "nl").unwrap(), ["nl", "mul"]);
        assert!(languages("", "x'y").is_err());
        assert_eq!(Format::parse("tsv"), Some(Format::Tsv));
        assert_eq!(Format::parse("xml"), None);
    }

    #[tokio::test]
    async fn form_unless_asked() {
        let dir = tempfile::tempdir().unwrap();
        for uri in ["/get_item_names/", "/get_item_names.php"] {
            let form = get(dir.path(), uri).await;
            assert_eq!(form.status.as_u16(), 200);
            assert!(form.body.contains("name=\"items\""));
        }
        assert_eq!(get(dir.path(), "/get_item_names.php?format=xml").await.status.as_u16(), 400);
        assert_eq!(get(dir.path(), "/get_item_names.php?items=Q1&onlylang=%27").await.status.as_u16(), 400);
    }
}
