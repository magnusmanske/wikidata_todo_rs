//! `duplicity_game_api.php`: the Distributed Game "Match new articles to
//! items", on Duplicity's candidates: is this article about this item?

use super::distributed_game::{self as game, Decision, Request};
use crate::replicas::placeholders;
use crate::state::AppState;
use crate::tool_db::Database;
use crate::web::Params;
use crate::wikidata;
use axum::extract::State;
use axum::response::Response;
use serde_json::{Value, json};
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Params as SqlParams, Value as SqlValue};

/// Below this many open candidates, the game is told it runs low.
const LOW: u64 = 200;
/// Shown with Wikispecies tiles: botanist and zoologist author abbreviations.
const SPECIES_PROPERTIES: [&str; 2] = ["P428", "P835"];

pub async fn handler(State(state): State<Arc<AppState>>, Params(request): Params<Request>) -> Response {
    let answer = match request.action.as_str() {
        "desc" => Ok(desc()),
        "tiles" => tiles(&state, &request).await,
        "log_action" => log_action(&state, &request).await,
        _ => Ok(game::no_action()),
    };
    game::respond(answer, request.callback)
}

fn desc() -> Value {
    json!({
        "label": {"en": "Match new articles to items"},
        "description": {"en": "Decide if a Wikipedia article and a Wikidata item describe the same entity. Using data from the duplicity tool."},
        "icon": "https://upload.wikimedia.org/wikipedia/commons/thumb/7/71/Grant_DeVolson_Wood_-_American_Gothic.jpg/100px-Grant_DeVolson_Wood_-_American_Gothic.jpg",
        "options": [{
            "name": "Wiki",
            "key": "wikitype",
            "values": {"lang": "Your main language", "species": "WikiSpecies", "commons": "Wikimedia Commons"},
        }],
    })
}

/// The wiki of the articles: the user's language's Wikipedia, or the chosen project.
fn wiki(request: &Request) -> String {
    match request.wikitype.as_str() {
        "species" => "specieswiki".to_string(),
        "commons" => "commonswiki".to_string(),
        _ => {
            let language: String =
                request.lang.chars().filter(|c| c.is_ascii_lowercase() || *c == '_' || *c == '-').collect();
            format!("{language}wiki")
        }
    }
}

/// A candidate: `(candidate ID, item number, article title, wiki)`.
type Candidate = (u64, u64, String, String);

async fn tiles(state: &AppState, request: &Request) -> anyhow::Result<Value> {
    let wiki = wiki(request);
    let mut conn = state.tool_db.conn(Database::Duplicity).await?;
    let left: u64 = conn
        .exec_first(
            "SELECT COUNT(*) FROM candidates INNER JOIN no_wd ON no_wd.id=no_wd_id WHERE checked=0 AND wiki=?",
            (&wiki,),
        )
        .await?
        .unwrap_or_default();
    let num = request.num();
    let mut exclude = request.in_cache_ids();
    // From a random point on, wrapping around at the end.
    let start: f64 = rand::random();
    let mut found = candidates(&mut conn, &wiki, (">=", start), &exclude, num).await?;
    if found.len() < num {
        exclude.extend(found.iter().map(|c| c.0));
        found.extend(candidates(&mut conn, &wiki, ("<", start), &exclude, num - found.len()).await?);
    }
    let mut tiles: Vec<Value> = found.into_iter().map(|c| tile(&c)).collect();
    if wiki == "specieswiki" {
        add_author_abbreviations(state, &mut tiles, &request.lang).await?;
    }
    let mut answer = json!({"left": left});
    if left < LOW {
        answer["low"] = json!(1);
    }
    answer["tiles"] = Value::Array(tiles);
    Ok(answer)
}

/// Up to `limit` open candidates on `wiki`, by their random number from `from` (`(">=", r)` or `("<", r)`).
async fn candidates(
    conn: &mut Conn,
    wiki: &str,
    from: (&str, f64),
    exclude: &[u64],
    limit: usize,
) -> anyhow::Result<Vec<Candidate>> {
    if limit == 0 {
        return Ok(vec![]);
    }
    let (operator, random) = from;
    let not_in = match exclude.is_empty() {
        true => String::new(),
        false => format!("AND candidates.id NOT IN ({})", placeholders(exclude.len())),
    };
    let sql = format!(
        "SELECT candidates.id, candidates.q, no_wd.title, no_wd.wiki FROM candidates
         INNER JOIN no_wd ON no_wd.id=no_wd_id
         WHERE checked=0 AND q>0 AND wiki=? AND random{operator}? {not_in} ORDER BY random LIMIT {limit}"
    );
    let mut params = vec![SqlValue::from(wiki), SqlValue::from(random)];
    params.extend(exclude.iter().map(|id| SqlValue::from(*id)));
    let rows: Vec<(u64, u64, Vec<u8>, Vec<u8>)> = conn.exec(sql, SqlParams::Positional(params)).await?;
    let text = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).into_owned();
    Ok(rows.into_iter().map(|(id, q, title, wiki)| (id, q, text(title), text(wiki))).collect())
}

