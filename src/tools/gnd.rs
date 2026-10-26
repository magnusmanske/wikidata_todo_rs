//! `gnd.php?action=get_qs&gnd=…` (or `&item=Q…`): QuickStatements commands
//! that create or complete a Wikidata item from a GND authority record: name,
//! description, dates, gender, places, occupations and relatives, each with
//! a reference to the record; what the item already has is left out.
//!
//! Records come from lobid.org as JSON: the DNB's MARC export, which the old
//! tool read, refuses automated requests.

use crate::sparql;
use crate::state::AppState;
use crate::web::{AppError, Params, api_json};
use crate::wikidata;
use anyhow::Context;
use axum::extract::State;
use axum::response::Response;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

const LABEL_LANGUAGES: &[&str] = &["de", "en", "fr", "it", "es", "nl"];
/// "Deutsche Nationalbibliothek", for references.
const DNB: &str = "Q23833686";
const MALE: &str = "Q6581097";
const FEMALE: &str = "Q6581072";
const HUMAN: &str = "Q5";
/// lobid fields linking to other GND records, and their properties; parents
/// get P22 or P25 by their gender on Wikidata.
const LINKS: &[(&str, &str)] = &[
    ("placeOfBirth", "P19"),
    ("placeOfDeath", "P20"),
    ("professionOrOccupation", "P106"),
    ("fieldOfStudy", "P101"),
    ("placeOfActivity", "P937"),
    ("hasParent", PARENT),
    ("hasChild", "P40"),
    ("hasSibling", "P3373"),
    ("hasSpouse", "P26"),
];
const PARENT: &str = "parent";
/// Items the GND search finds that are better replaced: "Parson" by "pastor".
const REPLACED: &[(&str, &str)] = &[("Q955464", "Q152002")];
/// Long names and descriptions are cut to this many characters.
const MAX_TEXT: usize = 250;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    action: String,
    #[serde(default)]
    item: String,
    #[serde(default)]
    gnd: String,
    callback: Option<String>,
}

#[derive(Debug, Serialize)]
struct Answer {
    #[serde(skip_serializing_if = "Option::is_none")]
    qs: Option<Vec<String>>,
    status: String,
}

/// Why there are no commands.
enum Failure {
    /// Shown as the status, as before.
    Status(String),
    Upstream(anyhow::Error),
}

impl From<anyhow::Error> for Failure {
    fn from(e: anyhow::Error) -> Self {
        Self::Upstream(e)
    }
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let result = match (request.action.as_str(), request.item.trim(), request.gnd.trim()) {
        ("get_qs", "", "") => Err(Failure::Status("item or gnd required".to_string())),
        ("get_qs", item, gnd) => commands(&state, item, gnd).await,
        _ => Err(Failure::Status("Missing or invalid action".to_string())),
    };
    let answer = match result {
        Ok(qs) => Answer { qs: Some(qs), status: "OK".to_string() },
        Err(Failure::Status(status)) => Answer { qs: None, status },
        Err(Failure::Upstream(e)) => return Err(AppError::Upstream(e)),
    };
    Ok(api_json(answer, request.callback))
}

