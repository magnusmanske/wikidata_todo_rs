//! Retired URLs: pages of the PHP tool that were not ported, and now
//! either redirect to a replacement or are gone for good.

use super::{AppError, moved};
use axum::Router;
use axum::http::Uri;
use axum::response::{IntoResponse, Redirect};
use axum::routing::{any, get};

enum Fate {
    Gone,
    /// Redirects there, keeping the query string (and, for POST, the body).
    MovedTo(&'static str),
}

const PETSCAN: &str = "https://petscan.wmcloud.org/";

const RETIRED: &[(&str, Fate)] = &[
    // Deactivated in the PHP tool already.
    ("/wdq_feed.php", Fate::Gone),
    ("/sexer.php", Fate::Gone),
    ("/random_item_without_instance.php", Fate::Gone),
    ("/no_statements.php", Fate::MovedTo("/important_blank_items/")),
    // Needs all humans at once, more than the query service gives.
    ("/firstnamebasis.php", Fate::Gone),
    // Wikidata no longer allows two items to link to the same page.
    ("/true_duplicates.php", Fate::Gone),
    // Never finished; its game was never registered.
    ("/viaf_game.php", Fate::Gone),
    // AutoList and its downloads became PetScan; QuickStatements has its own tool.
    ("/autolist2.php", Fate::MovedTo(PETSCAN)),
    ("/download_query.php", Fate::MovedTo(PETSCAN)),
    ("/quick_statements.php", Fate::MovedTo("https://quickstatements.toolforge.org/")),
];

/// Directories that became tools of their own: everything below redirects
/// to the same path there.
const MOVED_DIRECTORIES: &[(&str, &str)] = &[("/wikiflix", "https://wikiflix.toolforge.org")];

pub fn routes() -> Router {
    let router = RETIRED.iter().fold(Router::new(), |router, (path, fate)| {
        let handler = match fate {
            Fate::Gone => get(|| async { AppError::Gone.into_response() }),
            Fate::MovedTo(url) => moved(url),
        };
        router.route(path, handler)
    });
    MOVED_DIRECTORIES.iter().fold(router, |router, (dir, to)| {
        let handler = any(move |uri: Uri| async move {
            let rest = uri.path().strip_prefix(dir).unwrap_or_default().trim_start_matches('/');
            let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
            Redirect::permanent(&format!("{to}/{rest}{query}"))
        });
        router
            .route(dir, handler.clone())
            .route(&format!("{dir}/"), handler.clone())
            .route(&format!("{dir}/{{*rest}}"), handler)
    })
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

    #[tokio::test]
    async fn retired_urls() {
        let dir = tempfile::tempdir().unwrap();
        for gone in ["/sexer.php", "/true_duplicates.php", "/viaf_game.php?action=desc"] {
            assert_eq!(get(dir.path(), gone).await.status.as_u16(), 410, "{gone}");
        }
        let moved = get(dir.path(), "/autolist2.php?manual_list=Q1").await;
        assert_eq!(moved.status.as_u16(), 308);
        assert_eq!(moved.headers["location"], "https://petscan.wmcloud.org/?manual_list=Q1");
        let location = async |uri| get(dir.path(), uri).await.headers["location"].to_str().unwrap().to_string();
        assert_eq!(location("/wikiflix").await, "https://wikiflix.toolforge.org/");
        assert_eq!(location("/wikiflix/").await, "https://wikiflix.toolforge.org/");
        assert_eq!(
            location("/wikiflix/api.php?action=get_movie&q=1").await,
            "https://wikiflix.toolforge.org/api.php?action=get_movie&q=1"
        );
    }
}
