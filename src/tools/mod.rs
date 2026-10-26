//! The tools, one module each, at the URLs of their PHP originals.
//!
//! Pages live at `/<name>/`; their old addresses, `/<name>.php` and
//! `/<name>`, redirect there. APIs, which other tools and scripts call, stay
//! at their old addresses.

mod add_name_labels;
mod artwork_images;
mod awarder;
mod beacon;
mod best_cats;
mod bioacoustica;
mod canned_queries;
mod cloudy_concept;
mod commons2json;
mod coords2json;
mod country_stats;
mod creator;
mod creator_from_wikidata;
mod description_api;
mod dupe_finder;
mod duplicity;
mod duplicity_game_api;
mod findagrave;
mod get_item_names;
mod gnd;
mod images_in_categories_used_on_commons;
mod images_near_items;
mod important_blank_items;
mod index;
mod label_no_instance;
mod last_changed_items;
mod linked_items;
mod logger;
mod merge_candidates_by_shared_identifier;
mod missing_wp_animal_audio;
mod multibeacon;
mod new_items;
mod odnb_redirect;
mod pagepile2kml;
mod pasleim_projectmerge_game_api;
mod personendaten;
mod project_stats;
mod property_string_matcher;
mod relabel;
mod related_properties;
mod relator;
mod relator2;
mod resolver;
mod reverse_tree;
mod sourcerer_game;
mod sourcery_proxy;
mod sparql_rc;
mod stats;
mod subclass_dates;
mod suggestor;
mod test_descriptions;
mod translate_items_with_property;
mod unused_images_of_people;
mod user_edits;
mod wd_commons;
mod wd_edit_stats;
mod wdq2graph;
mod wdq_image_feed;
mod widar;
mod wikidata_cache;
mod wikiflix;
mod wp_no_image;

use crate::state::AppState;
use crate::web::moved;
use axum::Router;
use axum::extract::{Request, State as Extract};
use axum::middleware::{self, Next};
use axum::routing::{MethodRouter, get};
use std::sync::Arc;

type State = Arc<AppState>;

/// The tools with a page, by title, for the menu of every page.
pub const MENU: &[(&str, &str)] = &[
    ("missing_wp_animal_audio", "Animal sounds missing on Wikipedia"),
    ("artwork_images", "Artwork images"),
    ("awarder", "Awarder"),
    ("best_cats", "Best categories"),
    ("bioacoustica", "BioAcoustica file matcher"),
    ("cloudy_concept", "Concept cloud"),
    ("country_stats", "Country stats"),
    ("creator_from_wikidata", "Creator from Wikidata"),
    ("test_descriptions", "Description test"),
    ("dupe_finder", "Duplicate item finder"),
    ("wd_edit_stats", "Edit stats"),
    ("get_item_names", "Get item names"),
    ("wdq_image_feed", "Image feed"),
    ("images_in_categories_used_on_commons", "Images in categories used on Wikidata"),
    ("images_near_items", "Images near items"),
    ("important_blank_items", "Important blank items"),
    ("wdq2graph", "Item graph"),
    ("label_no_instance", "Labels without “instance of”"),
    ("linked_items", "Linked items"),
    ("multibeacon", "Multi-BEACON"),
    ("add_name_labels", "Names as labels"),
    ("new_items", "New items"),
    ("pagepile2kml", "PagePile to KML"),
    ("personendaten", "Personendaten"),
    ("project_stats", "Project stats"),
    ("property_string_matcher", "Property string matcher"),
    ("relabel", "ReLabel"),
    ("related_properties", "Related properties"),
    ("resolver", "Resolver"),
    ("reverse_tree", "Reverse category tree"),
    ("sourcerer_game", "Sourcerer game"),
    ("sparql_rc", "SPARQL recent changes"),
    ("stats", "Statement stats"),
    ("subclass_dates", "Subclass dates"),
    ("translate_items_with_property", "Translations of items with a property"),
    ("unused_images_of_people", "Unused images of people"),
    ("user_edits", "User edits"),
    ("beacon", "Wikidata BEACON"),
    ("wikidata_cache", "Wikidata cache"),
    ("wd_commons", "Wikidata Commons search"),
];

