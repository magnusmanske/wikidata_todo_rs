//! `subclass_dates.php`: how the dates (e.g. of birth) of items whose
//! property (e.g. occupation) is in a class tree spread over the years, in
//! buckets of some years.

use crate::sparql;
use crate::state::AppState;
use crate::web::{AppError, Params, render, thousands};
use crate::wikidata::parse_id;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

const MAX_EXCLUDED: usize = 20;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    prop: String,
    dateprop: String,
    bucket_size: String,
    root: String,
    exclude: String,
    doit: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            prop: String::new(),
            dateprop: "P569".to_string(),
            bucket_size: "50".to_string(),
            root: String::new(),
            exclude: String::new(),
            doit: None,
        }
    }
}

/// The parsed request.
struct Histogram {
    property: String,
    date_property: String,
    bucket_size: i64,
    root: String,
    excluded: Vec<String>,
}

impl Histogram {
    fn parse(request: &Request) -> Result<Self, AppError> {
        let bad = |what: &str| AppError::BadRequest(format!("Please give {what}."));
        let excluded: Vec<String> = request.exclude.split(',').filter_map(|e| parse_id(e, 'Q')).collect();
        if excluded.len() > MAX_EXCLUDED {
            return Err(bad(&format!("at most {MAX_EXCLUDED} items to exclude")));
        }
        Ok(Self {
            property: parse_id(&request.prop, 'P').ok_or_else(|| bad("a property, such as P106"))?,
            date_property: parse_id(&request.dateprop, 'P').ok_or_else(|| bad("a date property, such as P569"))?,
            bucket_size: request
                .bucket_size
                .trim()
                .parse()
                .ok()
                .filter(|n| (1..=10_000).contains(n))
                .ok_or_else(|| bad("a bucket size between 1 and 10000 years"))?,
            root: parse_id(&request.root, 'Q').ok_or_else(|| bad("a root item, such as Q483501"))?,
            excluded,
        })
    }

    /// Items and dates, each pair once, counted by year.
    fn query(&self) -> String {
        let Self { property: p, date_property: d, root, .. } = self;
        let minus: String =
            self.excluded.iter().map(|q| format!(" MINUS {{ ?item wdt:{p}/wdt:P279* wd:{q} }}")).collect();
        format!(
            "SELECT ?year (COUNT(*) AS ?n) {{ {{ SELECT DISTINCT ?item ?date {{ \
             ?item wdt:{p}/wdt:P279* wd:{root} ; wdt:{d} ?date .{minus} }} }} \
             BIND(YEAR(?date) AS ?year) }} GROUP BY ?year"
        )
    }

    /// `(first year, count)` of each bucket, in order.
    fn buckets(&self, years: impl IntoIterator<Item = (i64, u64)>) -> Vec<(i64, u64)> {
        let mut buckets: BTreeMap<i64, u64> = BTreeMap::new();
        for (year, n) in years {
            *buckets.entry(year.div_euclid(self.bucket_size) * self.bucket_size).or_default() += n;
        }
        buckets.into_iter().collect()
    }
}

struct Results {
    bucket_size: i64,
    buckets: Vec<(i64, String)>,
    total: String,
}

#[derive(Template)]
#[template(path = "tools/subclass_dates.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mut page = Page { title: "Subclass dates", request: &request, results: None };
    if request.doit.is_none() {
        return render(&page);
    }
    let histogram = Histogram::parse(&request)?;
    let json = state.sparql.json(&histogram.query()).await.map_err(|e| {
        tracing::info!("subclass_dates: {e:#}");
        AppError::BadRequest(
            "The query service could not answer in time; this class tree may have too many items.".to_string(),
        )
    })?;
    let years = sparql::bindings(&json).filter_map(|row| {
        let year = row["year"]["value"].as_str()?.parse().ok()?;
        Some((year, row["n"]["value"].as_str()?.parse().ok()?))
    });
    let buckets = histogram.buckets(years);
    let total = buckets.iter().map(|(_, n)| n).sum();
    page.results = Some(Results {
        bucket_size: histogram.bucket_size,
        buckets: buckets.into_iter().map(|(start, n)| (start, thousands(n))).collect(),
        total: thousands(total),
    });
    render(&page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    fn histogram(exclude: &str) -> Histogram {
        let request =
            Request { prop: "106".into(), root: "Q11063".into(), exclude: exclude.into(), ..Request::default() };
        Histogram::parse(&request).unwrap()
    }

    #[test]
    fn query_and_buckets() {
        assert_eq!(
            histogram("Q170790, x").query(),
            "SELECT ?year (COUNT(*) AS ?n) { { SELECT DISTINCT ?item ?date { \
             ?item wdt:P106/wdt:P279* wd:Q11063 ; wdt:P569 ?date . \
             MINUS { ?item wdt:P106/wdt:P279* wd:Q170790 } } } BIND(YEAR(?date) AS ?year) } GROUP BY ?year"
        );
        assert_eq!(histogram("").buckets([(1949, 1), (1900, 2), (-1, 4), (1950, 8)]), [(-50, 4), (1900, 3), (1950, 8)]);
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/subclass_dates/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("value=\"P569\""));
        let bad = get(dir.path(), "/subclass_dates/?prop=P106&root=Q5&bucket_size=0&doit=1").await;
        assert_eq!(bad.status.as_u16(), 400);
    }
}