/// The commands for the item (if given) or the GND record.
async fn commands(state: &AppState, item: &str, gnd: &str) -> Result<Vec<String>, Failure> {
    let status = |s: String| Failure::Status(s);
    let mut entity = None;
    let gnd = match item.is_empty() {
        true => gnd_id(gnd).ok_or_else(|| status(format!("Invalid GND ID: {gnd}")))?,
        false => {
            let q = wikidata::parse_id(item, 'Q').ok_or_else(|| status(format!("Invalid item: {item}")))?;
            let loaded = wikidata::entity(&state.http, &q, "labels|descriptions|claims").await?;
            let gnd = loaded.as_ref().and_then(|e| wikidata::first_string(e, "P227")).and_then(gnd_id);
            entity = loaded;
            gnd.ok_or_else(|| status(format!("Item {q} does not have a GND ID")))?
        }
    };
    let record = Record::parse(
        &gnd,
        &lobid(&state.http, &gnd).await?.ok_or_else(|| status(format!("No GND record {gnd} at lobid.org")))?,
    );
    let mut gnds: Vec<String> = record.links.iter().map(|(_, id)| id.clone()).collect();
    gnds.push(gnd.clone());
    let (people, countries) =
        tokio::try_join!(items_with(state, "P227", &gnds, true), items_with(state, "P297", &record.countries, false))?;
    if entity.is_none() {
        let items: Vec<&String> = people.get(&gnd).into_iter().flatten().map(|(q, _)| q).collect();
        match items.as_slice() {
            [] => {}
            [q] => {
                let loaded = wikidata::entity(&state.http, q, "labels|descriptions|claims").await?;
                entity = Some(loaded.ok_or_else(|| status(format!("Failed to load item {q}")))?);
            }
            items => {
                let list = items.iter().map(|q| q.as_str()).collect::<Vec<_>>().join(",");
                return Err(status(format!("MULTIPLE ITEMS WITH THAT GND ({list}), NOT PROCESSING!")));
            }
        }
    }
    let statements = record.statements(&people, &countries);
    let target = entity.as_ref().map(|e| Target { entity: e, gnd: &gnd });
    let today = chrono::Utc::now().format("+%Y-%m-%dT00:00:00Z/11").to_string();
    Ok(record.commands(target.as_ref(), statements, &today))
}

/// The record as lobid.org has it; `None` if there is no such record.
async fn lobid(http: &reqwest::Client, gnd: &str) -> anyhow::Result<Option<Value>> {
    let url = format!("https://lobid.org/gnd/{gnd}.json");
    let response = http.get(&url).send().await.with_context(|| format!("cannot load {url}"))?;
    // lobid.org answers unknown IDs with an error.
    if [reqwest::StatusCode::NOT_FOUND, reqwest::StatusCode::INTERNAL_SERVER_ERROR].contains(&response.status()) {
        return Ok(None);
    }
    Ok(Some(response.error_for_status()?.json().await.with_context(|| format!("{url} is not JSON"))?))
}

/// The items with each of `values` for `property`, and with `gender`, their gender.
async fn items_with(
    state: &AppState,
    property: &str,
    values: &[String],
    gender: bool,
) -> anyhow::Result<HashMap<String, Vec<(String, Option<String>)>>> {
    let mut found: HashMap<String, Vec<(String, Option<String>)>> = HashMap::new();
    if values.is_empty() {
        return Ok(found);
    }
    let values: Vec<String> = values.iter().map(|v| sparql::literal(v)).collect();
    let optional = if gender { "OPTIONAL { ?item wdt:P21 ?gender }" } else { "" };
    let query = format!(
        "SELECT DISTINCT ?value ?item ?gender {{ VALUES ?value {{ {} }} ?item wdt:{property} ?value . {optional} }}",
        values.join(" ")
    );
    let json = state.sparql.json(&query).await?;
    for row in sparql::bindings(&json) {
        let (Some(value), Some(item)) =
            (row["value"]["value"].as_str(), row["item"]["value"].as_str().and_then(sparql::entity_id))
        else {
            continue;
        };
        let gender = row["gender"]["value"].as_str().and_then(sparql::entity_id).map(str::to_string);
        found.entry(value.to_string()).or_default().push((item.to_string(), gender));
    }
    Ok(found)
}

/// The one item for a value, if there is exactly one.
fn unique<'a>(
    found: &'a HashMap<String, Vec<(String, Option<String>)>>,
    value: &str,
) -> Option<&'a (String, Option<String>)> {
    let items = found.get(value)?;
    let first = items.first()?;
    items.iter().all(|(q, _)| *q == first.0).then_some(first)
}

