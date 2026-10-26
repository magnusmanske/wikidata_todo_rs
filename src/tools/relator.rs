//! `relator/api.php`, the API of the first Relator app (`relator/`): a
//! random person with children, relatives listed on Geni, and read-only
//! Wikidata requests; other actions go to Widar.

use super::duplicity::wikidata_api_proxy;
use super::widar::{self, Login};
use crate::replicas::linktarget_ids;
use crate::state::AppState;
use crate::web::Params;
use crate::wiki::Wiki;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;

const NS_PROPERTY: i64 = 120;
const GENI_TIMEOUT: Duration = Duration::from_secs(20);
/// Relations as Geni words them, and the Wikidata property for each.
const GENI_RELATIONS: &[(&str, &str)] = &[
    ("son", "P40"),
    ("daughter", "P40"),
    ("wife", "P26"),
    ("husband", "P26"),
    ("mother", "P25"),
    ("father", "P22"),
    ("brother", "P3373"),
    ("sister", "P3373"),
];

static RELATION: LazyLock<Regex> = LazyLock::new(|| {
    let words: Vec<&str> = GENI_RELATIONS.iter().map(|(word, _)| *word).collect();
    Regex::new(&format!(r"(?i)({}) of (.+?)<br/>", words.join("|"))).expect("valid regex")
});
static PERSON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<a href="https://www\.geni\.com/people/.+?/(\d+)".*?>(.+?)</a>"#).expect("valid regex")
});

pub async fn api(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Params(params): Params<HashMap<String, String>>,
) -> Response {
    let get = |key: &str| params.get(key).map_or("", String::as_str);
    let answer = |status: &str, data: Value| Json(json!({"status": status, "data": data})).into_response();
    match get("action") {
        "random_person" => person_answer(random_person(&state, "P40").await),
        "load_geni_relations" => {
            let id = get("geni_id").trim();
            if id.is_empty() || id.len() > 30 || !id.bytes().all(|b| b.is_ascii_digit()) {
                return answer("Invalid Geni ID", json!([]));
            }
            match geni_page(&state.http, id).await {
                Ok(html) => match geni_relations(&html) {
                    Some(relations) => answer("OK", Value::Object(relations)),
                    None => answer("No data found", json!([])),
                },
                Err(e) => {
                    tracing::warn!("Geni: {e:#}");
                    answer("Geni did not answer", json!([]))
                }
            }
        }
        "wikidata_api_proxy" => match wikidata_api_proxy(&state.http, &params).await {
            Ok(response) => response,
            Err((code, status)) => (code, answer(&status, json!([]))).into_response(),
        },
        action => {
            let login = Login {
                callback: format!("{}/relator/api.php", state.config.public_url),
                forward: "/relator/".to_string(),
                authorize: "/relator/api.php?action=authorize".to_string(),
            };
            match widar::respond(&state, &headers, &params, &login).await {
                Some(response) => response,
                None => answer(&format!("Unknown action '{action}'"), json!([])),
            }
        }
    }
}

/// `{"status": "OK", "data": "Q42"}`, or why not.
pub fn person_answer(person: anyhow::Result<Option<String>>) -> Response {
    let (code, status, data) = match person {
        Ok(Some(item)) => (StatusCode::OK, "OK", json!(item)),
        Ok(None) => (StatusCode::OK, "No random person available", json!([])),
        Err(e) => {
            tracing::warn!("random person: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, "The database did not answer as expected", json!([]))
        }
    };
    (code, Json(json!({"status": status, "data": data}))).into_response()
}

/// A random item with a statement for `property` (e.g. P40, "child"):
/// the first one from a random page ID on, wrapping around at the end.
/// `property` must be a valid property ID.
pub async fn random_person(state: &AppState, property: &str) -> anyhow::Result<Option<String>> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page", "pagelinks", "linktarget"]).await?;
    let Some(target) = linktarget_ids(&mut conn, NS_PROPERTY, &[property]).await?.first().copied() else {
        return Ok(None);
    };
    let max: Option<u64> = conn.query_first("SELECT MAX(page_id) FROM page").await?;
    let from = rand::random_range(0..=max.unwrap_or_default());
    for start in [from, 0] {
        let sql = "SELECT pl_from FROM pagelinks WHERE pl_target_id=? AND pl_from_namespace=0 AND pl_from>=?
            ORDER BY pl_from LIMIT 1";
        if let Some(page) = conn.exec_first::<u64, _, _>(sql, (target, start)).await? {
            let title: Option<Vec<u8>> =
                conn.exec_first("SELECT page_title FROM page WHERE page_id=?", (page,)).await?;
            return Ok(title.map(|t| String::from_utf8_lossy(&t).into_owned()));
        }
    }
    Ok(None)
}

async fn geni_page(http: &reqwest::Client, id: &str) -> anyhow::Result<String> {
    let url = format!("https://www.geni.com/people/x/{id}");
    Ok(http.get(url).timeout(GENI_TIMEOUT).send().await?.error_for_status()?.text().await?)
}

/// The relatives on a Geni profile page, as `{property: [[Geni ID, name], …]}`;
/// `None` if the page lists no relations at all.
fn geni_relations(html: &str) -> Option<Map<String, Value>> {
    let mut relations = Map::new();
    let mut found = false;
    for relation in RELATION.captures_iter(html) {
        found = true;
        let (Some(word), Some(list)) = (relation.get(1), relation.get(2)) else { continue };
        let word = word.as_str().to_lowercase();
        let Some((_, property)) = GENI_RELATIONS.iter().find(|(w, _)| *w == word) else { continue };
        for person in PERSON.captures_iter(list.as_str()) {
            let (Some(id), Some(name)) = (person.get(1), person.get(2)) else { continue };
            let name = wikimisc::escape::unescape_html(name.as_str()).into_owned();
            if let Value::Array(people) = relations.entry(*property).or_insert_with(|| json!([])) {
                people.push(json!([id.as_str(), name]));
            }
        }
    }
    found.then_some(relations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn geni_profiles() {
        let html = r#"<td>Daughter of <a href="https://www.geni.com/people/John-Doe/6000000000000000001" class="x">John &amp; Doe</a> and <a href="https://www.geni.com/people/Jane-Doe/6000000000000000002">Jane Doe</a><br/>
            Wife of <a href="https://www.geni.com/people/Max-Roe/6000000000000000003">Max</a><br/>
            Mother of <a href="https://www.geni.com/people/Kid-Roe/6000000000000000004">Kid</a><br/>"#;
        let relations = geni_relations(html).unwrap();
        assert_eq!(
            Value::Object(relations),
            json!({
                "P40": [["6000000000000000001", "John & Doe"], ["6000000000000000002", "Jane Doe"]],
                "P26": [["6000000000000000003", "Max"]],
                "P25": [["6000000000000000004", "Kid"]],
            })
        );
        assert!(geni_relations("<html>nothing</html>").is_none());
    }

    #[tokio::test]
    async fn answers() {
        let dir = tempfile::tempdir().unwrap();
        let invalid = get(dir.path(), "/relator/api.php?action=load_geni_relations&geni_id=1/../../x").await;
        assert_eq!(invalid.body, r#"{"status":"Invalid Geni ID","data":[]}"#);
        let unknown = get(dir.path(), "/relator/api.php?action=x").await;
        assert_eq!(unknown.body, r#"{"status":"Unknown action 'x'","data":[]}"#);
        let denied = get(dir.path(), "/relator/api.php?action=wikidata_api_proxy&wdaction=edit").await;
        assert_eq!(denied.body, r#"{"status":"Wikidata action not allowed: edit","data":[]}"#);
    }
}
