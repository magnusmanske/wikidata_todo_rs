//! Retired URLs: pages of the PHP tool that were not ported, and now
//! either redirect to a replacement or are gone for good.

use super::{AppError, moved};
use axum::Router;
use axum::response::IntoResponse;
use axum::routing::get;

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

pub fn routes() -> Router {
    RETIRED.iter().fold(Router::new(), |router, (path, fate)| {
        let handler = match fate {
            Fate::Gone => get(|| async { AppError::Gone.into_response() }),
            Fate::MovedTo(url) => moved(url),
        };
        router.route(path, handler)
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
    }
}
