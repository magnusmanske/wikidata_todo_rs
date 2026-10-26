//! `personendaten.php`: a search for people (in German, as the Persondata
//! search of the German Wikipedia it imitates): by name and description,
//! birth and death date ranges and places, citizenship and gender.
//!
//! Name and description go to Wikidata's search (which also takes care of
//! citizenship and gender); the rest, or all if there is no text, to SPARQL.

use crate::cache::TimedCache;
use crate::mediawiki;
use crate::sparql::{self, Sparql};
use crate::state::AppState;
use crate::terms::{self, TermType};
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, is_language_code};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::{Datelike, Months, NaiveDate};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const MAX_RESULTS: usize = 500;
/// Items whose label is a place name, at most.
const MAX_PLACES: usize = 500;
const MALE: &str = "Q6581097";
const FEMALE: &str = "Q6581072";
const MONTHS: &[&str] = &[
    "Januar",
    "Februar",
    "März",
    "April",
    "Mai",
    "Juni",
    "Juli",
    "August",
    "September",
    "Oktober",
    "November",
    "Dezember",
];

/// The kinds of date filters, as the form numbers them.
const BIRTH_ARTS: &[(&str, &str)] = &[("1", "am"), ("2", "vor"), ("3", "nach"), ("4", "zwischen"), ("5", "unbekannt")];
const DEATH_ARTS: &[(&str, &str)] = &[("1", "am"), ("2", "vor"), ("3", "nach"), ("4", "zwischen"), ("5", "lebt noch")];

/// Countries, as `(item, ISO 3166-1 alpha-3 code)`.
static COUNTRIES: TimedCache<Vec<(String, String)>> = TimedCache::new(Duration::from_secs(24 * 3600));

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Request {
    lang: String,
    name: String,
    desc: String,
    staat: String,
    sex: String,
    geb_art: String,
    geb_tag1: String,
    geb_monat1: String,
    geb_jahr1: String,
    geb_tag2: String,
    geb_monat2: String,
    geb_jahr2: String,
    geb_ort: String,
    st_art: String,
    st_tag1: String,
    st_monat1: String,
    st_jahr1: String,
    st_tag2: String,
    st_monat2: String,
    st_jahr2: String,
    st_ort: String,
}

/// `(year, month, day)` as entered; year 0 if none.
type PartialDate = (i32, Option<u32>, Option<u32>);

/// A date filter; ranges include their start, not their end.
#[derive(Debug, PartialEq)]
enum DateFilter {
    Range(Option<NaiveDate>, Option<NaiveDate>),
    /// A day (or month) of any year.
    Anniversary(u32, Option<u32>),
    /// No such date on Wikidata ("unbekannt", "lebt noch").
    Unknown,
}

/// What the form asks for, parsed.
#[derive(Debug, Default)]
struct Criteria {
    text: String,
    country: Option<String>,
    gender: Option<&'static str>,
    born: Option<DateFilter>,
    died: Option<DateFilter>,
    birth_places: Option<Vec<String>>,
    death_places: Option<Vec<String>>,
}

struct Person {
    item: String,
    label: String,
    description: String,
    born: String,
    birth_place: String,
    died: String,
    death_place: String,
}

#[derive(Template)]
#[template(path = "tools/personendaten.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    countries: Vec<(String, String)>,
    months: &'static [&'static str],
    birth_arts: &'static [(&'static str, &'static str)],
    death_arts: &'static [(&'static str, &'static str)],
    results: Option<Vec<Person>>,
    max_results: usize,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let language = match request.lang.trim() {
        l if is_language_code(l) => l.to_string(),
        _ => "de".to_string(),
    };
    let languages = terms::languages_with(&language, &["mul", "de", "en"]);
    let codes = COUNTRIES.get_or_fetch(async || countries(&state.sparql).await).await?;
    let ids: Vec<String> = codes.iter().map(|(q, _)| q.clone()).collect();
    let labels = terms::labels(&state.replicas, &ids, &languages).await?;
    let mut countries: Vec<(String, String)> =
        codes.iter().map(|(q, _)| (q.clone(), labels.get(q).cloned().unwrap_or_else(|| q.clone()))).collect();
    countries.sort_by(|a, b| a.1.cmp(&b.1));
    let mut page = Page {
        title: "Personendaten",
        request: &request,
        countries,
        months: MONTHS,
        birth_arts: BIRTH_ARTS,
        death_arts: DEATH_ARTS,
        results: None,
        max_results: MAX_RESULTS,
        error: None,
    };
    let criteria = criteria(&state, &request, &codes).await?;
    if let Some(criteria) = criteria {
        match search(&state, &criteria).await? {
            Some(items) => page.results = Some(people(&state, &items, &languages).await?),
            None => page.error = Some("Bitte noch etwas genauer suchen.".to_string()),
        }
    }
    render(&page)
}

