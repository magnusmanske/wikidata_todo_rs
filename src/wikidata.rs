//! Wikidata entities from the API, as JSON.

use crate::mediawiki;
use crate::wiki::Wiki;
use anyhow::Result;
use futures::{StreamExt, TryStreamExt, stream};
use serde_json::Value;
use std::collections::HashMap;

/// IDs per `wbgetentities` request: the API's maximum.
const BATCH: usize = 50;
/// Requests at a time.
const CONCURRENCY: usize = 4;

/// The entities (`Q42`, `P31`, `L1`, …) by ID, with only `props`
/// (e.g. `labels|claims`; empty for everything). Missing ones are left out.
pub async fn entities(http: &reqwest::Client, ids: &[String], props: &str) -> Result<HashMap<String, Value>> {
    let wiki = Wiki::wikidata();
    let batches: Vec<String> = ids.chunks(BATCH).map(|c| c.join("|")).collect();
    let results: Vec<Value> = stream::iter(batches)
        .map(|ids| {
            let wiki = &wiki;
            async move {
                let mut params = vec![("action", "wbgetentities"), ("ids", ids.as_str())];
                if !props.is_empty() {
                    params.push(("props", props));
                }
                // formatversion=2 is the default here, and entities stay keyed by ID.
                mediawiki::api_patiently(http, wiki, &params).await
            }
        })
        .buffer_unordered(CONCURRENCY)
        .try_collect()
        .await?;
    Ok(results
        .into_iter()
        .flat_map(|json| json["entities"].as_object().cloned().unwrap_or_default())
        .filter(|(_, entity)| entity.get("missing").is_none())
        .collect())
}

/// One entity; `None` if there is no such entity.
pub async fn entity(http: &reqwest::Client, id: &str, props: &str) -> Result<Option<Value>> {
    Ok(entities(http, &[id.to_string()], props).await?.remove(id))
}

/// The label in the first of `languages` that has one.
pub fn label<'a>(entity: &'a Value, languages: &[&str]) -> Option<&'a str> {
    wikimisc::labels::json_term(entity, "labels", languages)
}

/// The main values of the statements for `property`, best rank first (no deprecated ones).
pub fn values<'a>(entity: &'a Value, property: &str) -> Vec<&'a Value> {
    let mut statements: Vec<&Value> = entity["claims"][property]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["rank"] != "deprecated" && s["mainsnak"]["snaktype"] == "value")
        .collect();
    statements.sort_by_key(|s| if s["rank"] == "preferred" { 0 } else { 1 });
    statements.iter().map(|s| &s["mainsnak"]["datavalue"]["value"]).collect()
}

/// The item IDs that `property` statements point to.
pub fn item_values(entity: &Value, property: &str) -> Vec<String> {
    values(entity, property).into_iter().filter_map(|v| v["id"].as_str().map(str::to_string)).collect()
}

/// The first string value (external ID, URL, file name, …) of `property`.
pub fn first_string<'a>(entity: &'a Value, property: &str) -> Option<&'a str> {
    values(entity, property).into_iter().find_map(Value::as_str)
}

/// A valid entity ID such as `Q42`, from user input like `q42`, `42` (as
/// `prefix`) or a Wikidata URL.
pub fn parse_id(input: &str, prefix: char) -> Option<String> {
    let id = input.trim().rsplit('/').next()?.trim();
    let digits = id.strip_prefix([prefix, prefix.to_ascii_lowercase()]).unwrap_or(id);
    (!digits.is_empty() && digits.len() < 12 && digits.bytes().all(|b| b.is_ascii_digit()) && digits != "0")
        .then(|| format!("{prefix}{}", digits.trim_start_matches('0')))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_from_input() {
        assert_eq!(parse_id("Q42", 'Q').as_deref(), Some("Q42"));
        assert_eq!(parse_id(" q42 ", 'Q').as_deref(), Some("Q42"));
        assert_eq!(parse_id("42", 'Q').as_deref(), Some("Q42"));
        assert_eq!(parse_id("https://www.wikidata.org/wiki/Q42", 'Q').as_deref(), Some("Q42"));
        assert_eq!(parse_id("P31", 'P').as_deref(), Some("P31"));
        assert_eq!(parse_id("Q4x", 'Q'), None);
        assert_eq!(parse_id("Q", 'Q'), None);
        assert_eq!(parse_id("0", 'Q'), None);
        assert_eq!(parse_id("P31", 'Q'), None);
    }

    #[test]
    fn statement_values() {
        let entity = json!({"claims": {"P31": [
            {"rank": "normal", "mainsnak": {"snaktype": "value", "datavalue": {"value": {"id": "Q1"}}}},
            {"rank": "deprecated", "mainsnak": {"snaktype": "value", "datavalue": {"value": {"id": "Q2"}}}},
            {"rank": "preferred", "mainsnak": {"snaktype": "value", "datavalue": {"value": {"id": "Q3"}}}},
            {"rank": "normal", "mainsnak": {"snaktype": "novalue"}},
        ], "P214": [{"rank": "normal", "mainsnak": {"snaktype": "value", "datavalue": {"value": "1234"}}}]}});
        assert_eq!(item_values(&entity, "P31"), ["Q3", "Q1"]);
        assert_eq!(first_string(&entity, "P214"), Some("1234"));
        assert_eq!(first_string(&entity, "P18"), None);
    }
}
