//! `index.php`: the main page. Lists the tools, and things to do on Wikidata:
//! a random batch of items that lack a basic statement, and per-country
//! lists (on PetScan and WD-FIST). With `bot=1`, the batch as JSON(P).

use crate::cache::TimedCache;
use crate::replicas::{linktarget_ids, placeholders, select_in};
use crate::sparql::{self, Sparql};
use crate::state::AppState;
use crate::terms::{self, TermType};
use crate::web::{AppError, Params, api_json, render, thousands};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Params as SqlParams, Value};

const MAX_BATCH: usize = 500;
/// Linking pages scanned per round, while looking for ones that lack the property.
const SCAN: usize = 2000;
const NS_PROPERTY: i64 = 120;
/// Properties that, besides the missing one, rule an item out: lists and
/// categories link to the class (e.g. "category contains: human"), yet need
/// none of the properties.
const EXCLUDING_PROPERTIES: &[&str] = &["P360", "P4224", "P971"];
/// "Wikimedia category"
const CATEGORY_ITEM: &str = "Q4167836";
const COUNT_FOR: Duration = Duration::from_secs(24 * 3600);

/// The things to do: items linking to a class, but lacking a property.
struct Action {
    key: &'static str,
    class: &'static str,
    property: &'static str,
    description: &'static str,
}

const ACTIONS: &[Action] = &[
    Action {
        key: "noisin",
        class: "Q618123",
        property: "P131",
        description: "locations with no assigned administrative unit",
    },
    Action { key: "nogender", class: "Q5", property: "P21", description: "people with no assigned gender" },
    Action { key: "noauthor", class: "Q571", property: "P50", description: "books with no assigned author" },
];

/// Wikimedia's apps on this site, which have their own interface.
const APPS: &[(&str, &str, &str)] = &[
    ("/duplicity/", "Duplicity", "Wikipedia articles without a Wikidata item, and items that may match"),
    ("/relator2/", "Relator", "Family relations of people on Wikidata"),
    ("/wikiflix/", "WikiFlix", "Films on Wikimedia Commons, by Wikidata"),
    ("/graveyard_shift/", "Graveyard shift", "Graves of people on Wikidata, on a map"),
    ("/tabernacle.html", "TABernacle", "Wikidata items and their statements as a table"),
    ("/sourcery.html", "Sourcery", "References for statements"),
];

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    action: String,
    start: Option<u64>,
    #[serde(default = "default_language")]
    lang: String,
    max: Option<usize>,
    #[serde(default)]
    bot: String,
    callback: Option<String>,
    country: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

struct Item {
    id: String,
    label: String,
}

struct Batch {
    description: &'static str,
    total: Option<String>,
    items: Vec<Item>,
    permalink: String,
}

#[derive(Template)]
#[template(path = "tools/index.html")]
struct Page {
    title: &'static str,
    actions: Vec<(&'static str, String)>,
    batch: Option<Batch>,
    countries: Arc<Vec<(String, String)>>,
    country: String,
    tools: &'static [(&'static str, &'static str)],
    apps: &'static [(&'static str, &'static str, &'static str)],
}

#[derive(Serialize)]
struct BotAnswer {
    total: u64,
    pages: BTreeMap<String, String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let languages = terms::languages_with(&request.lang, &["en", "de", "es", "it", "fr"]);
    let max = request.max.unwrap_or(100).clamp(1, MAX_BATCH);
    let action = ACTIONS.iter().find(|a| a.key == request.action);
    if !request.bot.is_empty() && request.bot != "0" {
        let action = action.ok_or_else(|| AppError::BadRequest("unknown action".to_string()))?;
        let (_, items) = batch(&state, action, request.start, max, &languages).await?;
        let total = cached_count(&state, action).unwrap_or_default();
        let pages = items.into_iter().map(|i| (i.id, i.label)).collect();
        return Ok(api_json(BotAnswer { total, pages }, request.callback));
    }
    let batch = match action {
        Some(action) => {
            let (start, items) = batch(&state, action, request.start, max, &languages).await?;
            let permalink =
                format!("/?action={}&lang={}&start={start}", action.key, crate::wiki::encode_component(&request.lang));
            let total = cached_count(&state, action).map(thousands);
            Some(Batch { description: action.description, total, items, permalink })
        }
        None => None,
    };
    let countries = COUNTRIES.get_or_fetch(async || countries(&state.sparql).await).await.unwrap_or_default();
    let page = Page {
        title: "Wikidata Todo",
        actions: ACTIONS.iter().map(|a| (a.key, upper_first(a.description))).collect(),
        batch,
        countries,
        country: request.country.and_then(|c| crate::wikidata::parse_id(&c, 'Q')).unwrap_or("Q889".to_string()),
        tools: crate::tools::MENU,
        apps: APPS,
    };
    Ok(render(&page)?.into_response())
}