/// The criteria; `None` if there are none.
async fn criteria(
    state: &AppState,
    request: &Request,
    countries: &[(String, String)],
) -> Result<Option<Criteria>, AppError> {
    let bad = |m: &str| AppError::BadRequest(m.to_string());
    let country = match request.staat.trim() {
        "" | "clearpixel" => None,
        s => Some(
            crate::wikidata::parse_id(s, 'Q')
                .or_else(|| countries.iter().find(|(_, code)| code.eq_ignore_ascii_case(s)).map(|(q, _)| q.clone()))
                .ok_or_else(|| bad("Unbekannter Staat."))?,
        ),
    };
    let gender = match request.sex.trim() {
        "1" => Some(MALE),
        "2" => Some(FEMALE),
        _ => None,
    };
    let date = |art: &str, d1: [&str; 3], d2: [&str; 3]| date_filter(art, d1, d2).map_err(bad);
    let born = date(
        &request.geb_art,
        [&request.geb_tag1, &request.geb_monat1, &request.geb_jahr1],
        [&request.geb_tag2, &request.geb_monat2, &request.geb_jahr2],
    )?;
    let died = date(
        &request.st_art,
        [&request.st_tag1, &request.st_monat1, &request.st_jahr1],
        [&request.st_tag2, &request.st_monat2, &request.st_jahr2],
    )?;
    let criteria = Criteria {
        text: format!("{} {}", request.name.trim(), request.desc.trim()).trim().to_string(),
        country,
        gender,
        born,
        died,
        birth_places: places(state, &request.geb_ort).await?,
        death_places: places(state, &request.st_ort).await?,
    };
    let any = !criteria.text.is_empty()
        || criteria.country.is_some()
        || criteria.gender.is_some()
        || criteria.born.is_some()
        || criteria.died.is_some()
        || criteria.birth_places.is_some()
        || criteria.death_places.is_some();
    Ok(any.then_some(criteria))
}

/// Items labelled exactly `name`, in any language.
async fn places(state: &AppState, name: &str) -> anyhow::Result<Option<Vec<String>>> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let found = terms::items_with_texts(&state.replicas, &[name.to_string()], &[TermType::Label], None).await?;
    let mut items: Vec<String> = found.into_iter().map(|(q, _)| q).collect();
    items.sort();
    items.dedup();
    items.truncate(MAX_PLACES);
    Ok(Some(items))
}

/// A date filter from the form: `art` 1 to 5 is on, before, after, between, unknown.
fn date_filter(art: &str, first: [&str; 3], second: [&str; 3]) -> Result<Option<DateFilter>, &'static str> {
    let (first, second) = (partial_date(first)?, partial_date(second)?);
    let period = |d: Option<PartialDate>| d.and_then(|(y, m, d)| y.ne(&0).then_some((y, m, d)));
    Ok(match art.trim() {
        "5" => Some(DateFilter::Unknown),
        "2" => period(first)
            .map(|d| period_bounds(d).map(|(start, _)| DateFilter::Range(None, Some(start))))
            .transpose()?,
        "3" => {
            period(first).map(|d| period_bounds(d).map(|(_, end)| DateFilter::Range(Some(end), None))).transpose()?
        }
        "4" => {
            let from = period(first).map(period_bounds).transpose()?.map(|(start, _)| start);
            let to = period(second).map(period_bounds).transpose()?.map(|(_, end)| end);
            (from.is_some() || to.is_some()).then_some(DateFilter::Range(from, to))
        }
        _ => match first {
            Some((0, Some(month), day)) => Some(DateFilter::Anniversary(month, day)),
            Some((0, None, Some(_))) => return Err("Bitte einen Monat oder ein Jahr angeben."),
            Some(d) => Some(period_bounds(d).map(|(start, end)| DateFilter::Range(Some(start), Some(end)))?),
            None => None,
        },
    })
}

/// A date from the form; `None` if nothing is given.
fn partial_date([day, month, year]: [&str; 3]) -> Result<Option<PartialDate>, &'static str> {
    let number = |s: &str| -> Result<Option<u32>, &'static str> {
        match s.trim() {
            "" | "0" => Ok(None),
            s => s.parse().map(Some).map_err(|_| "Ungültiges Datum."),
        }
    };
    let (day, month, year) = (number(day)?, number(month)?, number(year)?);
    if year.is_some_and(|y| y > 9999) || month.is_some_and(|m| m > 12) || day.is_some_and(|d| d > 31) {
        return Err("Ungültiges Datum.");
    }
    let year = year.and_then(|y| i32::try_from(y).ok()).unwrap_or(0);
    Ok((year != 0 || month.is_some() || day.is_some()).then_some((year, month, day)))
}

