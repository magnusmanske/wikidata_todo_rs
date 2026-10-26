//! `sparql_rc.php`: the changes in a time range to the items, lexemes and
//! properties of a SPARQL query (its first variable). Per changed entity, the
//! diff from before its first to its last edit in the range, and who edited
//! it; then the unchanged ones. As a page, or as JSON (`format=json`) in the
//! shape of the PHP tool, which other tools read.

use crate::mediawiki;
use crate::replicas::{entity_page_title, entity_pages, format_timestamp, select_in};
use crate::state::AppState;
use crate::terms;
use crate::timestamp::{self, Timestamp};
use crate::web::{AppError, Params, api_json, render, thousands};
use crate::wiki::{Wiki, encode_title};
use crate::wikidata;
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// Entities of the query that are looked at.
const MAX_ITEMS: usize = 100_000;
/// Changed entities that get a diff; the others get a link to one.
const MAX_DIFFS: usize = 500;
/// Unchanged entities listed on the page (JSON has them all).
const MAX_UNCHANGED_SHOWN: usize = 10_000;
/// Diffs loaded at a time.
const CONCURRENCY: usize = 4;
const LANGUAGES: &[&str] = &["en", "de", "fr", "es", "it", "nl", "sv", "ru", "zh"];
const TABLES: &[&str] = &["page", "revision", "actor", "actor_revision", "comment_revision", "user_groups"];

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    sparql: String,
    #[serde(default)]
    start: String,
    #[serde(default)]
    end: String,
    #[serde(default)]
    no_bots: String,
    #[serde(default)]
    skip_unchanged: String,
    #[serde(default)]
    sort_mode: String,
    #[serde(default)]
    user_lang: String,
    #[serde(default)]
    format: String,
}

/// A checkbox as PHP read it.
fn flag(value: &str) -> bool {
    !matches!(value.trim(), "" | "0")
}

/// Someone who edited an entity in the range.
#[derive(Debug, Clone)]
struct Editor {
    actor: u64,
    /// 0 for unregistered users, as in the PHP tool.
    user_id: u64,
    name: String,
    edits: u64,
}

impl Editor {
    fn url(&self) -> String {
        let page = if self.user_id == 0 { "Special:Contributions/" } else { "User:" };
        format!("https://www.wikidata.org/wiki/{page}{}", encode_title(&self.name))
    }
}

#[derive(Debug, Clone, Default)]
struct Revision {
    id: u64,
    parent: u64,
    timestamp: String,
    actor: u64,
    comment_id: u64,
}

#[derive(Debug)]
enum Diff {
    /// As the API gave it.
    Html(String),
    /// The edits undid each other; with the last edit summary.
    None(String),
    Failed(String),
    NotLoaded,
}

#[derive(Debug)]
struct Change {
    id: String,
    label: Option<String>,
    /// The first and last edit in the range.
    first: Revision,
    last: Revision,
    /// Fewest edits first, as in the PHP tool.
    editors: Vec<Editor>,
    diff: Diff,
}

impl Change {
    fn title(&self) -> String {
        entity_page_title(&self.id)
    }

    fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }

    fn created(&self) -> bool {
        self.first.parent == 0
    }

    /// The revision before the first edit; for a new entity, its first.
    fn old_id(&self) -> u64 {
        if self.created() { self.first.id } else { self.first.parent }
    }

    fn diff_url(&self) -> String {
        format!(
            "https://www.wikidata.org/w/index.php?title={}&type=revision&diff={}&oldid={}",
            self.title(),
            self.last.id,
            self.old_id()
        )
    }

    fn reverted(&self) -> bool {
        matches!(self.diff, Diff::None(_)) && !self.created()
    }

    fn last_editor(&self) -> Option<&Editor> {
        self.editors.iter().find(|e| e.actor == self.last.actor)
    }

    fn before(&self) -> String {
        format_timestamp(&self.first.timestamp)
    }

    fn after(&self) -> String {
        format_timestamp(&self.last.timestamp)
    }

    /// The diff as HTML, links absolute and opening in a new tab.
    fn diff_html(&self) -> Option<String> {
        let Diff::Html(html) = &self.diff else { return None };
        Some(
            html.replace(" href=\"//", " href=\"https://")
                .replace(" href=\"/", " href=\"https://www.wikidata.org/")
                .replace("<a ", "<a target=\"_blank\" "),
        )
    }
}

#[derive(Debug)]
struct Unchanged {
    id: String,
    label: Option<String>,
}