fn upper_first(s: &str) -> String {
    crate::wiki::upper_first(s)
}

/// `max` random items linking to the action's class but not to its
/// property, from page ID `start` (random if not given) on. Also returns that start.
async fn batch(
    state: &AppState,
    action: &Action,
    start: Option<u64>,
    max: usize,
    languages: &[&str],
) -> Result<(u64, Vec<Item>), AppError> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page", "pagelinks", "linktarget"]).await?;
    let start = match start {
        Some(start) => start,
        None => {
            let max_id: Option<u64> =
                conn.query_first("SELECT MAX(page_id) FROM page").await.map_err(anyhow::Error::from)?;
            rand::random_range(0..max_id.unwrap_or(1).max(1))
        }
    };
    let ids = lacking(&mut conn, action, start, max).await?;
    let labels = item_labels(state, &ids, languages).await?;
    let items =
        ids.into_iter().map(|id| Item { label: labels.get(&id).cloned().unwrap_or_else(|| id.clone()), id }).collect();
    Ok((start, items))
}

/// Items linking to the class but lacking the property, scanning linking
/// pages by page ID from `start`, wrapping around at the end.
async fn lacking(conn: &mut Conn, action: &Action, start: u64, max: usize) -> anyhow::Result<Vec<String>> {
    let Some(class) = linktarget_ids(conn, 0, &[action.class]).await?.first().copied() else {
        return Ok(vec![]);
    };
    let excluding = excluding_targets(conn, action).await?;
    let mut found = vec![];
    let mut from = start;
    let mut wrapped = false;
    while found.len() < max {
        let sql = format!(
            "SELECT pl_from FROM pagelinks WHERE pl_target_id=? AND pl_from_namespace=0 AND pl_from>=?
             ORDER BY pl_from LIMIT {SCAN}"
        );
        let candidates: Vec<u64> = conn.exec(sql, (class, from)).await?;
        if candidates.is_empty() {
            if wrapped || start == 0 {
                break;
            }
            (from, wrapped) = (0, true);
            continue;
        }
        from = candidates.last().map_or(from, |last| last + 1);
        let sql = format!(
            "SELECT DISTINCT pl_from FROM pagelinks WHERE pl_target_id IN ({}) AND pl_from IN",
            placeholders(excluding.len())
        );
        let params: Vec<Value> = excluding.iter().map(|id| Value::from(*id)).collect();
        let excluded: HashSet<u64> = select_in(conn, &sql, "", &params, &candidates).await?.into_iter().collect();
        let pages: Vec<u64> = candidates.into_iter().filter(|c| !excluded.contains(c)).collect();
        let sql = "SELECT page_title FROM page WHERE page_namespace=0 AND page_id IN";
        let titles: Vec<String> = select_in(conn, sql, "", &[], &pages).await?;
        found.extend(titles.into_iter().take(max - found.len()));
        if wrapped && from >= start {
            break;
        }
    }
    Ok(found)
}

/// The link targets that rule an item out: the property, and those of lists and categories.
async fn excluding_targets(conn: &mut Conn, action: &Action) -> anyhow::Result<Vec<u64>> {
    let mut properties = vec![action.property];
    properties.extend(EXCLUDING_PROPERTIES);
    let mut ids = linktarget_ids(conn, NS_PROPERTY, &properties).await?;
    ids.extend(linktarget_ids(conn, 0, &[CATEGORY_ITEM]).await?);
    Ok(ids)
}