/// What a GND record says, in Wikidata's terms.
#[derive(Debug, Default, PartialEq)]
struct Record {
    id: String,
    name: String,
    description: String,
    /// Property, QuickStatements date.
    dates: Vec<(&'static str, String)>,
    /// Statements whose values need no lookup: property, value.
    statements: Vec<(&'static str, String)>,
    /// Linked GND records: property (or [`PARENT`]), GND ID.
    links: Vec<(&'static str, String)>,
    /// ISO 3166 codes of the countries.
    countries: Vec<String>,
}

impl Record {
    fn parse(id: &str, json: &Value) -> Self {
        let mut record = Self { id: id.to_string(), ..Default::default() };
        record.name = json["preferredName"].as_str().map(parse_name).unwrap_or_default();
        record.description =
            strings(json, "biographicalOrHistoricalInformation").first().map_or("", |s| s.trim()).to_string();
        let first = |field: &str| strings(json, field).first().and_then(|s| gnd_date(s));
        record.dates.extend(first("dateOfBirth").map(|d| ("P569", d)));
        record.dates.extend(first("dateOfDeath").map(|d| ("P570", d)));
        if let Some((from, to)) = strings(json, "periodOfActivity").first().and_then(|p| p.split_once('-')) {
            record.dates.extend(gnd_date(from.trim()).map(|d| ("P2031", d)));
            record.dates.extend(gnd_date(to.trim()).map(|d| ("P2032", d)));
        }
        if strings(json, "type").contains(&"DifferentiatedPerson") {
            record.statements.push(("P31", HUMAN.to_string()));
        }
        for gender in strings(json, "gender") {
            match gender.rsplit('#').next() {
                Some("male") => record.statements.push(("P21", MALE.to_string())),
                Some("female") => record.statements.push(("P21", FEMALE.to_string())),
                _ => {}
            }
        }
        for (field, property) in LINKS {
            record.links.extend(strings(json, field).into_iter().filter_map(gnd_from_url).map(|id| (*property, id)));
        }
        record.statements.extend(strings(json, "homepage").into_iter().map(|url| ("P856", format!("\"{url}\""))));
        record.countries = strings(json, "geographicAreaCode")
            .into_iter()
            .filter_map(|code| code.rsplit('#').next()?.strip_prefix("XA-"))
            .map(str::to_string)
            .collect();
        record
    }

    /// The statements with item values: countries and linked records with
    /// one item each, as `(property, value)`.
    fn statements(
        &self,
        people: &HashMap<String, Vec<(String, Option<String>)>>,
        countries: &HashMap<String, Vec<(String, Option<String>)>>,
    ) -> Vec<(String, String)> {
        let mut statements: Vec<(String, String)> =
            self.countries.iter().filter_map(|c| Some(("P27".to_string(), unique(countries, c)?.0.clone()))).collect();
        statements.extend(self.statements.iter().map(|(p, v)| (p.to_string(), v.clone())));
        for (property, gnd) in &self.links {
            let Some((item, gender)) = unique(people, gnd) else { continue };
            let item = REPLACED.iter().find(|(old, _)| old == item).map_or(item.as_str(), |(_, new)| new);
            let property = match (*property, gender.as_deref()) {
                (PARENT, Some(MALE)) => "P22",
                (PARENT, Some(FEMALE)) => "P25",
                (PARENT, _) => continue,
                (property, _) => property,
            };
            statements.push((property.to_string(), item.to_string()));
        }
        statements
    }

    /// The QuickStatements commands, tab-separated: for `target` (or a new
    /// item), and only what it lacks.
    fn commands(&self, target: Option<&Target>, statements: Vec<(String, String)>, today: &str) -> Vec<String> {
        let item = target.and_then(|t| t.entity["id"].as_str()).unwrap_or("LAST");
        let mut commands: Vec<(String, String)> = vec![];
        if !self.name.is_empty() {
            let missing = LABEL_LANGUAGES.iter().filter(|l| !target.is_some_and(|t| t.has_label(l)));
            commands.extend(missing.map(|l| (format!("L{l}"), quote(&self.name))));
        }
        if !self.description.is_empty() && !target.is_some_and(|t| t.has_term("descriptions", "de")) {
            commands.push(("Dde".to_string(), quote(&self.description)));
        }
        commands.push(("P227".to_string(), quote(&self.id)));
        commands.extend(statements);
        for (property, date) in &self.dates {
            if !target.is_some_and(|t| t.has_property(property)) {
                commands.push((property.to_string(), date.clone()));
            }
        }
        let mut seen = HashSet::new();
        let mut lines: Vec<String> = match target {
            None => vec!["CREATE".to_string()],
            Some(_) => vec![],
        };
        for (key, value) in commands {
            if !seen.insert((key.clone(), value.clone())) || target.is_some_and(|t| t.has(&key, &value)) {
                continue;
            }
            let mut line = format!("{item}\t{key}\t{value}");
            if key.starts_with('P') && key != "P227" {
                line += &format!("\tS227\t{}\tS248\t{DNB}\tS813\t{today}", quote(&self.id));
            }
            lines.push(line);
        }
        lines
    }
}

/// The item a record's commands are for.
struct Target<'a> {
    entity: &'a Value,
    gnd: &'a str,
}

impl Target<'_> {
    fn has_term(&self, field: &str, language: &str) -> bool {
        self.entity[field][language]["value"].is_string()
    }

    /// A `mul` label counts for all languages.
    fn has_label(&self, language: &str) -> bool {
        self.has_term("labels", language) || self.has_term("labels", "mul")
    }

    fn has_property(&self, property: &str) -> bool {
        self.claims(property).next().is_some()
    }

    fn claims(&self, property: &str) -> impl Iterator<Item = &Value> {
        self.entity["claims"][property].as_array().into_iter().flatten()
    }

    /// Whether the command adds nothing: a term the item has (or a cut
    /// one), or a statement it has with a reference to the record.
    fn has(&self, key: &str, value: &str) -> bool {
        let (kind, rest) = key.split_at_checked(1).unwrap_or_default();
        match kind {
            "L" | "D" if value.contains("...") => true,
            "L" => self.has_label(rest),
            "D" => self.has_term("descriptions", rest),
            "P" => {
                let value = value.trim_matches('"');
                self.claims(key).any(|claim| {
                    let datavalue = &claim["mainsnak"]["datavalue"]["value"];
                    match datavalue {
                        Value::String(s) => s == value,
                        _ if datavalue["id"].as_str() == Some(value) => self.cites_record(claim),
                        _ => {
                            let time = format!(
                                "{}/{}",
                                datavalue["time"].as_str().unwrap_or_default(),
                                datavalue["precision"]
                            );
                            time == value && self.cites_record(claim)
                        }
                    }
                })
            }
            _ => false,
        }
    }

    fn cites_record(&self, claim: &Value) -> bool {
        claim["references"].as_array().into_iter().flatten().any(|reference| {
            reference["snaks"]["P227"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|snak| snak["datavalue"]["value"] == self.gnd)
        })
    }
}

/// The strings of a lobid field: plain ones, or the `id`s of objects.
fn strings<'a>(json: &'a Value, field: &str) -> Vec<&'a str> {
    let values = match &json[field] {
        Value::Array(values) => values.iter().collect(),
        value => vec![value],
    };
    values.into_iter().filter_map(|v| v.as_str().or_else(|| v["id"].as_str())).collect()
}