struct Report {
    total: usize,
    start: Timestamp,
    end: Timestamp,
    changes: Vec<Change>,
    unchanged: Vec<Unchanged>,
    skip_unchanged: bool,
}

impl Report {
    fn capped(&self) -> bool {
        self.total > MAX_ITEMS
    }

    fn unchanged_shown(&self) -> &[Unchanged] {
        self.unchanged.get(..MAX_UNCHANGED_SHOWN).unwrap_or(&self.unchanged)
    }
}

/// Why there is no report; the messages are the JSON statuses of the PHP tool.
enum Failure {
    Problem(&'static str),
    Error(AppError),
}

impl<E: Into<AppError>> From<E> for Failure {
    fn from(e: E) -> Self {
        Self::Error(e.into())
    }
}

#[derive(Template)]
#[template(path = "tools/sparql_rc.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    no_bots: bool,
    skip_unchanged: bool,
    sort_by_label: bool,
    help: &'static str,
    max_items: String,
    max_diffs: usize,
    max_unchanged: usize,
    error: Option<&'static str>,
    report: Option<Report>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let json = request.format == "json";
    if !json && (request.sparql.trim().is_empty() || request.start.trim().is_empty()) {
        return show(&request, Ok(None));
    }
    let outcome = match run(&state, &request).await {
        Ok(report) => Ok(report),
        Err(Failure::Problem(problem)) => Err(problem),
        Err(Failure::Error(e)) => return Err(e),
    };
    if json {
        return Ok(api_json(answer(outcome), None));
    }
    show(&request, outcome.map(Some))
}

fn show(request: &Request, outcome: Result<Option<Report>, &'static str>) -> Result<Response, AppError> {
    let (report, error) = match outcome {
        Ok(report) => (report, None),
        Err(problem) => (None, Some(problem)),
    };
    let page = Page {
        title: "SPARQL recent changes",
        request,
        no_bots: flag(&request.no_bots),
        skip_unchanged: flag(&request.skip_unchanged),
        sort_by_label: request.sort_mode == "label",
        help: timestamp::HELP,
        max_items: thousands(MAX_ITEMS as u64),
        max_diffs: MAX_DIFFS,
        max_unchanged: MAX_UNCHANGED_SHOWN,
        error,
        report,
    };
    Ok(render(&page)?.into_response())
}

async fn run(state: &AppState, request: &Request) -> Result<Report, Failure> {
    let start = Timestamp::parse(&request.start).ok_or(Failure::Problem("Invalid start date"))?;
    let end = match request.end.trim() {
        "" => Timestamp::now(),
        end => Timestamp::parse(end).ok_or(Failure::Problem("Invalid end date"))?,
    };
    let found = state
        .sparql
        .entities(&request.sparql, None, MAX_ITEMS)
        .await
        .map_err(|_| Failure::Problem("SPARQL query has failed"))?;
    if found.ids.is_empty() {
        return Err(Failure::Problem("SPARQL query has no results"));
    }
    let (ids, total) = (found.ids, found.total);
    let languages = terms::languages_with(&request.user_lang, LANGUAGES);
    let skip_unchanged = flag(&request.skip_unchanged);

    let mut conn = state.replicas.conn(&Wiki::wikidata(), TABLES).await?;
    let pages: HashMap<u64, String> = entity_pages(&mut conn, &ids).await?.into_iter().collect();
    let bots = if flag(&request.no_bots) { bot_actors(&mut conn).await? } else { HashSet::new() };
    let changes = changes(&mut conn, &pages, &start, &end, &bots).await?;
    let changed: HashSet<u64> = changes.iter().map(|(page, _)| *page).collect();
    let mut changes: Vec<Change> = changes.into_iter().map(|(_, c)| c).collect();
    changes.sort_by(|a, b| b.last.timestamp.cmp(&a.last.timestamp));
    let unchanged: HashSet<&String> =
        pages.iter().filter(|(page, _)| !changed.contains(page)).map(|(_, id)| id).collect();
    let mut unchanged: Vec<Unchanged> = match skip_unchanged {
        true => vec![],
        // In the order of the query.
        false => {
            ids.iter().filter(|id| unchanged.contains(id)).map(|id| Unchanged { id: id.clone(), label: None }).collect()
        }
    };

    // Labels: from the term store; for the latest changed entities without
    // one there, and lexemes, from the API.
    let mut wanted: Vec<String> = changes.iter().map(|c| c.id.clone()).collect();
    wanted.extend(unchanged.iter().map(|u| u.id.clone()));
    let mut labels = terms::labels(&state.replicas, &wanted, &languages).await?;
    let missing: Vec<String> = changes.iter().map(|c| c.id.clone()).filter(|id| !labels.contains_key(id)).collect();
    labels.extend(api_labels(state, &missing, &languages).await);
    for change in &mut changes {
        change.label = labels.get(&change.id).cloned();
    }
    for entity in &mut unchanged {
        entity.label = labels.get(&entity.id).cloned();
    }
    if request.sort_mode == "label" {
        changes.sort_by_cached_key(|c| c.label().to_lowercase());
    }
    load_diffs(state, &mut conn, &mut changes).await?;
    Ok(Report { total, start, end, changes, unchanged, skip_unchanged })
}

