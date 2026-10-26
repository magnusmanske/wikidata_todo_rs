//! `relabel.php`: the items whose label or alias, in any language, is
//! exactly one of some lines of text.

use crate::state::AppState;
use crate::terms::{self, TermType};
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

const MAX_LINES: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    labels: String,
    #[serde(default)]
    remove_brackets: String,
    #[serde(default)]
    lang: String,
}

/// A line of input, and what to look for.
#[derive(Debug, PartialEq, Eq)]
struct Line {
    text: String,
    search: String,
}

struct Match {
    line: String,
    link: Option<String>,
    /// Items, with their description.
    items: Vec<(String, String)>,
}

#[derive(Template)]
#[template(path = "tools/relabel.html")]
struct Page {
    title: &'static str,
    labels: String,
    remove_brackets: bool,
    lang: String,
    matches: Option<Vec<Match>>,
    max_lines: usize,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let remove_brackets = !matches!(request.remove_brackets.as_str(), "" | "0");
    let lang = request.lang.trim().to_lowercase();
    let wiki = match lang.as_str() {
        "" => None,
        lang => Some(Wiki::new(lang, "wikipedia")?),
    };
    let lines = parse_lines(&request.labels, remove_brackets);
    let matches = match lines.is_empty() {
        true => None,
        false => Some(find(&state, &lines, wiki.as_ref(), &lang).await?),
    };
    render(&Page { title: "ReLabel", labels: request.labels, remove_brackets, lang, matches, max_lines: MAX_LINES })
}

/// Non-empty lines, underscores as spaces; with `remove_brackets`, searched
/// without a trailing ` (…)`. At most [`MAX_LINES`], each once.
fn parse_lines(text: &str, remove_brackets: bool) -> Vec<Line> {
    let mut seen = BTreeSet::new();
    text.lines()
        .map(|l| l.replace('_', " ").trim().to_string())
        .filter(|l| !l.is_empty() && seen.insert(l.clone()))
        .take(MAX_LINES)
        .map(|text| {
            let search = match (remove_brackets, text.find(" (")) {
                (true, Some(i)) => text.get(..i).unwrap_or(&text).to_string(),
                _ => text.clone(),
            };
            Line { text, search }
        })
        .collect()
}

/// The matches of each line; with descriptions in `lang`, else English.
async fn find(state: &AppState, lines: &[Line], wiki: Option<&Wiki>, lang: &str) -> Result<Vec<Match>, AppError> {
    let texts: Vec<String> = lines.iter().map(|l| l.search.clone()).collect();
    let types = [TermType::Label, TermType::Alias];
    let mut by_text: HashMap<String, BTreeSet<(u64, String)>> = HashMap::new();
    for (item, text) in terms::items_with_texts(&state.replicas, &texts, &types, None).await? {
        let number = item.get(1..).and_then(|n| n.parse().ok()).unwrap_or_default();
        by_text.entry(text).or_default().insert((number, item));
    }
    let items: Vec<String> = by_text.values().flatten().map(|(_, q)| q.clone()).collect();
    let languages = terms::languages_with(lang, &["en"]);
    let mut descriptions: HashMap<String, (usize, String)> = HashMap::new();
    for term in terms::terms(&state.replicas, &items, &[TermType::Description], &languages).await? {
        let rank = languages.iter().position(|l| *l == term.language).unwrap_or(usize::MAX);
        if descriptions.get(&term.entity).is_none_or(|(r, _)| rank < *r) {
            descriptions.insert(term.entity, (rank, term.text));
        }
    }
    Ok(lines
        .iter()
        .map(|line| Match {
            line: line.text.clone(),
            link: wiki.map(|w| w.page_url(&line.text)),
            items: by_text
                .get(&line.search)
                .into_iter()
                .flatten()
                .map(|(_, q)| (q.clone(), descriptions.get(q).map(|(_, d)| d.clone()).unwrap_or_default()))
                .collect(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn lines() {
        let lines = parse_lines("Foo_bar (band)\n\n  Baz \nFoo bar (band)", true);
        assert_eq!(
            lines,
            [
                Line { text: "Foo bar (band)".into(), search: "Foo bar".into() },
                Line { text: "Baz".into(), search: "Baz".into() }
            ]
        );
        assert_eq!(parse_lines("A (b)", false)[0].search, "A (b)");
    }

    #[tokio::test]
    async fn form_and_bad_language() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/relabel/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"labels\""));
        assert_eq!(get(dir.path(), "/relabel/?labels=X&lang=evil.com%23").await.status.as_u16(), 400);
    }
}
