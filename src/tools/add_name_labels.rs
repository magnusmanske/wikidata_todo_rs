//! `add_name_labels.php`: for the items of a SPARQL query that have no label
//! in one language, but one in others, QuickStatements commands that copy
//! that label over; meant for names, which most languages share.

use crate::state::AppState;
use crate::terms::{self, TermType};
use crate::web::{AppError, Params, render};
use crate::wiki::{is_language_code, upper_first};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

/// Items from the query, at most.
const MAX_ITEMS: usize = 50_000;
const DEFAULT_QUERY: &str = "SELECT ?item { ?item wdt:P31 wd:Q5 ; wdt:P27 wd:Q29 }";
/// A label for all languages, used for names; an item with one needs no other.
const MUL: &str = "mul";

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_from")]
    from: String,
    #[serde(default = "default_to")]
    to: String,
    #[serde(default)]
    sparql: String,
    /// The old tool's WDQ query; WDQ is gone.
    #[serde(default)]
    wdq: String,
    doit: Option<String>,
}

fn default_from() -> String {
    "es,fr".to_string()
}

fn default_to() -> String {
    "en".to_string()
}

struct Results {
    items: usize,
    commands: String,
    count: usize,
}

#[derive(Template)]
#[template(path = "tools/add_name_labels.html")]
struct Page {
    title: &'static str,
    from: String,
    to: String,
    sparql: String,
    results: Option<Results>,
    max_items: usize,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let sparql = match request.sparql.trim() {
        "" => DEFAULT_QUERY.to_string(),
        query => query.to_string(),
    };
    let mut page = Page {
        title: "Names as labels",
        from: request.from.trim().to_string(),
        to: request.to.trim().to_lowercase(),
        sparql,
        results: None,
        max_items: MAX_ITEMS,
    };
    if !request.wdq.trim().is_empty() && request.sparql.trim().is_empty() {
        return Err(AppError::BadRequest("WDQ no longer exists; please use a SPARQL query instead.".into()));
    }
    if request.doit.is_none() {
        return render(&page);
    }
    let from = languages(&page.from)?;
    if !is_language_code(&page.to) {
        return Err(AppError::BadRequest("invalid target language".into()));
    }
    let mut items = state.sparql.entities(&page.sparql, None, MAX_ITEMS).await?.ids;
    items.retain(|q| q.starts_with('Q'));
    let mut wanted: Vec<&str> = from.clone();
    wanted.extend([page.to.as_str(), MUL]);
    let labels = terms::terms(&state.replicas, &items, &[TermType::Label], &wanted).await?;
    let mut by_item: HashMap<&str, HashMap<&str, &str>> = HashMap::new();
    for t in &labels {
        by_item.entry(&t.entity).or_default().insert(&t.language, &t.text);
    }
    let commands: Vec<String> = items
        .iter()
        .filter_map(|q| {
            let labels = by_item.get(q.as_str())?;
            let label = new_label(labels, &from, &page.to)?;
            Some(format!("{q}\tL{}\t\"{label}\"", page.to))
        })
        .collect();
    page.results = Some(Results { items: items.len(), count: commands.len(), commands: commands.join("\n") });
    render(&page)
}

/// Comma-separated language codes.
fn languages(list: &str) -> Result<Vec<&str>, AppError> {
    let codes: Vec<&str> = list.split(',').map(str::trim).filter(|l| !l.is_empty()).collect();
    match !codes.is_empty() && codes.iter().all(|l| is_language_code(l)) {
        true => Ok(codes),
        false => Err(AppError::BadRequest("invalid source languages".into())),
    }
}

/// The label for `to`, from the first of `from` that has one; none if the
/// item has a label in `to` or `mul` already. Without a trailing “(…)”.
fn new_label(labels: &HashMap<&str, &str>, from: &[&str], to: &str) -> Option<String> {
    if labels.contains_key(to) || labels.contains_key(MUL) {
        return None;
    }
    let label = from.iter().find_map(|l| labels.get(l))?;
    let label = label.split_once('(').map_or(*label, |(name, _)| name).trim();
    // Would break the QuickStatements format.
    (!label.is_empty() && !label.contains(['|', '\t', '"'])).then(|| upper_first(label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn labels() {
        let labels = HashMap::from([("es", "juan (pintor)"), ("fr", "Jean")]);
        assert_eq!(new_label(&labels, &["es", "fr"], "en").as_deref(), Some("Juan"));
        assert_eq!(new_label(&labels, &["fr", "es"], "en").as_deref(), Some("Jean"));
        assert_eq!(new_label(&labels, &["de"], "en"), None);
        assert_eq!(new_label(&labels, &["es"], "fr"), None);
        let named = HashMap::from([("mul", "Juan"), ("es", "Juan")]);
        assert_eq!(new_label(&named, &["es"], "en"), None);
        assert_eq!(languages(" es, fr ").unwrap(), ["es", "fr"]);
        assert!(languages("es,'").is_err());
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/add_name_labels/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("wdt:P27"));
        assert_eq!(get(dir.path(), "/add_name_labels/?from=x%27&doit=1").await.status.as_u16(), 400);
        assert_eq!(get(dir.path(), "/add_name_labels/?wdq=claim[31:5]&doit=1").await.status.as_u16(), 400);
    }
}
