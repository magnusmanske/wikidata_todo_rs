//! `beacon.php`: a BEACON file (text/plain) mapping Wikidata items, or the
//! IDs of another property, to the IDs of an external-identifier property.
//! See <https://meta.wikimedia.org/wiki/BEACON>.

use crate::cache::TimedCache;
use crate::sparql::{self, Sparql};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use crate::wikidata::parse_id;
use askama::Template;
use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{Uri, header};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Bytes per piece of the streamed response.
const PIECE: usize = 64 * 1024;
/// Ends a list that the query service cut short.
const INCOMPLETE: &str = "#ERROR: the query service stopped early; this list is incomplete\n";
/// How long the target list is used before it is fetched again.
const KEEP_FOR: Duration = Duration::from_secs(24 * 3600);
/// The target list, kept on disk so that a restart does not cost the query.
const TARGETS_FILE: &str = "beacon_targets.json";

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    prop: String,
    source: String,
    site: String,
}

impl Default for Request {
    fn default() -> Self {
        Self { prop: String::new(), source: "0".to_string(), site: String::new() }
    }
}

/// A property that BEACON files can map to or from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Target {
    /// `P214`; empty for Wikidata itself.
    id: String,
    name: String,
    /// The formatter URL without its final `$1`; empty if there is none.
    prefix: String,
}

impl Target {
    fn wikidata() -> Self {
        Self { id: String::new(), name: "Wikidata".to_string(), prefix: "http://www.wikidata.org/wiki/".to_string() }
    }
}

static TARGETS: TimedCache<Vec<Target>> = TimedCache::new(KEEP_FOR);

#[derive(Template)]
#[template(path = "tools/beacon.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
}

/// An entry of the form's autocomplete list.
#[derive(Serialize)]
struct Choice<'a> {
    id: &'a str,
    name: &'a str,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    uri: Uri,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let site = match request.site.trim() {
        "" => None,
        site => Some(Wiki::from_dbname(&site.to_lowercase())?),
    };
    if request.prop.trim().is_empty() {
        return Ok(render(&Page { title: "Wikidata BEACON", request: &request })?.into_response());
    }
    let targets = targets(&state).await?;
    let find = |input: &str| {
        let id = parse_id(input, 'P')?;
        targets.iter().find(|t| t.id == id).cloned()
    };
    let unknown = |input: &str| {
        AppError::BadRequest(format!("{input} is not an external identifier property with a formatter URL."))
    };
    let target = find(&request.prop).ok_or_else(|| unknown(&request.prop))?;
    let source = match request.source.trim() {
        "" | "0" => Target::wikidata(),
        source => find(source).ok_or_else(|| unknown(source))?,
    };
    let reader = state.sparql.csv(&query(&target, &source, site.as_ref())).await?;
    let homepage = format!("{}{uri}", state.config.public_url);
    let head = head(&target, &source, &homepage, &chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string());
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=UTF-8")], stream(head, reader, !source.id.is_empty()))
        .into_response())
}

/// The form's autocomplete, loaded after the page is shown: the list has
/// thousands of properties and takes a while to query.
pub async fn choices(State(state): State<Arc<AppState>>) -> Result<Response, AppError> {
    let targets = targets(&state).await?;
    let choices: Vec<Choice> = targets.iter().map(|t| Choice { id: &t.id, name: &t.name }).collect();
    let cache = format!("public, max-age={}", KEEP_FOR.as_secs());
    Ok(([(header::CACHE_CONTROL, cache)], Json(choices)).into_response())
}

/// The target list from the cache, the file kept across restarts, or the query service.
async fn targets(state: &AppState) -> anyhow::Result<Arc<Vec<Target>>> {
    TARGETS
        .get_or_fetch(async move || {
            let path = state.config.data_dir.join(TARGETS_FILE);
            if let Some(targets) = read_targets(&path).await {
                return Ok(targets);
            }
            let targets = fetch_targets(&state.sparql).await?;
            if let Err(e) = write_targets(&path, &targets).await {
                tracing::warn!("beacon: cannot keep the target list in {}: {e:#}", path.display());
            }
            Ok(targets)
        })
        .await
}

