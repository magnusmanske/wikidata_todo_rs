//! `artwork_images.php`: for some random artists with a Creator page on
//! Commons, their works on Wikidata without an image, next to the images of
//! their works on Commons that have no Wikidata item; to match them up.

use crate::cache::TimedCache;
use crate::replicas::{display_title, placeholders, select_in};
use crate::sparql::{self, Sparql};
use crate::state::AppState;
use crate::terms;
use crate::web::{AppError, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use rand::seq::IndexedRandom;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Params as SqlParams, Value};

const SHOWN: usize = 10;
/// Artists looked at, to find some with images.
const CANDIDATES: usize = 50;
const MAX_FILES: usize = 5000;
/// Works and files shown per artist.
const MAX_SHOWN: usize = 200;
const NS_CREATOR: i64 = 100;
const NS_CATEGORY: i64 = 14;
const CATEGORY: &str = "Artworks_without_Wikidata_item";
/// Labels in the first of these that has one, else in any language.
const LANGUAGES: &[&str] = &["en", "de", "fr", "es", "it", "pl", "pt", "ja", "ru", "hu", "nl"];

/// `(item, Creator page as database title)` of all artists that have one.
static CREATORS: TimedCache<Vec<(String, String)>> = TimedCache::new(Duration::from_secs(24 * 3600));

struct Work {
    item: String,
    label: String,
    described_at: Option<String>,
}

struct Artist {
    item: String,
    name: String,
    works: Vec<Work>,
    files: Vec<String>,
    /// How many more there are than shown.
    more_works: usize,
    more_files: usize,
}

#[derive(Template)]
#[template(path = "tools/artwork_images.html")]
struct Page {
    title: &'static str,
    artists: Vec<Artist>,
}

pub async fn handler(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let creators = CREATORS.get_or_fetch(async || creators(&state.sparql).await).await?;
    let candidates: Vec<&(String, String)> = creators.choose_multiple(&mut rand::rng(), CANDIDATES).collect();
    let pages: Vec<String> = candidates.iter().map(|(_, page)| page.clone()).collect();
    let mut files = files_without_items(&state, &pages).await?;
    let artists: Vec<&(String, String)> =
        candidates.into_iter().filter(|(_, page)| files.contains_key(page)).take(SHOWN).collect();
    let items: Vec<String> = artists.iter().map(|(item, _)| item.clone()).collect();
    let mut works = works_without_image(&state, &items).await?;
    let artists = artists
        .into_iter()
        .map(|(item, page)| {
            let (works, more_works) = shown(works.remove(item).unwrap_or_default());
            let (files, more_files) = shown(files.remove(page).unwrap_or_default());
            Artist { item: item.clone(), name: display_title(page), works, files, more_works, more_files }
        })
        .collect();
    render(&Page { title: "Artwork images", artists })
}

/// The first [`MAX_SHOWN`], and how many more there are.
fn shown<T>(mut list: Vec<T>) -> (Vec<T>, usize) {
    let more = list.len().saturating_sub(MAX_SHOWN);
    list.truncate(MAX_SHOWN);
    (list, more)
}

async fn creators(sparql: &Sparql) -> anyhow::Result<Vec<(String, String)>> {
    let json = sparql.json("SELECT ?creator ?page { ?creator wdt:P1472 ?page }").await?;
    Ok(sparql::bindings(&json)
        .filter_map(|row| {
            let item = sparql::entity_id(row["creator"]["value"].as_str()?)?;
            let page = row["page"]["value"].as_str()?;
            Some((item.to_string(), crate::replicas::db_title(page)))
        })
        .collect())
}

