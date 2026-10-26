//! `creator_from_wikidata.php`: a Commons Creator template for a Wikidata
//! item. The template takes everything from Wikidata now, so all it needs is
//! the item.

use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::encode_title;
use crate::wikidata;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    q: String,
}

struct Creator {
    wikitext: String,
    edit_url: String,
}

#[derive(Template)]
#[template(path = "tools/creator_from_wikidata.html")]
struct Page {
    title: &'static str,
    q: String,
    creator: Option<Creator>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mut page = Page { title: "Creator from Wikidata", q: request.q.trim().to_string(), creator: None, error: None };
    if page.q.is_empty() {
        return render(&page);
    }
    let q = wikidata::parse_id(&page.q, 'Q').ok_or_else(|| AppError::BadRequest("Not an item ID.".to_string()))?;
    match wikidata::entity(&state.http, &q, "labels").await? {
        None => page.error = Some(format!("There is no item {q}.")),
        Some(item) => {
            let label = wikimisc::labels::json_term_or_any(&item, "labels", &["en", "mul"]).unwrap_or(&q);
            page.creator = Some(Creator {
                wikitext: template(&q),
                edit_url: format!(
                    "https://commons.wikimedia.org/w/index.php?title=Creator:{}&action=edit",
                    encode_title(label)
                ),
            });
        }
    }
    page.q = q;
    render(&page)
}

fn template(q: &str) -> String {
    format!("{{{{Creator\n | Option = {{{{{{1|}}}}}} <!-- Do not modify -->\n | Wikidata = {q}\n}}}}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn wikitext() {
        assert_eq!(template("Q42"), "{{Creator\n | Option = {{{1|}}} <!-- Do not modify -->\n | Wikidata = Q42\n}}");
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/creator_from_wikidata/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("name=\"q\""));
        assert_eq!(get(dir.path(), "/creator_from_wikidata/?q=xyz").await.status.as_u16(), 400);
    }
}