/// The first day of a year, month or day, and the first day after it.
fn period_bounds((year, month, day): PartialDate) -> Result<(NaiveDate, NaiveDate), &'static str> {
    let invalid = "Ungültiges Datum.";
    let start = NaiveDate::from_ymd_opt(year, month.unwrap_or(1), month.and(day).unwrap_or(1)).ok_or(invalid)?;
    let end = match (month, day) {
        (Some(_), Some(_)) => start.succ_opt(),
        (Some(_), None) => start.checked_add_months(Months::new(1)),
        _ => start.checked_add_months(Months::new(12)),
    };
    Ok((start, end.ok_or(invalid)?))
}

/// SPARQL for a date filter on `property`, with `var` for the date.
fn date_sparql(filter: &DateFilter, property: &str, var: &str) -> String {
    let literal =
        |d: &NaiveDate| format!("\"{:04}-{:02}-{:02}T00:00:00Z\"^^xsd:dateTime", d.year(), d.month(), d.day());
    let pattern = format!("?item wdt:{property} ?{var} .");
    match filter {
        DateFilter::Unknown => format!("MINUS {{ ?item wdt:{property} [] }}"),
        DateFilter::Range(from, to) => {
            let conditions: Vec<String> =
                [from.map(|d| format!("?{var} >= {}", literal(&d))), to.map(|d| format!("?{var} < {}", literal(&d)))]
                    .into_iter()
                    .flatten()
                    .collect();
            format!("{pattern} FILTER({})", conditions.join(" && "))
        }
        DateFilter::Anniversary(month, day) => match day {
            Some(day) => format!("{pattern} FILTER(MONTH(?{var}) = {month} && DAY(?{var}) = {day})"),
            None => format!("{pattern} FILTER(MONTH(?{var}) = {month})"),
        },
    }
}

/// The SPARQL patterns of the criteria, but the text.
fn patterns(criteria: &Criteria) -> Vec<String> {
    let mut patterns = vec![];
    let values = |items: &[String]| items.iter().map(|q| format!("wd:{q}")).collect::<Vec<_>>().join(" ");
    for (places, property, var) in
        [(&criteria.birth_places, "P19", "birthPlace"), (&criteria.death_places, "P20", "deathPlace")]
    {
        if let Some(places) = places {
            patterns.push(format!("VALUES ?{var} {{ {} }} ?item wdt:{property} ?{var} .", values(places)));
        }
    }
    if let Some(country) = &criteria.country {
        patterns.push(format!("?item wdt:P27 wd:{country} ."));
    }
    if let Some(gender) = criteria.gender {
        patterns.push(format!("?item wdt:P21 wd:{gender} ."));
    }
    for (filter, property, var) in [(&criteria.born, "P569", "born"), (&criteria.died, "P570", "died")] {
        if let Some(filter) = filter {
            patterns.push(date_sparql(filter, property, var));
        }
    }
    patterns
}

/// The people found; `None` if the criteria are too vague.
async fn search(state: &AppState, criteria: &Criteria) -> anyhow::Result<Option<Vec<String>>> {
    if criteria.birth_places.as_ref().is_some_and(Vec::is_empty)
        || criteria.death_places.as_ref().is_some_and(Vec::is_empty)
    {
        return Ok(Some(vec![]));
    }
    let patterns = patterns(criteria);
    if criteria.text.is_empty() {
        // Just humans of one gender, or all with an unknown date, are too many.
        let narrow = criteria.country.is_some()
            || criteria.birth_places.is_some()
            || criteria.death_places.is_some()
            || [&criteria.born, &criteria.died]
                .iter()
                .any(|f| matches!(f, Some(DateFilter::Range(..) | DateFilter::Anniversary(..))));
        if !narrow {
            return Ok(None);
        }
        return Ok(Some(filtered(&state.sparql, "?item wdt:P31 wd:Q5 .", &patterns).await?));
    }
    let found = text_search(&state.http, criteria).await?;
    if found.is_empty() || patterns.is_empty() {
        return Ok(Some(found));
    }
    let values = format!("VALUES ?item {{ wd:{} }}", found.join(" wd:"));
    let matching = filtered(&state.sparql, &values, &patterns).await?;
    // In the order of the search, best match first.
    Ok(Some(found.into_iter().filter(|q| matching.contains(q)).collect()))
}

