//! `coords2json.php`: the items and coordinates of a SPARQL query (variables
//! `?item` and `?coord`) as GeoJSON points, for the heat map (`heatmap.js`).
//! Served as text/plain, as before.

use crate::sparql;
use crate::state::AppState;
use crate::web::{AppError, Params};
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct Request {
    sparql: Option<String>,
    /// A WDQ query: that service is gone.
    q: Option<String>,
}

#[derive(Debug, Serialize)]
struct FeatureCollection {
    r#type: &'static str,
    features: Vec<Feature>,
}

#[derive(Debug, Serialize)]
struct Feature {
    /// The item number, as a string.
    id: String,
    geometry: Geometry,
    properties: Properties,
}

#[derive(Debug, Serialize)]
struct Geometry {
    /// Longitude, latitude.
    coordinates: [f64; 2],
    r#type: &'static str,
}

#[derive(Debug, Serialize)]
struct Properties {
    accuracy: u8,
    duration: u8,
    placename: String,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let query = match (request.sparql, request.q) {
        (Some(query), _) if !query.trim().is_empty() => query,
        (_, Some(_)) => {
            return Err(AppError::BadRequest(
                "WDQ queries (q=…) are no longer supported, as WDQ is gone; use a SPARQL query (sparql=…) instead"
                    .to_string(),
            ));
        }
        _ => return Err(AppError::BadRequest("Missing parameter: sparql".to_string())),
    };
    // CSV: large results stay on disk until read.
    let mut rows = state.sparql.csv(&query).await?;
    let headers = rows.headers().map_err(anyhow::Error::from)?.clone();
    let column = |name: &str| headers.iter().position(|h| h == name);
    let (Some(item), Some(coord)) = (column("item"), column("coord")) else {
        return Err(AppError::BadRequest("The query must have the variables ?item and ?coord".to_string()));
    };
    let mut features = vec![];
    for row in rows.records() {
        let row = row.map_err(anyhow::Error::from)?;
        if let Some(feature) = row.get(item).zip(row.get(coord)).and_then(|(i, c)| feature(i, c)) {
            features.push(feature);
        }
    }
    let json = serde_json::to_string(&FeatureCollection { r#type: "FeatureCollection", features })
        .map_err(anyhow::Error::from)?;
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], json).into_response())
}

/// A point for an item URI and an Earth coordinate (`Point(lon lat)`).
fn feature(item: &str, coord: &str) -> Option<Feature> {
    let number = sparql::entity_id(item)?.strip_prefix('Q')?;
    // Coordinates on other globes start with the globe's URI.
    let (lon, lat) = sparql::point(coord)?;
    Some(Feature {
        id: number.to_string(),
        geometry: Geometry { coordinates: [lon, lat], r#type: "Point" },
        properties: Properties { accuracy: 1, duration: 0, placename: format!("Q{number}") },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn features() {
        let f = feature("http://www.wikidata.org/entity/Q64", "Point(13.38 52.51)").unwrap();
        assert_eq!(
            serde_json::to_string(&f).unwrap(),
            r#"{"id":"64","geometry":{"coordinates":[13.38,52.51],"type":"Point"},"properties":{"accuracy":1,"duration":0,"placename":"Q64"}}"#
        );
        assert!(feature("http://www.wikidata.org/entity/P625", "Point(1 2)").is_none());
        assert!(
            feature("http://www.wikidata.org/entity/Q1", "<http://www.wikidata.org/entity/Q405> Point(1 2)").is_none()
        );
    }

    #[tokio::test]
    async fn without_query_or_with_wdq() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(get(dir.path(), "/coords2json.php").await.status.as_u16(), 400);
        let wdq = get(dir.path(), "/coords2json.php?q=claim[31:5]").await;
        assert_eq!(wdq.status.as_u16(), 400);
        assert!(wdq.body.contains("SPARQL"));
    }
}
