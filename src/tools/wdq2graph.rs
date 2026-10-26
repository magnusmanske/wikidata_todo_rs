//! `wdq2graph.php`: the tree of items below a root item, along properties
//! that point upwards (e.g. "parent taxon"), as a graph in DOT or TGF.

use crate::sparql;
use crate::state::AppState;
use crate::terms;
use crate::web::{AppError, Params, render};
use crate::wikidata::parse_id;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::sync::Arc;

const MAX_NODES: usize = 5000;
const MAX_PROPERTIES: usize = 10;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    q: String,
    props: String,
    lang: String,
    format: String,
    doit: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self { q: String::new(), props: String::new(), lang: "en".to_string(), format: "dot".to_string(), doit: None }
    }
}

/// The tree: its nodes in result order, and its edges `(parent, child, property)`.
#[derive(Debug, Default, PartialEq)]
struct Graph {
    root: String,
    nodes: Vec<String>,
    edges: Vec<(String, String, String)>,
}

impl Graph {
    fn dot(&self, label: impl Fn(&str) -> String) -> String {
        let mut out = format!("digraph root_is_{} {{\n", self.root);
        for node in &self.nodes {
            let _ = writeln!(out, "\t{node} [label={}];", quoted(&label(node)));
        }
        for (parent, child, property) in &self.edges {
            let _ = writeln!(out, "\t{parent} -> {child} [label={}];", quoted(&label(property)));
        }
        out.push('}');
        out
    }

    /// Trivial Graph Format, with the numbers of the items as node IDs.
    fn tgf(&self, label: impl Fn(&str) -> String) -> String {
        let number = |id: &str| id.get(1..).unwrap_or_default().to_string();
        let mut out = String::new();
        for node in &self.nodes {
            let _ = writeln!(out, "{}\t{}", number(node), label(node));
        }
        out.push_str("#\n");
        for (parent, child, property) in &self.edges {
            let _ = writeln!(out, "{}\t{}\t{}", number(parent), number(child), label(property));
        }
        out
    }
}

/// A DOT string.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[derive(Template)]
#[template(path = "tools/wdq2graph.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    graph: Option<String>,
    is_dot: bool,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let is_dot = request.format != "tgf";
    let mut page = Page { title: "Item graph", request: &request, graph: None, is_dot };
    if request.doit.is_none() {
        return render(&page);
    }
    let root = parse_id(&request.q, 'Q')
        .ok_or_else(|| AppError::BadRequest("Please give a root item, such as Q146481.".to_string()))?;
    let properties: Vec<String> = request.props.split(',').filter_map(|p| parse_id(p, 'P')).collect();
    if properties.is_empty() || properties.len() > MAX_PROPERTIES {
        return Err(AppError::BadRequest(format!("Please give one to {MAX_PROPERTIES} properties, such as P171.")));
    }
    let json = state.sparql.json(&query(&root, &properties)).await?;
    let graph = graph(root, &json)?;
    let mut ids = graph.nodes.clone();
    ids.extend(properties);
    let language = request.lang.trim();
    let labels = match crate::wiki::is_language_code(language) {
        true => terms::labels(&state.replicas, &ids, &[language]).await?,
        false => HashMap::new(),
    };
    let label = |id: &str| labels.get(id).map_or_else(|| id.to_string(), |label| format!("{label} ({id})"));
    page.graph = Some(if is_dot { graph.dot(label) } else { graph.tgf(label) });
    render(&page)
}

/// The tree's items, each with its links upwards.
fn query(root: &str, properties: &[String]) -> String {
    let properties: Vec<String> = properties.iter().map(|p| format!("wdt:{p}")).collect();
    format!(
        "SELECT ?item ?prop ?parent {{ ?item ({})* wd:{root} . \
         OPTIONAL {{ VALUES ?prop {{ {} }} ?item ?prop ?parent }} }} LIMIT {}",
        properties.join("|"),
        properties.join(" "),
        MAX_NODES * 10
    )
}

fn graph(root: String, json: &serde_json::Value) -> Result<Graph, AppError> {
    let id =
        |row: &serde_json::Value, var: &str| row[var]["value"].as_str().and_then(sparql::entity_id).map(str::to_string);
    let mut graph = Graph { root, ..Graph::default() };
    let mut nodes = HashSet::new();
    let mut links = vec![];
    for row in sparql::bindings(json) {
        let Some(item) = id(row, "item") else { continue };
        if nodes.insert(item.clone()) {
            graph.nodes.push(item.clone());
        }
        if let (Some(parent), Some(property)) = (id(row, "parent"), id(row, "prop")) {
            links.push((parent, item, property));
        }
    }
    if graph.nodes.len() > MAX_NODES {
        return Err(AppError::BadRequest(format!(
            "This tree has more than {MAX_NODES} items; please pick a smaller one."
        )));
    }
    // Links to items outside the tree are no edges of it.
    let mut seen = HashSet::new();
    graph.edges =
        links.into_iter().filter(|(parent, ..)| nodes.contains(parent)).filter(|e| seen.insert(e.clone())).collect();
    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;
    use serde_json::json;

    fn sample() -> Graph {
        let uri = |id: &str| json!({"type": "uri", "value": format!("http://www.wikidata.org/entity/{id}")});
        let direct = json!({"type": "uri", "value": "http://www.wikidata.org/prop/direct/P171"});
        let json = json!({"results": {"bindings": [
            {"item": uri("Q1"), "prop": direct, "parent": uri("Q99")},
            {"item": uri("Q2"), "prop": direct, "parent": uri("Q1")},
            {"item": uri("Q2"), "prop": direct, "parent": uri("Q1")},
        ]}});
        graph("Q1".to_string(), &json).unwrap()
    }

    #[test]
    fn graphs() {
        let graph = sample();
        assert_eq!(graph.nodes, ["Q1", "Q2"]);
        assert_eq!(graph.edges, [("Q1".to_string(), "Q2".to_string(), "P171".to_string())]);
        let label = |id: &str| if id == "Q1" { "a \"b\" (Q1)".to_string() } else { id.to_string() };
        assert_eq!(
            graph.dot(label),
            "digraph root_is_Q1 {\n\tQ1 [label=\"a \\\"b\\\" (Q1)\"];\n\tQ2 [label=\"Q2\"];\n\tQ1 -> Q2 [label=\"P171\"];\n}"
        );
        assert_eq!(graph.tgf(label), "1\ta \"b\" (Q1)\n2\tQ2\n#\n1\t2\tP171\n");
    }

    #[test]
    fn tree_query() {
        assert_eq!(
            query("Q5", &["P361".to_string(), "P279".to_string()]),
            "SELECT ?item ?prop ?parent { ?item (wdt:P361|wdt:P279)* wd:Q5 . \
             OPTIONAL { VALUES ?prop { wdt:P361 wdt:P279 } ?item ?prop ?parent } } LIMIT 50000"
        );
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/wdq2graph/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"props\""));
        assert_eq!(get(dir.path(), "/wdq2graph/?q=Q1&props=&doit=1").await.status.as_u16(), 400);
    }
}
