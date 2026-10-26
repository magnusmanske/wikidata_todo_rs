//! `pagepile2kml.php`: the items of a PagePile that have coordinates (P625),
//! as KML. Streamed, batch by batch, as large piles take a while.

use crate::sparql;
use crate::state::AppState;
use crate::terms::{self, DEFAULT_LANGUAGES};
use crate::web::{AppError, Params, render};
use askama::Template;
use axum::body::Body;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use futures::{StreamExt, stream};
use serde::Deserialize;
use std::sync::Arc;
use wikimisc::escape;

/// Items per query.
const BATCH: usize = 500;
const CONCURRENCY: usize = 4;
/// Larger piles are cut off.
const MAX_ITEMS: usize = 100_000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    pagepile: String,
}

#[derive(Template)]
#[template(path = "tools/pagepile2kml.html")]
struct Page {
    title: &'static str,
    max_items: usize,
}

struct Placemark {
    item: String,
    label: String,
    lon: f64,
    lat: f64,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let pile = request.pagepile.trim();
    if pile.is_empty() {
        return Ok(render(&Page { title: "PagePile to KML", max_items: MAX_ITEMS })?.into_response());
    }
    let id: u64 = pile.parse().map_err(|_| AppError::BadRequest("The PagePile ID must be a number.".into()))?;
    let mut items = crate::pagepile::items(&state.http, &state.replicas, id).await?;
    // Pile entries go into queries: only well-formed IDs.
    items.retain(|item| sparql::entity_id(item) == Some(item.as_str()) && item.starts_with('Q'));
    items.truncate(MAX_ITEMS);
    let batches: Vec<Vec<String>> = items.chunks(BATCH).map(<[String]>::to_vec).collect();
    let placemarks = stream::iter(batches)
        .map(move |batch| {
            let state = state.clone();
            async move {
                match placemarks(&state, &batch).await {
                    Ok(placemarks) => placemarks.iter().map(kml_placemark).collect::<String>(),
                    Err(e) => {
                        tracing::warn!("pagepile2kml: a batch of pile {id} failed: {e:#}");
                        String::new()
                    }
                }
            }
        })
        .buffered(CONCURRENCY);
    let body = stream::once(async move { kml_head(id) })
        .chain(placemarks)
        .chain(stream::once(async { "\n</Document>\n</kml>\n".to_string() }))
        .map(Ok::<_, std::convert::Infallible>);
    Ok((
        [
            (header::CONTENT_TYPE, "application/xml; charset=UTF-8"),
            (header::CACHE_CONTROL, "no-cache, must-revalidate"),
        ],
        Body::from_stream(body),
    )
        .into_response())
}

/// Those of `items` with coordinates on Earth, labelled.
async fn placemarks(state: &AppState, items: &[String]) -> anyhow::Result<Vec<Placemark>> {
    let query = format!("SELECT ?q ?coord {{ VALUES ?q {{ wd:{} }} ?q wdt:P625 ?coord }}", items.join(" wd:"));
    let json = state.sparql.json(&query).await?;
    let mut placemarks: Vec<Placemark> = vec![];
    for row in sparql::bindings(&json) {
        let Some(item) = row["q"]["value"].as_str().and_then(sparql::entity_id) else { continue };
        // Other globes' coordinates start with the globe's URI, and are left out.
        let Some((lon, lat)) = row["coord"]["value"].as_str().and_then(sparql::point) else { continue };
        if !placemarks.iter().any(|p| p.item == item) {
            placemarks.push(Placemark { item: item.to_string(), label: String::new(), lon, lat });
        }
    }
    let ids: Vec<String> = placemarks.iter().map(|p| p.item.clone()).collect();
    let labels = terms::labels(&state.replicas, &ids, DEFAULT_LANGUAGES).await?;
    for p in &mut placemarks {
        p.label = labels.get(&p.item).cloned().unwrap_or_else(|| p.item.clone());
    }
    Ok(placemarks)
}

fn kml_head(pile: u64) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<kml xmlns=\"http://www.opengis.net/kml/2.2\" xmlns:kml=\"http://www.opengis.net/kml/2.2\" xmlns:atom=\"http://www.w3.org/2005/Atom\">
<Document>
<open>1</open>

<name>PagePile #{pile}</name>\n"
    )
}

fn kml_placemark(p: &Placemark) -> String {
    let q = &p.item;
    format!(
        "<Placemark id='{q}'>\n<name>{}</name>\n<Point>\n<coordinates>{},{},0</coordinates>\n</Point>\n\
         <Description>   <![CDATA[ <a href='https://www.wikidata.org/wiki/{q}'>{q}</a> ]]> </Description>\n</Placemark>\n",
        escape::xml(&p.label),
        p.lon,
        p.lat
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn placemark() {
        let p = Placemark { item: "Q64".into(), label: "Berlin & <Co>".into(), lon: 13.38, lat: 52.5 };
        assert_eq!(
            kml_placemark(&p),
            "<Placemark id='Q64'>\n<name>Berlin &amp; &lt;Co&gt;</name>\n<Point>\n<coordinates>13.38,52.5,0</coordinates>\n\
             </Point>\n<Description>   <![CDATA[ <a href='https://www.wikidata.org/wiki/Q64'>Q64</a> ]]> </Description>\n\
             </Placemark>\n"
        );
        assert!(kml_head(253).starts_with("<?xml"));
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/pagepile2kml/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("name=\"pagepile\""));
        assert_eq!(get(dir.path(), "/pagepile2kml/?pagepile=abc").await.status.as_u16(), 400);
    }
}