fn tile((id, q, title, wiki): &Candidate) -> Value {
    let q = format!("Q{q}");
    let sections = vec![json!({"type": "wikipage", "title": title, "wiki": wiki}), game::item_section(&q)];
    let api_action = json!({"action": "wbsetsitelink", "id": q, "linksite": wiki, "linktitle": title});
    game::tile(game::id_string(*id), sections, "Yes", api_action, "No")
}

/// Wikispecies articles are mostly about taxa and taxonomists; for the
/// latter, the author abbreviations help to decide.
async fn add_author_abbreviations(state: &AppState, tiles: &mut [Value], language: &str) -> anyhow::Result<()> {
    let mut ids: Vec<String> = SPECIES_PROPERTIES.iter().map(|p| p.to_string()).collect();
    ids.extend(tiles.iter().filter_map(|t| t["sections"][1]["q"].as_str().map(str::to_string)));
    let entities = wikidata::entities(&state.http, &ids, "labels|claims").await?;
    for tile in tiles {
        let Some(item) = tile["sections"][1]["q"].as_str().and_then(|q| entities.get(q)) else { continue };
        let parts: Vec<String> = SPECIES_PROPERTIES
            .iter()
            .filter_map(|p| {
                let value = wikidata::first_string(item, p)?;
                let label = wikimisc::labels::json_term_or_any(entities.get(*p)?, "labels", &[language, "en"])?;
                Some(format!("{label}: {value}"))
            })
            .collect();
        if let (false, Some(sections)) = (parts.is_empty(), tile["sections"].as_array_mut()) {
            sections.push(json!({"type": "text", "text": parts.join("\n")}));
        }
    }
    Ok(())
}

/// Every decision marks the candidate as checked; "yes" also all other
/// candidates for the article, which now has its item.
async fn log_action(state: &AppState, request: &Request) -> anyhow::Result<Value> {
    if let (Some(id), Some(decision)) = (request.tile_id(), request.decision()) {
        let mut conn = state.tool_db.conn(Database::Duplicity).await?;
        conn.exec_drop(checked_sql(decision), (id,)).await?;
    }
    Ok(game::logged())
}

fn checked_sql(decision: Decision) -> &'static str {
    match decision {
        // MariaDB allows the table itself in the subquery of an UPDATE.
        Decision::Yes => "UPDATE candidates SET checked=1 WHERE no_wd_id=(SELECT no_wd_id FROM candidates WHERE id=?)",
        Decision::No | Decision::Skip => "UPDATE candidates SET checked=1 WHERE id=?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    fn request(query: &str) -> Request {
        serde_urlencoded::from_str(query).unwrap()
    }

    #[test]
    fn wikis() {
        assert_eq!(wiki(&request("lang=de")), "dewiki");
        assert_eq!(wiki(&request("lang=be-x_old%27;")), "be-x_oldwiki");
        assert_eq!(wiki(&request("lang=de&wikitype=species")), "specieswiki");
        assert_eq!(wiki(&request("wikitype=commons")), "commonswiki");
    }

    #[test]
    fn decisions() {
        assert!(checked_sql(Decision::Yes).contains("no_wd_id=(SELECT"));
        assert_eq!(checked_sql(Decision::Skip), "UPDATE candidates SET checked=1 WHERE id=?");
        let tile = tile(&(7, 42, "Foo bar".into(), "dewiki".into()));
        assert_eq!(tile["id"], "7");
        assert_eq!(tile["sections"][0]["title"], "Foo bar");
        assert_eq!(tile["controls"][0]["entries"][0]["api_action"]["linksite"], "dewiki");
    }

    #[tokio::test]
    async fn description_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let desc = get(dir.path(), "/duplicity_game_api.php?action=desc&callback=cb").await;
        assert_eq!(desc.status.as_u16(), 200);
        assert!(desc.body.starts_with("cb({\"label\":{\"en\":\"Match new articles to items\"}"));
        let none = get(dir.path(), "/duplicity_game_api.php").await;
        assert_eq!(none.body, r#"{"error":"No valid action!"}"#);
        let bad = get(dir.path(), "/duplicity_game_api.php?action=desc&callback=alert(1)").await;
        assert_eq!(bad.status.as_u16(), 400);
    }
}
