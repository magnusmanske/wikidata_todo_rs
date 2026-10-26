//! What the games for The Distributed Game
//! (<https://wikidata-game.toolforge.org/distributed/>) share. The game asks
//! by JSONP for a description (`action=desc`), for tiles to show
//! (`action=tiles`), and reports each decision (`action=log_action`).

use crate::replicas::select_in;
use crate::state::AppState;
use crate::web::api_json;
use crate::wiki::Wiki;
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

/// Tiles per request, at most.
pub const MAX_TILES: usize = 50;
/// Tile IDs the game already has, at most.
const MAX_IN_CACHE: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    num: String,
    #[serde(default = "default_language")]
    pub lang: String,
    #[serde(default)]
    pub in_cache: String,
    #[serde(default)]
    pub tile: String,
    #[serde(default)]
    decision: String,
    pub callback: Option<String>,
    /// Duplicity: `lang`, `species` or `commons`.
    #[serde(default)]
    pub wikitype: String,
    /// Merge candidates: the property number, 0 for all.
    #[serde(default)]
    pub prop: String,
}

fn default_language() -> String {
    "en".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Yes,
    No,
    Skip,
}

impl Request {
    /// The number of tiles asked for, capped.
    pub fn num(&self) -> usize {
        self.num.trim().parse::<usize>().unwrap_or(0).min(MAX_TILES)
    }

    pub fn decision(&self) -> Option<Decision> {
        match self.decision.as_str() {
            "yes" => Some(Decision::Yes),
            "no" => Some(Decision::No),
            "skip" => Some(Decision::Skip),
            _ => None,
        }
    }

    /// The numeric tile IDs the game already has.
    pub fn in_cache_ids(&self) -> Vec<u64> {
        self.in_cache.split(',').filter_map(|id| id.trim().parse().ok()).take(MAX_IN_CACHE).collect()
    }

    /// The tile ID as a number, if it is one.
    pub fn tile_id(&self) -> Option<u64> {
        self.tile.trim().parse().ok().filter(|id| *id > 0)
    }
}

/// The answer as JSON(P). A failure becomes `{"error": …}`, as the game expects.
pub fn respond(answer: anyhow::Result<Value>, callback: Option<String>) -> Response {
    let value = answer.unwrap_or_else(|e| {
        tracing::warn!("distributed game: {e:#}");
        json!({"error": "The tool's data is not available at the moment."})
    });
    api_json(value, callback)
}

/// The answer to an unknown action.
pub fn no_action() -> Value {
    json!({"error": "No valid action!"})
}

/// The answer to `log_action`: nothing (an empty list, as the PHP tool's empty array).
pub fn logged() -> Value {
    json!([])
}

pub fn item_section(q: &str) -> Value {
    json!({"type": "item", "q": q})
}

/// A tile with the usual buttons: `yes` runs `api_action` on Wikidata.
pub fn tile(id: Value, sections: Vec<Value>, yes: &str, api_action: Value, no: &str) -> Value {
    json!({
        "id": id,
        "sections": sections,
        "controls": [{
            "type": "buttons",
            "entries": [
                {"type": "green", "decision": "yes", "label": yes, "api_action": api_action},
                {"type": "white", "decision": "skip", "label": "Skip"},
                {"type": "blue", "decision": "no", "label": no},
            ],
        }],
    })
}

/// The `wbmergeitems` action of a "same topic" decision: `q2` into `q1`.
pub fn merge_action(q1: &str, q2: &str) -> Value {
    json!({"action": "wbmergeitems", "fromid": q2, "toid": q1, "ignoreconflicts": "description|sitelink"})
}

/// Those of `items` that are redirects now (merged already).
pub async fn redirects(state: &AppState, items: &[String]) -> anyhow::Result<HashSet<String>> {
    if items.is_empty() {
        return Ok(HashSet::new());
    }
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page"]).await?;
    let sql = "SELECT page_title FROM page WHERE page_namespace=0 AND page_is_redirect=1 AND page_title IN";
    let titles: Vec<String> = select_in(&mut conn, sql, "", &[], items).await?;
    Ok(titles.into_iter().collect())
}

/// A tile number from the database, as a string: the PHP tool's IDs were.
pub fn id_string(id: u64) -> Value {
    Value::String(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(query: &str) -> Request {
        serde_urlencoded::from_str(query).unwrap()
    }

    #[test]
    fn parameters() {
        let r = request("num=5&in_cache=1,%202,x,,3&tile=17&decision=yes");
        assert_eq!(
            (r.num(), r.in_cache_ids(), r.tile_id(), r.decision()),
            (5, vec![1, 2, 3], Some(17), Some(Decision::Yes))
        );
        let r = request("num=100000&tile=-1&decision=maybe");
        assert_eq!((r.num(), r.tile_id(), r.decision(), r.lang.as_str()), (MAX_TILES, None, None, "en"));
        assert_eq!(request("num=x").num(), 0);
    }

    #[test]
    fn tiles() {
        let t = tile(id_string(3), vec![item_section("Q1")], "Same", merge_action("Q1", "Q2"), "Different");
        assert_eq!(t["id"], "3");
        assert_eq!(t["controls"][0]["entries"][0]["api_action"]["fromid"], "Q2");
        assert_eq!(t["controls"][0]["entries"][2]["label"], "Different");
        assert_eq!(serde_json::to_string(&logged()).unwrap(), "[]");
    }
}
