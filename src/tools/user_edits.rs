//! `user_edits.php`: a user's edits to items, newest first; optionally only
//! to the items of a SPARQL query, in a date range, page creations, or with
//! edit summaries matching a regular expression. As a table, as text, or as
//! JSON lines (streamed, not capped).

use crate::replicas::{entity_pages, format_timestamp};
use crate::state::AppState;
use crate::terms;
use crate::timestamp::{self, Timestamp};
use crate::web::{AppError, Params, query_body, render, thousands};
use crate::wiki::{Wiki, encode_title};
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock};
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Params as SqlParams, Value};

/// Edits shown as a table or text.
const MAX_ROWS: usize = 5000;
/// Edits scanned for a table or text, when a pattern filters them.
const MAX_SCANNED: usize = 200_000;
/// Items of the query that count.
const MAX_ITEMS: usize = 100_000;
/// Page IDs per query.
const CHUNK: usize = 5000;
const LANGUAGES: &[&str] = &["en", "de", "nl", "fr", "es", "it", "zh"];
/// Size limit of a compiled pattern.
const PATTERN_SIZE: usize = 1 << 20;
/// The actions of Wikibase's automatic edit summaries.
const ACTIONS: &[&str] = &[
    "wbcreateclaim-create",
    "wbcreateredirect",
    "wbeditentity-create",
    "wbeditentity-override",
    "wbeditentity-update",
    "wbmergeitems-from",
    "wbmergeitems-to",
    "wbremoveclaims-remove",
    "wbsetaliases-add",
    "wbsetaliases-remove",
    "wbsetaliases-set",
    "wbsetaliases-update",
    "wbsetclaim-create",
    "wbsetclaim-update",
    "wbsetclaimvalue",
    "wbsetdescription-add",
    "wbsetdescription-remove",
    "wbsetdescription-set",
    "wbsetlabel-add",
    "wbsetlabeldescriptionaliases",
    "wbsetlabel-remove",
    "wbsetlabel-set",
    "wbsetqualifier-add",
    "wbsetqualifier-update",
    "wbsetreference-add",
    "wbsetsitelink-add",
    "wbsetsitelink-remove",
    "wbsetsitelink-set",
    "wbsetsitelink-set-both",
];

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_format")]
    format: String,
    #[serde(default)]
    sparql: String,
    #[serde(default)]
    user: String,
    #[serde(default)]
    pattern: String,
    #[serde(default)]
    start: String,
    #[serde(default)]
    end: String,
    #[serde(default)]
    only_page_creations: String,
    doit: Option<String>,
}

fn default_format() -> String {
    "html".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Html,
    Text,
    JsonLines,
}

/// The columns the queries select.
type Row = (u64, String, u64, String, u8, u8, Option<u64>, u64, u64, Vec<u8>);

/// An edit, as a JSON line: the fields of the PHP tool, values as strings as it had them.
#[derive(Debug, Serialize)]
struct Edit {
    page_id: String,
    page_title: String,
    page_namespace: String,
    rev_id: String,
    rev_page: String,
    rev_actor: String,
    rev_user: String,
    rev_user_text: String,
    rev_timestamp: String,
    rev_minor_edit: String,
    rev_deleted: String,
    rev_len: Option<String>,
    rev_parent_id: String,
    rev_comment_id: String,
    rev_comment: String,
}

/// The user, as the edits need them.
struct User {
    actor: u64,
    id: u64,
    name: String,
}

impl Edit {
    fn new(row: Row, user: &User) -> Self {
        let (page, title, rev, timestamp, minor, deleted, len, parent, comment_id, comment) = row;
        Self {
            page_id: page.to_string(),
            page_title: title,
            page_namespace: "0".to_string(),
            rev_id: rev.to_string(),
            rev_page: page.to_string(),
            rev_actor: user.actor.to_string(),
            rev_user: user.id.to_string(),
            rev_user_text: user.name.clone(),
            rev_timestamp: timestamp,
            rev_minor_edit: minor.to_string(),
            rev_deleted: deleted.to_string(),
            rev_len: len.map(|len| len.to_string()),
            rev_parent_id: parent.to_string(),
            rev_comment_id: comment_id.to_string(),
            rev_comment: String::from_utf8_lossy(&comment).into_owned(),
        }
    }
}

