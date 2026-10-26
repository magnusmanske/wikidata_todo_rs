//! `bioacoustica.php`: the files from BioAcoustica on Commons, by the
//! species in their names, matched to Wikidata taxa (by taxon name, P225),
//! and whether those have a sound file (P51) yet.

use crate::sparql;
use crate::state::AppState;
use crate::web::{AppError, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use futures::{StreamExt, TryStreamExt, stream};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};

const CATEGORY: &str = "Files_from_BioAcoustica";
const NS_FILE: i64 = 6;
/// Names per query.
const CHUNK: usize = 300;
const CONCURRENCY: usize = 4;

/// `BioAcoustica 01 - Pedostibes hosei (short).flac`: the species, up to the first digit, dot or dash.
static SPECIES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^BioAcoustica [0-9 \-]+ (.+?)[0-9.\-]").expect("valid regex"));
static PARENTHESES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(.+?\)\s*").expect("valid regex"));

struct Taxon {
    item: String,
    has_audio: bool,
}

struct Row {
    species: Option<String>,
    taxa: Vec<Taxon>,
    files: Vec<String>,
}

#[derive(Template)]
#[template(path = "tools/bioacoustica.html")]
struct Page {
    title: &'static str,
    files: usize,
    rows: Vec<Row>,
}

pub async fn handler(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let files = state.replicas.pages_in_tree(&Wiki::commons(), CATEGORY, 0, NS_FILE).await?;
    let total = files.len();
    let by_species = by_species(files);
    let names: Vec<&str> = by_species.keys().filter_map(|s| s.as_deref()).collect();
    let taxa = taxa(&state, &names).await?;
    let mut rows: Vec<Row> = by_species
        .into_iter()
        .map(|(species, files)| {
            let taxa = species
                .as_ref()
                .and_then(|s| taxa.get(s))
                .map(|t| t.iter().map(|(item, &has_audio)| Taxon { item: item.clone(), has_audio }).collect())
                .unwrap_or_default();
            Row { species, taxa, files }
        })
        .collect();
    // The files of no recognisable species last.
    let unidentified = rows.iter().take_while(|r| r.species.is_none()).count();
    rows.rotate_left(unidentified);
    render(&Page { title: "BioAcoustica file matcher", files: total, rows })
}

/// File names (with spaces) by species; `None` for no recognisable species.
fn by_species(files: Vec<String>) -> BTreeMap<Option<String>, Vec<String>> {
    let mut by_species: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
    for file in files {
        let name = file.replace('_', " ");
        by_species.entry(species(&name)).or_default().push(name);
    }
    by_species
}

fn species(file: &str) -> Option<String> {
    let file = file.replace(" MHV ", " ");
    let species = SPECIES.captures(&file)?.get(1)?.as_str().trim();
    let species = crate::wiki::upper_first(species);
    Some(PARENTHESES.replace_all(&species, " ").trim().to_string()).filter(|s| !s.is_empty())
}

/// The items with each taxon name, and whether they have a sound file.
async fn taxa(state: &AppState, names: &[&str]) -> anyhow::Result<HashMap<String, BTreeMap<String, bool>>> {
    let queries: Vec<String> = names
        .chunks(CHUNK)
        .map(|chunk| {
            let names: Vec<String> = chunk.iter().map(|n| sparql::literal(n)).collect();
            format!(
                "SELECT ?name ?q ?audio {{ VALUES ?name {{ {} }} ?q wdt:P225 ?name OPTIONAL {{ ?q wdt:P51 ?audio }} }}",
                names.join(" ")
            )
        })
        .collect();
    let sparql = &state.sparql;
    let results: Vec<Value> =
        stream::iter(queries).map(|q| async move { sparql.json(&q).await }).buffered(CONCURRENCY).try_collect().await?;
    let mut taxa: HashMap<String, BTreeMap<String, bool>> = HashMap::new();
    for row in results.iter().flat_map(sparql::bindings) {
        let (Some(name), Some(item)) =
            (row["name"]["value"].as_str(), row["q"]["value"].as_str().and_then(sparql::entity_id))
        else {
            continue;
        };
        let has_audio = taxa.entry(name.to_string()).or_default().entry(item.to_string()).or_default();
        *has_audio |= row.get("audio").is_some();
    }
    Ok(taxa)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn species_in_file_names() {
        assert_eq!(
            species("BioAcoustica 01 - Leptobrachium heteropus.flac").as_deref(),
            Some("Leptobrachium heteropus")
        );
        assert_eq!(species("BioAcoustica 01 - Pedostibes hosei (short).flac").as_deref(), Some("Pedostibes hosei"));
        assert_eq!(species("BioAcoustica 12 MHV gryllus campestris 3.flac").as_deref(), Some("Gryllus campestris"));
        assert_eq!(species("Something else.flac"), None);
        let groups = by_species(vec!["X.flac".into(), "BioAcoustica 2 - A b.flac".into()]);
        assert_eq!(groups.keys().collect::<Vec<_>>(), [&None, &Some("A b".to_string())]);
    }
}
