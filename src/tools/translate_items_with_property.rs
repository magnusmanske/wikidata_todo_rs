//! `translate_items_with_property.php`: the labels and aliases, in all
//! languages, of the items with a string value for a property, with that
//! value; as tab-separated text, streamed. Without a property, a form.

use crate::sparql;
use crate::state::AppState;
use crate::terms::{self, TermType};
use crate::web::{AppError, Params, render};
use askama::Template;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use futures::{StreamExt, TryStreamExt, stream};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::sync::Arc;

/// Items per term store query.
const CHUNK: usize = 5000;
const HEADER: &str = "Item\tID\tLanguage\tType\tText\n";

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    prop: String,
}

#[derive(Template)]
#[template(path = "tools/translate_items_with_property.html")]
struct Page {
    title: &'static str,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let prop = request.prop.trim();
    if prop.is_empty() || prop == "0" {
        return Ok(render(&Page { title: "Translations of items with a property" })?.into_response());
    }
    let prop = crate::wikidata::parse_id(prop, 'P').ok_or_else(|| AppError::BadRequest("invalid property".into()))?;
    let query = format!("SELECT ?q ?pv {{ ?q wdt:{prop} ?pv FILTER(isLiteral(?pv)) }}");
    let reader = state.sparql.csv(&query).await?;
    let rows = stream::try_unfold((reader, HashSet::new(), state), |(mut reader, mut seen, state)| async move {
        let chunk = next_chunk(&mut reader, &mut seen)?;
        if chunk.is_empty() {
            return Ok(None);
        }
        let text = lines(&state, &chunk).await?;
        anyhow::Ok(Some((Bytes::from(text), (reader, seen, state))))
    });
    let body = stream::once(async { Ok(Bytes::from_static(HEADER.as_bytes())) }).chain(rows.into_stream());
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], Body::from_stream(body)).into_response())
}

/// The next up to [`CHUNK`] items not `seen` before, with their value.
fn next_chunk(reader: &mut csv::Reader<File>, seen: &mut HashSet<String>) -> anyhow::Result<Vec<(String, String)>> {
    let mut chunk = vec![];
    let mut record = csv::StringRecord::new();
    while chunk.len() < CHUNK && reader.read_record(&mut record)? {
        let (Some(item), Some(value)) = (record.get(0).and_then(sparql::entity_id), record.get(1)) else { continue };
        if item.starts_with('Q') && seen.insert(item.to_string()) {
            chunk.push((item.to_string(), value.to_string()));
        }
    }
    Ok(chunk)
}

/// The output lines for `items`, as `(item, value)`.
async fn lines(state: &AppState, items: &[(String, String)]) -> anyhow::Result<String> {
    let ids: Vec<String> = items.iter().map(|(q, _)| q.clone()).collect();
    let mut terms = terms::terms(&state.replicas, &ids, &[TermType::Label, TermType::Alias], &[]).await?;
    terms.sort_by(|a, b| (&a.entity, a.term_type as u8, &a.language).cmp(&(&b.entity, b.term_type as u8, &b.language)));
    let values: HashMap<&str, &str> = items.iter().map(|(q, v)| (q.as_str(), v.as_str())).collect();
    Ok(terms
        .iter()
        .filter_map(|t| {
            let value = values.get(t.entity.as_str())?;
            let kind = if t.term_type == TermType::Label { "label" } else { "alias" };
            Some(format!("{}\t{value}\t{}\t{kind}\t{}\n", t.entity, t.language, t.text))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

    #[tokio::test]
    async fn form_and_bad_property() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/translate_items_with_property/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"prop\""));
        assert_eq!(get(dir.path(), "/translate_items_with_property/?prop=x1").await.status.as_u16(), 400);
    }
}
