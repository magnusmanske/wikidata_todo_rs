//! `resolver.php`: redirects to the item with a string value (such as an
//! external ID) for a property; or to its page on another wiki, or in a tool
//! (the URL patterns of Q43649390). `quick=VIAF:12307054` finds the property
//! by the start of its English label. External links use this: every
//! parameter stays.

use crate::cache::TimedCache;
use crate::sparql::{self, literal};
use crate::state::AppState;
use crate::terms::{self, DEFAULT_LANGUAGES};
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use crate::wikidata::{self, parse_id};
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// The item whose "formatter URL" (P3303) statements are the tools' URL patterns.
const TOOLS_ITEM: &str = "Q43649390";
const DAY: Duration = Duration::from_secs(24 * 3600);
/// Tool name → URL pattern (`$1` is the item).
static TOOLS: TimedCache<BTreeMap<String, String>> = TimedCache::new(DAY);
/// Property → lower-case English label.
static PROPERTIES: TimedCache<Vec<(String, String)>> = TimedCache::new(DAY);

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Request {
    prop: String,
    value: String,
    quick: String,
    project: String,
    no_autoload: String,
}

struct Found {
    property: String,
    value: String,
    /// `(label, item)`.
    items: Vec<(String, String)>,
}

#[derive(Template)]
#[template(path = "tools/resolver.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    property: String,
    found: Option<Found>,
    error: Option<String>,
    tools: Vec<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let lookup = lookup(&state, &request).await?;
    let mut error = None;
    let found = match lookup {
        Lookup::None => None,
        Lookup::Failed(message) => {
            error = Some(message);
            None
        }
        Lookup::Value(property, value) => {
            let items = items_with_value(&state, &property, &value).await?;
            if let [item] = items.as_slice()
                && request.no_autoload.trim().is_empty()
            {
                return Ok(Redirect::to(&target(&state, item, &request.project).await?).into_response());
            }
            let labels = terms::labels(&state.replicas, &items, DEFAULT_LANGUAGES).await?;
            let items = items.into_iter().map(|q| (labels.get(&q).cloned().unwrap_or_else(|| q.clone()), q)).collect();
            Some(Found { property, value, items })
        }
    };
    let tools = tools(&state).await.map(|t| t.keys().cloned().collect()).unwrap_or_default();
    let property = parse_id(&request.prop, 'P').unwrap_or_default();
    Ok(render(&Page { title: "Resolver", request: &request, property, found, error, tools })?.into_response())
}

enum Lookup {
    None,
    Failed(String),
    /// Property, value.
    Value(String, String),
}

async fn lookup(state: &AppState, request: &Request) -> Result<Lookup, AppError> {
    let value = request.value.trim();
    if !request.prop.trim().is_empty() && !value.is_empty() {
        let property = parse_id(&request.prop, 'P')
            .ok_or_else(|| AppError::BadRequest(format!("Not a property: {}", request.prop)))?;
        return Ok(Lookup::Value(property, value.to_string()));
    }
    let quick = request.quick.trim();
    if quick.is_empty() {
        return Ok(Lookup::None);
    }
    let Some((name, value)) = parse_quick(quick) else {
        return Ok(Lookup::Failed("Quick lookups look like VIAF:12307054.".to_string()));
    };
    if let Some(property) = parse_id(&name, 'P') {
        return Ok(Lookup::Value(property, value));
    }
    let properties = PROPERTIES.get_or_fetch(async || english_labels(&state.sparql).await).await?;
    Ok(match property_named(&properties, &name) {
        Some(property) => Lookup::Value(property.to_string(), value),
        None => Lookup::Failed(format!("No single property's English label starts with “{name}”.")),
    })
}

/// `VIAF ID:123` is `("viaf", "123")`: the property name up to the first
/// space or underscore, lower case; the value without quotes.
fn parse_quick(quick: &str) -> Option<(String, String)> {
    let (name, value) = quick.split_once(':')?;
    let name = name.split([' ', '_']).next().unwrap_or_default().trim().to_lowercase();
    let value = value.replace('"', "").trim().to_string();
    (!name.is_empty() && !value.is_empty()).then_some((name, value))
}

/// The one property whose label starts with `name`; of several, the one
/// called just that (or that, and "ID").
fn property_named<'a>(properties: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let matching: Vec<&(String, String)> = properties.iter().filter(|(_, label)| label.starts_with(name)).collect();
    match matching.as_slice() {
        [(property, _)] => Some(property),
        _ => {
            matching.iter().find(|(_, label)| *label == name || *label == format!("{name} id")).map(|(p, _)| p.as_str())
        }
    }
}

async fn english_labels(sparql: &sparql::Sparql) -> anyhow::Result<Vec<(String, String)>> {
    let query = "SELECT ?p ?label { ?p a wikibase:Property ; rdfs:label ?label FILTER(LANG(?label) = \"en\") }";
    let json = sparql.json(query).await?;
    Ok(sparql::bindings(&json)
        .filter_map(|row| {
            let property = sparql::entity_id(row["p"]["value"].as_str()?)?;
            Some((property.to_string(), row["label"]["value"].as_str()?.to_lowercase()))
        })
        .collect())
}

/// Items with the same value: more would not be shown anyway.
const MAX_MATCHES: usize = 1000;

/// Items with the value as is, in lower or upper case.
async fn items_with_value(state: &AppState, property: &str, value: &str) -> anyhow::Result<Vec<String>> {
    Ok(state.sparql.entities(&value_query(property, value), Some("q"), MAX_MATCHES).await?.ids)
}

fn value_query(property: &str, value: &str) -> String {
    let values = [value.to_string(), value.to_lowercase(), value.to_uppercase()].map(|v| literal(&v)).join(" ");
    format!("SELECT DISTINCT ?q {{ VALUES ?value {{ {values} }} ?q wdt:{property} ?value }}")
}

