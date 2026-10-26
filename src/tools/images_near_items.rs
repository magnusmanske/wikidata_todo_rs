//! `images_near_items.php`: Commons files with coordinates in a category
//! tree, paired with nearby items of a class that have coordinates but no
//! image.

use crate::replicas::{display_title, select_in};
use crate::sparql;
use crate::state::AppState;
use crate::terms::{self, DEFAULT_LANGUAGES};
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_title};
use crate::wikidata::parse_id;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;

const MAX_DEPTH: u32 = 10;
const MAX_FILES: usize = 50_000;
const MAX_DISTANCE_M: u32 = 10_000;
const MAX_PAIRS: usize = 5_000;
const EARTH_RADIUS_M: f64 = 6_371_000.0;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    category: String,
    depth: Option<u32>,
    p31: String,
    max_distance_m: Option<u32>,
}

impl Default for Request {
    fn default() -> Self {
        Self { category: String::new(), depth: Some(5), p31: String::new(), max_distance_m: Some(150) }
    }
}

/// A point with a name: a file or an item.
#[derive(Debug, Clone, PartialEq)]
struct Place {
    name: String,
    lat: f64,
    lon: f64,
}

struct Pair {
    file: String,
    file_url: String,
    item: String,
    label: String,
    distance_m: u32,
}

struct Results {
    files: usize,
    items: usize,
    pairs: Vec<Pair>,
    truncated: bool,
}

#[derive(Template)]
#[template(path = "tools/images_near_items.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    depth: u32,
    max_distance_m: u32,
    results: Option<Results>,
    max_files: usize,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let depth = request.depth.unwrap_or(5).min(MAX_DEPTH);
    let max_distance_m = request.max_distance_m.unwrap_or(150).clamp(1, MAX_DISTANCE_M);
    let category = request.category.trim();
    let results = match (category.is_empty(), request.p31.trim().is_empty()) {
        (false, false) => {
            let class = parse_id(&request.p31, 'Q')
                .ok_or_else(|| AppError::BadRequest(format!("Not an item: {}", request.p31)))?;
            Some(results(&state, category, depth, &class, max_distance_m).await?)
        }
        _ => None,
    };
    render(&Page {
        title: "Images near items",
        request: &request,
        depth,
        max_distance_m,
        results,
        max_files: MAX_FILES,
    })
}

async fn results(
    state: &AppState,
    category: &str,
    depth: u32,
    class: &str,
    max_distance_m: u32,
) -> anyhow::Result<Results> {
    let (files, items) = tokio::try_join!(files(state, category, depth), items(state, class))?;
    let mut pairs = nearby(&files, &items, max_distance_m);
    let truncated = pairs.len() > MAX_PAIRS;
    pairs.truncate(MAX_PAIRS);
    let ids: Vec<String> = pairs.iter().map(|(_, item, _)| item.name.clone()).collect();
    let labels = terms::labels(&state.replicas, &ids, DEFAULT_LANGUAGES).await?;
    let pairs = pairs
        .into_iter()
        .map(|(file, item, distance_m)| Pair {
            file: display_title(&file.name),
            file_url: format!("https://commons.wikimedia.org/wiki/File:{}", encode_title(&file.name)),
            label: labels.get(&item.name).cloned().unwrap_or_else(|| item.name.clone()),
            item: item.name.clone(),
            distance_m,
        })
        .collect();
    Ok(Results { files: files.len(), items: items.len(), pairs, truncated })
}

/// Files in the category tree with (primary, earthly) coordinates.
async fn files(state: &AppState, category: &str, depth: u32) -> anyhow::Result<Vec<Place>> {
    let commons = Wiki::commons();
    let category = crate::mediawiki::strip_namespace(category, &["category"]).unwrap_or(category);
    let titles = state.replicas.some_pages_in_tree(&commons, category, depth, 6, Some(MAX_FILES)).await?;
    let mut conn = state.replicas.conn(&commons, &["page", "geo_tags"]).await?;
    let sql = "SELECT page_title, gt_lat, gt_lon FROM page JOIN geo_tags ON gt_page_id=page_id
        WHERE page_namespace=6 AND gt_primary=1 AND gt_globe='earth' AND page_title IN";
    let rows: Vec<(String, f64, f64)> = select_in(&mut conn, sql, "", &[], &titles).await?;
    Ok(rows.into_iter().map(|(name, lat, lon)| Place { name, lat, lon }).collect())
}

