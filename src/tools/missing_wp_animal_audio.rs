//! `missing_wp_animal_audio.php`: taxa with a sound file on Wikidata
//! (P51, audio) whose article on a wiki has no sound at all.

use crate::cache::KeyedCache;
use crate::replicas::{display_title, select_in};
use crate::sparql;
use crate::state::AppState;
use crate::web::{AppError, Paging, Params, render};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use percent_encoding::percent_decode_str;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// Files with these endings are sounds.
const AUDIO: &[&str] = &[".flac", ".oga", ".ogg", ".opus", ".wav", ".mp3"];

/// Rows per page, by default and at most.
const PAGE_SIZE: usize = 100;
const MAX_PAGE_SIZE: usize = 500;
/// Lists are made once in a while: they take a query and many lookups.
static LISTS: KeyedCache<Vec<Row>> = KeyedCache::new(Duration::from_secs(3600), 100);

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    wiki: String,
    start: usize,
    size: usize,
}

impl Default for Request {
    fn default() -> Self {
        Self { wiki: "enwiki".to_string(), start: 0, size: PAGE_SIZE }
    }
}

struct Animal {
    item: String,
    file: String,
}

#[derive(Clone)]
struct Row {
    url: String,
    title: String,
    item: String,
    file: String,
}

#[derive(Template)]
#[template(path = "tools/missing_wp_animal_audio.html")]
struct Page {
    title: &'static str,
    wiki: String,
    /// `(number, row)`
    rows: Vec<(usize, Row)>,
    nav: Paging,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let dbname = request.wiki.trim().to_lowercase();
    let wiki = Wiki::from_dbname(&dbname)?;
    let list = LISTS.get_or_fetch(wiki.dbname(), async || list(&state, &wiki).await).await?;
    let size = request.size.clamp(1, MAX_PAGE_SIZE);
    let start = request.start.min(list.len());
    let rows = list.iter().cloned().enumerate().skip(start).take(size).map(|(i, row)| (i + 1, row)).collect();
    let nav = Paging::new(start, size, list.len(), &format!("wiki={}", encode_component(&dbname)));
    render(&Page { title: "Animal sounds missing on Wikipedia", wiki: dbname, rows, nav })
}

/// The articles on taxa with a sound on Wikidata, but none on the wiki.
async fn list(state: &AppState, wiki: &Wiki) -> anyhow::Result<Vec<Row>> {
    let animals = animals(state, wiki).await?;
    let titles: Vec<String> = animals.keys().cloned().collect();
    let silent = without_audio(state, wiki, &titles).await?;
    Ok(animals
        .into_iter()
        .filter(|(title, _)| silent.contains(title))
        .map(|(title, animal)| Row {
            url: wiki.page_url(&title),
            title: display_title(&title),
            item: animal.item,
            file: animal.file,
        })
        .collect())
}

/// Taxa with a sound file and an article on the wiki, by article (database title).
async fn animals(state: &AppState, wiki: &Wiki) -> anyhow::Result<BTreeMap<String, Animal>> {
    let query = format!(
        "SELECT ?animal ?file ?article {{ ?animal wdt:P31 wd:Q16521 ; wdt:P51 ?file . \
         ?article schema:about ?animal ; schema:isPartOf <https://{}/> }}",
        wiki.host()
    );
    let json = state.sparql.json(&query).await?;
    let article_prefix = format!("https://{}/wiki/", wiki.host());
    Ok(sparql::bindings(&json)
        .filter_map(|row| {
            let item = sparql::entity_id(row["animal"]["value"].as_str()?)?.to_string();
            let file = file_name(row["file"]["value"].as_str()?)?;
            let article = row["article"]["value"].as_str()?.strip_prefix(&article_prefix)?;
            let title = percent_decode_str(article).decode_utf8().ok()?.replace(' ', "_");
            Some((title, Animal { item, file }))
        })
        .collect())
}

/// The file name in a `Special:FilePath` URL.
fn file_name(url: &str) -> Option<String> {
    let (_, name) = url.split_once("/Special:FilePath/")?;
    Some(percent_decode_str(name).decode_utf8().ok()?.replace('_', " "))
}

/// Those of the articles (database titles) that exist and use no sound file.
async fn without_audio(state: &AppState, wiki: &Wiki, titles: &[String]) -> anyhow::Result<HashSet<String>> {
    let mut conn = state.replicas.conn(wiki, &["page", "imagelinks", "linktarget"]).await?;
    let sql = "SELECT page_id, page_title FROM page WHERE page_namespace=0 AND page_title IN";
    let pages: Vec<(u64, String)> = select_in(&mut conn, sql, "", &[], titles).await?;
    let ids: Vec<u64> = pages.iter().map(|(id, _)| *id).collect();
    let audio = AUDIO.iter().map(|_| "lt_title LIKE ?").collect::<Vec<_>>().join(" OR ");
    // STRAIGHT_JOIN: from these pages' links, not from all links to files.
    let sql = format!(
        "SELECT DISTINCT il_from FROM imagelinks STRAIGHT_JOIN linktarget ON il_target_id=lt_id
         WHERE lt_namespace=6 AND ({audio}) AND il_from IN"
    );
    let patterns: Vec<_> = AUDIO.iter().map(|ending| format!("%{ending}").into()).collect();
    let with_audio: HashSet<u64> = select_in(&mut conn, &sql, "", &patterns, &ids).await?.into_iter().collect();
    Ok(pages.into_iter().filter(|(id, _)| !with_audio.contains(id)).map(|(_, title)| title).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn file_names() {
        assert_eq!(
            file_name("http://commons.wikimedia.org/wiki/Special:FilePath/Bubo%20bubo%20-%20call.ogg").as_deref(),
            Some("Bubo bubo - call.ogg")
        );
        assert_eq!(file_name("http://example.org/x.ogg"), None);
    }

    #[tokio::test]
    async fn unknown_wiki() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(get(dir.path(), "/missing_wp_animal_audio/?wiki=nonsense").await.status.as_u16(), 400);
    }
}
