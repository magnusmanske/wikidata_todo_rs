//! `sourcerer_game.php`: a game. Web pages that may support a Wikidata
//! statement (found by Sourcerer, in `sourcerer_p`) are shown with the
//! statement; the player adds the ones that do as references (via Widar),
//! then marks the statement done (`set_done`).

use crate::state::AppState;
use crate::tool_db::Database;
use crate::web::{AppError, Params, render};
use crate::wikidata;
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

const LANGUAGES: &[&str] = &["en", "de", "es", "it", "fr"];
/// Suggestions tried for one page: some statements are gone from Wikidata
/// (the PHP tried forever).
const MAX_TRIES: usize = 5;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Request {
    set_done: Option<String>,
    sid: Option<String>,
    text: String,
}

struct Source {
    /// Only web addresses are links.
    link: Option<String>,
    url: String,
    server: String,
    /// HTML: escaped, but for the `<b>` around the matching words.
    blurbs: Vec<String>,
    /// `YYYYMMDD`, when the page was read.
    date: String,
}

struct Game {
    item: String,
    item_label: String,
    property: String,
    property_label: String,
    /// The statement's ID, as Wikidata has it now.
    statement: String,
    text: String,
    sources: Vec<Source>,
}

#[derive(Template)]
#[template(path = "tools/sourcerer_game.html")]
struct Page {
    title: &'static str,
    game: Option<Game>,
    message: Option<String>,
}

/// A suggestion: the item (numeric), statement ID and the statement's value as text.
type Suggestion = (u64, String, String);

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let mut db = state.tool_db.conn(Database::Sourcerer).await?;
    if request.set_done.is_some() {
        let sid = request.sid.unwrap_or_default();
        db.exec_drop(SET_DONE, (sid.trim(), request.text.as_str())).await.map_err(anyhow::Error::from)?;
        return Ok(([(header::CONTENT_TYPE, "text/plain")], "{}").into_response());
    }
    let sid = request.sid.filter(|s| !s.trim().is_empty());
    let tries = if sid.is_some() { 1 } else { MAX_TRIES };
    let mut message = None;
    let mut game = None;
    for _ in 0..tries {
        let Some(suggestion) = suggestion(&mut db, sid.as_deref()).await? else {
            message = Some(match sid {
                Some(_) => "There is no suggestion for this statement.".to_string(),
                None => "There are no suggestions left to check.".to_string(),
            });
            break;
        };
        game = load_game(&state, &mut db, suggestion).await?;
        if game.is_some() {
            break;
        }
        message = Some("The statement is no longer on Wikidata.".to_string());
    }
    let message = if game.is_some() { None } else { message };
    Ok(render(&Page { title: "Sourcerer game", game, message })?.into_response())
}

const SET_DONE: &str = "UPDATE suggestions SET status=1 WHERE statement_id=? AND statement_text=? AND status=0";

/// The given statement's suggestion, or a random open one.
async fn suggestion(db: &mut Conn, sid: Option<&str>) -> anyhow::Result<Option<Suggestion>> {
    let row: Option<(u64, Vec<u8>, Vec<u8>)> = match sid {
        Some(sid) => {
            let sql = "SELECT q, statement_id, statement_text FROM suggestions WHERE statement_id=? LIMIT 1";
            db.exec_first(sql, (sid.trim(),)).await?
        }
        None => {
            let sql = "SELECT q, statement_id, statement_text FROM suggestions WHERE rand>=? AND status=0
                ORDER BY rand LIMIT 1";
            let from: f64 = rand::random();
            match db.exec_first(sql, (from,)).await? {
                Some(row) => Some(row),
                None => db.exec_first(sql, (0.0,)).await?,
            }
        }
    };
    Ok(row.map(|(q, sid, text)| (q, string(sid), string(text))))
}

