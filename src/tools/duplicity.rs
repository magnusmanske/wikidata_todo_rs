//! `duplicity.php`, which forwards into the Duplicity app (`duplicity/`),
//! and `duplicity/api.php`, its API: Wikipedia articles without a Wikidata
//! item (`no_wd`, kept up to date by a bot), checked against the live wiki,
//! as the app needs them. Other actions go to Widar.

use super::widar::{self, Login};
use crate::replicas::{linktarget_ids, placeholders, select_in};
use crate::state::AppState;
use crate::tool_db::Database;
use crate::web::{AppError, Params};
use crate::wiki::Wiki;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Params as SqlParams, Value as SqlValue};

/// Random candidates checked at once, and how many such rounds at most.
const RANDOM_BATCH: usize = 20;
const RANDOM_ROUNDS: usize = 5;
/// Titles per database lookup.
const TITLE_CHUNK: usize = 5000;
/// What the app may read from Wikidata through [`wikidata_api_proxy`].
const PROXY_ACTIONS: &[&str] = &["query", "wbgetentities"];
/// Short, so that bursts of requests cannot pile up here.
const PROXY_TIMEOUT: Duration = Duration::from_secs(10);
const WIKIDATA_API: &str = "https://www.wikidata.org/w/api.php";
/// The wiki tables the checks use (all on one server for Wikipedias).
const WIKI_TABLES: &[&str] = &["page", "page_props", "categorylinks", "linktarget"];

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RedirectRequest {
    wiki: String,
    page: String,
    mode: String,
    cat: String,
}

/// `duplicity.php?wiki=enwiki&page=X` → `/duplicity/#/article/enwiki/X`, as the PHP did.
pub async fn redirect(Params(request): Params<RedirectRequest>) -> Response {
    widar::redirect(&app_url(&request))
}

fn app_url(request: &RedirectRequest) -> String {
    let mode = match request.mode.as_str() {
        "cats" => "categories",
        "" if !request.page.is_empty() => "article",
        mode => mode,
    };
    let mut url = format!("/duplicity/#/{mode}");
    for part in [request.wiki.as_str(), request.cat.trim(), request.page.as_str()] {
        if !part.is_empty() {
            url.push('/');
            url.push_str(&fragment(part));
        }
    }
    url
}

/// A part of a URL fragment, with what a `Location` header cannot carry encoded.
fn fragment(part: &str) -> String {
    const UNSAFE: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>');
    percent_encoding::utf8_percent_encode(part, UNSAFE).to_string()
}

pub async fn api(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Params(params): Params<HashMap<String, String>>,
) -> Response {
    let get = |key: &str| params.get(key).map_or("", String::as_str);
    let wiki = get("wiki");
    let result = match get("action") {
        "random_nowd_page" => random_page(&state, wiki).await,
        "invalidate_page" => invalidate_page(&state, wiki, get("page")).await,
        "wikis" => wikis(&state).await,
        "articles" => {
            return articles(&state, wiki, get("category"), get("exclude_category")).await.unwrap_or_else(failure);
        }
        "stats" => stats(&state, wiki).await,
        "categories" => categories(&state, wiki, get("min_pages").trim().parse().unwrap_or(10)).await,
        "wikidata_api_proxy" => {
            return match wikidata_api_proxy(&state.http, &params).await {
                Ok(response) => response,
                Err((code, status)) => (code, Json(json!({ "status": status }))).into_response(),
            };
        }
        action => {
            let public = &state.config.public_url;
            let login = Login {
                callback: format!("{public}/duplicity/api.php?path={public}/duplicity/api.php"),
                forward: "/duplicity/#/article/enwiki".to_string(),
                authorize: "/duplicity/api.php?action=authorize".to_string(),
            };
            if let Some(response) = widar::respond(&state, &headers, &params, &login).await {
                return response;
            }
            Ok(status(format!("Unknown action: {action}")))
        }
    };
    match result {
        Ok(out) => Json(Value::Object(out)).into_response(),
        Err(e) => failure(e),
    }
}