/// The actors of bots.
async fn bot_actors(conn: &mut Conn) -> anyhow::Result<HashSet<u64>> {
    let sql = "SELECT actor_id FROM actor JOIN user_groups ON ug_user=actor_user WHERE ug_group='bot'";
    Ok(conn.query::<u64, _>(sql).await?.into_iter().collect())
}

/// The changed pages, with their first and last revisions in the range and
/// their editors (not those in `skip`); diffs and labels still missing.
async fn changes(
    conn: &mut Conn,
    pages: &HashMap<u64, String>,
    start: &Timestamp,
    end: &Timestamp,
    skip: &HashSet<u64>,
) -> anyhow::Result<Vec<(u64, Change)>> {
    let ids: Vec<u64> = pages.keys().copied().collect();
    let sql = "SELECT rev_page, rev_actor, COUNT(*), MIN(rev_id), MAX(rev_id) FROM revision
               WHERE rev_timestamp BETWEEN ? AND ? AND rev_page IN";
    let range = [start.as_str().into(), end.as_str().into()];
    let rows: Vec<(u64, u64, u64, u64, u64)> =
        select_in(conn, sql, "GROUP BY rev_page, rev_actor", &range, &ids).await?;
    let per_page = per_page(rows.into_iter().filter(|(_, actor, ..)| !skip.contains(actor)));

    let revision_ids: Vec<u64> = per_page.values().flat_map(|(_, first, last)| [*first, *last]).collect();
    let sql = "SELECT rev_id, rev_parent_id, rev_timestamp, rev_actor, rev_comment_id FROM revision WHERE rev_id IN";
    let rows: Vec<(u64, u64, String, u64, u64)> = select_in(conn, sql, "", &[], &revision_ids).await?;
    let revisions: HashMap<u64, Revision> = rows
        .into_iter()
        .map(|(id, parent, timestamp, actor, comment_id)| (id, Revision { id, parent, timestamp, actor, comment_id }))
        .collect();

    let actors: Vec<u64> = per_page
        .values()
        .flat_map(|(editors, ..)| editors.iter().map(|(actor, _)| *actor))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let sql = "SELECT actor_id, actor_user, actor_name FROM actor_revision WHERE actor_id IN";
    let rows: Vec<(u64, Option<u64>, String)> = select_in(conn, sql, "", &[], &actors).await?;
    let users: HashMap<u64, (u64, String)> =
        rows.into_iter().map(|(actor, user, name)| (actor, (user.unwrap_or_default(), name))).collect();

    Ok(per_page
        .into_iter()
        .filter_map(|(page, (editors, first, last))| {
            let mut editors: Vec<Editor> = editors
                .into_iter()
                .map(|(actor, edits)| {
                    let (user_id, name) = users.get(&actor).cloned().unwrap_or_default();
                    Editor { actor, user_id, name, edits }
                })
                .collect();
            editors.sort_by(|a, b| a.edits.cmp(&b.edits).then_with(|| a.name.cmp(&b.name)));
            let change = Change {
                id: pages.get(&page)?.clone(),
                label: None,
                first: revisions.get(&first)?.clone(),
                last: revisions.get(&last)?.clone(),
                editors,
                diff: Diff::NotLoaded,
            };
            Some((page, change))
        })
        .collect())
}

/// Per page: `(actor, edits)`, the first and the last revision ID.
type PageEdits = (Vec<(u64, u64)>, u64, u64);