struct Shown {
    timestamp: String,
    item: String,
    label: String,
    /// Escaped, with item and property links.
    comment_html: String,
    comment: String,
    rev_id: String,
    parent_id: String,
    diff_url: String,
    undo_url: String,
}

struct Results {
    edits: Vec<Shown>,
    capped: bool,
}

impl Results {
    /// Tab-separated: timestamp, item, label, edit summary, revision ID, parent revision ID.
    fn text(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n'], " ");
        self.edits
            .iter()
            .map(|e| {
                let (label, comment) = (clean(&e.label), clean(&e.comment));
                format!("{}\t{}\t{label}\t{comment}\t{}\t{}\n", e.timestamp, e.item, e.rev_id, e.parent_id)
            })
            .collect()
    }
}

#[derive(Template)]
#[template(path = "tools/user_edits.html")]
struct Page {
    title: &'static str,
    request: Request,
    format: Format,
    creations: bool,
    actions: &'static [&'static str],
    help: &'static str,
    max_rows: String,
    max_scanned: String,
    max_items: String,
    error: Option<String>,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let format = match request.format.as_str() {
        "text" => Format::Text,
        "jsonl" => Format::JsonLines,
        _ => Format::Html,
    };
    let mut page = Page {
        title: "User edits",
        format,
        creations: !matches!(request.only_page_creations.as_str(), "" | "0"),
        request,
        actions: ACTIONS,
        help: timestamp::HELP,
        max_rows: thousands(MAX_ROWS as u64),
        max_scanned: thousands(MAX_SCANNED as u64),
        max_items: thousands(MAX_ITEMS as u64),
        error: None,
        results: None,
    };
    if page.request.doit.is_none() {
        return Ok(render(&page)?.into_response());
    }
    let query = match parse(&page) {
        Ok(query) => query,
        Err(message) => return error(page, message),
    };
    let mut conn =
        state.replicas.conn(&Wiki::wikidata(), &["actor", "page", "revision_userindex", "comment_revision"]).await?;
    let Some(user) = user(&mut conn, &query.user).await? else {
        return error(page, format!("Unknown user {}", query.user));
    };
    let pages = match page.request.sparql.trim() {
        "" => None,
        sparql => {
            let items = match state.sparql.entities(sparql, None).await {
                Ok(items) => items,
                Err(e) => return error(page, format!("The SPARQL query failed: {e:#}")),
            };
            let items: Vec<String> = items.into_iter().filter(|id| id.starts_with('Q')).take(MAX_ITEMS).collect();
            if items.is_empty() {
                return error(page, "The SPARQL query found no items.".to_string());
            }
            Some(entity_pages(&mut conn, &items).await?.into_iter().map(|(page, _)| page).collect::<Vec<_>>())
        }
    };
    let limit = match (format, &query.pattern) {
        (Format::JsonLines, _) => None,
        (_, None) => Some(MAX_ROWS),
        (_, Some(_)) => Some(MAX_SCANNED),
    };
    let queries = queries(&query, user.actor, pages.as_deref(), limit);
    if format == Format::JsonLines {
        let body = query_body(conn, queries, String::new(), String::new(), move |row: Row, _| {
            if !query.matches(&row.9) {
                return None;
            }
            serde_json::to_string(&Edit::new(row, &user)).ok().map(|line| line + "\n")
        })
        .await?;
        return Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response());
    }
    let (rows, capped) = collect(&mut conn, queries, &query).await?;
    page.results = Some(shown(&state, rows, capped).await?);
    Ok(render(&page)?.into_response())
}

