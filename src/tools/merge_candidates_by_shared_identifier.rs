//! `merge_candidates_by_shared_identifier.php`: the Distributed Game "Merge
//! by shared identifier": two items with the same external ID (VIAF etc.),
//! to be merged or not.

use super::distributed_game::{self as game, Decision, MAX_TILES, Request};
use crate::cache::TimedCache;
use crate::state::AppState;
use crate::terms::{self, TermType};
use crate::tool_db::Database;
use crate::web::Params;
use crate::wiki::encode_component;
use crate::wikidata;
use axum::extract::State;
use axum::response::Response;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Params as SqlParams, Value as SqlValue};

/// Batches of random candidates to look at for a request.
const ROUNDS: usize = 5;
/// The languages of `getLabel()` in the PHP tool.
const LABEL_LANGUAGES: &[&str] = &["en", "de", "nl", "fr", "es", "it", "zh"];

pub async fn handler(State(state): State<Arc<AppState>>, Params(request): Params<Request>) -> Response {
    let answer = match request.action.as_str() {
        "desc" => desc(&state).await,
        "tiles" => tiles(&state, &request).await,
        "log_action" => log_action(&state, &request).await,
        _ => Ok(game::no_action()),
    };
    game::respond(answer, request.callback)
}

static PROPERTIES: TimedCache<Map<String, Value>> = TimedCache::new(Duration::from_secs(24 * 3600));

async fn desc(state: &AppState) -> anyhow::Result<Value> {
    let properties = PROPERTIES.get_or_fetch(async || property_options(state).await).await?;
    Ok(json!({
        "label": {"en": "Merge by shared identifier"},
        "description": {"en": "Potential items to be merged, via shared identifiers (VIAF etc)."},
        "icon": "https://upload.wikimedia.org/wikipedia/commons/thumb/b/bd/Share_Icon.svg/100px-Share_Icon.svg.png",
        "options": [{"name": "Property", "key": "prop", "values": (*properties).clone()}],
    }))
}

/// The properties to choose from: `{"0": "All", "214": "P214: VIAF ID", …}`.
async fn property_options(state: &AppState) -> anyhow::Result<Map<String, Value>> {
    let mut conn = state.tool_db.conn(Database::MergeById).await?;
    let numbers: Vec<u64> = conn.query("SELECT DISTINCT prop FROM candidates ORDER BY prop").await?;
    let ids: Vec<String> = numbers.iter().map(|n| format!("P{n}")).collect();
    let mut labels = terms::labels(&state.replicas, &ids, LABEL_LANGUAGES).await?;
    let unlabelled: Vec<String> = ids.iter().filter(|id| !labels.contains_key(*id)).cloned().collect();
    if !unlabelled.is_empty() {
        for term in terms::terms(&state.replicas, &unlabelled, &[TermType::Label], &[]).await? {
            labels.entry(term.entity).or_insert(term.text);
        }
    }
    let mut options = Map::new();
    options.insert("0".to_string(), json!("All"));
    for (n, id) in numbers.iter().zip(&ids) {
        let text = labels.get(id).map_or_else(|| id.clone(), |label| format!("{id}: {label}"));
        options.insert(n.to_string(), json!(text));
    }
    Ok(options)
}

/// `(id, q1, q2, property number, shared value)`.
type Candidate = (u64, u64, u64, u64, String);