/// Pages that are APIs as well (other tools call them with `format=json`
/// and such), so their old address answers directly instead of redirecting.
const ALSO_AT_OLD_ADDRESS: &[&str] =
    &["get_item_names", "related_properties", "resolver", "sparql_rc", "user_edits", "wikidata_cache"];

pub fn routes(state: &State) -> Router<State> {
    let router = Router::new().route("/", get(index::handler)).route("/index.php", get(index::handler));
    let router = pages().into_iter().fold(router, |router, (name, handler)| {
        let handler = counted(state, name, handler);
        let path = format!("/{name}/");
        let router = router.route(&path, handler.clone()).route(&format!("/{name}"), moved(&path));
        match ALSO_AT_OLD_ADDRESS.contains(&name) {
            true => router.route(&format!("/{name}.php"), handler),
            false => router.route(&format!("/{name}.php"), moved(&path)),
        }
    });
    apis().into_iter().fold(router, |router, (path, name, handler)| router.route(path, counted(state, name, handler)))
}

/// Counts the uses of a tool (see [`logger`]).
fn counted(state: &State, name: &'static str, handler: MethodRouter<State>) -> MethodRouter<State> {
    handler.layer(middleware::from_fn_with_state(
        state.clone(),
        move |Extract(state): Extract<State>, request: Request, next: Next| {
            logger::count_uses(state, name, request, next)
        },
    ))
}

/// Both methods: the PHP tools read `$_REQUEST`, so their forms and callers use either.
macro_rules! any_method {
    ($handler:path) => {
        get($handler).post($handler)
    };
}

fn pages() -> Vec<(&'static str, MethodRouter<State>)> {
    vec![
        ("add_name_labels", any_method!(add_name_labels::handler)),
        ("artwork_images", any_method!(artwork_images::handler)),
        ("awarder", any_method!(awarder::handler)),
        ("beacon", any_method!(beacon::handler)),
        ("best_cats", any_method!(best_cats::handler)),
        ("bioacoustica", any_method!(bioacoustica::handler)),
        ("cloudy_concept", any_method!(cloudy_concept::handler)),
        ("country_stats", any_method!(country_stats::handler)),
        ("creator_from_wikidata", any_method!(creator_from_wikidata::handler)),
        ("dupe_finder", any_method!(dupe_finder::handler)),
        ("get_item_names", any_method!(get_item_names::handler)),
        ("images_in_categories_used_on_commons", any_method!(images_in_categories_used_on_commons::handler)),
        ("images_near_items", any_method!(images_near_items::handler)),
        ("important_blank_items", any_method!(important_blank_items::handler)),
        ("label_no_instance", any_method!(label_no_instance::handler)),
        ("linked_items", any_method!(linked_items::handler)),
        ("missing_wp_animal_audio", any_method!(missing_wp_animal_audio::handler)),
        ("multibeacon", any_method!(multibeacon::handler)),
        ("new_items", any_method!(new_items::handler)),
        ("pagepile2kml", any_method!(pagepile2kml::handler)),
        ("personendaten", any_method!(personendaten::handler)),
        ("project_stats", any_method!(project_stats::handler)),
        ("property_string_matcher", any_method!(property_string_matcher::handler)),
        ("relabel", any_method!(relabel::handler)),
        ("related_properties", any_method!(related_properties::handler)),
        ("resolver", any_method!(resolver::handler)),
        ("reverse_tree", any_method!(reverse_tree::handler)),
        ("sourcerer_game", any_method!(sourcerer_game::handler)),
        ("sparql_rc", any_method!(sparql_rc::handler)),
        ("stats", any_method!(stats::handler)),
        ("subclass_dates", any_method!(subclass_dates::handler)),
        ("test_descriptions", any_method!(test_descriptions::handler)),
        ("translate_items_with_property", any_method!(translate_items_with_property::handler)),
        ("unused_images_of_people", any_method!(unused_images_of_people::handler)),
        ("user_edits", any_method!(user_edits::handler)),
        ("wd_commons", any_method!(wd_commons::handler)),
        ("wd_edit_stats", any_method!(wd_edit_stats::handler)),
        ("wdq2graph", any_method!(wdq2graph::handler)),
        ("wdq_image_feed", any_method!(wdq_image_feed::handler)),
        ("wikidata_cache", any_method!(wikidata_cache::handler)),
    ]
}