/// `(page, actor, edits, first, last)` rows, per page.
fn per_page(rows: impl Iterator<Item = (u64, u64, u64, u64, u64)>) -> BTreeMap<u64, PageEdits> {
    let mut pages: BTreeMap<u64, PageEdits> = BTreeMap::new();
    for (page, actor, edits, first, last) in rows {
        let entry = pages.entry(page).or_insert((vec![], first, last));
        entry.0.push((actor, edits));
        entry.1 = entry.1.min(first);
        entry.2 = entry.2.max(last);
    }
    pages
}

/// Labels (for lexemes: lemmas) from the API, for at most [`MAX_DIFFS`]
/// entities. Failures leave the IDs in place of labels.
async fn api_labels(state: &AppState, ids: &[String], languages: &[&str]) -> HashMap<String, String> {
    let ids: Vec<String> = ids.iter().take(MAX_DIFFS).cloned().collect();
    let entities = match wikidata::entities(&state.http, &ids, "labels").await {
        Ok(entities) => entities,
        Err(e) => {
            tracing::warn!("cannot load labels: {e:#}");
            return HashMap::new();
        }
    };
    entities
        .into_iter()
        .filter_map(|(id, entity)| {
            let label = wikidata::label(&entity, languages)
                .or_else(|| wikimisc::labels::json_term_or_any(&entity, "lemmas", languages))?;
            Some((id, label.to_string()))
        })
        .collect()
}

/// The diffs of the first [`MAX_DIFFS`] changes, and the last edit summary where the edits cancel out.
async fn load_diffs(state: &AppState, conn: &mut Conn, changes: &mut [Change]) -> anyhow::Result<()> {
    let wanted: Vec<(usize, u64, u64)> =
        changes.iter().take(MAX_DIFFS).enumerate().map(|(i, c)| (i, c.old_id(), c.last.id)).collect();
    let diffs: Vec<(usize, Diff)> = stream::iter(wanted)
        .map(|(i, from, to)| async move { (i, diff(&state.http, from, to).await) })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    for (i, diff) in diffs {
        if let Some(change) = changes.get_mut(i) {
            change.diff = diff;
        }
    }
    let comment_ids: Vec<u64> =
        changes.iter().filter(|c| matches!(c.diff, Diff::None(_))).map(|c| c.last.comment_id).collect();
    let sql = "SELECT comment_id, comment_text FROM comment_revision WHERE comment_id IN";
    let rows: Vec<(u64, Vec<u8>)> = select_in(conn, sql, "", &[], &comment_ids).await?;
    let comments: HashMap<u64, Vec<u8>> = rows.into_iter().collect();
    for change in changes.iter_mut() {
        if let (Diff::None(comment), Some(text)) = (&mut change.diff, comments.get(&change.last.comment_id)) {
            *comment = String::from_utf8_lossy(text).into_owned();
        }
    }
    Ok(())
}

async fn diff(http: &reqwest::Client, from: u64, to: u64) -> Diff {
    let (from, to) = (from.to_string(), to.to_string());
    let params = [("action", "compare"), ("fromrev", from.as_str()), ("torev", to.as_str())];
    match mediawiki::api_patiently(http, &Wiki::wikidata(), &params).await {
        Ok(json) => match json["compare"]["body"].as_str() {
            Some("") => Diff::None(String::new()),
            Some(body) => Diff::Html(body.to_string()),
            None => Diff::Failed(json["error"]["info"].as_str().unwrap_or("no diff").to_string()),
        },
        Err(e) => Diff::Failed(format!("{e:#}")),
    }
}

/// The JSON answer of the PHP tool.
#[derive(Serialize)]
struct Answer {
    status: String,
    items: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_items: Option<usize>,
}

#[derive(Serialize)]
struct JsonEditor<'a> {
    user_id: String,
    user_text: &'a str,
    edits: u64,
}

#[derive(Serialize)]
struct JsonChange<'a> {
    id: String,
    changed: bool,
    label: &'a str,
    diff_url: String,
    created: bool,
    reverted: bool,
    ts_before: &'a str,
    ts_after: &'a str,
    editors: Vec<JsonEditor<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    msg: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    comment: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff: Option<&'a str>,
}

#[derive(Serialize)]
struct JsonUnchanged<'a> {
    id: &'a str,
    changed: bool,
    label: &'a str,
}

