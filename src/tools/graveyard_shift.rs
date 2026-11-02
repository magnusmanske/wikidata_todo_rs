//! `graveyard_shift/`: the app's page, in the look of the other tools. Its
//! scripts, component templates and styles are static files.

use crate::web::{AppError, render};
use askama::Template;
use axum::response::Html;

#[derive(Template)]
#[template(path = "graveyard_shift.html")]
struct Page {
    title: &'static str,
}

pub async fn page() -> Result<Html<String>, AppError> {
    render(&Page { title: "Graveyard shift" })
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

    #[tokio::test]
    async fn page_and_files() {
        let dir = tempfile::tempdir().unwrap();
        for path in ["/graveyard_shift/", "/graveyard_shift/index.html"] {
            let page = get(dir.path(), path).await;
            assert_eq!(page.status.as_u16(), 200, "{path}");
            assert!(page.body.contains("/assets/style.css"), "{path}");
            assert!(page.body.contains("id=\"app\""), "{path}");
            assert!(page.body.contains("/graveyard_shift/vue.js"), "{path}");
        }
        // The app's page, addressed without the trailing slash, as it was served.
        let slashless = get(dir.path(), "/graveyard_shift").await;
        assert_eq!(slashless.status.as_u16(), 308);
        assert_eq!(slashless.headers["location"], "/graveyard_shift/");
        // The files the app loads are unchanged.
        for (path, kind) in [
            ("/graveyard_shift/vue.js", "javascript"),
            ("/graveyard_shift/styles.css", "text/css"),
            ("/graveyard_shift/vue_components/main-page.html", "text/html"),
        ] {
            let file = get(dir.path(), path).await;
            assert_eq!(file.status.as_u16(), 200, "{path}");
            assert!(file.headers["content-type"].to_str().unwrap().contains(kind), "{path}");
        }
    }
}