/// The items of `source` that match `patterns`.
async fn filtered(sparql: &Sparql, source: &str, patterns: &[String]) -> anyhow::Result<Vec<String>> {
    let query = format!("SELECT DISTINCT ?item {{ {source} {} }} LIMIT {MAX_RESULTS}", patterns.join(" "));
    sparql.entities(&query, Some("item")).await
}

/// Humans matching the text, citizenship and gender, by Wikidata's search.
async fn text_search(http: &reqwest::Client, criteria: &Criteria) -> anyhow::Result<Vec<String>> {
    let mut search = format!("{} haswbstatement:P31=Q5", criteria.text);
    if let Some(country) = &criteria.country {
        search += &format!(" haswbstatement:P27={country}");
    }
    if let Some(gender) = criteria.gender {
        search += &format!(" haswbstatement:P21={gender}");
    }
    let limit = MAX_RESULTS.to_string();
    let params = [
        ("action", "query"),
        ("list", "search"),
        ("srsearch", search.as_str()),
        ("srnamespace", "0"),
        ("srlimit", limit.as_str()),
        ("srprop", ""),
    ];
    let json = mediawiki::api(http, &Wiki::wikidata(), &params).await?;
    Ok(json["query"]["search"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["title"].as_str())
        .filter(|t| crate::wikidata::parse_id(t, 'Q').as_deref() == Some(*t))
        .map(String::from)
        .collect())
}

/// Labels, descriptions, and life dates and places of the people.
async fn people(state: &AppState, items: &[String], languages: &[&str]) -> anyhow::Result<Vec<Person>> {
    if items.is_empty() {
        return Ok(vec![]);
    }
    let query = format!(
        "SELECT ?item ?born ?bornPrecision ?died ?diedPrecision ?birthPlace ?deathPlace {{
        VALUES ?item {{ wd:{} }}
        OPTIONAL {{ ?item p:P569/psv:P569 [ wikibase:timeValue ?born ; wikibase:timePrecision ?bornPrecision ] }}
        OPTIONAL {{ ?item p:P570/psv:P570 [ wikibase:timeValue ?died ; wikibase:timePrecision ?diedPrecision ] }}
        OPTIONAL {{ ?item wdt:P19 ?birthPlace }} OPTIONAL {{ ?item wdt:P20 ?deathPlace }} }}",
        items.join(" wd:")
    );
    let json = state.sparql.json(&query).await?;
    let mut details: HashMap<String, &Value> = HashMap::new();
    for row in sparql::bindings(&json) {
        if let Some(item) = row["item"]["value"].as_str().and_then(sparql::entity_id) {
            details.entry(item.to_string()).or_insert(row);
        }
    }
    let place = |row: &Value, var: &str| row[var]["value"].as_str().and_then(sparql::entity_id).map(String::from);
    let mut ids: Vec<String> = items.to_vec();
    ids.extend(details.values().flat_map(|row| [place(row, "birthPlace"), place(row, "deathPlace")]).flatten());
    let terms = best_terms(state, &ids, languages).await?;
    let text = |id: Option<String>, kind: TermType| {
        id.and_then(|id| terms.get(&(id.clone(), kind)).cloned().or((kind == TermType::Label).then_some(id)))
            .unwrap_or_default()
    };
    Ok(items
        .iter()
        .map(|item| {
            let row = details.get(item).copied().unwrap_or(&Value::Null);
            Person {
                label: text(Some(item.clone()), TermType::Label),
                description: text(Some(item.clone()), TermType::Description),
                born: german_date(row, "born"),
                birth_place: text(place(row, "birthPlace"), TermType::Label),
                died: german_date(row, "died"),
                death_place: text(place(row, "deathPlace"), TermType::Label),
                item: item.clone(),
            }
        })
        .collect())
}

/// Labels and descriptions, each in the first of `languages` that has one.
async fn best_terms(
    state: &AppState,
    ids: &[String],
    languages: &[&str],
) -> anyhow::Result<HashMap<(String, TermType), String>> {
    let found = terms::terms(&state.replicas, ids, &[TermType::Label, TermType::Description], languages).await?;
    let rank = |l: &str| languages.iter().position(|x| *x == l).unwrap_or(usize::MAX);
    let mut best: HashMap<(String, TermType), (usize, String)> = HashMap::new();
    for term in found {
        let r = rank(&term.language);
        let key = (term.entity, term.term_type);
        if best.get(&key).is_none_or(|(existing, _)| r < *existing) {
            best.insert(key, (r, term.text));
        }
    }
    Ok(best.into_iter().map(|(k, (_, text))| (k, text)).collect())
}

