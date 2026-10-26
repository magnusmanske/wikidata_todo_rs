//! The Wikidata Query Service.
//!
//! Users write their queries for WDQS as it is (Blazegraph), so that is where
//! queries go. The tools' own queries avoid what its successor (QLever) lacks,
//! such as `SERVICE wikibase:label`: labels come from the term store instead
//! (see [`crate::terms`]).

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::File;
use std::time::Duration;
use wikimisc::sparql_client::SparqlClient;
use wikimisc::sparql_endpoint::SparqlEndpoint;

/// WDQS stops a query after 60 seconds; this leaves time for the answer.
const TIMEOUT: Duration = Duration::from_secs(90);
/// JSON results larger than this are refused: parsed, they take several
/// times their size in memory, and the service has 512 MiB.
const MAX_JSON_BYTES: usize = 32 * 1024 * 1024;
/// CSV results go to a temporary file and are read row by row.
const MAX_CSV_BYTES: usize = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Sparql {
    /// For JSON results, which are parsed in memory.
    client: SparqlClient,
    /// For CSV results, which are buffered on disk.
    bulk: SparqlClient,
    /// For heavy queries of the jobs, which WDQS cannot finish in time.
    qlever: SparqlClient,
}

impl Sparql {
    pub fn new(http: reqwest::Client) -> Result<Self> {
        // The shared client's timeout is too short for slow queries.
        let http =
            reqwest::Client::builder().user_agent(crate::state::USER_AGENT).timeout(TIMEOUT).build().unwrap_or(http);
        let wdqs = SparqlClient::with_client(SparqlEndpoint::wdqs(), http.clone()).with_retries(2);
        let client = wdqs.clone().with_max_response_bytes(MAX_JSON_BYTES);
        let bulk = wdqs.with_max_response_bytes(MAX_CSV_BYTES);
        let qlever = SparqlClient::with_client(SparqlEndpoint::qlever_wikidata(), http)
            .with_retries(1)
            .with_max_response_bytes(MAX_CSV_BYTES);
        Ok(Self { client, bulk, qlever })
    }

    /// The query's result, as WDQS's JSON.
    pub async fn json(&self, query: &str) -> Result<Value> {
        self.client.query_json(&tagged(query)).await.context("SPARQL query failed")
    }

    /// The result as CSV, buffered in a temporary file: for large results.
    pub async fn csv(&self, query: &str) -> Result<csv::Reader<File>> {
        self.bulk.query_csv(&tagged(query)).await.context("SPARQL query failed")
    }

    /// As [`Self::csv`], but if WDQS fails (mostly: times out), from QLever,
    /// whose data may be a little older. For jobs; the query must suit both.
    pub async fn heavy_csv(&self, query: &str) -> Result<csv::Reader<File>> {
        match self.bulk.query_csv(&tagged(query)).await {
            Ok(reader) => Ok(reader),
            Err(e) => {
                tracing::info!("WDQS failed ({e}), trying QLever");
                self.qlever.query_csv(query).await.context("SPARQL query failed on WDQS and QLever")
            }
        }
    }

    /// The first `limit` entity IDs (`Q42`, `P31`, `L1`, …) in variable
    /// `var` of the result, or in its first variable; each once, in result
    /// order. Read row by row, so that a huge result costs no memory.
    pub async fn entities(&self, query: &str, var: Option<&str>, limit: usize) -> Result<Entities> {
        entities_from_csv(self.csv(query).await?, var, limit)
    }
}

/// See [`Sparql::entities`].
fn entities_from_csv<R: std::io::Read>(mut csv: csv::Reader<R>, var: Option<&str>, limit: usize) -> Result<Entities> {
    let headers = csv.headers()?.clone();
    let column = match var {
        Some(var) => headers.iter().position(|h| h == var).context("no such variable in the result")?,
        None => 0,
    };
    let mut found = Entities { var: headers.get(column).unwrap_or_default().to_string(), ..Default::default() };
    let mut seen = HashSet::new();
    for record in csv.records() {
        let Some(id) = record?.get(column).and_then(entity_id).map(str::to_string) else { continue };
        found.total += 1;
        if found.ids.len() < limit && seen.insert(id.clone()) {
            found.ids.push(id);
        }
    }
    Ok(found)
}

