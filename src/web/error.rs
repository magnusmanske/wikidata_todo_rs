//! Errors a request can end in, shown as themed pages.

use crate::wiki::UnknownWiki;
use askama::Template;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The message is shown to the user.
    #[error("{0}")]
    BadRequest(String),
    #[error("not found")]
    NotFound,
    /// A retired page.
    #[error("gone")]
    Gone,
    /// A wiki or other service failed us. Details are logged, not shown.
    #[error(transparent)]
    Upstream(#[from] anyhow::Error),
    #[error("{0}")]
    Internal(String),
    /// Too much expensive work is in flight. Not a server failure, but a
    /// request to come back later.
    #[error("too many requests")]
    TooManyRequests,
}

impl From<UnknownWiki> for AppError {
    fn from(e: UnknownWiki) -> Self {
        Self::BadRequest(e.to_string())
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage<'a> {
    title: &'a str,
    message: &'a str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message.as_str()),
            Self::NotFound => (StatusCode::NOT_FOUND, "There is nothing at this address."),
            Self::Gone => (StatusCode::GONE, "This page has been retired."),
            Self::Upstream(e) => {
                tracing::warn!("upstream failure: {e:#}");
                (
                    StatusCode::BAD_GATEWAY,
                    "A server this tool relies on did not answer as expected. Please try again later.",
                )
            }
            Self::Internal(e) => {
                tracing::error!("internal error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on our side.")
            }
            Self::TooManyRequests => {
                (StatusCode::TOO_MANY_REQUESTS, "This tool is busy with other requests. Please try again in a minute.")
            }
        };
        let title = status.canonical_reason().unwrap_or("Error");
        let mut response = match (ErrorPage { title, message }).render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => (status, message.to_string()).into_response(),
        };
        if matches!(self, Self::TooManyRequests) {
            response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_many_requests_asks_to_retry() {
        let response = AppError::TooManyRequests.into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "60");
    }
}