/// Files that link to the Creator pages and await a Wikidata item, by Creator page.
async fn files_without_items(state: &AppState, pages: &[String]) -> anyhow::Result<HashMap<String, Vec<String>>> {
    let mut conn = state.replicas.conn(&Wiki::commons(), &["linktarget", "pagelinks", "categorylinks", "page"]).await?;
    let sql = "SELECT lt_id, lt_title FROM linktarget WHERE lt_namespace=? AND lt_title IN";
    let targets: HashMap<u64, String> =
        select_in(&mut conn, sql, "", &[NS_CREATOR.into()], pages).await?.into_iter().collect();
    let category: Option<u64> = conn
        .exec_first("SELECT lt_id FROM linktarget WHERE lt_namespace=? AND lt_title=?", (NS_CATEGORY, CATEGORY))
        .await?;
    let (Some(category), false) = (category, targets.is_empty()) else { return Ok(HashMap::new()) };
    // From the links to the Creator pages, not from the millions of files in the category.
    let sql = format!(
        "SELECT pl_target_id, page_title FROM pagelinks
         STRAIGHT_JOIN categorylinks ON cl_from=pl_from STRAIGHT_JOIN page ON page_id=pl_from
         WHERE pl_from_namespace=6 AND cl_target_id=? AND pl_target_id IN ({}) LIMIT {MAX_FILES}",
        placeholders(targets.len())
    );
    let mut params = vec![Value::from(category)];
    params.extend(targets.keys().map(|id| Value::from(*id)));
    let rows: Vec<(u64, String)> = conn.exec(sql, SqlParams::Positional(params)).await?;
    let mut files: HashMap<String, Vec<String>> = HashMap::new();
    for (target, file) in rows {
        if let Some(page) = targets.get(&target) {
            files.entry(page.clone()).or_default().push(display_title(&file));
        }
    }
    Ok(files)
}

/// The works of each artist that have no image, with labels.
async fn works_without_image(state: &AppState, artists: &[String]) -> anyhow::Result<HashMap<String, Vec<Work>>> {
    if artists.is_empty() {
        return Ok(HashMap::new());
    }
    let values: Vec<String> = artists.iter().map(|q| format!("wd:{q}")).collect();
    let query = format!(
        "SELECT ?creator ?work (SAMPLE(?url) AS ?described) {{ VALUES ?creator {{ {} }} ?work wdt:P170 ?creator . \
         MINUS {{ ?work wdt:P18 [] }} OPTIONAL {{ ?work wdt:P973 ?url }} }} GROUP BY ?creator ?work",
        values.join(" ")
    );
    let json = state.sparql.json(&query).await?;
    let rows: Vec<(String, String, Option<String>)> = sparql::bindings(&json)
        .filter_map(|row| {
            let id = |var: &str| Some(sparql::entity_id(row[var]["value"].as_str()?)?.to_string());
            Some((
                id("creator")?,
                id("work")?,
                row["described"]["value"].as_str().filter(|u| u.starts_with("http")).map(str::to_string),
            ))
        })
        .collect();
    let items: Vec<String> = rows.iter().map(|(_, work, _)| work.clone()).collect();
    let mut labels = terms::labels(&state.replicas, &items, LANGUAGES).await?;
    // Else in any language.
    let unlabelled: Vec<String> = items.iter().filter(|i| !labels.contains_key(*i)).cloned().collect();
    if !unlabelled.is_empty() {
        for term in terms::terms(&state.replicas, &unlabelled, &[terms::TermType::Label], &[]).await? {
            labels.entry(term.entity).or_insert(term.text);
        }
    }
    let mut works: HashMap<String, Vec<Work>> = HashMap::new();
    for (creator, item, described_at) in rows {
        let label = labels.get(&item).cloned().unwrap_or_default();
        works.entry(creator).or_default().push(Work { item, label, described_at });
    }
    for list in works.values_mut() {
        list.sort_by(|a, b| a.label.cmp(&b.label));
    }
    Ok(works)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped() {
        assert_eq!(shown(vec![1, 2]), (vec![1, 2], 0));
        assert_eq!(shown((0..MAX_SHOWN + 5).collect::<Vec<_>>()).1, 5);
    }
}
