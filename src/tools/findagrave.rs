//! `findagrave_recent_celeb_deaths_xml.php`: once a proxy of Find a Grave's
//! RSS feed of recent famous deaths, for `recent_deaths.html`. That feed is
//! gone (Find a Grave refuses all automated requests), so this answers with
//! a valid, empty feed: the page then lists nothing instead of failing.

use axum::http::header;
use axum::response::IntoResponse;

const EMPTY_FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel>
<title>Find a Grave: recent deaths of famous people</title>
<link>https://www.findagrave.com/</link>
<description>Find a Grave no longer offers this feed.</description>
</channel></rss>
"#;

pub async fn handler() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/rss+xml; charset=utf-8")], EMPTY_FEED)
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

    #[tokio::test]
    async fn empty_feed() {
        let dir = tempfile::tempdir().unwrap();
        let response = get(dir.path(), "/findagrave_recent_celeb_deaths_xml.php").await;
        assert_eq!(response.status.as_u16(), 200);
        assert!(response.body.contains("<channel>") && !response.body.contains("<item>"));
    }
}
