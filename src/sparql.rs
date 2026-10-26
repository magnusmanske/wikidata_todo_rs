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
/// Results larger than this are refused rather than parsed into memory.
const MAX_JSON_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Sparql {
    client: SparqlClient,
}

impl Sparql {
    pub fn new(http: reqwest::Client) -> Result<Self> {
        // The shared client's timeout is too short for slow queries.
        let http =
            reqwest::Client::builder().user_agent(crate::state::USER_AGENT).timeout(TIMEOUT).build().unwrap_or(http);
        let client = SparqlClient::with_client(SparqlEndpoint::wdqs(), http)
            .with_retries(2)
            .with_max_response_bytes(MAX_JSON_BYTES);
        Ok(Self { client })
    }

    /// The query's result, as WDQS's JSON.
    pub async fn json(&self, query: &str) -> Result<Value> {
        self.client.query_json(&tagged(query)).await.context("SPARQL query failed")
    }

    /// The result as CSV, buffered in a temporary file: for large results.
    pub async fn csv(&self, query: &str) -> Result<csv::Reader<File>> {
        self.client.query_csv(&tagged(query)).await.context("SPARQL query failed")
    }

    /// The entity IDs (`Q42`, `P31`, `L1`, …) in variable `var` of the
    /// result, or in its first variable; each once, in result order.
    pub async fn entities(&self, query: &str, var: Option<&str>) -> Result<Vec<String>> {
        let json = self.json(query).await?;
        Ok(entities_in(&json, var))
    }
}

/// Lets the query service see where queries come from.
fn tagged(query: &str) -> String {
    format!("{query}\n#TOOL: wikidata-todo")
}

/// See [`Sparql::entities`].
pub fn entities_in(json: &Value, var: Option<&str>) -> Vec<String> {
    let var = var.or_else(|| json["head"]["vars"][0].as_str()).unwrap_or_default();
    let mut seen = HashSet::new();
    bindings(json)
        .filter_map(|row| row[var]["value"].as_str().and_then(entity_id))
        .filter(|id| seen.insert(id.to_string()))
        .map(str::to_string)
        .collect()
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
    use serde_json::json;

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
    fn entities_in_results() {
        let json = json!({
            "head": {"vars": ["q", "x"]},
            "results": {"bindings": [
                {"q": {"type": "uri", "value": "http://www.wikidata.org/entity/Q1"}},
                {"q": {"type": "uri", "value": "http://www.wikidata.org/entity/Q2"}, "x": {"value": "http://www.wikidata.org/entity/Q9"}},
                {"q": {"type": "uri", "value": "http://www.wikidata.org/entity/Q1"}},
            ]}
        });
        assert_eq!(entities_in(&json, None), ["Q1", "Q2"]);
        assert_eq!(entities_in(&json, Some("x")), ["Q9"]);
    }

    #[test]
    fn points_and_literals() {
        assert_eq!(point("Point(13.38 52.51)"), Some((13.38, 52.51)));
        assert_eq!(point("POINT(-1 2.5)"), Some((-1.0, 2.5)));
        assert_eq!(point("Pt(1 2)"), None);
        assert_eq!(literal("a \"b\" \\ c"), r#""a \"b\" \\ c""#);
    }
}
