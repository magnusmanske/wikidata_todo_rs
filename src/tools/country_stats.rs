//! `country_stats.php`: how many items there were about some countries'
//! people and places, day by day, as a job counted them from 2014 to 2022
//! (`monitor_p.country`; static now). The counts came from WDQ queries,
//! which are shown on hover: WDQ and AutoList, which ran them, are gone.

use crate::cache::TimedCache;
use crate::replicas::format_date;
use crate::state::AppState;
use crate::tool_db::Database;
use crate::web::{AppError, render, thousands};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;

/// Counts shown as shares of another count of the same day.
const SHARES: &[(&str, &str)] = &[
    ("citizens_male", "citizens"),
    ("citizens_female", "citizens"),
    ("citizens_with_image", "citizens"),
    ("by_admin_only", "by_admin_or_coord"),
    ("by_coord_only", "by_admin_or_coord"),
    ("by_coord", "by_admin_or_coord"),
    ("by_admin", "by_admin_or_coord"),
    ("by_admin_and_coord", "by_admin_or_coord"),
    ("by_admin_or_coord_with_images", "by_admin_or_coord"),
];

#[derive(Template)]
#[template(path = "tools/country_stats.html")]
struct Page {
    title: &'static str,
    countries: Arc<Vec<Country>>,
}

#[derive(Debug)]
struct Country {
    code: String,
    columns: Vec<String>,
    days: Vec<Day>,
}

#[derive(Debug)]
struct Day {
    date: String,
    cells: Vec<Option<Cell>>,
}

#[derive(Debug, PartialEq)]
struct Cell {
    items: String,
    query: String,
    /// Since the day before.
    change: Option<i64>,
    /// Of the count it is a part of.
    share: Option<String>,
}

/// `(country, day, key, items, query, main language)`
type Row = (String, String, String, i64, String, String);

static COUNTRIES: TimedCache<Vec<Country>> = TimedCache::new(Duration::from_secs(24 * 3600));

pub async fn handler(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let countries = COUNTRIES
        .get_or_fetch(async || {
            let mut conn = state.tool_db.conn(Database::Monitor).await?;
            let sql = "SELECT country_code, day, `key`, items, query, main_lang FROM country ORDER BY day";
            let rows: Vec<Row> = conn.query(sql).await?;
            Ok::<_, anyhow::Error>(countries(rows))
        })
        .await?;
    render(&Page { title: "Country stats", countries })
}

/// The countries, in the order they were first counted.
fn countries(rows: Vec<Row>) -> Vec<Country> {
    let mut shares: HashMap<String, &str> = SHARES.iter().map(|(k, base)| (k.to_string(), *base)).collect();
    for (_, _, _, _, _, language) in &rows {
        shares.insert(format!("citizens_no_{language}.wp"), "citizens");
        shares.insert(format!("by_admin_or_coord_no_{language}.wp"), "by_admin_or_coord");
    }
    let mut order: Vec<String> = vec![];
    // Country → day → key → (items, query)
    let mut data: HashMap<String, BTreeMap<String, HashMap<String, (i64, String)>>> = HashMap::new();
    let mut keys: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (country, day, key, items, query, _) in rows {
        if !data.contains_key(&country) {
            order.push(country.clone());
        }
        keys.entry(country.clone()).or_default().insert(key.clone());
        data.entry(country).or_default().entry(day).or_default().insert(key, (items, query));
    }
    order
        .into_iter()
        .map(|code| {
            let columns: Vec<String> = keys.remove(&code).unwrap_or_default().into_iter().collect();
            let mut previous: Option<&HashMap<String, (i64, String)>> = None;
            let mut days = vec![];
            for (day, counts) in data.get(&code).into_iter().flatten() {
                let cells = columns
                    .iter()
                    .map(|key| {
                        let (items, query) = counts.get(key)?;
                        let change = previous.and_then(|p| p.get(key)).map(|(before, _)| items - before);
                        let base = shares.get(key).and_then(|base| counts.get(*base)).map(|(n, _)| *n);
                        Some(Cell {
                            items: thousands(u64::try_from(*items).unwrap_or(0)),
                            query: query.clone(),
                            change: change.filter(|c| *c != 0),
                            share: base
                                .filter(|b| *b != 0)
                                .map(|b| format!("{:.1}%", 100.0 * *items as f64 / b as f64)),
                        })
                    })
                    .collect();
                days.push(Day { date: format_date(day), cells });
                previous = Some(counts);
            }
            Country { columns: columns.iter().map(|c| c.replace('_', " ")).collect(), code, days }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(country: &str, day: &str, key: &str, items: i64) -> Row {
        (
            country.into(),
            day.into(),
            key.into(),
            items,
            if key == "citizens" { "CLAIM[27:145]".into() } else { String::new() },
            "en".into(),
        )
    }

    #[test]
    fn days_with_changes_and_shares() {
        let rows = vec![
            row("UK", "20141111", "citizens", 1000),
            row("UK", "20141111", "citizens_no_en.wp", 250),
            row("FR", "20141112", "citizens", 5),
            row("UK", "20141112", "citizens", 1200),
        ];
        let countries = countries(rows);
        assert_eq!(countries.iter().map(|c| c.code.as_str()).collect::<Vec<_>>(), ["UK", "FR"]);
        let uk = &countries[0];
        assert_eq!(uk.columns, ["citizens", "citizens no en.wp"]);
        assert_eq!(uk.days[0].date, "2014-11-11");
        let share = uk.days[0].cells[1].as_ref().unwrap();
        assert_eq!((share.items.as_str(), share.share.as_deref(), share.change), ("250", Some("25.0%"), None));
        let later = uk.days[1].cells[0].as_ref().unwrap();
        assert_eq!((later.items.as_str(), later.change, later.query.as_str()), ("1,200", Some(200), "CLAIM[27:145]"));
        assert_eq!(uk.days[1].cells[1], None);
    }

    #[tokio::test]
    async fn old_address_redirects() {
        let dir = tempfile::tempdir().unwrap();
        let response = crate::web::test_support::get(dir.path(), "/country_stats.php").await;
        assert_eq!(response.headers["location"], "/country_stats/");
    }
}
