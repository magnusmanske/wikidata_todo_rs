//! `last_changed_items.php`: the items edited since `date` (until `until`,
//! if given), as a JSON(P) array of IDs; with `deleted`, the items deleted
//! or turned into redirects in that time instead. The array is streamed.
//!
//! Both times are database timestamps (shorter is padded) or relative
//! phrases such as "yesterday". A range covers at most [`MAX_RANGE`]: a later
//! (or missing) `until` is moved back to `date` plus that.

use crate::state::AppState;
use crate::timestamp::{self, Timestamp};
use crate::web::{AppError, Params, is_valid_callback, query_body};
use crate::wiki::Wiki;
use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use chrono::TimeDelta;
use serde::Deserialize;
use std::sync::Arc;
use wikimisc::mysql_async::Params as SqlParams;
use wikimisc::server::jsonp;

/// About a million items change on Wikidata in a week.
const MAX_RANGE: TimeDelta = TimeDelta::days(7);

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    date: String,
    #[serde(default)]
    until: String,
    deleted: Option<String>,
    callback: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let callback = request.callback.filter(|c| !c.is_empty());
    if callback.as_deref().is_some_and(|c| !is_valid_callback(c)) {
        return Err(AppError::BadRequest("invalid callback name".to_string()));
    }
    let bad = |name: &str| AppError::BadRequest(format!("{name}: please give {}", timestamp::HELP));
    let from = Timestamp::parse(&request.date).ok_or_else(|| bad("date"))?;
    let until = match request.until.trim() {
        "" => None,
        until => Some(Timestamp::parse(until).ok_or_else(|| bad("until"))?),
    };
    let until = bounded(&from, until);
    // `parts`: how many parts of the query have the range.
    let (sql, tables, parts) = match request.deleted {
        Some(_) => (
            "SELECT log_title FROM logging WHERE log_type='delete' AND log_action='delete' AND log_namespace=0
             AND log_timestamp BETWEEN ? AND ?
             UNION SELECT page_title FROM page WHERE page_namespace=0 AND page_is_redirect=1
             AND page_links_updated BETWEEN ? AND ?",
            &["logging", "page"][..],
            2,
        ),
        None => (
            "SELECT DISTINCT page_title FROM revision JOIN page ON page_id=rev_page
             WHERE page_namespace=0 AND rev_timestamp BETWEEN ? AND ?",
            &["revision", "page"][..],
            1,
        ),
    };
    let range = [from.as_str(), until.as_str()];
    let params = SqlParams::Positional(range.repeat(parts).into_iter().map(Into::into).collect());
    let conn = state.replicas.conn(&Wiki::wikidata(), tables).await?;
    let (head, tail) = match &callback {
        Some(callback) => (format!("{callback}(["), "])".to_string()),
        None => ("[".to_string(), "]".to_string()),
    };
    let body = query_body(conn, sql.to_string(), params, head, tail, |title: String, first| {
        let quoted = serde_json::to_string(&title).ok()?;
        Some(if first { quoted } else { format!(",{quoted}") })
    })
    .await?;
    let content_type = if callback.is_some() { jsonp::CONTENT_TYPE } else { "application/json" };
    let mut response = ([(header::CONTENT_TYPE, content_type)], body).into_response();
    response.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    Ok(response)
}

/// `until`, or now, but no later than [`MAX_RANGE`] after `from`.
fn bounded(from: &Timestamp, until: Option<Timestamp>) -> Timestamp {
    let until = until.unwrap_or_else(Timestamp::now);
    match from.to_datetime().and_then(|from| from.checked_add_signed(MAX_RANGE)) {
        Some(latest) => until.min(Timestamp::from_datetime(latest)),
        None => until,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn ranges_are_bounded() {
        let t = |s: &str| Timestamp::parse(s).unwrap();
        assert_eq!(bounded(&t("20240101"), Some(t("20240103"))), t("20240103"));
        assert_eq!(bounded(&t("20240101"), Some(t("20250101"))), t("20240108"));
        assert_eq!(bounded(&t("20240101"), None), t("20240108"));
    }

    #[tokio::test]
    async fn bad_input_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(get(dir.path(), "/last_changed_items.php").await.status.as_u16(), 400);
        assert_eq!(get(dir.path(), "/last_changed_items.php?date=soon").await.status.as_u16(), 400);
        let response = get(dir.path(), "/last_changed_items.php?date=20240101&callback=alert(1)").await;
        assert_eq!(response.status.as_u16(), 400);
    }
}