fn error(mut page: Page, message: String) -> Result<Response, AppError> {
    if page.format == Format::JsonLines {
        return Err(AppError::BadRequest(message));
    }
    page.error = Some(message);
    Ok(render(&page)?.into_response())
}

/// What to look for.
struct Query {
    user: String,
    pattern: Option<Regex>,
    start: Option<Timestamp>,
    end: Option<Timestamp>,
    creations: bool,
}

impl Query {
    fn matches(&self, comment: &[u8]) -> bool {
        self.pattern.as_ref().is_none_or(|p| p.is_match(&String::from_utf8_lossy(comment)))
    }
}

fn parse(page: &Page) -> Result<Query, String> {
    let request = &page.request;
    let user = crate::wiki::upper_first(&request.user.trim().replace('_', " "));
    if user.is_empty() {
        return Err("Please give a user.".to_string());
    }
    let pattern = match request.pattern.trim() {
        "" => None,
        pattern => Some(
            RegexBuilder::new(pattern).size_limit(PATTERN_SIZE).build().map_err(|e| format!("Invalid pattern: {e}"))?,
        ),
    };
    let time = |name: &str, input: &str| match input.trim() {
        "" => Ok(None),
        input => Timestamp::parse(input).map(Some).ok_or_else(|| format!("{name}: {}", timestamp::HELP)),
    };
    Ok(Query {
        user,
        pattern,
        start: time("Start date", &request.start)?,
        end: time("End date", &request.end)?,
        creations: page.creations,
    })
}

async fn user(conn: &mut Conn, name: &str) -> anyhow::Result<Option<User>> {
    let row: Option<(u64, Option<u64>)> =
        conn.exec_first("SELECT actor_id, actor_user FROM actor WHERE actor_name=?", (name,)).await?;
    Ok(row.map(|(actor, id)| User { actor, id: id.unwrap_or_default(), name: name.to_string() }))
}

/// The queries for the edits: one per chunk of `pages`, if given.
fn queries(query: &Query, actor: u64, pages: Option<&[u64]>, limit: Option<usize>) -> Vec<(String, SqlParams)> {
    // Without STRAIGHT_JOIN, the database starts from the pages, and takes ages.
    let mut sql =
        "SELECT STRAIGHT_JOIN page_id, page_title, rev_id, rev_timestamp, rev_minor_edit, rev_deleted, rev_len,
        rev_parent_id, rev_comment_id, comment_text
        FROM revision_userindex JOIN page ON page_id=rev_page JOIN comment_revision ON comment_id=rev_comment_id
        WHERE rev_actor=? AND page_namespace=0"
            .to_string();
    let mut params = vec![Value::from(actor)];
    if let Some(start) = &query.start {
        sql += " AND rev_timestamp>=?";
        params.push(start.as_str().into());
    }
    if let Some(end) = &query.end {
        sql += " AND rev_timestamp<=?";
        params.push(end.as_str().into());
    }
    if query.creations {
        sql += " AND rev_parent_id=0";
    }
    let tail = match limit {
        Some(limit) => format!("ORDER BY rev_timestamp DESC LIMIT {limit}"),
        None => "ORDER BY rev_timestamp DESC".to_string(),
    };
    let Some(pages) = pages else { return vec![(format!("{sql} {tail}"), SqlParams::Positional(params))] };
    pages
        .chunks(CHUNK)
        .map(|chunk| {
            let sql = format!("{sql} AND rev_page IN ({}) {tail}", crate::replicas::placeholders(chunk.len()));
            let mut params = params.clone();
            params.extend(chunk.iter().map(|page| Value::from(*page)));
            (sql, SqlParams::Positional(params))
        })
        .collect()
}

