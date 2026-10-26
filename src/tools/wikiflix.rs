//! `wikiflix/api.php`: the data of WikiFlix (the Vue app in
//! `public_html/wikiflix/`): films free to watch, by Wikidata, from the
//! `wikiflix_p` database that a job fills. Rows go out as the PHP tool sent
//! them: every column a string (or null).

use crate::replicas::placeholders;
use crate::state::AppState;
use crate::tool_db::Database;
use crate::web::{AppError, Params, api_json};
use crate::wikidata;
use axum::extract::State;
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Params as SqlParams, Row, Value as SqlValue};

/// Cast member, director: sections of their own (people).
const PEOPLE_PROPERTIES: &str = "161,57";
/// Instance of, genre, …: the sections of films.
const SECTION_PROPERTIES: &str = "31,166,136,462,495,364,361";
/// Too broad to be sections: film, silent film, black-and-white film.
const SKIPPED_SECTIONS: &str = "838368,226730,11424";
/// Films per section, unless asked otherwise.
const SECTION_FILMS: u64 = 25;
/// Search results per kind.
const SEARCH_LIMIT: u32 = 50;
/// The languages of `getLabel()` in the PHP tool.
const LABEL_LANGUAGES: &[&str] = &["en", "de", "nl", "fr", "es", "it", "zh"];
const RANKED: &str = "SELECT * FROM vw_ranked_movies";
/// The order of `vw_ranked_movies`, which a view does not guarantee.
const RANK_ORDER: &str = "ORDER BY sites DESC, minutes DESC, q";

type Object = Map<String, Value>;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    action: String,
    #[serde(default = "default_language")]
    language: String,
    #[serde(default)]
    q: String,
    #[serde(default)]
    max: Option<String>,
    #[serde(default)]
    prop: String,
    #[serde(default)]
    query: String,
}

fn default_language() -> String {
    "en".to_string()
}

/// The number in `s`, ignoring anything else (`Q42` is 42), as PHP's tool did.
fn number(s: &str) -> u64 {
    s.chars().filter(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)
}

/// The actions, checked before connecting to the database.
const ACTIONS: &[&str] = &["get_movie", "get_all_sections", "get_section", "search", "get_person"];

pub async fn api(State(state): State<Arc<AppState>>, Params(request): Params<Request>) -> Result<Response, AppError> {
    if !ACTIONS.contains(&request.action.as_str()) {
        return Ok(api_json(json!({"status": format!("Bad action: {}", request.action)}), None));
    }
    let mut conn = state.tool_db.conn(Database::WikiFlix).await?;
    let language: String = request.language.chars().filter(|c| c.is_ascii_lowercase() || *c == '-').collect();
    let mut out = json!({"status": "OK"});
    match request.action.as_str() {
        "get_movie" => out["data"] = movie(&state, &mut conn, number(&request.q)).await?,
        "get_all_sections" => out["data"] = json!(all_sections(&mut conn, &language).await?),
        "get_section" => {
            let q = number(&request.q);
            let max = match request.max.as_deref() {
                Some("all") => None,
                Some(max) => Some(max.trim().parse().unwrap_or(0)),
                None => Some(SECTION_FILMS),
            };
            let titles = titles(&state, &[q]).await?;
            match titles.get(&q) {
                Some(title) => {
                    let section = (json!(q), json!(number(&request.prop)), title.clone());
                    out["data"] = json!(populate(&mut conn, q, section, max).await?);
                }
                None => out["status"] = json!(format!("No such item Q{q}")),
            }
        }
        "search" => {
            let query = request.query.trim();
            out["data"] = json!({
                "movies": search_movies(&mut conn, query).await?,
                "sections": search_sections(&state, &mut conn, query, &language).await?,
                "people": search_people(&mut conn, query).await?,
            });
        }
        "get_person" => out["data"] = json!(person(&mut conn, number(&request.q)).await?),
        _ => {}
    }
    Ok(api_json(out, None))
}