/// A GND ID such as `118540238` or `4018118-2`.
fn gnd_id(input: &str) -> Option<String> {
    let id = input.trim().to_uppercase();
    (!id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit() || b == b'X' || b == b'-')).then_some(id)
}

/// The GND ID in a `https://d-nb.info/gnd/…` URL.
fn gnd_from_url(url: &str) -> Option<String> {
    let id = url.strip_prefix("https://d-nb.info/gnd/").or_else(|| url.strip_prefix("http://d-nb.info/gnd/"))?;
    gnd_id(id.split(['?', '#']).next()?)
}

/// `Goethe, Johann Wolfgang von` → `Johann Wolfgang von Goethe`;
/// `Holbein, Hans, der Jüngere` → `Hans Holbein der Jüngere`.
fn parse_name(name: &str) -> String {
    static EPITHET: LazyLock<Regex> = LazyLock::new(|| regex(r"^(.+?), (.+), (der|die) ([^,]+)$"));
    static INVERTED: LazyLock<Regex> = LazyLock::new(|| regex(r"^(.+?), (.+)$"));
    static HYPHEN: LazyLock<Regex> = LazyLock::new(|| regex(r"- +"));
    static SPACES: LazyLock<Regex> = LazyLock::new(|| regex(r"\s{2,}"));
    let name = match EPITHET.is_match(name) {
        true => EPITHET.replace(name, "$2 $1 $3 $4"),
        false => INVERTED.replace(name, "$2 $1"),
    };
    let name = HYPHEN.replace_all(&name, "-");
    SPACES.replace_all(&name, " ").trim().to_string()
}

