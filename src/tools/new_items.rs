//! `new_items.php`: the newest items, created at or before a time (or now),
//! as a table or as a new PagePile.
//!
//! Page IDs grow with creation, so the newest items are those with the
//! highest page IDs up to that of the last page created by then; their first
//! revisions say who created them, and when.

use crate::pagepile;
use crate::replicas::{display_title, format_timestamp, select_in};
use crate::state::AppState;
use crate::terms::{self, DEFAULT_LANGUAGES};
use crate::timestamp::{self, Timestamp};
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_title};
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;

const MAX_ITEMS: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    timestamp: String,
    max: Option<usize>,
    #[serde(default = "default_format")]
    format: String,
    doit: Option<String>,
}

fn default_format() -> String {
    "html".to_string()
}

struct Item {
    id: String,
    label: Option<String>,
    creator: String,
    creator_url: String,
    created: String,
}

#[derive(Template)]
#[template(path = "tools/new_items.html")]
struct Page {
    title: &'static str,
    timestamp: String,
    max: usize,
    pagepile: bool,
    max_items: usize,
    help: &'static str,
    items: Option<Vec<Item>>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let max = request.max.unwrap_or(100).clamp(1, MAX_ITEMS);
    let mut page = Page {
        title: "New items",
        timestamp: request.timestamp.trim().to_string(),
        max,
        pagepile: request.format == "pagepile",
        max_items: MAX_ITEMS,
        help: timestamp::HELP,
        items: None,
    };
    if request.doit.is_none() {
        return Ok(render(&page)?.into_response());
    }
    let before = match page.timestamp.as_str() {
        "" => None,
        ts => {
            Some(Timestamp::parse(ts).ok_or_else(|| AppError::BadRequest(format!("Timestamp: {}", timestamp::HELP)))?)
        }
    };
    let items = newest(&state, before.as_ref(), max).await?;
    if page.pagepile {
        let titles: Vec<String> = items.into_iter().map(|i| i.id).collect();
        let id = pagepile::create(&state.http, "wikidatawiki", &titles).await?;
        return Ok(Redirect::to(&pagepile::url(id)).into_response());
    }
    page.items = Some(items);
    Ok(render(&page)?.into_response())
}

/// The `max` newest items created at or before `before` (or now), newest first.
async fn newest(state: &AppState, before: Option<&Timestamp>, max: usize) -> Result<Vec<Item>, AppError> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page", "revision", "actor_revision"]).await?;
    let last_page: Option<u64> = match before {
        Some(before) => {
            let sql = "SELECT rev_page FROM revision WHERE rev_parent_id=0 AND rev_timestamp<=?
                       ORDER BY rev_timestamp DESC LIMIT 1";
            let page: Option<u64> = conn.exec_first(sql, (before.as_str(),)).await.map_err(anyhow::Error::from)?;
            // Nothing was created by then.
            let Some(page) = page else { return Ok(vec![]) };
            Some(page)
        }
        None => None,
    };
    let sql = format!(
        "SELECT page_id, page_title FROM page WHERE page_namespace=0 AND page_is_redirect=0 AND page_id<=?
         ORDER BY page_id DESC LIMIT {max}"
    );
    let pages: Vec<(u64, String)> =
        conn.exec(sql, (last_page.unwrap_or(u64::MAX),)).await.map_err(anyhow::Error::from)?;
    let ids: Vec<u64> = pages.iter().map(|(id, _)| *id).collect();
    let sql = "SELECT MIN(rev_id) FROM revision WHERE rev_page IN";
    let first_revisions: Vec<u64> = select_in(&mut conn, sql, "GROUP BY rev_page", &[], &ids).await?;
    let sql = "SELECT rev_page, rev_timestamp, actor_name FROM revision
               JOIN actor_revision ON actor_id=rev_actor WHERE rev_id IN";
    let rows: Vec<(u64, String, String)> = select_in(&mut conn, sql, "", &[], &first_revisions).await?;
    let creations: HashMap<u64, (String, String)> =
        rows.into_iter().map(|(page, ts, user)| (page, (ts, user))).collect();
    let titles: Vec<String> = pages.iter().map(|(_, title)| title.clone()).collect();
    let mut labels = terms::labels(&state.replicas, &titles, DEFAULT_LANGUAGES).await?;
    Ok(pages
        .into_iter()
        .map(|(page, id)| {
            let (created, creator) = creations.get(&page).cloned().unwrap_or_default();
            Item {
                label: labels.remove(&id),
                creator_url: format!("https://www.wikidata.org/wiki/Special:Contributions/{}", encode_title(&creator)),
                creator: display_title(&creator),
                created: format_timestamp(&created),
                id,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/new_items/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"timestamp\""));
        assert_eq!(get(dir.path(), "/new_items/?doit=1&timestamp=soon").await.status.as_u16(), 400);
        assert_eq!(get(dir.path(), "/new_items/?doit=1&max=many").await.status.as_u16(), 400);
    }
}
