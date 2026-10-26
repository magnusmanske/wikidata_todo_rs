//! `multibeacon.php`: for values of a key property (an external ID, say),
//! the items that have them, and their values of other properties, as a
//! tab-separated table.

use crate::sparql;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wikidata::parse_id;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use futures::{StreamExt, TryStreamExt, stream};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

const MAX_VALUES: usize = 5000;
const MAX_COLUMNS: usize = 20;
/// Values per query.
const CHUNK: usize = 200;
const CONCURRENCY: usize = 4;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    keyprop: String,
    values: String,
    columns: String,
}

impl Default for Request {
    fn default() -> Self {
        Self { keyprop: String::new(), values: String::new(), columns: "Q".to_string() }
    }
}

/// The parsed request.
struct Table<'a> {
    key: String,
    /// `(heading as given, property)`.
    columns: Vec<(&'a str, String)>,
    values: Vec<&'a str>,
}

impl<'a> Table<'a> {
    fn parse(request: &'a Request) -> Result<Self, AppError> {
        let key = parse_id(&request.keyprop, 'P')
            .ok_or_else(|| AppError::BadRequest("Please give a key property, such as P214.".to_string()))?;
        let columns: Vec<(&str, String)> = request
            .columns
            .lines()
            .map(str::trim)
            .filter(|c| c.trim_start_matches(['P', 'p']).bytes().all(|b| b.is_ascii_digit()))
            .filter_map(|c| Some((c, parse_id(c, 'P')?)))
            .collect();
        let values: Vec<&str> = request.values.lines().map(str::trim).filter(|v| !v.is_empty()).collect();
        if columns.len() > MAX_COLUMNS || values.len() > MAX_VALUES {
            return Err(AppError::BadRequest(format!(
                "At most {MAX_VALUES} values and {MAX_COLUMNS} columns, please."
            )));
        }
        Ok(Self { key, columns, values })
    }

    fn query(&self, values: &[&str]) -> String {
        let variables: String = (0..self.columns.len()).map(|i| format!(" ?c{i}")).collect();
        let optionals: String =
            self.columns.iter().enumerate().map(|(i, (_, p))| format!(" OPTIONAL {{ ?q wdt:{p} ?c{i} }}")).collect();
        let values: Vec<String> = values.iter().map(|v| sparql::literal(v)).collect();
        format!(
            "SELECT ?q ?key{variables} {{ VALUES ?key {{ {} }} ?q wdt:{} ?key .{optionals} }}",
            values.join(" "),
            self.key
        )
    }

    fn heading(&self) -> String {
        let mut cells = vec!["Item".to_string(), self.key.clone()];
        cells.extend(self.columns.iter().map(|(heading, _)| heading.to_string()));
        cells.join("\t")
    }

    /// The result rows of a query, by key value.
    fn rows(&self, json: &Value, rows: &mut HashMap<String, Vec<String>>) {
        for row in sparql::bindings(json) {
            let (Some(item), Some(key)) = (cell(&row["q"]), row["key"]["value"].as_str()) else { continue };
            let mut cells = vec![item, key.to_string()];
            cells.extend((0..self.columns.len()).map(|i| cell(&row[format!("c{i}")]).unwrap_or_default()));
            rows.entry(key.to_string()).or_default().push(cells.join("\t"));
        }
    }
}

/// A value; items as their IDs.
fn cell(value: &Value) -> Option<String> {
    let text = value["value"].as_str()?;
    let entity = (value["type"] == "uri").then(|| text.strip_prefix("http://www.wikidata.org/entity/")).flatten();
    Some(entity.unwrap_or(text).to_string())
}

#[derive(Template)]
#[template(path = "tools/multibeacon.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    max_values: usize,
    output: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mut page = Page { title: "Multi-BEACON", request: &request, max_values: MAX_VALUES, output: None };
    if request.keyprop.trim().is_empty() {
        return render(&page);
    }
    let table = Table::parse(&request)?;
    let queries: Vec<String> = table.values.chunks(CHUNK).map(|chunk| table.query(chunk)).collect();
    let sparql = &state.sparql;
    let results: Vec<Value> = stream::iter(queries)
        .map(|query| async move { sparql.json(&query).await })
        .buffered(CONCURRENCY)
        .try_collect()
        .await?;
    let mut rows = HashMap::new();
    for json in &results {
        table.rows(json, &mut rows);
    }
    let mut lines = vec![table.heading()];
    for value in &table.values {
        match rows.get(*value) {
            Some(found) => lines.extend(found.iter().cloned()),
            // Values without an item stand alone.
            None => lines.push(value.to_string()),
        }
    }
    page.output = Some(lines.join("\n"));
    render(&page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;
    use serde_json::json;

    fn request() -> Request {
        Request { keyprop: "214".into(), values: "113230702\n\n x\"y ".into(), columns: "Q\nP31\nfoo\n569".into() }
    }

    #[test]
    fn parsed_and_queried() {
        let request = request();
        let table = Table::parse(&request).unwrap();
        assert_eq!(table.heading(), "Item\tP214\tP31\t569");
        assert_eq!(table.values, ["113230702", "x\"y"]);
        assert_eq!(
            table.query(&table.values),
            r#"SELECT ?q ?key ?c0 ?c1 { VALUES ?key { "113230702" "x\"y" } ?q wdt:P214 ?key . OPTIONAL { ?q wdt:P31 ?c0 } OPTIONAL { ?q wdt:P569 ?c1 } }"#
        );
    }

    #[test]
    fn rows_by_key() {
        let request = request();
        let table = Table::parse(&request).unwrap();
        let json = json!({"results": {"bindings": [{
            "q": {"type": "uri", "value": "http://www.wikidata.org/entity/Q42"},
            "key": {"type": "literal", "value": "113230702"},
            "c0": {"type": "uri", "value": "http://www.wikidata.org/entity/Q5"},
        }]}});
        let mut rows = HashMap::new();
        table.rows(&json, &mut rows);
        assert_eq!(rows["113230702"], ["Q42\t113230702\tQ5\t"]);
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/multibeacon/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains(">Q</textarea>"));
        assert_eq!(get(dir.path(), "/multibeacon/?keyprop=x").await.status.as_u16(), 400);
    }
}