/// The matching edits, up to [`MAX_ROWS`], newest first; and whether there may be more.
async fn collect(
    conn: &mut Conn,
    queries: Vec<(String, SqlParams)>,
    query: &Query,
) -> anyhow::Result<(Vec<Row>, bool)> {
    let mut found = vec![];
    let mut capped = false;
    for (sql, params) in queries {
        let mut scanned = 0;
        let mut rows = conn.exec_stream::<Row, _, _>(sql, params).await?;
        while let Some(row) = rows.next().await {
            let row = row?;
            scanned += 1;
            if query.matches(&row.9) {
                found.push(row);
            }
            if found.len() >= MAX_ROWS {
                break;
            }
        }
        // A query that hit its limit may have missed some.
        capped |= found.len() >= MAX_ROWS || scanned >= MAX_SCANNED;
        if found.len() >= MAX_ROWS {
            break;
        }
    }
    found.sort_by(|a, b| b.3.cmp(&a.3));
    Ok((found, capped))
}

async fn shown(state: &AppState, rows: Vec<Row>, capped: bool) -> anyhow::Result<Results> {
    let mut items: Vec<String> = rows.iter().map(|row| row.1.clone()).collect();
    items.sort_unstable();
    items.dedup();
    let labels = terms::labels(&state.replicas, &items, LANGUAGES).await?;
    let edits = rows
        .into_iter()
        .map(|(_, item, rev, timestamp, _, _, _, parent, _, comment)| {
            let comment = String::from_utf8_lossy(&comment).into_owned();
            let title = encode_title(&item);
            Shown {
                timestamp: format_timestamp(&timestamp),
                label: labels.get(&item).cloned().unwrap_or_else(|| item.clone()),
                comment_html: comment_html(&comment),
                comment,
                rev_id: rev.to_string(),
                parent_id: parent.to_string(),
                diff_url: format!("https://www.wikidata.org/w/index.php?title={title}&diff={rev}&oldid={parent}"),
                undo_url: format!(
                    "https://www.wikidata.org/w/index.php?title={title}&action=edit&undoafter={parent}&undo={rev}"
                ),
                item,
            }
        })
        .collect();
    Ok(Results { edits, capped })
}

/// An edit summary as HTML: escaped, its `[[Q1]]` and `[[Property:P1]]` as links.
fn comment_html(comment: &str) -> String {
    static LINK: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\[\[((?:Property:P|Q)\d+)\]\]").ok());
    let escaped = wikimisc::escape::html(comment);
    match LINK.as_ref() {
        Some(link) => {
            link.replace_all(&escaped, r#"[[<a href="https://www.wikidata.org/wiki/$1">$1</a>]]"#).into_owned()
        }
        None => escaped.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn summaries_get_links() {
        assert_eq!(
            comment_html("/* wbsetclaim-create:2||1 */ [[Property:P31]]: [[Q5]] <b>"),
            "/* wbsetclaim-create:2||1 */ [[<a href=\"https://www.wikidata.org/wiki/Property:P31\">Property:P31</a>]]: \
             [[<a href=\"https://www.wikidata.org/wiki/Q5\">Q5</a>]] &lt;b&gt;"
        );
    }

    #[test]
    fn queries_per_chunk() {
        let query =
            Query { user: "X".into(), pattern: None, start: Timestamp::parse("2024"), end: None, creations: true };
        let all = queries(&query, 7, None, Some(10));
        assert_eq!(all.len(), 1);
        assert!(all[0].0.contains("rev_timestamp>=? AND rev_parent_id=0 ORDER BY rev_timestamp DESC LIMIT 10"));
        let pages: Vec<u64> = (0..CHUNK as u64 + 1).collect();
        assert_eq!(queries(&query, 7, Some(&pages), None).len(), 2);
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/user_edits/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"pattern\""));
        let bad = get(dir.path(), "/user_edits/?doit=1&user=X&pattern=(").await;
        assert!(bad.body.contains("Invalid pattern"));
        let bad = get(dir.path(), "/user_edits.php?doit=1&user=X&start=soon&format=jsonl").await;
        assert_eq!(bad.status.as_u16(), 400);
    }
}