/// A date of the SPARQL result, to its precision, as Germans write it.
fn german_date(row: &Value, var: &str) -> String {
    let Some(time) = row[var]["value"].as_str() else { return String::new() };
    let precision: u8 = row[format!("{var}Precision")]["value"].as_str().and_then(|p| p.parse().ok()).unwrap_or(9);
    let (bce, time) = match time.strip_prefix('-') {
        Some(t) => (" v. Chr.", t),
        None => ("", time.trim_start_matches('+')),
    };
    let mut parts = time.get(..10).unwrap_or_default().splitn(3, '-');
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else { return String::new() };
    let year = year.trim_start_matches('0');
    match precision {
        11.. => format!("{day}.{month}.{year}{bce}"),
        10 => format!("{month}.{year}{bce}"),
        _ => format!("{year}{bce}"),
    }
}

async fn countries(sparql: &Sparql) -> anyhow::Result<Vec<(String, String)>> {
    let json = sparql.json("SELECT ?country ?code { ?country wdt:P31 wd:Q6256 ; wdt:P298 ?code }").await?;
    let mut countries: Vec<(String, String)> = sparql::bindings(&json)
        .filter_map(|row| {
            let q = sparql::entity_id(row["country"]["value"].as_str()?)?;
            Some((q.to_string(), row["code"]["value"].as_str()?.to_string()))
        })
        .collect();
    countries.sort();
    countries.dedup_by(|a, b| a.0 == b.0);
    Ok(countries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn date_filters() {
        let none = ["", "0", ""];
        assert_eq!(date_filter("1", none, none), Ok(None));
        assert_eq!(
            date_filter("1", ["", "", "1952"], none),
            Ok(Some(DateFilter::Range(Some(ymd(1952, 1, 1)), Some(ymd(1953, 1, 1)))))
        );
        assert_eq!(
            date_filter("1", ["11", "3", "1952"], none),
            Ok(Some(DateFilter::Range(Some(ymd(1952, 3, 11)), Some(ymd(1952, 3, 12)))))
        );
        assert_eq!(date_filter("1", ["11", "3", ""], none), Ok(Some(DateFilter::Anniversary(3, Some(11)))));
        assert_eq!(
            date_filter("2", ["", "12", "1900"], none),
            Ok(Some(DateFilter::Range(None, Some(ymd(1900, 12, 1)))))
        );
        assert_eq!(
            date_filter("3", ["", "12", "1900"], none),
            Ok(Some(DateFilter::Range(Some(ymd(1901, 1, 1)), None)))
        );
        assert_eq!(
            date_filter("4", ["", "", "1900"], ["", "", "1910"]),
            Ok(Some(DateFilter::Range(Some(ymd(1900, 1, 1)), Some(ymd(1911, 1, 1)))))
        );
        assert_eq!(date_filter("5", none, none), Ok(Some(DateFilter::Unknown)));
        assert!(date_filter("1", ["31", "2", "1900"], none).is_err());
        assert!(date_filter("1", ["1", "1", "x"], none).is_err());
    }

    #[test]
    fn sparql_patterns() {
        let range = DateFilter::Range(Some(ymd(1952, 1, 1)), Some(ymd(1953, 1, 1)));
        assert_eq!(
            date_sparql(&range, "P569", "born"),
            "?item wdt:P569 ?born . FILTER(?born >= \"1952-01-01T00:00:00Z\"^^xsd:dateTime && ?born < \"1953-01-01T00:00:00Z\"^^xsd:dateTime)"
        );
        assert_eq!(date_sparql(&DateFilter::Unknown, "P570", "died"), "MINUS { ?item wdt:P570 [] }");
        let criteria =
            Criteria { country: Some("Q183".into()), birth_places: Some(vec!["Q64".into()]), ..Default::default() };
        assert_eq!(
            patterns(&criteria),
            ["VALUES ?birthPlace { wd:Q64 } ?item wdt:P19 ?birthPlace .", "?item wdt:P27 wd:Q183 ."]
        );
    }

    #[test]
    fn german_dates() {
        let row = serde_json::json!({
            "born": {"value": "1952-03-11T00:00:00Z"}, "bornPrecision": {"value": "11"},
            "died": {"value": "-0043-01-01T00:00:00Z"}, "diedPrecision": {"value": "9"}
        });
        assert_eq!(german_date(&row, "born"), "11.03.1952");
        assert_eq!(german_date(&row, "died"), "43 v. Chr.");
        assert_eq!(german_date(&row, "x"), "");
    }
}
