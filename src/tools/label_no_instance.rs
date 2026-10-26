//! `label_no_instance.php`: items with a label matching a `LIKE` pattern,
//! but neither "instance of" (P31) nor "subclass of" (P279).

use crate::replicas::{linktarget_ids, titles_linking_to};
use crate::state::AppState;
use crate::terms::{self, TermType};
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, is_language_code};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;

/// Matching labels looked at, at most.
const MAX_CANDIDATES: usize = 20_000;
const NS_PROPERTY: i64 = 120;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    lang: String,
    #[serde(default)]
    pattern: String,
    run: Option<String>,
}

struct Results {
    /// `(item, label)`
    items: Vec<(String, String)>,
    capped: bool,
}

#[derive(Template)]
#[template(path = "tools/label_no_instance.html")]
struct Page {
    title: &'static str,
    lang: String,
    pattern: String,
    results: Option<Results>,
    max_candidates: usize,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let lang = request.lang.trim().to_lowercase();
    let pattern = request.pattern.trim().to_string();
    let results = match request.run {
        Some(_) => {
            check(&lang, &pattern)?;
            Some(search(&state, &lang, &pattern).await?)
        }
        None => None,
    };
    let page =
        Page { title: "Labels without “instance of”", lang, pattern, results, max_candidates: MAX_CANDIDATES };
    render(&page)
}

/// A leading wildcard would make the database read every text there is.
fn check(lang: &str, pattern: &str) -> Result<(), AppError> {
    if !is_language_code(lang) {
        return Err(AppError::BadRequest("Please give a valid language code.".into()));
    }
    if pattern.is_empty() || pattern.starts_with(['%', '_']) {
        return Err(AppError::BadRequest("The pattern must not be empty or start with a wildcard.".into()));
    }
    Ok(())
}

async fn search(state: &AppState, lang: &str, pattern: &str) -> Result<Results, AppError> {
    let mut items =
        terms::items_with_text_like(&state.replicas, pattern, &[TermType::Label], lang, MAX_CANDIDATES).await?;
    let capped = items.len() >= MAX_CANDIDATES;
    let ids: Vec<String> = items.iter().map(|(q, _)| q.clone()).collect();
    let classified = classified(state, &ids).await?;
    items.retain(|(q, _)| !classified.contains(q));
    items.sort_by_key(|(q, _)| q.get(1..).and_then(|n| n.parse::<u64>().ok()));
    items.dedup_by(|a, b| a.0 == b.0);
    Ok(Results { items, capped })
}

/// Those of `items` with a P31 or P279 statement: they link to those properties.
async fn classified(state: &AppState, items: &[String]) -> anyhow::Result<HashSet<String>> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page", "pagelinks", "linktarget"]).await?;
    let targets = linktarget_ids(&mut conn, NS_PROPERTY, &["P31", "P279"]).await?;
    titles_linking_to(&mut conn, items, &targets).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn checks() {
        assert!(check("de", "Burg %").is_ok());
        assert!(check("de", "%burg").is_err());
        assert!(check("de", "").is_err());
        assert!(check("d'e", "Burg %").is_err());
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/label_no_instance/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"pattern\""));
        let bad = get(dir.path(), "/label_no_instance/?lang=de&pattern=%25x&run=1").await;
        assert_eq!(bad.status.as_u16(), 400);
    }
}
