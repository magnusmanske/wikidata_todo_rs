//! `pasleim_projectmerge_game_api.php`: the Distributed Game "Pasleim
//! project merge": pairs of items that may be about the same topic, from
//! `pasleim_projectmerge.json` (written by a job); decisions go to a log file.

use super::distributed_game::{self as game, Decision, Request};
use crate::state::AppState;
use crate::web::Params;
use axum::extract::State;
use axum::response::Response;
use rand::seq::SliceRandom;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

const CANDIDATES_FILE: &str = "pasleim_projectmerge.json";
const LOG_FILE: &str = "pasleim_projectmerge_game.log";
/// Candidates looked at per tile asked for: some are merged already.
const SPARE: usize = 3;

pub async fn handler(State(state): State<Arc<AppState>>, Params(request): Params<Request>) -> Response {
    let dir = &state.config.public_html;
    let answer = match request.action.as_str() {
        "desc" => Ok(desc()),
        "tiles" => tiles(&state, dir, &request).await,
        "log_action" => log_action(dir, &request).await,
        _ => Ok(game::no_action()),
    };
    game::respond(answer, request.callback)
}

fn desc() -> Value {
    json!({
        "label": {"en": "Pasleim project merge"},
        "description": {"en": "Potential items to be merged, via Pasleim projectmerge."},
        "icon": "https://upload.wikimedia.org/wikipedia/commons/thumb/5/52/Merge-arrows.svg/100px-Merge-arrows.svg.png",
        "options": [],
    })
}

/// A pair of items, and its tile ID (`Q1|Q2`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pair {
    q1: String,
    q2: String,
}

impl Pair {
    fn id(&self) -> String {
        format!("{}|{}", self.q1, self.q2)
    }

    /// A tile ID back into a pair; `None` for anything else.
    fn parse(id: &str) -> Option<Self> {
        let item = |s: &str| crate::wikidata::parse_id(s, 'Q').filter(|q| q == s);
        let (q1, q2) = id.trim().split_once('|')?;
        Some(Self { q1: item(q1)?, q2: item(q2)? })
    }
}

/// The candidate pairs: `[[1, 2], …]`, item numbers as numbers or strings.
fn parse_candidates(json: &str) -> anyhow::Result<Vec<Pair>> {
    let rows: Vec<Vec<Value>> = serde_json::from_str(json)?;
    let number = |v: &Value| v.as_u64().or_else(|| v.as_str()?.parse().ok());
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(Pair { q1: format!("Q{}", number(row.first()?)?), q2: format!("Q{}", number(row.get(1)?)?) })
        })
        .collect())
}

/// The tile IDs decided on, from the log (`<id>\t<decision>` lines).
fn decided(log: &str) -> HashSet<String> {
    log.lines().filter_map(|line| line.split_once('\t')).map(|(id, _)| id.to_string()).collect()
}

async fn read_or_empty(path: &Path) -> anyhow::Result<String> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

async fn tiles(state: &AppState, dir: &Path, request: &Request) -> anyhow::Result<Value> {
    let num = request.num();
    let text = read_or_empty(&dir.join(CANDIDATES_FILE)).await?;
    let candidates = if text.trim().is_empty() { vec![] } else { parse_candidates(&text)? };
    let mut done = decided(&read_or_empty(&dir.join(LOG_FILE)).await?);
    done.extend(request.in_cache.split(',').map(|id| id.trim().to_string()));
    let picked = pick(candidates, &done, num * SPARE);
    let items: Vec<String> = picked.iter().flat_map(|p| [p.q1.clone(), p.q2.clone()]).collect();
    let merged = game::redirects(state, &items).await?;
    let tiles: Vec<Value> = picked
        .into_iter()
        .filter(|p| !merged.contains(&p.q1) && !merged.contains(&p.q2))
        .take(num)
        .map(|p| {
            let sections = vec![game::item_section(&p.q1), game::item_section(&p.q2)];
            game::tile(json!(p.id()), sections, "Same topic", game::merge_action(&p.q1, &p.q2), "Different")
        })
        .collect();
    Ok(json!({"tiles": tiles}))
}

/// Up to `n` random candidates not decided on yet.
fn pick(candidates: Vec<Pair>, done: &HashSet<String>, n: usize) -> Vec<Pair> {
    let mut open: Vec<Pair> = candidates.into_iter().filter(|p| !done.contains(&p.id())).collect();
    let (picked, _) = open.partial_shuffle(&mut rand::rng(), n);
    picked.to_vec()
}

async fn log_action(dir: &Path, request: &Request) -> anyhow::Result<Value> {
    if let (Some(pair), Some(line)) = (Pair::parse(&request.tile), request.decision().and_then(log_line)) {
        crate::append::line_once(&dir.join(LOG_FILE), &format!("{}\t{line}", pair.id())).await?;
    }
    Ok(game::logged())
}

/// The decision as logged; skipping is not.
fn log_line(decision: Decision) -> Option<&'static str> {
    match decision {
        Decision::Yes => Some("yes"),
        Decision::No => Some("no"),
        Decision::Skip => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn pairs() {
        assert!(parse_candidates(r#"[[1, 2], "x"]"#).is_err());
        let pairs = parse_candidates(r#"[[1, 2], ["3", "4"], [5]]"#).unwrap();
        assert_eq!(pairs.iter().map(Pair::id).collect::<Vec<_>>(), ["Q1|Q2", "Q3|Q4"]);
        assert_eq!(Pair::parse("Q1|Q2").map(|p| p.id()).as_deref(), Some("Q1|Q2"));
        for bad in ["Q1", "Q1|Q2\nQ3|Q4", "Q1|Q02", "Q1|P2", "1|2"] {
            assert_eq!(Pair::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn picks_open_candidates_only() {
        let pairs = parse_candidates("[[1,2],[3,4],[5,6]]").unwrap();
        let done = decided("Q1|Q2\tyes\nQ5|Q6\tno\nbroken\n");
        assert_eq!(pick(pairs.clone(), &done, 10).iter().map(Pair::id).collect::<Vec<_>>(), ["Q3|Q4"]);
        assert_eq!(pick(pairs, &HashSet::new(), 2).len(), 2);
    }

    #[tokio::test]
    async fn logs_decisions() {
        let dir = tempfile::tempdir().unwrap();
        let api = "/pasleim_projectmerge_game_api.php?action=log_action";
        for query in ["&tile=Q1|Q2&decision=yes", "&tile=Q3|Q4&decision=skip", "&tile=Q5%0A|Q6&decision=no"] {
            assert_eq!(get(dir.path(), &format!("{api}{query}")).await.body, "[]");
        }
        assert_eq!(std::fs::read_to_string(dir.path().join(LOG_FILE)).unwrap(), "Q1|Q2\tyes\n");
        // No candidates file: no tiles, and no database asked.
        let tiles = get(dir.path(), "/pasleim_projectmerge_game_api.php?action=tiles&num=3").await;
        assert_eq!(tiles.body, r#"{"tiles":[]}"#);
    }
}