/// Properties of external identifiers, or with a formatter URL; by name.
async fn fetch_targets(sparql: &Sparql) -> anyhow::Result<Vec<Target>> {
    let query = "SELECT ?p ?label ?url { { ?p wikibase:propertyType wikibase:ExternalId } UNION { ?p wdt:P1630 [] }
        ?p rdfs:label ?label FILTER(LANG(?label) = \"en\") OPTIONAL { ?p wdt:P1630 ?url } }";
    let json = sparql.json(query).await?;
    let mut targets: Vec<Target> = vec![];
    for row in sparql::bindings(&json) {
        let (Some(id), Some(name)) =
            (row["p"]["value"].as_str().and_then(sparql::entity_id), row["label"]["value"].as_str())
        else {
            continue;
        };
        let prefix = row["url"]["value"].as_str().and_then(prefix).unwrap_or_default();
        match targets.iter_mut().find(|t| t.id == id) {
            // Of several formatter URLs, the first usable one.
            Some(target) if target.prefix.is_empty() => target.prefix = prefix,
            Some(_) => {}
            None => targets.push(Target { id: id.to_string(), name: name.to_string(), prefix }),
        }
    }
    targets.sort_by_cached_key(|t| t.name.to_lowercase());
    Ok(targets)
}

/// The target list as saved by [`write_targets`].
#[derive(Deserialize, Serialize)]
struct SavedTargets {
    /// When it was fetched, in seconds since the Unix epoch.
    fetched: u64,
    targets: Vec<Target>,
}

/// The saved target list, if there is one and it is fresh.
async fn read_targets(path: &Path) -> Option<Vec<Target>> {
    let saved: SavedTargets = serde_json::from_slice(&tokio::fs::read(path).await.ok()?).ok()?;
    (now().saturating_sub(saved.fetched) < KEEP_FOR.as_secs()).then_some(saved.targets)
}

/// Replaces `path` at once, so that a reader never sees half a file.
async fn write_targets(path: &Path, targets: &[Target]) -> anyhow::Result<()> {
    let saved = SavedTargets { fetched: now(), targets: targets.to_vec() };
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, serde_json::to_vec(&saved)?).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

/// A formatter URL without its final `$1` (and any slashes after it).
fn prefix(formatter: &str) -> Option<String> {
    let prefix = formatter.trim_end_matches('/').strip_suffix("$1")?;
    (!prefix.is_empty()).then(|| prefix.to_string())
}

fn query(target: &Target, source: &Target, site: Option<&Wiki>) -> String {
    let site = site
        .map(|wiki| format!(" ?article schema:about ?item ; schema:isPartOf <https://{}/> .", wiki.host()))
        .unwrap_or_default();
    match source.id.is_empty() {
        true => format!("SELECT DISTINCT ?item ?ext1 WHERE {{ ?item wdt:{} ?ext1 .{site} }}", target.id),
        false => format!(
            "SELECT DISTINCT ?item ?ext1 ?ext2 WHERE {{ ?item wdt:{} ?ext1 ; wdt:{} ?ext2 .{site} }}",
            target.id, source.id
        ),
    }
}

fn head(target: &Target, source: &Target, homepage: &str, timestamp: &str) -> String {
    let mut head = format!(
        "#NAME: {name}\n#DESCRIPTION: Mapping from {} IDs to {name} IDs\n#CREATOR: Magnus Manske\n\
         #CREATOR: https://www.wikidata.org/wiki/User:Magnus_Manske\n#HOMEPAGE: {homepage}\n#TIMESTAMP: {timestamp}\n",
        source.name,
        name = target.name,
    );
    for (field, value) in [("PREFIX", &source.prefix), ("TARGET", &target.prefix)] {
        if !value.is_empty() {
            head += &format!("#{field}: {value}\n");
        }
    }
    head.push('\n');
    head
}

/// The response body: `head`, then a line per result row, written from the
/// buffered result as the client reads.
fn stream(head: String, mut reader: csv::Reader<File>, with_source: bool) -> Body {
    let (sender, receiver) = mpsc::channel::<Bytes>(8);
    tokio::task::spawn_blocking(move || {
        let mut piece = head;
        for record in reader.records() {
            match record.ok().as_ref().map(|r| line(r, with_source)) {
                Some(Ok(Some(line))) => piece += &line,
                Some(Ok(None)) => {}
                // What follows a malformed row is the service's error message.
                _ => {
                    piece += INCOMPLETE;
                    break;
                }
            }
            if piece.len() >= PIECE && sender.blocking_send(Bytes::from(std::mem::take(&mut piece))).is_err() {
                return; // The client is gone.
            }
        }
        let _ = sender.blocking_send(Bytes::from(piece));
    });
    Body::from_stream(pieces(receiver))
}

/// The body's chunks, fused: a body may be polled again after it ended,
/// e.g. by the compression layer looking for trailers, and `unfold` panics
/// on that.
fn pieces(receiver: mpsc::Receiver<Bytes>) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> {
    futures::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|piece| (Ok::<_, std::io::Error>(piece), receiver))
    })
    .fuse()
}

