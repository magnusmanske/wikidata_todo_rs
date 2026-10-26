//! `wikidata_cache.php`: items that have all of some external-ID
//! properties, from the tool's cache of Wikidata's external IDs
//! (`wikidata_cache_p`, filled by a job). As a page, or as BEACON or JSON
//! (SPARQL-like bindings, one row per combination of values), streamed.

use crate::cache::TimedCache;
use crate::replicas::{placeholders, select_in};
use crate::state::AppState;
use crate::tool_db::Database;
use crate::web::{AppError, Params, render, thousands};
use crate::wikidata;
use askama::Template;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use futures::{Stream, StreamExt, stream};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Params as SqlParams, Value as SqlValue};

/// Items per database round of the output.
const BATCH: usize = 1000;
/// Items listed on the page; the other formats have them all.
const MAX_ON_PAGE: usize = 5000;
const MAX_PROPERTIES: usize = 20;
const ENTITY_PREFIX: &str = "http://www.wikidata.org/entity/";

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    action: String,
    #[serde(default)]
    format: String,
    #[serde(default)]
    beacon_target_prop: String,
    #[serde(default)]
    all_properties: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Html,
    Beacon,
    Json,
    JsonVerbose,
}

impl Format {
    fn parse(s: &str) -> Self {
        match s {
            "beacon" => Self::Beacon,
            "json" => Self::Json,
            "json_verbose" => Self::JsonVerbose,
            _ => Self::Html,
        }
    }
}

#[derive(Template)]
#[template(path = "tools/wikidata_cache.html")]
struct Page {
    title: &'static str,
    all_properties: String,
    beacon_target_prop: String,
    stats: Option<Vec<(&'static str, String)>>,
    result: Option<Found>,
}

struct Found {
    description: String,
    total: String,
    items: Vec<u64>,
    capped: bool,
    query: String,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let format = Format::parse(&request.format);
    let mut page = Page {
        title: "Wikidata cache",
        all_properties: request.all_properties.trim().to_string(),
        beacon_target_prop: request.beacon_target_prop.trim().to_string(),
        stats: None,
        result: None,
    };
    match request.action.as_str() {
        "" => {}
        "stats" => page.stats = Some(STATS.get_or_fetch(async || stats(&state).await).await?.to_vec()),
        "items_with_properties" => {
            let properties = parse_properties(&request.all_properties)?;
            let target = match page.beacon_target_prop.as_str() {
                "" => None,
                p => Some(wikidata::parse_id(p, 'P').ok_or_else(|| bad_property(p))?),
            };
            let mut conn = state.tool_db.conn(Database::WikidataCache).await?;
            let items = items_with(&mut conn, &properties).await?;
            if format != Format::Html {
                let header = header_text(&state, format, &properties, target.as_deref()).await?;
                let output = Output { format, properties, target: header.target.clone() };
                return Ok(stream_response(format, header.text, output.body(conn, items)));
            }
            let total = thousands(items.len() as u64);
            let capped = items.len() > MAX_ON_PAGE;
            let query = serde_urlencoded::to_string([
                ("action", "items_with_properties"),
                ("all_properties", page.all_properties.as_str()),
                ("beacon_target_prop", page.beacon_target_prop.as_str()),
            ])
            .unwrap_or_default();
            let items = items.into_iter().take(MAX_ON_PAGE).collect();
            page.result = Some(Found { description: description(&properties), total, items, capped, query });
        }
        _ => return Err(AppError::BadRequest("unknown action".to_string())),
    }
    Ok(render(&page)?.into_response())
}

fn bad_property(input: &str) -> AppError {
    AppError::BadRequest(format!("“{input}” is not a property ID"))
}

/// One property ID per line, as `P214` (or `214`); each once.
fn parse_properties(text: &str) -> Result<Vec<String>, AppError> {
    let mut properties: Vec<String> = vec![];
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let property = wikidata::parse_id(line, 'P').ok_or_else(|| bad_property(line))?;
        if !properties.contains(&property) {
            properties.push(property);
        }
    }
    match properties.len() {
        0 => Err(AppError::BadRequest("Please give at least one property.".to_string())),
        n if n > MAX_PROPERTIES => Err(AppError::BadRequest(format!("At most {MAX_PROPERTIES} properties, please."))),
        _ => Ok(properties),
    }
}

/// `P214` as the number the database has.
fn number(property: &str) -> u64 {
    property.trim_start_matches('P').parse().unwrap_or(0)
}

fn description(properties: &[String]) -> String {
    format!("Wikidata items that have properties {}", properties.join(" and "))
}

/// The items (numbers) with all of `properties`.
async fn items_with(conn: &mut Conn, properties: &[String]) -> anyhow::Result<Vec<u64>> {
    let sql = format!(
        "SELECT item FROM pq_extid WHERE property IN ({}) GROUP BY item HAVING COUNT(DISTINCT property)=?",
        placeholders(properties.len())
    );
    let mut params: Vec<SqlValue> = properties.iter().map(|p| number(p).into()).collect();
    params.push(properties.len().into());
    Ok(conn.exec(sql, SqlParams::Positional(params)).await?)
}