/// An error as the app expects it: JSON with a `status`.
fn failure(e: AppError) -> Response {
    match e {
        AppError::BadRequest(message) => (StatusCode::BAD_REQUEST, Json(json!({ "status": message }))).into_response(),
        e => {
            tracing::warn!("duplicity API: {e:#}");
            let message = "The database did not answer as expected; please try again later.";
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "status": message }))).into_response()
        }
    }
}

type Answer = Result<Map<String, Value>, AppError>;

fn status(status: impl Into<String>) -> Map<String, Value> {
    Map::from_iter([("status".to_string(), Value::String(status.into()))])
}

fn ok() -> Map<String, Value> {
    status("OK")
}

/// A random article that still lacks an item; stale entries met on the way are dropped.
async fn random_page(state: &AppState, wiki: &str) -> Answer {
    let mut wiki_db = state.replicas.conn(&Wiki::from_dbname(wiki)?, WIKI_TABLES).await?;
    let mut db = state.tool_db.conn(Database::Duplicity).await?;
    for _ in 0..RANDOM_ROUNDS {
        let candidates = random_candidates(&mut db, wiki).await?;
        if candidates.is_empty() {
            break;
        }
        let titles: Vec<&str> = candidates.iter().map(|(_, title)| title.as_str()).collect();
        let open = open_articles(&mut wiki_db, &titles).await?;
        let (live, stale): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|(_, title)| open.contains(title));
        let stale: Vec<u32> = stale.into_iter().map(|(id, _)| id).collect();
        // Only housekeeping: the answer does not depend on it.
        if let Err(e) = forget(&mut db, &stale).await {
            tracing::warn!("duplicity: cannot drop stale pages: {e:#}");
        }
        if let Some((id, title)) = live.into_iter().next() {
            let mut out = ok();
            out.insert("page".to_string(), title.into());
            out.insert("id".to_string(), id.to_string().into());
            return Ok(out);
        }
    }
    Ok(status(format!("No page left to check on {wiki}")))
}

/// A batch of `no_wd` entries from a random point on, wrapping around at the end.
async fn random_candidates(db: &mut Conn, wiki: &str) -> anyhow::Result<Vec<(u32, String)>> {
    let from: f64 = rand::random();
    for window in ["AND random>=?", ""] {
        let sql = format!("SELECT id, title FROM no_wd WHERE wiki=? {window} ORDER BY random LIMIT {RANDOM_BATCH}");
        let mut params = vec![SqlValue::from(wiki)];
        if !window.is_empty() {
            params.push(from.into());
        }
        let rows: Vec<(u32, Vec<u8>)> = db.exec(sql, SqlParams::Positional(params)).await?;
        if !rows.is_empty() {
            return Ok(rows.into_iter().map(|(id, title)| (id, text(title))).collect());
        }
    }
    Ok(vec![])
}

/// Forgets a page the app has dealt with, once the wiki shows it no longer
/// needs an item: anyone can call this, so it must not delete open pages.
async fn invalidate_page(state: &AppState, wiki: &str, page: &str) -> Answer {
    let mut wiki_db = state.replicas.conn(&Wiki::from_dbname(wiki)?, WIKI_TABLES).await?;
    if !open_articles(&mut wiki_db, &[page]).await?.is_empty() {
        return Ok(ok());
    }
    let mut db = state.tool_db.conn(Database::Duplicity).await?;
    let ids: Vec<u32> =
        db.exec("SELECT id FROM no_wd WHERE wiki=? AND title=?", (wiki, page)).await.map_err(anyhow::Error::from)?;
    forget(&mut db, &ids).await?;
    Ok(ok())
}

/// Removes `no_wd` entries, and the game's candidates for them.
async fn forget(db: &mut Conn, ids: &[u32]) -> anyhow::Result<()> {
    for (sql, params) in forget_statements(ids) {
        db.exec_drop(sql, SqlParams::Positional(params)).await?;
    }
    Ok(())
}

