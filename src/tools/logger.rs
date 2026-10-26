//! Counts uses of the tools, per tool and day, in the usage log of
//! magnustools (tool `wikidata-todo`, the tool's name as method).

use crate::state::AppState;
use axum::extract::Request;
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

const LOGGER: &str = "https://magnustools.toolforge.org/logger.php";

/// Middleware: counts a request as a use if it brings parameters (not just
/// opening the form) and does not fail.
pub async fn count_uses(state: Arc<AppState>, tool: &'static str, request: Request, next: Next) -> Response {
    let with_parameters = request.method() == Method::POST || request.uri().query().is_some_and(|q| !q.is_empty());
    let response = next.run(request).await;
    if with_parameters && response.status().as_u16() < 400 {
        // In the background: counting must not slow down or break the tool.
        tokio::spawn(async move {
            let request = state.http.get(LOGGER).query(&[("tool", "wikidata-todo"), ("method", tool)]);
            if let Err(e) = request.send().await.and_then(reqwest::Response::error_for_status) {
                tracing::debug!("cannot count a use of {tool}: {e}");
            }
        });
    }
    response
}