/// Items of the class (or a subclass) with coordinates, but without an image.
async fn items(state: &AppState, class: &str) -> anyhow::Result<Vec<Place>> {
    let query =
        format!("SELECT ?q ?coords {{ ?q wdt:P31/wdt:P279* wd:{class} ; wdt:P625 ?coords MINUS {{ ?q wdt:P18 [] }} }}");
    let mut csv = state.sparql.csv(&query).await?;
    let mut items = vec![];
    for record in csv.records() {
        let record = record?;
        let (Some(q), Some((lon, lat))) =
            (record.get(0).and_then(sparql::entity_id), record.get(1).and_then(sparql::point))
        else {
            continue;
        };
        items.push(Place { name: q.to_string(), lat, lon });
    }
    Ok(items)
}

/// Pairs of a file and an item at most `max_m` meters apart, with that
/// distance; by file, then distance. Items are sorted by latitude, so each
/// file only looks at those in a narrow band around it.
fn nearby<'a>(files: &'a [Place], items: &'a [Place], max_m: u32) -> Vec<(&'a Place, &'a Place, u32)> {
    let mut items: Vec<&Place> = items.iter().collect();
    items.sort_by(|a, b| a.lat.total_cmp(&b.lat));
    // Degrees of latitude per meter, with a little room for rounding.
    let band = f64::from(max_m) * 1.01 / (EARTH_RADIUS_M.to_radians());
    let mut files: Vec<&Place> = files.iter().collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    let mut pairs = vec![];
    for file in files {
        let start = items.partition_point(|i| i.lat < file.lat - band);
        let mut near: Vec<(&Place, &Place, u32)> = items
            .get(start..)
            .unwrap_or_default()
            .iter()
            .take_while(|i| i.lat <= file.lat + band)
            .map(|item| (file, *item, distance_m(file, item).round() as u32))
            .filter(|(.., d)| *d <= max_m)
            .collect();
        near.sort_by_key(|(.., d)| *d);
        pairs.extend(near);
    }
    pairs
}

/// The great-circle distance (haversine).
fn distance_m(a: &Place, b: &Place) -> f64 {
    let (lat1, lat2) = (a.lat.to_radians(), b.lat.to_radians());
    let (dlat, dlon) = (lat2 - lat1, (b.lon - a.lon).to_radians());
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().min(1.0).asin()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    fn place(name: &str, lat: f64, lon: f64) -> Place {
        Place { name: name.into(), lat, lon }
    }

    #[test]
    fn distances() {
        let d = distance_m(&place("a", 51.5, -0.1), &place("b", 51.501, -0.1));
        assert!((d - 111.2).abs() < 0.5, "{d}");
        assert_eq!(distance_m(&place("a", 1.0, 2.0), &place("b", 1.0, 2.0)), 0.0);
    }

    #[test]
    fn pairs() {
        let files = [place("F2", 51.5, -0.1), place("F1", 10.0, 10.0)];
        let items =
            [place("Q1", 51.5009, -0.1), place("Q2", 51.5, -0.1), place("Q3", 51.6, -0.1), place("Q4", 10.0, 10.001)];
        let found: Vec<(&str, &str, u32)> =
            nearby(&files, &items, 150).iter().map(|(f, i, d)| (f.name.as_str(), i.name.as_str(), *d)).collect();
        assert_eq!(found, [("F1", "Q4", 110), ("F2", "Q2", 0), ("F2", "Q1", 100)]);
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/images_near_items/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("value=\"150\""));
        assert_eq!(get(dir.path(), "/images_near_items/?category=X&p31=wd:Q1%7D").await.status.as_u16(), 400);
    }
}