/// The rows, each as the PHP tool's `fetch_object()` gave them: strings or null.
async fn rows(conn: &mut Conn, sql: &str, params: Vec<SqlValue>) -> anyhow::Result<Vec<Object>> {
    let rows: Vec<Row> = conn.exec(sql, SqlParams::Positional(params)).await?;
    Ok(rows.into_iter().map(object).collect())
}

fn object(row: Row) -> Object {
    let names: Vec<String> = row.columns_ref().iter().map(|c| c.name_str().into_owned()).collect();
    names.into_iter().zip(row.unwrap_raw()).map(|(name, value)| (name, value.map_or(Value::Null, text))).collect()
}

fn text(value: SqlValue) -> Value {
    match value {
        SqlValue::NULL => Value::Null,
        SqlValue::Bytes(bytes) => Value::String(String::from_utf8_lossy(&bytes).into_owned()),
        SqlValue::Int(n) => Value::String(n.to_string()),
        SqlValue::UInt(n) => Value::String(n.to_string()),
        SqlValue::Float(n) => Value::String(n.to_string()),
        SqlValue::Double(n) => Value::String(n.to_string()),
        other => Value::String(other.as_sql(true).trim_matches('\'').to_string()),
    }
}

/// A film row with its video files as JSON, and a film still (from the
/// first Commons video) if it has no image.
fn film(mut row: Object) -> Object {
    let files: Value =
        row.get("video_files").and_then(Value::as_str).and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    if row.get("image").is_none_or(Value::is_null) {
        let commons = files.as_array().into_iter().flatten().find(|f| f["property"] == 10);
        if let Some(key) = commons.and_then(|f| f["key"].as_str()) {
            row.insert("image".to_string(), json!(key));
        }
    }
    row.insert("video_files".to_string(), files);
    row
}

async fn films(conn: &mut Conn, sql: &str, params: Vec<SqlValue>) -> anyhow::Result<Vec<Object>> {
    Ok(rows(conn, sql, params).await?.into_iter().map(film).collect())
}

/// Labels (as titles) of items by number; items that do not exist are left out.
async fn titles(state: &AppState, items: &[u64]) -> anyhow::Result<HashMap<u64, String>> {
    let ids: Vec<String> = items.iter().map(|q| format!("Q{q}")).collect();
    let entities = wikidata::entities(&state.http, &ids, "labels").await?;
    Ok(items
        .iter()
        .zip(&ids)
        .filter_map(|(q, id)| {
            let entity = entities.get(id)?;
            let label = wikimisc::labels::json_term_or_any(entity, "labels", LABEL_LANGUAGES).unwrap_or(id);
            Some((*q, label.to_string()))
        })
        .collect())
}

/// A section: `(q, property, title)`, q and property as the caller had them.
type Section = (Value, Value, String);

/// A section with its (best) films, `max` of them at most.
async fn populate(conn: &mut Conn, item: u64, (q, prop, title): Section, max: Option<u64>) -> anyhow::Result<Object> {
    let in_section = "WHERE q IN (SELECT movie_q FROM section WHERE section_q=?)";
    let total: u64 =
        conn.exec_first(format!("SELECT COUNT(*) FROM vw_ranked_movies {in_section}"), (item,)).await?.unwrap_or(0);
    let limit = max.map(|n| format!("LIMIT {n}")).unwrap_or_default();
    let movies = films(conn, &format!("{RANKED} {in_section} {RANK_ORDER} {limit}"), vec![item.into()]).await?;
    let mut section = Object::new();
    section.insert("q".into(), q);
    section.insert("title".into(), json!(title));
    section.insert("prop".into(), prop);
    section.insert("total".into(), json!(total));
    section.insert("movies".into(), json!(movies));
    Ok(section)
}