async fn tiles(state: &AppState, request: &Request) -> anyhow::Result<Value> {
    let num = request.num();
    let property: u64 = request.prop.trim().parse().unwrap_or(0);
    let mut seen: HashSet<u64> = request.in_cache_ids().into_iter().collect();
    let mut properties: HashMap<String, Value> = HashMap::new();
    let mut tiles = vec![];
    let mut conn = state.tool_db.conn(Database::MergeById).await?;
    for _ in 0..ROUNDS {
        if tiles.len() >= num {
            break;
        }
        let mut sql = "SELECT id, q1, q2, prop, value FROM candidates WHERE random>=? AND done='OPEN'".to_string();
        let mut params = vec![SqlValue::from(rand::random::<f64>())];
        if property != 0 {
            sql += " AND prop=?";
            params.push(property.into());
        }
        // Some turn out to be merged already.
        sql += &format!(" ORDER BY random LIMIT {}", (num * 2).min(MAX_TILES));
        let found: Vec<Candidate> = conn.exec(sql, SqlParams::Positional(params)).await?;
        let found: Vec<Candidate> = found.into_iter().filter(|c| seen.insert(c.0)).collect();
        let items: Vec<String> = found.iter().flat_map(|c| [format!("Q{}", c.1), format!("Q{}", c.2)]).collect();
        let merged = game::redirects(state, &items).await?;
        let missing: Vec<String> =
            found.iter().map(|c| format!("P{}", c.3)).filter(|p| !properties.contains_key(p)).collect();
        if !missing.is_empty() {
            properties.extend(wikidata::entities(&state.http, &missing, "labels|claims").await?);
        }
        for candidate in found {
            let (q1, q2) = (format!("Q{}", candidate.1), format!("Q{}", candidate.2));
            if tiles.len() >= num || merged.contains(&q1) || merged.contains(&q2) {
                continue;
            }
            if let Some(property) = properties.get(&format!("P{}", candidate.3)) {
                tiles.push(tile(&candidate, property, &request.lang));
            }
        }
    }
    Ok(json!({"tiles": tiles}))
}

fn tile((id, q1, q2, property, value): &Candidate, entity: &Value, language: &str) -> Value {
    let (q1, q2) = (format!("Q{q1}"), format!("Q{q2}"));
    let languages = terms::languages_with(language, LABEL_LANGUAGES);
    let label = wikimisc::labels::json_term_or_any(entity, "labels", &languages).unwrap_or_default();
    let mut shared = json!({
        "type": "text",
        "title": format!("Sharing property value for: {label} (P{property})"),
        "text": value,
    });
    if let Some(formatter) = wikidata::first_string(entity, "P1630") {
        shared["url"] = json!(formatter.replace("$1", &encode_component(value)));
    }
    let sections = vec![shared, game::item_section(&q1), game::item_section(&q2)];
    game::tile(game::id_string(*id), sections, "Same topic", game::merge_action(&q1, &q2), "Ignore")
}

async fn log_action(state: &AppState, request: &Request) -> anyhow::Result<Value> {
    if let (Some(id), Some(done)) = (request.tile_id(), request.decision().and_then(outcome)) {
        let mut conn = state.tool_db.conn(Database::MergeById).await?;
        conn.exec_drop("UPDATE candidates SET done=? WHERE id=?", (done, id)).await?;
    }
    Ok(game::logged())
}

/// What a decision makes of a candidate; skipping leaves it open.
fn outcome(decision: Decision) -> Option<&'static str> {
    match decision {
        Decision::Yes => Some("MERGED"),
        Decision::No => Some("IGNORE"),
        Decision::Skip => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn outcomes() {
        assert_eq!(outcome(Decision::Yes), Some("MERGED"));
        assert_eq!(outcome(Decision::No), Some("IGNORE"));
        assert_eq!(outcome(Decision::Skip), None);
    }

    #[test]
    fn tiles_link_the_shared_value() {
        let property = json!({
            "labels": {"en": {"value": "VIAF ID"}, "de": {"value": "VIAF-ID"}},
            "claims": {"P1630": [{"rank": "normal", "mainsnak": {"snaktype": "value", "datavalue": {"value": "https://viaf.org/viaf/$1"}}}]},
        });
        let tile = tile(&(9, 1, 2, 214, "12 3".into()), &property, "de");
        assert_eq!(tile["id"], "9");
        assert_eq!(tile["sections"][0]["title"], "Sharing property value for: VIAF-ID (P214)");
        assert_eq!(tile["sections"][0]["url"], "https://viaf.org/viaf/12%203");
        assert_eq!(tile["sections"][2]["q"], "Q2");
        assert_eq!(tile["controls"][0]["entries"][0]["api_action"]["toid"], "Q1");
    }

    #[tokio::test]
    async fn unknown_action() {
        let dir = tempfile::tempdir().unwrap();
        let response = get(dir.path(), "/merge_candidates_by_shared_identifier.php?callback=x").await;
        assert_eq!(response.body, r#"x({"error":"No valid action!"});"#);
    }
}