fn answer(outcome: Result<Report, &'static str>) -> Answer {
    let report = match outcome {
        Ok(report) => report,
        Err(problem) => return Answer { status: problem.to_string(), items: vec![], total_items: None },
    };
    let mut items: Vec<serde_json::Value> = report.changes.iter().filter_map(|c| json_change(c).ok()).collect();
    if !report.skip_unchanged {
        items.extend(report.unchanged.iter().filter_map(|u| {
            let label = u.label.as_deref().unwrap_or(&u.id);
            serde_json::to_value(JsonUnchanged { id: &u.id, changed: false, label }).ok()
        }));
    }
    Answer { status: "OK".to_string(), items, total_items: Some(report.total) }
}

fn json_change(change: &Change) -> serde_json::Result<serde_json::Value> {
    let mut json = JsonChange {
        id: change.title(),
        changed: true,
        label: change.label(),
        diff_url: change.diff_url(),
        created: change.created(),
        reverted: change.reverted(),
        ts_before: &change.first.timestamp,
        ts_after: &change.last.timestamp,
        editors: change
            .editors
            .iter()
            .map(|e| JsonEditor { user_id: e.user_id.to_string(), user_text: &e.name, edits: e.edits })
            .collect(),
        msg: None,
        comment: None,
        user_id: None,
        user_text: None,
        diff: None,
    };
    match &change.diff {
        Diff::Html(html) => json.diff = Some(html),
        Diff::None(comment) if json.reverted => {
            let editor = change.last_editor();
            json.comment = Some(comment);
            json.user_id = Some(editor.map_or(0, |e| e.user_id).to_string());
            json.user_text = editor.map(|e| e.name.as_str());
        }
        Diff::None(_) => {}
        Diff::Failed(message) => json.msg = Some(message),
        Diff::NotLoaded => json.msg = Some("diff not loaded: too many changed items"),
    }
    serde_json::to_value(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    fn change(parent: u64, diff: Diff) -> Change {
        let revision = |id, parent, timestamp: &str| Revision {
            id,
            parent,
            timestamp: timestamp.to_string(),
            actor: 9,
            comment_id: 0,
        };
        Change {
            id: "L7".to_string(),
            label: None,
            first: revision(10, parent, "20240501000000"),
            last: revision(12, 11, "20240502000000"),
            editors: vec![Editor { actor: 9, user_id: 3, name: "A b".to_string(), edits: 2 }],
            diff,
        }
    }

    #[test]
    fn edits_per_page() {
        let pages = per_page([(1, 5, 2, 10, 20), (1, 6, 1, 8, 9), (2, 5, 1, 30, 30)].into_iter());
        assert_eq!(pages[&1], (vec![(5, 2), (6, 1)], 8, 20));
        assert_eq!(pages[&2], (vec![(5, 1)], 30, 30));
    }

    #[test]
    fn json_shape() {
        let json = json_change(&change(9, Diff::None("Reverted edits".to_string()))).unwrap();
        assert_eq!(
            json.to_string(),
            r#"{"id":"Lexeme:L7","changed":true,"label":"L7","diff_url":"https://www.wikidata.org/w/index.php?title=Lexeme:L7&type=revision&diff=12&oldid=9","created":false,"reverted":true,"ts_before":"20240501000000","ts_after":"20240502000000","editors":[{"user_id":"3","user_text":"A b","edits":2}],"comment":"Reverted edits","user_id":"3","user_text":"A b"}"#
        );
        let created = json_change(&change(0, Diff::Html("<tr></tr>".to_string()))).unwrap();
        assert_eq!((created["created"].as_bool(), created["diff"].as_str()), (Some(true), Some("<tr></tr>")));
        assert!(created["diff_url"].as_str().unwrap().ends_with("oldid=10"));
        let failed = answer(Err("SPARQL query has no results"));
        assert_eq!(serde_json::to_string(&failed).unwrap(), r#"{"status":"SPARQL query has no results","items":[]}"#);
    }

    #[test]
    fn diff_links() {
        let c = change(9, Diff::Html(r#"<a href="/wiki/Q5">Q5</a><a href="//x.org/">x</a>"#.to_string()));
        assert_eq!(
            c.diff_html().unwrap(),
            r#"<a target="_blank" href="https://www.wikidata.org/wiki/Q5">Q5</a><a target="_blank" href="https://x.org/">x</a>"#
        );
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/sparql_rc.php").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"sparql\""));
        let bad = get(dir.path(), "/sparql_rc/?sparql=x&start=soon").await;
        assert!(bad.body.contains("Invalid start date"));
        let json = get(dir.path(), "/sparql_rc.php?format=json&sparql=x").await;
        assert_eq!(json.body, r#"{"status":"Invalid start date","items":[]}"#);
    }
}