/// A film with its people (by property and item) and sections; null if there is no such film.
async fn movie(state: &AppState, conn: &mut Conn, q: u64) -> anyhow::Result<Value> {
    let Some(mut movie) = films(conn, &format!("{RANKED} WHERE q=?"), vec![q.into()]).await?.pop() else {
        return Ok(Value::Null);
    };
    let sections = rows(conn, "SELECT * FROM section WHERE movie_q=?", vec![q.into()]).await?;
    let number = |row: &Object, key: &str| row.get(key).and_then(Value::as_str).map_or(0, self::number);
    let is_people =
        |row: &Object| PEOPLE_PROPERTIES.split(',').any(|p| row.get("property").and_then(Value::as_str) == Some(p));
    let (people, sections): (Vec<Object>, Vec<Object>) = sections.into_iter().partition(is_people);

    let person_ids: Vec<u64> = people.iter().map(|row| number(row, "section_q")).collect();
    let sql = format!("SELECT * FROM person WHERE q IN ({})", placeholders(person_ids.len()));
    let persons = match person_ids.is_empty() {
        true => vec![],
        false => rows(conn, &sql, person_ids.iter().map(|q| SqlValue::from(*q)).collect()).await?,
    };
    let persons: HashMap<u64, &Object> = persons.iter().map(|p| (number(p, "q"), p)).collect();
    let mut by_property = Object::new();
    for row in &people {
        let (property, item) = (row["property"].as_str().unwrap_or_default(), number(row, "section_q"));
        let entry = by_property.entry(format!("P{property}")).or_insert_with(|| json!({}));
        entry[format!("Q{item}")] = json!(person_entry(row["section_q"].clone(), persons.get(&item).copied()));
    }
    // An empty PHP array, as the app got it.
    movie.insert("people".into(), if by_property.is_empty() { json!([]) } else { Value::Object(by_property) });

    let items: Vec<u64> = sections.iter().map(|row| number(row, "section_q")).collect();
    let titles = titles(state, &items).await?;
    let mut populated = vec![];
    for (row, item) in sections.iter().zip(items) {
        if let Some(title) = titles.get(&item) {
            let section = (row["section_q"].clone(), row["property"].clone(), title.clone());
            populated.push(populate(conn, item, section, Some(SECTION_FILMS)).await?);
        }
    }
    movie.insert("sections".into(), json!(populated));
    Ok(Value::Object(movie))
}

/// A person, as listed with a film: no films of their own.
fn person_entry(q: Value, row: Option<&Object>) -> Object {
    let mut person = Object::new();
    person.insert("q".into(), q);
    person.insert("movies".into(), json!([]));
    if let Some(row) = row {
        for key in ["label", "gender", "image"] {
            person.insert(key.into(), row.get(key).cloned().unwrap_or_default());
        }
    }
    person
}

/// A person with their films (as cast member or director).
async fn person(conn: &mut Conn, q: u64) -> anyhow::Result<Object> {
    let row = rows(conn, "SELECT * FROM person WHERE q=?", vec![q.into()]).await?.pop();
    let mut person = person_entry(json!(q), row.as_ref());
    let sql = format!(
        "{RANKED} WHERE q IN (SELECT DISTINCT movie_q FROM section WHERE property IN ({PEOPLE_PROPERTIES}) AND section_q=?) {RANK_ORDER}"
    );
    person.insert("movies".into(), json!(films(conn, &sql, vec![q.into()]).await?));
    Ok(person)
}

/// Sections with their number of films and their label in `language`.
fn sections_sql(condition: &str, limit: &str) -> String {
    format!(
        "SELECT *, (SELECT value FROM label WHERE language=? AND q=section_q) AS label FROM vw_section_property_q
         WHERE property IN ({SECTION_PROPERTIES}) AND section_q NOT IN ({SKIPPED_SECTIONS}) {condition} {limit}"
    )
}

async fn all_sections(conn: &mut Conn, language: &str) -> anyhow::Result<Vec<Object>> {
    rows(conn, &sections_sql("", ""), vec![language.into()]).await
}