/// `(path, name for the usage log, handler)`.
fn apis() -> Vec<(&'static str, &'static str, MethodRouter<State>)> {
    vec![
        ("/api.php", "widar", any_method!(widar::handler)),
        // Static pages here call the Widar of the old shared host.
        ("/widar/index.php", "widar", any_method!(widar::handler)),
        ("/canned_queries.php", "canned_queries", any_method!(canned_queries::handler)),
        ("/commons2json.php", "commons2json", any_method!(commons2json::handler)),
        ("/coords2json.php", "coords2json", any_method!(coords2json::handler)),
        ("/creator.php", "creator", any_method!(creator::handler)),
        ("/description_api.php", "description_api", any_method!(description_api::handler)),
        ("/duplicity.php", "duplicity", any_method!(duplicity::redirect)),
        ("/duplicity/api.php", "duplicity", any_method!(duplicity::api)),
        ("/duplicity_game_api.php", "duplicity_game_api", any_method!(duplicity_game_api::handler)),
        ("/findagrave_recent_celeb_deaths_xml.php", "findagrave", any_method!(findagrave::handler)),
        ("/gnd.php", "gnd", any_method!(gnd::handler)),
        ("/last_changed_items.php", "last_changed_items", any_method!(last_changed_items::handler)),
        (
            "/merge_candidates_by_shared_identifier.php",
            "merge_candidates_by_shared_identifier",
            any_method!(merge_candidates_by_shared_identifier::handler),
        ),
        ("/odnb_redirect.php", "odnb_redirect", any_method!(odnb_redirect::handler)),
        (
            "/pasleim_projectmerge_game_api.php",
            "pasleim_projectmerge_game_api",
            any_method!(pasleim_projectmerge_game_api::handler),
        ),
        ("/relator/api.php", "relator", any_method!(relator::api)),
        ("/relator2/api.php", "relator2", any_method!(relator2::api)),
        ("/sourcery_proxy.php", "sourcery_proxy", any_method!(sourcery_proxy::handler)),
        ("/suggestor.php", "suggestor", any_method!(suggestor::handler)),
        ("/wikiflix/api.php", "wikiflix", any_method!(wikiflix::api)),
        ("/wp_no_image/api.php", "wp_no_image", any_method!(wp_no_image::api)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn menu_has_every_page_in_title_order() {
        let mut pages: Vec<&str> = pages().into_iter().map(|(name, _)| name).collect();
        let mut menu: Vec<&str> = MENU.iter().map(|(name, _)| *name).collect();
        pages.sort_unstable();
        menu.sort_unstable();
        assert_eq!(menu, pages);
        assert!(MENU.windows(2).all(|w| w[0].1.to_lowercase() < w[1].1.to_lowercase()));
    }

    #[tokio::test]
    async fn old_addresses_redirect() {
        let dir = tempfile::tempdir().unwrap();
        let old = get(dir.path(), "/reverse_tree.php?title=X&doit=1").await;
        assert_eq!(old.status.as_u16(), 308);
        assert_eq!(old.headers["location"], "/reverse_tree/?title=X&doit=1");
        assert_eq!(get(dir.path(), "/reverse_tree").await.headers["location"], "/reverse_tree/");
    }
}