/// Where to send the user for `item`: Wikidata; a tool; or the item's page
/// on `project` (e.g. `enwiki`), if it has one.
async fn target(state: &AppState, item: &str, project: &str) -> anyhow::Result<String> {
    let wikidata = format!("https://www.wikidata.org/wiki/{item}");
    let project = project.trim().to_lowercase();
    if project.is_empty() {
        return Ok(wikidata);
    }
    if let Some(pattern) = tools(state).await?.get(&project) {
        return Ok(pattern.replace("$1", item));
    }
    let Ok(wiki) = Wiki::from_dbname(&project) else { return Ok(wikidata) };
    let entity = wikidata::entity(&state.http, item, "sitelinks").await?;
    Ok(match entity.as_ref().and_then(|e| e["sitelinks"][&project]["title"].as_str()) {
        Some(title) => wiki.page_url(title),
        None => wikidata,
    })
}

async fn tools(state: &AppState) -> anyhow::Result<Arc<BTreeMap<String, String>>> {
    TOOLS.get_or_fetch(async || tool_patterns(&state.http).await).await
}

/// The URL patterns of Q43649390, by tool name. The name was the shortest
/// English name of the "part of" (P361) qualifier, which the statements no
/// longer have; now it is taken from the host name, as in `reasonator`.
async fn tool_patterns(http: &reqwest::Client) -> anyhow::Result<BTreeMap<String, String>> {
    let Some(item) = wikidata::entity(http, TOOLS_ITEM, "claims").await? else { return Ok(BTreeMap::new()) };
    let statements: Vec<&Value> = item["claims"]["P3303"].as_array().into_iter().flatten().collect();
    let parts: Vec<String> = statements
        .iter()
        .flat_map(|s| s["qualifiers"]["P361"].as_array().into_iter().flatten())
        .filter_map(|q| q["datavalue"]["value"]["id"].as_str().map(String::from))
        .collect();
    let names = wikidata::entities(http, &parts, "labels|aliases").await?;
    let mut tools = BTreeMap::new();
    for statement in statements {
        let Some(pattern) = statement["mainsnak"]["datavalue"]["value"].as_str() else { continue };
        if !(pattern.starts_with("https://") || pattern.starts_with("http://")) {
            continue;
        }
        let named_parts: Vec<String> = statement["qualifiers"]["P361"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|q| names.get(q["datavalue"]["value"]["id"].as_str()?).and_then(shortest_english_name))
            .collect();
        for name in named_parts.into_iter().chain(host_name(pattern)) {
            tools.entry(name).or_insert_with(|| pattern.to_string());
        }
    }
    Ok(tools)
}

/// The shortest English label or alias, lower case, spaces as underscores.
fn shortest_english_name(entity: &Value) -> Option<String> {
    let label = entity["labels"]["en"]["value"].as_str();
    let aliases = entity["aliases"]["en"].as_array().into_iter().flatten().filter_map(|a| a["value"].as_str());
    let name = label.into_iter().chain(aliases).min_by_key(|n| n.len())?;
    Some(name.trim().to_lowercase().replace(' ', "_"))
}

/// `reasonator` for `https://reasonator.toolforge.org/?q=$1`; `wikidata` for `https://www.wikidata.org/…`.
fn host_name(url: &str) -> Option<String> {
    let host = url.split_once("://")?.1.split(['/', '?', '#']).next()?;
    let mut labels = host.split('.').filter(|l| *l != "www");
    let name = labels.next()?;
    labels.next()?;
    Some(name.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;
    use serde_json::json;

    #[test]
    fn quick_lookups() {
        assert_eq!(parse_quick("VIAF:12307054"), Some(("viaf".into(), "12307054".into())));
        assert_eq!(parse_quick("GND_ID: \"118540238\""), Some(("gnd".into(), "118540238".into())));
        assert_eq!(parse_quick("VIAF"), None);
        assert_eq!(parse_quick("VIAF:"), None);
    }

    #[test]
    fn properties_by_name() {
        let properties = [("P214", "viaf cluster id"), ("P227", "gnd id"), ("P1", "gnd format"), ("P2", "isni")]
            .map(|(p, l)| (p.to_string(), l.to_string()));
        assert_eq!(property_named(&properties, "viaf"), Some("P214"));
        assert_eq!(property_named(&properties, "gnd"), Some("P227"));
        assert_eq!(property_named(&properties, "is"), Some("P2"));
        assert_eq!(property_named(&properties, "x"), None);
    }

    #[test]
    fn value_queries() {
        assert_eq!(
            value_query("P214", "a\"B"),
            r#"SELECT DISTINCT ?q { VALUES ?value { "a\"B" "a\"b" "A\"B" } ?q wdt:P214 ?value }"#
        );
    }

    #[test]
    fn tool_names() {
        assert_eq!(host_name("https://reasonator.toolforge.org/?q=$1").as_deref(), Some("reasonator"));
        assert_eq!(host_name("https://www.wikidata.org/wiki/Special:Nearby#/page/$1").as_deref(), Some("wikidata"));
        assert_eq!(host_name("https://viaf.org/processed/WKP|$1").as_deref(), Some("viaf"));
        assert_eq!(host_name("https://localhost/$1"), None);
        let entity =
            json!({"labels": {"en": {"value": "Reasonator tool"}}, "aliases": {"en": [{"value": "Reason Ator"}]}});
        assert_eq!(shortest_english_name(&entity).as_deref(), Some("reason_ator"));
    }

    #[tokio::test]
    async fn bad_property() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(get(dir.path(), "/resolver.php?prop=xyz&value=1").await.status.as_u16(), 400);
    }
}
