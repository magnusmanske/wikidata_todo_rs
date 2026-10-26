//! `relator2/api.php`, the API of the Relator app (`relator2/`): a random
//! person with one of the family relations; other actions go to Widar.

use super::relator::{person_answer, random_person};
use super::widar::{self, Login};
use crate::state::AppState;
use crate::web::Params;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

/// The relations `random_person` may pick a person by.
const RELATIONS: &[&str] = &["P22", "P25", "P26", "P40", "P3373", "P1066", "P802", "P1038", "P451", "P1290", "P3448"];

pub async fn api(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Params(params): Params<HashMap<String, String>>,
) -> Response {
    let action = params.get("action").map_or("", String::as_str);
    if action == "random_person" {
        let properties = relations(params.get("prop").map_or("P40", String::as_str));
        let property = properties.get(rand::random_range(0..properties.len())).copied().unwrap_or("P40");
        return person_answer(random_person(&state, property).await);
    }
    let login = Login {
        callback: format!("{}/relator2/api.php", state.config.public_url),
        forward: "/relator2/".to_string(),
        authorize: "/relator2/api.php?action=authorize".to_string(),
    };
    match widar::respond(&state, &headers, &params, &login).await {
        Some(response) => response,
        None => Json(json!({"status": format!("Unknown action '{action}'"), "data": []})).into_response(),
    }
}

/// `P40`, `p22,P25`, `any` (or nothing): the allowed ones of them, at least P40.
fn relations(prop: &str) -> Vec<&'static str> {
    let prop = prop.trim();
    if prop.is_empty() || prop.eq_ignore_ascii_case("any") {
        return RELATIONS.to_vec();
    }
    let wanted: Vec<String> = prop.split(',').map(|p| p.trim().to_uppercase()).collect();
    let chosen: Vec<&str> = RELATIONS.iter().copied().filter(|r| wanted.iter().any(|w| w == r)).collect();
    if chosen.is_empty() { vec!["P40"] } else { chosen }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn relation_lists() {
        assert_eq!(relations("p22, P25"), ["P22", "P25"]);
        assert_eq!(relations("any").len(), RELATIONS.len());
        assert_eq!(relations("").len(), RELATIONS.len());
        assert_eq!(relations("P31,x'"), ["P40"]);
    }

    #[tokio::test]
    async fn unknown_action() {
        let dir = tempfile::tempdir().unwrap();
        let response = get(dir.path(), "/relator2/api.php?action=x").await;
        assert_eq!(response.body, r#"{"status":"Unknown action 'x'","data":[]}"#);
    }
}