/// The values of `property` of `items`, in database order.
async fn values(conn: &mut Conn, property: &str, items: &[u64]) -> anyhow::Result<HashMap<u64, Vec<String>>> {
    let sql = "SELECT item, text FROM vw_pq_text WHERE property=? AND item IN";
    let rows: Vec<(u64, Option<Vec<u8>>)> = select_in(conn, sql, "", &[number(property).into()], items).await?;
    let mut values: HashMap<u64, Vec<String>> = HashMap::new();
    for (item, text) in rows {
        values.entry(item).or_default().push(String::from_utf8_lossy(&text.unwrap_or_default()).into_owned());
    }
    Ok(values)
}

struct Header {
    text: String,
    /// BEACON: the property whose values are the link targets, if it has a formatter URL.
    target: Option<String>,
}

async fn header_text(
    state: &AppState,
    format: Format,
    properties: &[String],
    target: Option<&str>,
) -> anyhow::Result<Header> {
    let keys: Vec<String> = properties.iter().map(|p| format!(",\"{}\"", p.to_lowercase())).collect();
    let text = match format {
        Format::Html => String::new(),
        Format::Json => "[".to_string(),
        Format::JsonVerbose => format!("{{\"head\":{{\"vars\":[\"q\"{}]}},\"results\":{{\"bindings\":[", keys.concat()),
        Format::Beacon => {
            let mut text = format!("#FORMAT: BEACON\n#PREFIX: {ENTITY_PREFIX}\n");
            let mut description = description(properties);
            let formatter = match target {
                Some(p) => wikidata::entity(&state.http, p, "claims").await?.and_then(|e| beacon_target(&e)),
                None => None,
            };
            if let (Some(url), Some(p)) = (&formatter, target) {
                text += &format!("#TARGET: {url}\n");
                description += &format!("; linking to {p}");
            }
            text += &format!("#DESCRIPTION: {description}\n");
            let target = formatter.and(target.map(str::to_string));
            return Ok(Header { text, target });
        }
    };
    Ok(Header { text, target: None })
}

/// The BEACON target of a property: its formatter URL without the trailing `$1`.
fn beacon_target(property: &Value) -> Option<String> {
    let formatter = wikidata::first_string(property, "P1630")?;
    let trimmed = formatter.trim_end_matches('/');
    Some(trimmed.strip_suffix("$1").unwrap_or(formatter).to_string())
}

