//! `sourcery_proxy.php`: fetches a page for `sourcery.html`, which cannot
//! load it across sites itself. Only HTTPS URLs on wikipedia.org and
//! wikimedia.org and their subdomains, redirects included; `http://` URLs
//! there (which the page sends) are fetched over HTTPS.

use crate::state::USER_AGENT;
use crate::web::Params;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use reqwest::Url;
use reqwest::redirect::Policy;
use serde::Deserialize;
use std::sync::LazyLock;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BYTES: usize = 10 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const DOMAINS: &[&str] = &["wikipedia.org", "wikimedia.org"];

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    url: String,
}

/// A client that follows redirects only to allowed URLs.
static CLIENT: LazyLock<reqwest::Result<reqwest::Client>> = LazyLock::new(|| {
    let policy = Policy::custom(|attempt| match attempt.previous().len() < MAX_REDIRECTS && allowed(attempt.url()) {
        true => attempt.follow(),
        false => attempt.error("redirect not allowed"),
    });
    reqwest::Client::builder().user_agent(USER_AGENT).timeout(TIMEOUT).redirect(policy).build()
});

pub async fn handler(Params(request): Params<Request>) -> Response {
    let Some(url) = parse(&request.url) else {
        return error(StatusCode::FORBIDDEN, "URL not allowed");
    };
    match fetch(url).await {
        Ok((content_type, body)) => {
            let mut response = body.into_response();
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, content_type);
            // Wiki pages must not run as pages of this site.
            headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox"));
            response
        }
        Err(e) => {
            tracing::debug!("sourcery proxy: {e:#}");
            error(StatusCode::BAD_GATEWAY, "Failed to fetch URL")
        }
    }
}

/// An allowed URL, from user input.
fn parse(input: &str) -> Option<Url> {
    let input = input.trim();
    let input = match input.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("http://") => format!("https://{}", input.get(7..)?),
        _ => input.to_string(),
    };
    Url::parse(&input).ok().filter(allowed)
}

/// HTTPS on the default port, no credentials, a host on one of [`DOMAINS`].
fn allowed(url: &Url) -> bool {
    let Some(host) = url.domain() else { return false };
    let on_domain = DOMAINS.iter().any(|domain| {
        host.strip_suffix(domain).is_some_and(|sub| {
            sub.is_empty()
                || sub.strip_suffix('.').is_some_and(|sub| {
                    sub.split('.')
                        .all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
                })
        })
    });
    on_domain
        && url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

/// The content type (JSON if not given) and body, at most [`MAX_BYTES`] of it.
async fn fetch(url: Url) -> anyhow::Result<(HeaderValue, Vec<u8>)> {
    let client = CLIENT.as_ref().map_err(|e| anyhow::anyhow!("no HTTP client: {e}"))?;
    let mut response = client.get(url).send().await?.error_for_status()?;
    let content_type =
        response.headers().get(header::CONTENT_TYPE).cloned().unwrap_or(HeaderValue::from_static("application/json"));
    let mut body = vec![];
    while let Some(chunk) = response.chunk().await? {
        body.extend_from_slice(&chunk);
        anyhow::ensure!(body.len() <= MAX_BYTES, "response larger than {MAX_BYTES} bytes");
    }
    Ok((content_type, body))
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], serde_json::json!({ "error": message }).to_string())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn only_wikimedia_urls() {
        let ok = |url: &str| parse(url).map(|u| u.to_string());
        assert_eq!(ok("https://en.wikipedia.org/wiki/X").as_deref(), Some("https://en.wikipedia.org/wiki/X"));
        assert_eq!(ok("http://en.wikipedia.org/wiki/X").as_deref(), Some("https://en.wikipedia.org/wiki/X"));
        assert!(ok("https://wikimedia.org").is_some());
        assert!(ok("https://upload.wikimedia.org/a.jpg").is_some());
        for bad in [
            "",
            "https://evil.com/",
            "https://wikipedia.org.evil.com/",
            "https://evilwikipedia.org/",
            "https://en.wikipedia.org@evil.com/",
            "https://user@en.wikipedia.org/",
            "https://en.wikipedia.org:8443/",
            "ftp://en.wikipedia.org/",
            "https://127.0.0.1/",
            "file:///etc/passwd",
        ] {
            assert_eq!(ok(bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn refuses_other_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let response = get(dir.path(), "/sourcery_proxy.php?url=https%3A%2F%2Fevil.com%2F").await;
        assert_eq!(response.status.as_u16(), 403);
        assert_eq!(response.body, r#"{"error":"URL not allowed"}"#);
    }
}