fn forget_statements(ids: &[u32]) -> Vec<(String, Vec<SqlValue>)> {
    if ids.is_empty() {
        return vec![];
    }
    let list = placeholders(ids.len());
    let params: Vec<SqlValue> = ids.iter().map(|id| SqlValue::from(*id)).collect();
    vec![
        (format!("DELETE FROM candidates WHERE no_wd_id IN ({list})"), params.clone()),
        (format!("DELETE FROM no_wd WHERE id IN ({list})"), params),
    ]
}

/// The wikis, with how many of their articles lack an item.
async fn wikis(state: &AppState) -> Answer {
    let mut db = state.tool_db.conn(Database::Duplicity).await?;
    let rows: Vec<(Vec<u8>, u64)> =
        db.query("SELECT wiki, COUNT(*) FROM no_wd GROUP BY wiki ORDER BY wiki").await.map_err(anyhow::Error::from)?;
    // Counts as strings, as the PHP gave them.
    let wikis: Vec<Value> =
        rows.into_iter().map(|(wiki, n)| json!({"wiki": text(wiki), "cnt": n.to_string()})).collect();
    let mut out = ok();
    out.insert("wikis".to_string(), wikis.into());
    Ok(out)
}

/// The articles that still lack an item, oldest first; optionally only
/// those in a category, or not in another.
#[derive(Serialize)]
struct Articles<'a> {
    status: &'static str,
    articles: Vec<Article<'a>>,
}

#[derive(Serialize)]
struct Article<'a> {
    title: &'a str,
    creation_date: &'a str,
}

/// The wiki's articles without an item, oldest first; optionally in a
/// category, or not in one. Lean on memory: for some wikis, these are
/// over half a million.
async fn articles(state: &AppState, wiki: &str, category: &str, exclude: &str) -> Result<Response, AppError> {
    let mut db = state.tool_db.conn(Database::Duplicity).await?;
    let rows: Vec<(Vec<u8>, Vec<u8>)> = db
        .exec("SELECT title, creation_date FROM no_wd WHERE wiki=? ORDER BY creation_date", (wiki,))
        .await
        .map_err(anyhow::Error::from)?;
    drop(db);
    // In place: the bytes become the strings.
    let rows: Vec<(String, String)> = rows.into_iter().map(|(t, d)| (text(t), text(d))).collect();
    let mut keep = None;
    if !rows.is_empty() {
        let mut wiki_db = state.replicas.conn(&Wiki::from_dbname(wiki)?, WIKI_TABLES).await?;
        let titles: Vec<&str> = rows.iter().map(|(title, _)| title.as_str()).collect();
        let mut open = open_articles(&mut wiki_db, &titles).await?;
        drop(titles);
        if !category.trim().is_empty() && !open.is_empty() {
            let titles: Vec<&str> = open.iter().map(String::as_str).collect();
            let in_it = in_category(&mut wiki_db, &titles, category).await?;
            drop(titles);
            open.retain(|title| in_it.contains(title));
        }
        if !exclude.trim().is_empty() && !open.is_empty() {
            let titles: Vec<&str> = open.iter().map(String::as_str).collect();
            let excluded = in_category(&mut wiki_db, &titles, exclude).await?;
            drop(titles);
            open.retain(|title| !excluded.contains(title));
        }
        keep = Some(open);
    }
    let articles = rows
        .iter()
        .filter(|(title, _)| keep.as_ref().is_none_or(|keep| keep.contains(title)))
        .map(|(title, date)| Article { title, creation_date: date })
        .collect();
    Ok(Json(Articles { status: "OK", articles }).into_response())
}

/// How many articles lacked an item, by day (`YYYYMMDD`).
async fn stats(state: &AppState, wiki: &str) -> Answer {
    let mut db = state.tool_db.conn(Database::Duplicity).await?;
    let rows: Vec<(String, i64)> =
        db.exec("SELECT date, count FROM stats WHERE wiki=?", (wiki,)).await.map_err(anyhow::Error::from)?;
    // An empty PHP array is a JSON list.
    let data = match rows.is_empty() {
        true => json!([]),
        false => Value::Object(rows.into_iter().map(|(date, n)| (date, n.into())).collect()),
    };
    let mut out = ok();
    out.insert("data".to_string(), data);
    Ok(out)
}

