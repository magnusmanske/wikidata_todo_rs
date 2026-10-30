//! HTTP layer: the tools, the PHP tool's static files, and what became of
//! retired URLs.

mod apps;
mod error;
mod legacy;
mod page;
mod paging;
mod params;
mod static_files;
mod stream;

pub use error::AppError;
pub use page::{api_json, is_valid_callback, jsonp, render, thousands};
pub use paging::Paging;
pub use params::Params;
pub use stream::query_body;

use crate::state::AppState;
use axum::Router;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use std::sync::Arc;
use std::time::Duration;
use tower_http::set_header::SetResponseHeaderLayer;

/// Some replica queries and SPARQL queries legitimately take minutes.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
const HEALTH_PATH: &str = "/healthz";

pub fn router(state: Arc<AppState>) -> Router {
    let static_files = static_files::service(&state.config.public_html);
    let router = crate::tools::routes()
        .route(HEALTH_PATH, get(wikimisc::server::health::ok))
        .route(page::STYLESHEET_PATH, get(page::stylesheet))
        .with_state(state)
        .merge(legacy::routes())
        .fallback_service(static_files)
        .layer(header_layer(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(header_layer(header::REFERRER_POLICY, "strict-origin-when-cross-origin"));
    let router = wikimisc::server::timeout(router, REQUEST_TIMEOUT, StatusCode::GATEWAY_TIMEOUT);
    let router = wikimisc::server::standard_layers(router);
    // Each request's status and duration, for the dashboard.
    wikimisc::server::request_metrics(router, &[HEALTH_PATH])
}

fn header_layer(name: header::HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

pub use wikimisc::server::{is_same_origin, moved};

/// The themed 404 page, for anything no route or file matches.
async fn not_found() -> impl IntoResponse {
    AppError::NotFound
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    pub struct TestResponse {
        pub status: StatusCode,
        pub headers: axum::http::HeaderMap,
        pub body: String,
    }

    /// The full router, serving static files from `public_html`.
    pub async fn get(public_html: &std::path::Path, uri: &str) -> TestResponse {
        send(public_html, Request::get(uri).body(Body::empty()).unwrap()).await
    }

    pub async fn send(public_html: &std::path::Path, request: Request<Body>) -> TestResponse {
        // Credentials are only read here; nothing connects.
        let credentials = public_html.join("test.my.cnf");
        std::fs::write(&credentials, "[client]\nuser=test\npassword=test\n").unwrap();
        let config: crate::config::Config = serde_json::from_value(serde_json::json!({
            "public_html": public_html,
            "data_dir": public_html,
            "replica_credentials": credentials,
            "oauth": public_html.join("none.ini"),
        }))
        .unwrap();
        let state = AppState::new(config).unwrap();
        let response = router(Arc::new(state)).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        TestResponse { status, headers, body: String::from_utf8_lossy(&bytes).into_owned() }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::get;
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn same_origin() {
        let headers = |pairs: &[(&'static str, &str)]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.insert(*k, v.parse().unwrap());
            }
            h
        };
        assert!(is_same_origin(&headers(&[("sec-fetch-site", "same-origin")])));
        assert!(!is_same_origin(&headers(&[
            ("sec-fetch-site", "cross-site"),
            ("host", "a.org"),
            ("referer", "https://a.org/")
        ])));
        assert!(is_same_origin(&headers(&[("host", "a.org"), ("referer", "https://a.org/duplicity/")])));
        assert!(!is_same_origin(&headers(&[("host", "a.org"), ("origin", "https://a.org.evil.com")])));
        assert!(!is_same_origin(&headers(&[("host", "a.org")])));
    }

    #[tokio::test]
    async fn health_and_stylesheet() {
        let dir = tempfile::tempdir().unwrap();
        let health = get(dir.path(), "/healthz").await;
        assert_eq!((health.status.as_u16(), health.body.as_str()), (200, "ok"));
        let css = get(dir.path(), "/assets/style.css").await;
        assert_eq!(css.status.as_u16(), 200);
        assert_eq!(css.headers["content-type"], "text/css; charset=utf-8");
        assert_eq!(css.headers["x-content-type-options"], "nosniff");
    }

    #[tokio::test]
    async fn unknown_paths_get_the_themed_404() {
        let dir = tempfile::tempdir().unwrap();
        let response = get(dir.path(), "/no_such_tool.php").await;
        assert_eq!(response.status.as_u16(), 404);
        assert!(response.body.contains("/assets/style.css"));
    }
}