/// A GND date (`28.08.1749`, `08.1749`, `1749`, or ISO `1749-08-28`) for QuickStatements.
fn gnd_date(date: &str) -> Option<String> {
    static GERMAN: LazyLock<Regex> = LazyLock::new(|| regex(r"^(?:(?:(\d{1,2})\.)?(\d{1,2})\.)?(\d{3,4})$"));
    static ISO: LazyLock<Regex> = LazyLock::new(|| regex(r"^(\d{3,4})(?:-(\d{1,2})(?:-(\d{1,2}))?)?$"));
    let (year, month, day) = match (GERMAN.captures(date), ISO.captures(date)) {
        (Some(c), _) => (c.get(3), c.get(2), c.get(1)),
        (None, Some(c)) => (c.get(1), c.get(2), c.get(3)),
        (None, None) => return None,
    };
    let number = |m: Option<regex::Match>| m.and_then(|m| m.as_str().parse::<u32>().ok());
    let year = number(year)?;
    Some(match (number(month), number(day)) {
        (Some(month), Some(day)) => format!("+{year:04}-{month:02}-{day:02}T00:00:00Z/11"),
        (Some(month), None) => format!("+{year:04}-{month:02}-01T00:00:00Z/10"),
        _ => format!("+{year:04}-01-01T00:00:00Z/09"),
    })
}

/// A string value, trimmed and cut, in quotes.
fn quote(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(MAX_TEXT) {
        Some((cut, _)) => format!("\"{}...\"", &s[..cut]),
        None => format!("\"{s}\""),
    }
}

