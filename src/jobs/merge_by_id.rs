//! The "merge by shared identifier" game's data (`wikidata_merge_by_id_p`):
//! pairs of items with the same value for an external ID that should be
//! unique, unless they are marked as different from each other (P1889).

use super::Writes;
use crate::replicas::{placeholders, select_in};
use crate::sparql::entity_id;
use crate::state::AppState;
use crate::tool_db::Database;
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// Properties whose values are legitimately shared.
const IGNORED: &[&str] = &["P709", "P638", "P484", "P594", "P705", "P1566", "P2249"];
/// The width of the `value` column.
const MAX_VALUE_CHARS: usize = 32;
const WRITE_CHUNK: usize = 1000;
/// From this many open pairs on, a result with less than half of them is suspect.
const MIN_OPEN_FOR_CHECK: usize = 100;

pub async fn update(state: &AppState, only: &[String], writes: Writes) -> Result<()> {
    let properties = match only.is_empty() {
        true => {
            // External IDs with a "distinct values" constraint.
            let query = "SELECT ?p { ?p wikibase:propertyType wikibase:ExternalId ; wdt:P2302 wd:Q21502410 }";
            state.sparql.entities(query, None, usize::MAX).await?.ids
        }
        false => only.to_vec(),
    };
    let mut tool = state.tool_db.conn(Database::MergeById).await?;
    let (mut done, mut failed) = (0, 0);
    for property in properties.iter().filter(|p| !IGNORED.contains(&p.as_str())) {
        let Ok(number) = property.trim_start_matches('P').parse::<u64>() else { continue };
        match update_property(state, &mut tool, property, number, writes).await {
            Ok(()) => done += 1,
            Err(e) => {
                // Mostly query timeouts on huge properties; their pairs stay as they were.
                eprintln!("{property}: {e:#}");
                failed += 1;
            }
        }
    }
    println!("{done} properties updated, {failed} failed");
    Ok(())
}

async fn update_property(state: &AppState, tool: &mut Conn, property: &str, number: u64, writes: Writes) -> Result<()> {
    let pairs = pairs(state, property).await?;
    let existing: Vec<(u64, u64, u64, String)> =
        tool.exec("SELECT id, q1, q2, done FROM candidates WHERE prop=?", (number,)).await?;
    let known: HashSet<(u64, u64)> = existing.iter().map(|(_, q1, q2, _)| (*q1, *q2)).collect();

    // Pairs are unique across properties, so also check those of others.
    let unknown: Vec<(u64, u64)> = pairs.keys().filter(|p| !known.contains(p)).copied().collect();
    let first: Vec<u64> = unknown.iter().map(|(q1, _)| *q1).collect();
    let elsewhere: HashSet<(u64, u64)> =
        select_in(tool, "SELECT q1, q2 FROM candidates WHERE q1 IN", "", &[], &first).await?.into_iter().collect();
    let new: Vec<(u64, u64)> = unknown.into_iter().filter(|p| !elsewhere.contains(p)).collect();
    for chunk in new.chunks(WRITE_CHUNK) {
        let sql = format!(
            "INSERT IGNORE INTO candidates (q1,q2,prop,`value`,random,tag) VALUES {}",
            vec!["(?,?,?,?,RAND(),0)"; chunk.len()].join(",")
        );
        let params: Vec<wikimisc::mysql_async::Value> = chunk
            .iter()
            .flat_map(|(q1, q2)| {
                let value = pairs.get(&(*q1, *q2)).cloned().unwrap_or_default();
                [(*q1).into(), (*q2).into(), number.into(), value.into()]
            })
            .collect();
        writes.exec(tool, &sql, params).await?;
    }

    // Open pairs that no longer share the value; unless so many vanished
    // at once that the result looks incomplete.
    let open = existing.iter().filter(|(.., done)| done == "OPEN").count();
    if open >= MIN_OPEN_FOR_CHECK && pairs.len() * 2 < open {
        bail!("only {} pairs, against {open} open ones: not deleting any", pairs.len());
    }
    let gone: Vec<u64> = existing
        .iter()
        .filter(|(_, q1, q2, done)| done == "OPEN" && !pairs.contains_key(&(*q1, *q2)))
        .map(|(id, ..)| *id)
        .collect();
    for chunk in gone.chunks(WRITE_CHUNK) {
        let sql = format!("DELETE FROM candidates WHERE id IN ({})", placeholders(chunk.len()));
        writes.exec(tool, &sql, chunk.to_vec()).await?;
    }
    Ok(())
}

/// `(q1, q2) → value` for items sharing a value of `property`, `q1 < q2`.
async fn pairs(state: &AppState, property: &str) -> Result<HashMap<(u64, u64), String>> {
    let query = format!(
        "SELECT ?q1 ?q2 ?value {{ ?q1 wdt:{property} ?value . ?q2 wdt:{property} ?value .
         FILTER(STR(?q1) < STR(?q2)) MINUS {{ ?q1 wdt:P1889 ?q2 }} MINUS {{ ?q2 wdt:P1889 ?q1 }} }}"
    );
    let mut csv = state.sparql.heavy_csv(&query).await?;
    let mut pairs = HashMap::new();
    for row in csv.records() {
        let row = row?;
        let number = |i: usize| row.get(i).and_then(entity_id).and_then(|id| id.strip_prefix('Q')?.parse::<u64>().ok());
        // WDQS can break off a result midway, with an error message in it.
        let (Some(a), Some(b), Some(value)) = (number(0), number(1), row.get(2)) else {
            bail!("unreadable result row, the result may be incomplete: {row:?}");
        };
        pairs.insert((a.min(b), a.max(b)), value.chars().take(MAX_VALUE_CHARS).collect());
    }
    Ok(pairs)
}