/// The statement with its suggested sources; `None` if it is gone from Wikidata.
async fn load_game(state: &AppState, db: &mut Conn, (q, sid, text): Suggestion) -> anyhow::Result<Option<Game>> {
    let item = format!("Q{q}");
    let Some(entity) = wikidata::entity(&state.http, &item, "labels|claims").await? else { return Ok(None) };
    let Some((property, statement)) = find_statement(&entity, &sid) else { return Ok(None) };
    let property_label = match wikidata::entity(&state.http, &property, "labels").await? {
        Some(p) => wikidata::label(&p, LANGUAGES).unwrap_or(&property).to_string(),
        None => property.clone(),
    };
    let sql = "SELECT source_url, source_blurb, source_date FROM suggestions
        WHERE q=? AND statement_id=? AND statement_text=? ORDER BY id";
    let rows: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> = db.exec(sql, (q, &sid, &text)).await?;
    let rows = rows.into_iter().map(|(url, blurb, date)| (string(url), string(blurb), string(date)));
    Ok(Some(Game {
        item_label: wikidata::label(&entity, LANGUAGES).unwrap_or(&item).to_string(),
        item,
        property,
        property_label,
        statement,
        text,
        sources: sources(rows),
    }))
}

/// The property and current ID of statement `sid` (older IDs start with a lower-case `q`).
fn find_statement(entity: &Value, sid: &str) -> Option<(String, String)> {
    entity["claims"].as_object()?.iter().find_map(|(property, statements)| {
        let id =
            statements.as_array()?.iter().filter_map(|s| s["id"].as_str()).find(|id| id.eq_ignore_ascii_case(sid))?;
        Some((property.clone(), id.to_string()))
    })
}

/// The suggestions by source URL, in order.
fn sources(rows: impl Iterator<Item = (String, String, String)>) -> Vec<Source> {
    let mut sources: Vec<Source> = vec![];
    for (url, blurb, date) in rows {
        let blurb = highlighted(&blurb);
        match sources.iter_mut().find(|s| s.url == url) {
            Some(source) => {
                source.blurbs.push(blurb);
                source.date = date;
            }
            None => {
                let parsed = reqwest::Url::parse(&url).ok().filter(|u| ["http", "https"].contains(&u.scheme()));
                let server = parsed.as_ref().and_then(|u| u.host_str()).unwrap_or(&url).to_string();
                sources.push(Source { link: parsed.map(|_| url.clone()), url, server, blurbs: vec![blurb], date });
            }
        }
    }
    sources
}

/// A text excerpt (from HTML, with entities) as HTML: escaped, but with
/// the `<b>` that mark what matched.
fn highlighted(blurb: &str) -> String {
    let text = wikimisc::escape::unescape_html(blurb);
    wikimisc::escape::html(&text).replace("&lt;b&gt;", "<b>").replace("&lt;/b&gt;", "</b>")
}

fn string(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn statements() {
        let entity = json!({"claims": {"P21": [{"id": "Q229840$E78391D6"}], "P106": [{"id": "Q229840$03198261"}]}});
        assert_eq!(
            find_statement(&entity, "q229840$e78391d6"),
            Some(("P21".to_string(), "Q229840$E78391D6".to_string()))
        );
        assert_eq!(find_statement(&entity, "Q1$x"), None);
    }

    #[test]
    fn sources_by_url() {
        let row = |url: &str, blurb: &str, date: &str| (url.to_string(), blurb.to_string(), date.to_string());
        let sources = sources(
            [
                row("https://viaf.org/viaf/1/", "Gender: <b>Female</b>", "20150601"),
                row("javascript:alert(1)", "<script>", ""),
                row("https://viaf.org/viaf/1/", "<i>author</i>&nbsp;&lt;b&#039;", "20150602"),
            ]
            .into_iter(),
        );
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].server, "viaf.org");
        assert_eq!(sources[0].blurbs, ["Gender: <b>Female</b>", "&lt;i&gt;author&lt;/i&gt;\u{a0}&lt;b&#39;"]);
        assert_eq!(sources[0].date, "20150602");
        assert_eq!(sources[1].link, None);
        assert_eq!(sources[1].blurbs, ["&lt;script&gt;"]);
    }
}