fn regex(pattern: &str) -> Regex {
    // The patterns are constants, covered by the tests.
    #[allow(clippy::expect_used)]
    Regex::new(pattern).expect("valid regex")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;
    use serde_json::json;

    #[test]
    fn names_dates_and_ids() {
        assert_eq!(parse_name("Goethe, Johann Wolfgang von"), "Johann Wolfgang von Goethe");
        assert_eq!(parse_name("Holbein, Hans, der Jüngere"), "Hans Holbein der Jüngere");
        assert_eq!(parse_name("Müller- Lüdenscheidt,  Otto"), "Otto Müller-Lüdenscheidt");
        assert_eq!(gnd_date("28.8.1749").as_deref(), Some("+1749-08-28T00:00:00Z/11"));
        assert_eq!(gnd_date("08.1749").as_deref(), Some("+1749-08-01T00:00:00Z/10"));
        assert_eq!(gnd_date("1749").as_deref(), Some("+1749-01-01T00:00:00Z/09"));
        assert_eq!(gnd_date("1749-08-28").as_deref(), Some("+1749-08-28T00:00:00Z/11"));
        assert_eq!(gnd_date("ca. 1749"), None);
        assert_eq!(gnd_id(" 4018118-2 ").as_deref(), Some("4018118-2"));
        assert_eq!(gnd_id("11854022x").as_deref(), Some("11854022X"));
        assert_eq!(gnd_id("1/../x"), None);
        assert_eq!(gnd_from_url("https://d-nb.info/gnd/4065105-8").as_deref(), Some("4065105-8"));
        assert_eq!(quote(&"ä".repeat(300)), format!("\"{}...\"", "ä".repeat(250)));
    }

    fn goethe() -> Record {
        let json = json!({
            "preferredName": "Goethe, Johann Wolfgang von",
            "biographicalOrHistoricalInformation": ["Staatsmann, Geheimrat"],
            "dateOfBirth": ["1749-08-28"], "dateOfDeath": ["1832-03-22"],
            "type": ["AuthorityResource", "DifferentiatedPerson", "Person"],
            "gender": [{"id": "https://d-nb.info/standards/vocab/gnd/gender#male"}],
            "placeOfDeath": [{"id": "https://d-nb.info/gnd/4065105-8", "label": "Weimar"}],
            "hasParent": [{"id": "https://d-nb.info/gnd/118540246"}, {"id": "https://d-nb.info/gnd/1"}],
            "geographicAreaCode": [{"id": "https://d-nb.info/standards/vocab/gnd/geographic-area-code#XA-DE"}],
        });
        Record::parse("118540238", &json)
    }

    #[test]
    fn records() {
        let record = goethe();
        assert_eq!(record.name, "Johann Wolfgang von Goethe");
        assert_eq!(
            record.dates,
            [("P569", "+1749-08-28T00:00:00Z/11".to_string()), ("P570", "+1832-03-22T00:00:00Z/11".to_string())]
        );
        assert_eq!(record.countries, ["DE"]);
        let found = |pairs: &[(&str, &str, Option<&str>)]| {
            let mut map: HashMap<String, Vec<(String, Option<String>)>> = HashMap::new();
            for (value, item, gender) in pairs {
                map.entry(value.to_string()).or_default().push((item.to_string(), gender.map(str::to_string)));
            }
            map
        };
        let people = found(&[("4065105-8", "Q1", None), ("118540246", "Q2", Some(FEMALE)), ("1", "Q3", None)]);
        let statements = record.statements(&people, &found(&[("DE", "Q183", None)]));
        let pairs: Vec<(&str, &str)> = statements.iter().map(|(p, v)| (p.as_str(), v.as_str())).collect();
        assert_eq!(pairs, [("P27", "Q183"), ("P31", "Q5"), ("P21", MALE), ("P20", "Q1"), ("P25", "Q2")]);
        let new = record.commands(None, statements.clone(), "+2026-01-01T00:00:00Z/11");
        assert_eq!(new[0], "CREATE");
        assert_eq!(new[1], "LAST\tLde\t\"Johann Wolfgang von Goethe\"");
        assert!(new.contains(&"LAST\tP227\t\"118540238\"".to_string()));
        assert!(new.contains(
            &"LAST\tP20\tQ1\tS227\t\"118540238\"\tS248\tQ23833686\tS813\t+2026-01-01T00:00:00Z/11".to_string()
        ));

        let gnd_reference = json!([{"snaks": {"P227": [{"datavalue": {"value": "118540238"}}]}}]);
        let entity = json!({"id": "Q5879", "labels": {"mul": {"value": "Goethe"}}, "descriptions": {},
            "claims": {
                "P227": [{"mainsnak": {"datavalue": {"value": "118540238"}}}],
                "P20": [{"mainsnak": {"datavalue": {"value": {"id": "Q1"}}}, "references": gnd_reference}],
                "P21": [{"mainsnak": {"datavalue": {"value": {"id": MALE}}}}],
                "P569": [{"mainsnak": {"datavalue": {"value": {"time": "+1749-08-28T00:00:00Z", "precision": 11}}}}],
        }});
        let target = Target { entity: &entity, gnd: "118540238" };
        let existing = record.commands(Some(&target), statements, "+2026-01-01T00:00:00Z/11");
        let keys: Vec<&str> = existing.iter().filter_map(|l| l.split('\t').nth(1)).collect();
        assert_eq!(keys, ["Dde", "P27", "P31", "P21", "P25", "P570"]);
        assert!(existing.iter().all(|l| l.starts_with("Q5879\t")));
    }

    #[tokio::test]
    async fn bad_requests() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let body = |uri: &'static str| async move { get(path, uri).await.body };
        assert_eq!(body("/gnd.php").await, r#"{"status":"Missing or invalid action"}"#);
        assert_eq!(body("/gnd.php?action=get_qs&callback=cb").await, r#"cb({"status":"item or gnd required"});"#);
        assert_eq!(body("/gnd.php?action=get_qs&gnd=a/b").await, r#"{"status":"Invalid GND ID: a/b"}"#);
    }
}