fn stream_response(
    format: Format,
    header: String,
    body: impl Stream<Item = anyhow::Result<String>> + Send + 'static,
) -> Response {
    let (content_type, footer) = match format {
        Format::Beacon | Format::Html => ("text/plain; charset=utf-8", ""),
        Format::Json => ("application/json", "]"),
        Format::JsonVerbose => ("application/json", "]}}"),
    };
    let all = stream::once(async { Ok(header) }).chain(body).chain(stream::once(async move { Ok(footer.to_string()) }));
    let bytes = all.map(|chunk| chunk.map(Bytes::from).map_err(std::io::Error::other));
    ([(header::CONTENT_TYPE, content_type), (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")], Body::from_stream(bytes))
        .into_response()
}

/// How the items go out, batch by batch.
struct Output {
    format: Format,
    properties: Vec<String>,
    target: Option<String>,
}

impl Output {
    /// The items, a batch at a time; stops at the first error.
    fn body(self, conn: Conn, items: Vec<u64>) -> impl Stream<Item = anyhow::Result<String>> + Send + 'static {
        let batches: Vec<Vec<u64>> = items.chunks(BATCH).map(<[u64]>::to_vec).collect();
        let state = (self, conn, batches.into_iter(), true);
        stream::unfold(Some(state), |state| async move {
            let (output, mut conn, mut batches, first) = state?;
            let batch = batches.next()?;
            match output.batch(&mut conn, &batch, first).await {
                Ok(text) => Some((Ok(text), Some((output, conn, batches, false)))),
                Err(e) => {
                    tracing::warn!("wikidata_cache output: {e:#}");
                    Some((Err(e), None))
                }
            }
        })
    }

    async fn batch(&self, conn: &mut Conn, items: &[u64], first: bool) -> anyhow::Result<String> {
        let mut out = String::new();
        match self.format {
            Format::Beacon | Format::Html => {
                let targets = match &self.target {
                    Some(p) => Some(values(conn, p, items).await?),
                    None => None,
                };
                for item in items {
                    match &targets {
                        // The last value, as in the PHP tool.
                        Some(t) => {
                            out +=
                                &format!("Q{item}||{}\n", t.get(item).and_then(|v| v.last()).map_or("", String::as_str))
                        }
                        None => out += &format!("Q{item}\n"),
                    }
                }
            }
            Format::Json | Format::JsonVerbose => {
                let mut by_property = vec![];
                for p in &self.properties {
                    by_property.push((p.to_lowercase(), values(conn, p, items).await?));
                }
                let mut first = first;
                for item in items {
                    let values: Vec<(&str, &[String])> = by_property
                        .iter()
                        .map(|(key, v)| (key.as_str(), v.get(item).map_or(&[][..], Vec::as_slice)))
                        .collect();
                    for row in bindings(*item, &values, self.format == Format::JsonVerbose) {
                        if !std::mem::take(&mut first) {
                            out.push(',');
                        }
                        out += &serde_json::to_string(&row)?;
                    }
                }
            }
        }
        Ok(out)
    }
}

/// One row per combination of the item's property values; verbose: as SPARQL JSON results.
fn bindings(item: u64, values: &[(&str, &[String])], verbose: bool) -> Vec<Map<String, Value>> {
    let cell = |kind: &str, value: String| if verbose { json!({"type": kind, "value": value}) } else { json!(value) };
    let mut rows = vec![Map::from_iter([("q".to_string(), cell("uri", format!("{ENTITY_PREFIX}Q{item}")))])];
    for (key, values) in values {
        rows = values
            .iter()
            .flat_map(|value| {
                rows.iter().map(|row| {
                    let mut row = row.clone();
                    row.insert(key.to_string(), cell("literal", value.clone()));
                    row
                })
            })
            .collect();
    }
    rows
}

static STATS: TimedCache<Vec<(&'static str, String)>> = TimedCache::new(Duration::from_secs(3600));

async fn stats(state: &AppState) -> anyhow::Result<Vec<(&'static str, String)>> {
    const COLUMNS: &[(&str, &str)] = &[
        ("items", "Items"),
        ("properties", "Properties"),
        ("text_8", "Values of up to 8 bytes"),
        ("text_16", "Values of 9 to 16 bytes"),
        ("text_32", "Values of 17 to 32 bytes"),
        ("text_64", "Values of 33 to 64 bytes"),
        ("text_medium", "Longer values"),
        ("longest_text", "Longest value (bytes)"),
    ];
    let mut conn = state.tool_db.conn(Database::WikidataCache).await?;
    let sql = format!("SELECT {} FROM vw_overview", COLUMNS.iter().map(|(c, _)| *c).collect::<Vec<_>>().join(","));
    let row: Option<wikimisc::mysql_async::Row> = conn.query_first(sql).await?;
    let row = row.ok_or_else(|| anyhow::anyhow!("no overview"))?;
    Ok(COLUMNS
        .iter()
        .enumerate()
        .map(|(i, (_, label))| {
            (*label, row.get_opt::<u64, _>(i).and_then(Result::ok).map(thousands).unwrap_or_default())
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn properties() {
        assert_eq!(parse_properties("P214\n 227 \n\nP214\r\n").unwrap(), ["P214", "P227"]);
        assert!(parse_properties("").is_err());
        assert!(parse_properties("P214\nQ5").is_err());
        assert_eq!(number("P214"), 214);
    }

    #[test]
    fn rows_per_combination() {
        let (a, b) = (vec!["1".to_string(), "2".to_string()], vec!["x".to_string()]);
        let rows = bindings(42, &[("p214", &a), ("p227", &b)], false);
        assert_eq!(
            serde_json::to_string(&rows).unwrap(),
            r#"[{"q":"http://www.wikidata.org/entity/Q42","p214":"1","p227":"x"},{"q":"http://www.wikidata.org/entity/Q42","p214":"2","p227":"x"}]"#
        );
        let verbose = bindings(42, &[("p214", &b)], true);
        assert_eq!(verbose[0]["p214"], json!({"type": "literal", "value": "x"}));
        assert_eq!(verbose[0]["q"]["type"], "uri");
        assert!(bindings(42, &[("p214", &[])], false).is_empty());
    }

    #[test]
    fn beacon_targets() {
        let property = |url: &str| json!({"claims": {"P1630": [{"rank": "normal", "mainsnak": {"snaktype": "value", "datavalue": {"value": url}}}]}});
        assert_eq!(beacon_target(&property("https://viaf.org/viaf/$1/")).as_deref(), Some("https://viaf.org/viaf/"));
        assert_eq!(beacon_target(&property("https://d-nb.info/gnd/$1")).as_deref(), Some("https://d-nb.info/gnd/"));
        assert_eq!(beacon_target(&property("https://x.org/$1.html")).as_deref(), Some("https://x.org/$1.html"));
        assert_eq!(beacon_target(&json!({})), None);
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/wikidata_cache/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"all_properties\""));
        let old = get(dir.path(), "/wikidata_cache.php?action=items_with_properties&all_properties=Q5").await;
        assert_eq!(old.status.as_u16(), 400);
        assert_eq!(get(dir.path(), "/wikidata_cache/?action=x").await.status.as_u16(), 400);
    }
}