/// The categories with at least `min_pages` of the articles that lack an item, most first.
async fn categories(state: &AppState, wiki: &str, min_pages: u64) -> Answer {
    let mut db = state.tool_db.conn(Database::Duplicity).await?;
    let titles: Vec<Vec<u8>> =
        db.exec("SELECT title FROM no_wd WHERE wiki=?", (wiki,)).await.map_err(anyhow::Error::from)?;
    drop(db);
    let mut out = ok();
    let mut counts: HashMap<String, u64> = HashMap::new();
    if !titles.is_empty() {
        let titles: Vec<String> = titles.into_iter().map(|t| text(t).replace(' ', "_")).collect();
        let mut wiki_db = state.replicas.conn(&Wiki::from_dbname(wiki)?, WIKI_TABLES).await?;
        let sql = "SELECT lt_title, COUNT(*) FROM page
            JOIN categorylinks ON cl_from=page_id JOIN linktarget ON cl_target_id=lt_id
            WHERE page_namespace=0 AND page_is_redirect=0 AND lt_namespace=14 AND page_title IN";
        let rows: Vec<(Vec<u8>, u64)> = select_in(&mut wiki_db, sql, "GROUP BY lt_title", &[], &titles).await?;
        for (category, n) in rows {
            *counts.entry(text(category)).or_default() += n;
        }
    }
    let mut categories: Vec<(String, u64)> = counts.into_iter().filter(|(_, n)| *n >= min_pages).collect();
    categories.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let categories: Vec<Value> = categories.into_iter().map(|(c, n)| json!([c, n.to_string()])).collect();
    out.insert("categories".to_string(), categories.into());
    Ok(out)
}

/// Those of `titles` (with spaces) that are live articles without an item:
/// `no_wd` is only as fresh as the bot's last run.
async fn open_articles(wiki_db: &mut Conn, titles: &[&str]) -> anyhow::Result<HashSet<String>> {
    let sql = "SELECT DISTINCT page_title FROM page WHERE page_namespace=0 AND page_is_redirect=0
        AND NOT EXISTS (SELECT * FROM page_props WHERE pp_page=page_id AND pp_propname='wikibase_item')
        AND page_title IN";
    matching(wiki_db, sql, &[], titles).await
}

/// Those of `titles` (with spaces) that are articles in `category`.
async fn in_category(wiki_db: &mut Conn, titles: &[&str], category: &str) -> anyhow::Result<HashSet<String>> {
    let category = category.trim().replace(' ', "_");
    let Some(target) = linktarget_ids(wiki_db, 14, &[category.as_str()]).await?.first().copied() else {
        return Ok(HashSet::new());
    };
    let sql = "SELECT DISTINCT page_title FROM page WHERE page_namespace=0 AND page_is_redirect=0
        AND EXISTS (SELECT * FROM categorylinks WHERE cl_from=page_id AND cl_target_id=?)
        AND page_title IN";
    matching(wiki_db, sql, &[target.into()], titles).await
}