/// A BEACON line for a result row (`item, ext1[, ext2]`): `Q42||ext1`, or
/// `ext2|Q42|ext1`. `None` for rows without a real value; an error for rows
/// that are no result rows.
fn line(record: &csv::StringRecord, with_source: bool) -> Result<Option<String>, ()> {
    let item = record.get(0).and_then(sparql::entity_id).filter(|id| id.starts_with('Q')).ok_or(())?;
    let field = |i: usize| record.get(i).filter(|v| is_value(v));
    Ok(match (field(1), with_source) {
        (Some(ext1), false) => Some(format!("{item}||{ext1}\n")),
        (Some(ext1), true) => field(2).map(|ext2| format!("{ext2}|{item}|{ext1}\n")),
        (None, _) => None,
    })
}

/// Not "no value" or "unknown value".
fn is_value(v: &str) -> bool {
    !v.is_empty()
        && v != "novalue"
        && !v.starts_with("_:")
        && !v.starts_with("http://www.wikidata.org/.well-known/genid/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    fn viaf() -> Target {
        Target { id: "P214".into(), name: "VIAF ID".into(), prefix: "https://viaf.org/viaf/".into() }
    }

    #[test]
    fn prefixes() {
        assert_eq!(prefix("https://viaf.org/viaf/$1/").as_deref(), Some("https://viaf.org/viaf/"));
        assert_eq!(prefix("https://x.org/?id=$1&y"), None);
        assert_eq!(prefix("$1"), None);
    }

    #[test]
    fn queries() {
        let gnd = Target { id: "P227".into(), name: "GND ID".into(), prefix: String::new() };
        assert_eq!(
            query(&viaf(), &Target::wikidata(), None),
            "SELECT DISTINCT ?item ?ext1 WHERE { ?item wdt:P214 ?ext1 . }"
        );
        let wiki = Wiki::from_dbname("enwiki").unwrap();
        assert_eq!(
            query(&viaf(), &gnd, Some(&wiki)),
            "SELECT DISTINCT ?item ?ext1 ?ext2 WHERE { ?item wdt:P214 ?ext1 ; wdt:P227 ?ext2 . \
             ?article schema:about ?item ; schema:isPartOf <https://en.wikipedia.org/> . }"
        );
    }

    #[test]
    fn heads_and_lines() {
        assert_eq!(
            head(&viaf(), &Target::wikidata(), "https://h/beacon/?prop=P214", "2026-01-02T03:04:05+00:00"),
            "#NAME: VIAF ID\n#DESCRIPTION: Mapping from Wikidata IDs to VIAF ID IDs\n#CREATOR: Magnus Manske\n\
             #CREATOR: https://www.wikidata.org/wiki/User:Magnus_Manske\n#HOMEPAGE: https://h/beacon/?prop=P214\n\
             #TIMESTAMP: 2026-01-02T03:04:05+00:00\n#PREFIX: http://www.wikidata.org/wiki/\n\
             #TARGET: https://viaf.org/viaf/\n\n"
        );
        let record = |fields: &[&str]| csv::StringRecord::from(fields.to_vec());
        let item = "http://www.wikidata.org/entity/Q42";
        assert_eq!(line(&record(&[item, "113230702"]), false), Ok(Some("Q42||113230702\n".to_string())));
        assert_eq!(line(&record(&[item, "1", "x"]), true), Ok(Some("x|Q42|1\n".to_string())));
        assert_eq!(line(&record(&[item, "http://www.wikidata.org/.well-known/genid/abc"]), false), Ok(None));
        assert_eq!(line(&record(&["\tat java.lang.Thread.run(Thread.java:750)"]), false), Err(()));
    }

    #[tokio::test]
    async fn pieces_can_be_polled_after_the_end() {
        let (sender, receiver) = mpsc::channel(1);
        drop(sender);
        let mut pieces = Box::pin(pieces(receiver));
        assert!(pieces.next().await.is_none());
        // Safe: a body may be polled again after its stream ended. Without
        // `fuse`, `unfold` panicked here (seen from the compression layer).
        assert!(pieces.next().await.is_none());
    }

    #[tokio::test]
    async fn saved_targets_are_reused_until_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TARGETS_FILE);
        assert!(read_targets(&path).await.is_none());
        write_targets(&path, &[viaf()]).await.unwrap();
        assert_eq!(read_targets(&path).await, Some(vec![viaf()]));
        let stale = SavedTargets { fetched: now().saturating_sub(KEEP_FOR.as_secs()), targets: vec![viaf()] };
        tokio::fs::write(&path, serde_json::to_vec(&stale).unwrap()).await.unwrap();
        assert!(read_targets(&path).await.is_none());
    }

    #[tokio::test]
    async fn blank_page_needs_no_query() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/beacon/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("name=\"prop\""));
    }

    #[tokio::test]
    async fn bad_site() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(get(dir.path(), "/beacon/?prop=P214&site=nowiki_x").await.status.as_u16(), 400);
    }
}
