//! `reverse_tree.php`: all categories above a page, level by level, up to
//! the top of the category tree.

use crate::replicas::{db_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;

const NS_CATEGORY: i64 = 14;
/// Trees of some wikis reach thousands of categories; stop there.
const MAX_CATEGORIES: usize = 10_000;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    project: String,
    namespace: String,
    title: String,
    doit: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            language: "en".to_string(),
            project: "wikipedia".to_string(),
            namespace: "0".to_string(),
            title: String::new(),
            doit: None,
        }
    }
}

/// `child` is in category `parent` (database titles).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Edge {
    child: String,
    parent: String,
}

struct Tree {
    /// The edges that reach a new category, level by level.
    levels: Vec<Vec<Edge>>,
    /// All categories, in the order they were reached.
    categories: Vec<String>,
    truncated: bool,
    permalink: String,
    category_url: String,
}

#[derive(Template)]
#[template(path = "tools/reverse_tree.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    namespace: i64,
    tree: Option<Tree>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let namespace: i64 = request.namespace.trim().parse().map_err(|_| AppError::BadRequest("bad namespace".into()))?;
    let tree = match request.doit.is_some() && !request.title.trim().is_empty() {
        true => {
            let wiki = Wiki::new(&request.language, &request.project)?;
            Some(tree(&state, &wiki, &request, namespace).await?)
        }
        false => None,
    };
    render(&Page { title: "Reverse category tree", request: &request, namespace, tree })
}

async fn tree(state: &AppState, wiki: &Wiki, request: &Request, namespace: i64) -> Result<Tree, AppError> {
    let title = db_title(&request.title);
    let mut conn = state.replicas.conn(wiki, crate::replicas::CATEGORY_TABLES).await?;
    let sql = "SELECT page_title, lt_title FROM page
        JOIN categorylinks ON cl_from=page_id JOIN linktarget ON cl_target_id=lt_id
        WHERE page_namespace=? AND page_title IN";
    let mut seen = HashSet::new();
    let (mut levels, mut categories) = (vec![], vec![]);
    let mut current = vec![title.clone()];
    let mut ns = namespace;
    while !current.is_empty() && categories.len() < MAX_CATEGORIES {
        let mut rows: Vec<(String, String)> = select_in(&mut conn, sql, "", &[ns.into()], &current).await?;
        rows.sort();
        let edges: Vec<Edge> = rows
            .into_iter()
            .filter(|(_, parent)| seen.insert(parent.clone()))
            .map(|(child, parent)| Edge { child, parent })
            .collect();
        current = edges.iter().map(|e| e.parent.clone()).collect();
        categories.extend(current.iter().cloned());
        if !edges.is_empty() {
            levels.push(edges);
        }
        ns = NS_CATEGORY;
    }
    let permalink = format!(
        "/reverse_tree/?doit=1&language={}&project={}&title={}&namespace={namespace}",
        encode_component(request.language.trim()),
        encode_component(request.project.trim()),
        encode_component(&title)
    );
    Ok(Tree {
        levels,
        truncated: categories.len() >= MAX_CATEGORIES,
        categories,
        permalink,
        category_url: format!("https://{}/wiki/Category:", wiki.host()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/reverse_tree/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("name=\"title\""));
        assert_eq!(get(dir.path(), "/reverse_tree/?namespace=x").await.status.as_u16(), 400);
        let unknown = get(dir.path(), "/reverse_tree/?doit=1&title=X&language=evil.com%23").await;
        assert_eq!(unknown.status.as_u16(), 400);
    }
}