fn like(query: &str) -> SqlValue {
    format!("%{query}%").into()
}

async fn search_movies(conn: &mut Conn, query: &str) -> anyhow::Result<Vec<Object>> {
    if query.is_empty() {
        return Ok(vec![]);
    }
    films(conn, &format!("{RANKED} WHERE title LIKE ? {RANK_ORDER} LIMIT {SEARCH_LIMIT}"), vec![like(query)]).await
}

async fn search_people(conn: &mut Conn, query: &str) -> anyhow::Result<Vec<Object>> {
    if query.is_empty() {
        return Ok(vec![]);
    }
    rows(conn, &format!("SELECT * FROM person WHERE label LIKE ? LIMIT {SEARCH_LIMIT}"), vec![like(query)]).await
}

async fn search_sections(
    state: &AppState,
    conn: &mut Conn,
    query: &str,
    language: &str,
) -> anyhow::Result<Vec<Object>> {
    if query.is_empty() {
        return Ok(vec![]);
    }
    let condition = "AND section_q IN (SELECT DISTINCT q FROM label WHERE value LIKE ?)";
    let sql = sections_sql(condition, &format!("LIMIT {SEARCH_LIMIT}"));
    let found = rows(conn, &sql, vec![language.into(), like(query)]).await?;
    let items: Vec<u64> =
        found.iter().map(|row| row.get("section_q").and_then(Value::as_str).map_or(0, number)).collect();
    let titles = titles(state, &items).await?;
    let mut sections = vec![];
    for (row, item) in found.iter().zip(items) {
        if let Some(title) = titles.get(&item) {
            let section = (row["section_q"].clone(), row["property"].clone(), title.clone());
            sections.push(populate(conn, item, section, Some(SECTION_FILMS)).await?);
        }
    }
    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bad_action() {
        let dir = tempfile::tempdir().unwrap();
        let response = crate::web::test_support::get(dir.path(), "/wikiflix/api.php?action=x").await;
        assert_eq!(response.body, r#"{"status":"Bad action: x"}"#);
        assert_eq!(response.headers["content-type"], "application/json");
    }

    #[test]
    fn numbers() {
        assert_eq!(number("Q42"), 42);
        assert_eq!(number("42"), 42);
        assert_eq!(number(""), 0);
        assert_eq!(number("x"), 0);
    }

    #[test]
    fn films_get_a_still_if_they_lack_an_image() {
        let files = r#"[{"property": 724, "key": "ia"}, {"property": 10, "key": "Film.webm", "is_trailer": 0}]"#;
        let row =
            |image: Value| Object::from_iter([("image".to_string(), image), ("video_files".to_string(), json!(files))]);
        let fixed = film(row(Value::Null));
        assert_eq!(fixed["image"], "Film.webm");
        assert_eq!(fixed["video_files"][1]["is_trailer"], 0);
        assert_eq!(film(row(json!("Poster.jpg")))["image"], "Poster.jpg");
    }

    #[test]
    fn people_without_a_row() {
        let person = person_entry(json!("42"), None);
        assert_eq!(serde_json::to_string(&person).unwrap(), r#"{"q":"42","movies":[]}"#);
        let row = Object::from_iter([("label".to_string(), json!("Ann")), ("sites".to_string(), json!("3"))]);
        let person = person_entry(json!(42), Some(&row));
        assert_eq!(
            serde_json::to_string(&person).unwrap(),
            r#"{"q":42,"movies":[],"label":"Ann","gender":null,"image":null}"#
        );
    }

    #[test]
    fn database_values_as_text() {
        assert_eq!(text(SqlValue::Int(-3)), json!("-3"));
        assert_eq!(text(SqlValue::UInt(7)), json!("7"));
        assert_eq!(text(SqlValue::Bytes(b"M".to_vec())), json!("M"));
        assert_eq!(text(SqlValue::NULL), Value::Null);
    }
}