async fn matching(
    wiki_db: &mut Conn,
    sql: &str,
    params: &[SqlValue],
    titles: &[&str],
) -> anyhow::Result<HashSet<String>> {
    let mut found = HashSet::new();
    // Chunk by chunk, so that only one chunk is copied at a time.
    for chunk in titles.chunks(TITLE_CHUNK) {
        let db_titles: Vec<String> = chunk.iter().map(|t| t.replace(' ', "_")).collect();
        let rows: Vec<Vec<u8>> = select_in(wiki_db, sql, "", params, &db_titles).await?;
        found.extend(rows.into_iter().map(|t| text(t).replace('_', " ")));
    }
    Ok(found)
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// A read-only request to the Wikidata API on the app's behalf; Wikidata's
/// answer as it is, or the HTTP status and a message. A rate limit (429) is
/// passed on with its `Retry-After`, so that the app backs off.
pub async fn wikidata_api_proxy(
    http: &reqwest::Client,
    params: &HashMap<String, String>,
) -> Result<Response, (StatusCode, String)> {
    let action = params.get("wdaction").map_or("", String::as_str);
    if !PROXY_ACTIONS.contains(&action) {
        return Err((StatusCode::OK, format!("Wikidata action not allowed: {action}")));
    }
    let mut query: Vec<(&str, &str)> = vec![("action", action), ("format", "json")];
    let skip = ["action", "wdaction", "format", "callback"];
    query.extend(params.iter().filter(|(k, _)| !skip.contains(&k.as_str())).map(|(k, v)| (k.as_str(), v.as_str())));
    let failed = || (StatusCode::GATEWAY_TIMEOUT, "Wikidata API request failed (connection error)".to_string());
    let response = http.get(WIKIDATA_API).query(&query).timeout(PROXY_TIMEOUT).send().await.map_err(|_| failed())?;
    let code = response.status();
    if code != reqwest::StatusCode::OK {
        let message = format!("Wikidata API request failed (HTTP {})", code.as_u16());
        let code = StatusCode::from_u16(code.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let retry_after = response.headers().get(header::RETRY_AFTER).cloned();
        let mut answer = (code, Json(json!({ "status": message }))).into_response();
        if let (StatusCode::TOO_MANY_REQUESTS, Some(retry_after)) = (code, retry_after) {
            answer.headers_mut().insert(header::RETRY_AFTER, retry_after);
        }
        return Ok(answer);
    }
    let body = response.bytes().await.map_err(|_| failed())?;
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn app_urls() {
        let request = |wiki: &str, page: &str, mode: &str, cat: &str| RedirectRequest {
            wiki: wiki.into(),
            page: page.into(),
            mode: mode.into(),
            cat: cat.into(),
        };
        assert_eq!(
            app_url(&request("enwiki", "Real_Pro_Volleyball", "", "")),
            "/duplicity/#/article/enwiki/Real_Pro_Volleyball"
        );
        assert_eq!(app_url(&request("dewiki", "", "cats", "")), "/duplicity/#/categories/dewiki");
        assert_eq!(app_url(&request("", "", "", "")), "/duplicity/#/");
        assert_eq!(app_url(&request("enwiki", "", "list", " A b ")), "/duplicity/#/list/enwiki/A%20b");
        assert_eq!(app_url(&request("enwiki", "a\r\nb", "", "")), "/duplicity/#/article/enwiki/a%0D%0Ab");
    }

    #[test]
    fn forget_sql() {
        assert!(forget_statements(&[]).is_empty());
        let statements = forget_statements(&[3, 5]);
        assert_eq!(statements[0].0, "DELETE FROM candidates WHERE no_wd_id IN (?,?)");
        assert_eq!(statements[1].0, "DELETE FROM no_wd WHERE id IN (?,?)");
        assert_eq!(statements[1].1, vec![SqlValue::from(3u32), SqlValue::from(5u32)]);
    }

    #[tokio::test]
    async fn routes() {
        let dir = tempfile::tempdir().unwrap();
        let moved = get(dir.path(), "/duplicity.php?wiki=enwiki&norand=1&page=Real%5FPro%5FVolleyball").await;
        assert_eq!(moved.status.as_u16(), 302);
        assert_eq!(moved.headers["location"], "/duplicity/#/article/enwiki/Real_Pro_Volleyball");
        let denied = get(dir.path(), "/duplicity/api.php?action=wikidata_api_proxy&wdaction=wbeditentity").await;
        assert_eq!(denied.body, r#"{"status":"Wikidata action not allowed: wbeditentity"}"#);
        let unknown = get(dir.path(), "/duplicity/api.php?action=nonsense").await;
        assert_eq!(unknown.body, r#"{"status":"Unknown action: nonsense"}"#);
        let bad_wiki = get(dir.path(), "/duplicity/api.php?action=random_nowd_page&wiki=evil.org").await;
        assert_eq!(bad_wiki.status.as_u16(), 400);
    }
}