/// See [`Sparql::entities`].
#[derive(Debug, Default)]
pub struct Entities {
    /// The variable they are from.
    pub var: String,
    pub ids: Vec<String>,
    /// Rows with an entity, including those beyond the limit.
    pub total: usize,
}

/// Lets the query service see where queries come from.
fn tagged(query: &str) -> String {
    format!("{query}\n#TOOL: wikidata-todo")
}

/// The result rows.
pub fn bindings(json: &Value) -> impl Iterator<Item = &Value> {
    json["results"]["bindings"].as_array().into_iter().flatten()
}

/// The entity ID at the end of an entity URI: `…/entity/Q42` is `Q42`;
/// lexeme forms and senses (`L7-F2`) included.
pub fn entity_id(uri: &str) -> Option<&str> {
    let id = uri.rsplit('/').next()?;
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let valid = match id.split_at_checked(1)? {
        ("Q" | "P" | "M", rest) => number(rest),
        ("L", rest) => match rest.split_once('-') {
            None => number(rest),
            Some((lexeme, sub)) => number(lexeme) && sub.strip_prefix(['F', 'S']).is_some_and(number),
        },
        _ => false,
    };
    valid.then_some(id)
}

/// Longitude and latitude of a WKT point, as WDQS (`Point(…)`) and QLever (`POINT(…)`) write it.
pub fn point(wkt: &str) -> Option<(f64, f64)> {
    let inner = wkt.trim().get(5..)?.strip_prefix('(')?.strip_suffix(')')?;
    wkt.get(..5).filter(|p| p.eq_ignore_ascii_case("point"))?;
    let (lon, lat) = inner.trim().split_once(char::is_whitespace)?;
    Some((lon.trim().parse().ok()?, lat.trim().parse().ok()?))
}

/// A string as a SPARQL literal, quotes and backslashes escaped.
pub fn literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_ids() {
        assert_eq!(entity_id("http://www.wikidata.org/entity/Q42"), Some("Q42"));
        assert_eq!(entity_id("http://www.wikidata.org/entity/P31"), Some("P31"));
        assert_eq!(entity_id("http://www.wikidata.org/entity/L7-F2"), Some("L7-F2"));
        assert_eq!(entity_id("http://www.wikidata.org/entity/statement/Q1-abc"), None);
        assert_eq!(entity_id("Q"), None);
        assert_eq!(entity_id("http://x.org/Qx1"), None);
    }

    #[test]
    fn entities_from_rows() {
        let csv = |text: &str| csv::Reader::from_reader(std::io::Cursor::new(text.to_string()));
        let e = "http://www.wikidata.org/entity/";
        let text = &format!("q,x\n{e}Q1,{e}Q9\n{e}Q2,\n{e}Q1,\nnot an entity,\n{e}Q3,\n");
        let found = entities_from_csv(csv(text), None, 2).unwrap();
        assert_eq!((found.var.as_str(), found.ids, found.total), ("q", vec!["Q1".to_string(), "Q2".to_string()], 4));
        assert_eq!(entities_from_csv(csv(text), Some("x"), 10).unwrap().ids, ["Q9"]);
        assert!(entities_from_csv(csv(text), Some("y"), 10).is_err());
    }

    #[test]
    fn points_and_literals() {
        assert_eq!(point("Point(13.38 52.51)"), Some((13.38, 52.51)));
        assert_eq!(point("POINT(-1 2.5)"), Some((-1.0, 2.5)));
        assert_eq!(point("Pt(1 2)"), None);
        assert_eq!(literal("a \"b\" \\ c"), r#""a \"b\" \\ c""#);
    }
}