/// Labels in the first of `languages` that has one, else in any language.
async fn item_labels(state: &AppState, ids: &[String], languages: &[&str]) -> anyhow::Result<HashMap<String, String>> {
    let mut labels = terms::labels(&state.replicas, ids, languages).await?;
    let missing: Vec<String> = ids.iter().filter(|id| !labels.contains_key(*id)).cloned().collect();
    if !missing.is_empty() {
        for term in terms::terms(&state.replicas, &missing, &[TermType::Label], &[]).await? {
            labels.entry(term.entity).or_insert(term.text);
        }
    }
    Ok(labels)
}

/// The number of items lacking the property, counted in the background
/// now and then: it takes the database a while.
fn cached_count(state: &Arc<AppState>, action: &'static Action) -> Option<u64> {
    /// When each count was last started, and its result.
    type Counts = HashMap<&'static str, (Instant, Option<u64>)>;
    static COUNTS: LazyLock<Mutex<Counts>> = LazyLock::new(Default::default);
    let mut counts = COUNTS.lock().ok()?;
    let entry = counts.get(action.key).copied();
    let stale = entry.is_none_or(|(at, _)| at.elapsed() > COUNT_FOR);
    if stale {
        // Marks the count as being renewed, so that only one request starts it.
        counts.insert(action.key, (Instant::now(), entry.and_then(|(_, n)| n)));
        let state = state.clone();
        tokio::spawn(async move {
            match count(&state, action).await {
                Ok(n) => {
                    if let Ok(mut counts) = COUNTS.lock() {
                        counts.insert(action.key, (Instant::now(), Some(n)));
                    }
                }
                Err(e) => tracing::warn!("cannot count {}: {e:#}", action.key),
            }
        });
    }
    entry.and_then(|(_, n)| n)
}

async fn count(state: &AppState, action: &Action) -> anyhow::Result<u64> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["pagelinks", "linktarget"]).await?;
    let Some(class) = linktarget_ids(&mut conn, 0, &[action.class]).await?.first().copied() else {
        return Ok(0);
    };
    let excluding = excluding_targets(&mut conn, action).await?;
    let sql = format!(
        "SELECT COUNT(*) FROM pagelinks p1 WHERE p1.pl_target_id=? AND p1.pl_from_namespace=0
         AND NOT EXISTS (SELECT * FROM pagelinks p2 WHERE p2.pl_from=p1.pl_from AND p2.pl_target_id IN ({}))",
        placeholders(excluding.len())
    );
    let mut params = vec![Value::from(class)];
    params.extend(excluding.into_iter().map(Value::from));
    let n: Option<u64> = conn.exec_first(sql, SqlParams::Positional(params)).await?;
    Ok(n.unwrap_or_default())
}

static COUNTRIES: TimedCache<Vec<(String, String)>> = TimedCache::new(Duration::from_secs(24 * 3600));

/// Countries (and similar), as `(item, English name)`, by name.
async fn countries(sparql: &Sparql) -> anyhow::Result<Vec<(String, String)>> {
    let query = "SELECT DISTINCT ?item ?label { VALUES ?class { wd:Q6256 wd:Q3624078 wd:Q1763527 }
        ?item wdt:P31 ?class ; rdfs:label ?label FILTER(LANG(?label) = \"en\") }";
    let json = sparql.json(query).await?;
    let mut countries: Vec<(String, String)> = sparql::bindings(&json)
        .filter_map(|row| {
            let id = sparql::entity_id(row["item"]["value"].as_str()?)?;
            Some((id.to_string(), row["label"]["value"].as_str()?.to_string()))
        })
        .collect();
    countries.sort_by_key(|(_, name)| name.to_lowercase());
    countries.dedup_by(|a, b| a.0 == b.0);
    Ok(countries)
}
