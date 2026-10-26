//! `wd_edit_stats.php`: edits per month and per user, to the items of a
//! SPARQL query, in a date range.

use crate::replicas::{entity_pages, select_in};
use crate::state::AppState;
use crate::timestamp::{self, Timestamp};
use crate::web::{AppError, Params, render, thousands};
use crate::wiki::{Wiki, encode_title};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Items (and other entities) of the query that are counted.
const MAX_ITEMS: usize = 100_000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    sparql: String,
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
    doit: Option<String>,
}

struct Stats {
    items: String,
    capped: bool,
    total: String,
    unregistered: String,
    /// `(2017-08, edits)`, by month.
    months: Vec<(String, String)>,
    /// `(user, user page URL, edits)`, most edits first.
    users: Vec<(String, String, String)>,
}

#[derive(Template)]
#[template(path = "tools/wd_edit_stats.html")]
struct Page {
    title: &'static str,
    request: Request,
    help: &'static str,
    max_items: String,
    stats: Option<Stats>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mut page = Page {
        title: "Edit stats",
        request,
        help: timestamp::HELP,
        max_items: thousands(MAX_ITEMS as u64),
        stats: None,
    };
    if page.request.doit.is_none() || page.request.sparql.trim().is_empty() {
        return render(&page);
    }
    let parse = |name: &str, input: &str| match input.trim() {
        "" => Ok(None),
        input => Timestamp::parse(input)
            .map(Some)
            .ok_or_else(|| AppError::BadRequest(format!("{name}: {}", timestamp::HELP))),
    };
    // Empty: from the beginning, until now.
    let from = parse("Start date", &page.request.from)?;
    let to = parse("End date", &page.request.to)?.unwrap_or_else(Timestamp::now);
    let found = state
        .sparql
        .entities(&page.request.sparql, None, MAX_ITEMS)
        .await
        .map_err(|e| AppError::BadRequest(format!("The SPARQL query failed: {e:#}")))?;
    let (items, capped) = (found.ids, found.total > MAX_ITEMS);
    page.stats = Some(stats(&state, &items, from.as_ref(), &to, capped).await?);
    render(&page)
}

async fn stats(
    state: &AppState,
    items: &[String],
    from: Option<&Timestamp>,
    to: &Timestamp,
    capped: bool,
) -> anyhow::Result<Stats> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page", "revision", "actor_revision"]).await?;
    let pages: Vec<u64> = entity_pages(&mut conn, items).await?.into_iter().map(|(page, _)| page).collect();
    let range = [from.map_or("", Timestamp::as_str).into(), to.as_str().into()];
    let sql =
        "SELECT LEFT(rev_timestamp, 6), COUNT(*) FROM revision WHERE rev_timestamp BETWEEN ? AND ? AND rev_page IN";
    let rows: Vec<(String, u64)> = select_in(&mut conn, sql, "GROUP BY 1", &range, &pages).await?;
    let months = sum(rows);
    let sql = "SELECT actor_name, COUNT(*) FROM revision JOIN actor_revision ON actor_id=rev_actor
               WHERE actor_user IS NOT NULL AND rev_timestamp BETWEEN ? AND ? AND rev_page IN";
    let rows: Vec<(String, u64)> = select_in(&mut conn, sql, "GROUP BY actor_name", &range, &pages).await?;
    let mut users: Vec<(String, u64)> = sum(rows).into_iter().collect();
    users.sort_by(|(u1, n1), (u2, n2)| n2.cmp(n1).then_with(|| u1.cmp(u2)));
    let total: u64 = months.values().sum();
    let registered: u64 = users.iter().map(|(_, n)| n).sum();
    Ok(Stats {
        items: thousands(items.len() as u64),
        capped,
        total: thousands(total),
        unregistered: thousands(total.saturating_sub(registered)),
        months: months.into_iter().map(|(month, n)| (pretty_month(&month), thousands(n))).collect(),
        users: users
            .into_iter()
            .map(|(user, n)| {
                let url = format!("https://www.wikidata.org/wiki/User:{}", encode_title(&user));
                (user, url, thousands(n))
            })
            .collect(),
    })
}

/// Counts per key, summed over the chunks of a query.
fn sum(rows: Vec<(String, u64)>) -> BTreeMap<String, u64> {
    let mut sums = BTreeMap::new();
    for (key, n) in rows {
        *sums.entry(key).or_default() += n;
    }
    sums
}

/// `201708` as `2017-08`.
fn pretty_month(month: &str) -> String {
    match (month.get(..4), month.get(4..)) {
        (Some(year), Some(month)) => format!("{year}-{month}"),
        _ => month.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn sums_and_months() {
        let sums = sum(vec![("b".into(), 1), ("a".into(), 2), ("b".into(), 3)]);
        assert_eq!(sums.into_iter().collect::<Vec<_>>(), [("a".to_string(), 2), ("b".to_string(), 4)]);
        assert_eq!(pretty_month("201708"), "2017-08");
    }

    #[tokio::test]
    async fn form_and_bad_dates() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/wd_edit_stats/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"sparql\""));
        let bad = get(dir.path(), "/wd_edit_stats/?doit=1&sparql=SELECT+%3Fq+%7B%7D&from=soon").await;
        assert_eq!(bad.status.as_u16(), 400);
    }
}
