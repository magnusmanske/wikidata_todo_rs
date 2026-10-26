//! `unused_images_of_people.php`: a random sample of files in the Commons
//! category tree of people that no wiki uses.

use crate::cache::TimedCache;
use crate::replicas::{linktarget_ids, placeholders};
use crate::state::AppState;
use crate::web::{AppError, render};
use crate::wiki::{Wiki, encode_title};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use rand::seq::SliceRandom;
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Params as SqlParams, Value};

const ROOT: &str = "People";
const DEPTH: u32 = 2;
const MAX_FILES: usize = 100;
/// Categories per query; small, so that a few queries find enough.
const CHUNK: usize = 500;
const NS_CATEGORY: i64 = 14;

static TREE: TimedCache<Vec<u64>> = TimedCache::new(Duration::from_secs(24 * 3600));

struct File {
    name: String,
    url: String,
    thumbnail: String,
}

#[derive(Template)]
#[template(path = "tools/unused_images_of_people.html")]
struct Page {
    title: &'static str,
    files: Vec<File>,
}

pub async fn handler(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let files = unused_files(&state).await?;
    let files = files
        .into_iter()
        .map(|title| {
            let encoded = encode_title(&title);
            File {
                name: title.replace('_', " "),
                url: format!("https://commons.wikimedia.org/wiki/File:{encoded}"),
                thumbnail: format!("https://commons.wikimedia.org/wiki/Special:FilePath/{encoded}?width=180"),
            }
        })
        .collect();
    render(&Page { title: "Unused images of people", files })
}

/// Unused files of random categories of the tree.
async fn unused_files(state: &AppState) -> anyhow::Result<Vec<String>> {
    let commons = Wiki::commons();
    let tree = TREE
        .get_or_fetch(async || {
            let categories = state.replicas.category_tree(&commons, ROOT, DEPTH).await?;
            let mut conn = state.replicas.conn(&commons, &["linktarget"]).await?;
            let categories: Vec<&str> = categories.iter().map(String::as_str).collect();
            linktarget_ids(&mut conn, NS_CATEGORY, &categories).await
        })
        .await?;
    let mut targets = tree.to_vec();
    targets.shuffle(&mut rand::rng());
    let mut conn = state.replicas.conn(&commons, &["page", "categorylinks", "globalimagelinks"]).await?;
    let mut files: Vec<String> = vec![];
    for chunk in targets.chunks(CHUNK) {
        let sql = format!(
            "SELECT DISTINCT page_title FROM page JOIN categorylinks ON cl_from=page_id
             WHERE cl_target_id IN ({}) AND cl_type='file' AND page_namespace=6 AND page_is_redirect=0
             AND NOT EXISTS (SELECT 1 FROM globalimagelinks WHERE gil_to=page_title)
             LIMIT {}",
            placeholders(chunk.len()),
            MAX_FILES - files.len()
        );
        let params = chunk.iter().map(|id| Value::from(*id)).collect();
        let rows: Vec<String> = conn.exec(sql, SqlParams::Positional(params)).await?;
        files.extend(rows.into_iter().filter(|f| !files.contains(f)).collect::<Vec<_>>());
        if files.len() >= MAX_FILES {
            break;
        }
    }
    Ok(files)
}
