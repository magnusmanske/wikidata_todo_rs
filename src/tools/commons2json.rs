//! `commons2json.php`: what the `{{Information}}` and licence templates of a
//! Commons file page say, as JSON(P): `data` (dates, descriptions, authors,
//! licences) and `problem` (what could not be understood). Templates come
//! from the page's Parsoid HTML, as they did from the old VisualEditor API.

use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, api_json};
use crate::wiki::{Wiki, encode_component};
use anyhow::Context;
use axum::extract::State;
use axum::response::Response;
use futures::{StreamExt, TryStreamExt, stream};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

/// Licence templates: name as normalized by [`template_name`], parameter
/// with the attribution, item, version, language.
const LICENCES: &[(&str, &str, &str, &str, &str)] = &[("Cc-by-sa-2.0-de", "1", "Q6905942", "2.0", "de")];
const IGNORED: &[&str] = &["Int:license-header", "Int:filedesc", "Original upload log"];
/// Sources that say the uploader made the file; lower case.
const SELF_MADE: &[&str] = &["self-made", "selbst fotografiert", "{{own}}", "{{own work}}"];
/// Wiki texts rendered at a time.
const CONCURRENCY: usize = 4;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    file: String,
    callback: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let file = request.file.trim();
    let file = mediawiki::strip_namespace(file, &["file", "image"]).unwrap_or(file);
    if file.is_empty() {
        return Err(AppError::BadRequest("Missing parameter: file".to_string()));
    }
    let html = page_html(&state.http, &format!("File:{file}"))
        .await?
        .ok_or_else(|| AppError::BadRequest(format!("There is no file {file} on Commons")))?;
    let templates = templates(&html);
    let mut wikitexts = wiki_texts(&templates);
    wikitexts.sort_unstable();
    wikitexts.dedup();
    let texts = plain_texts(&state.http, &wikitexts).await?;
    Ok(api_json(Report::of(&templates, &texts), request.callback))
}

/// The Parsoid HTML of a Commons page; `None` if there is no such page.
async fn page_html(http: &reqwest::Client, title: &str) -> anyhow::Result<Option<String>> {
    let wiki = Wiki::commons();
    let url = format!("https://{}/w/rest.php/v1/page/{}/html", wiki.host(), encode_component(&title.replace(' ', "_")));
    let response = http.get(url).send().await.with_context(|| format!("cannot load {title}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    Ok(Some(response.error_for_status()?.text().await?))
}

/// The templates (`{"target": {"wt": …}, "params": {…}}`) of the page, in order.
fn templates(html: &str) -> Vec<Value> {
    static DATA_MW: LazyLock<Regex> = LazyLock::new(|| regex(r#"data-mw=(?:'([^']*)'|"([^"]*)")"#));
    DATA_MW
        .captures_iter(html)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)))
        .filter_map(|m| serde_json::from_str::<Value>(&wikimisc::escape::unescape_html(m.as_str())).ok())
        .flat_map(|data| data["parts"].as_array().cloned().unwrap_or_default())
        .filter_map(|mut part| part.get_mut("template").map(Value::take))
        .collect()
}

/// `Information`, `Cc-by-sa-2.0-de`: trimmed, lower case but the first letter, spaces for underscores.
fn template_name(template: &Value) -> Option<String> {
    let name = template["target"]["wt"].as_str()?.replace('_', " ");
    Some(crate::wiki::upper_first(&name.trim().to_lowercase()))
}

/// The wiki text of a template parameter; for `{{Information}}`, its name in any case.
fn param<'a>(template: &'a Value, name: &str) -> Option<&'a str> {
    let params = template["params"].as_object()?;
    let found = params.get(name).or_else(|| params.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v));
    found?["wt"].as_str()
}

/// The descriptions in a description's wiki text: `(language, wiki text)`
/// from `{{en|1=…}}` templates, else all of it, as English.
fn descriptions(wikitext: &str) -> Vec<(String, String)> {
    static LANGUAGE_TEMPLATE: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)\{\{([a-z-]+)\|\d+=(.+?)\}\}"));
    let found: Vec<(String, String)> =
        LANGUAGE_TEMPLATE.captures_iter(wikitext).map(|c| (c[1].to_string(), c[2].to_string())).collect();
    match found.is_empty() {
        true => vec![("en".to_string(), wikitext.to_string())],
        false => found,
    }
}

/// The wiki texts to render as plain text: descriptions and attributions.
fn wiki_texts(templates: &[Value]) -> Vec<String> {
    let mut texts = vec![];
    for template in templates {
        let name = template_name(template).unwrap_or_default();
        if name == "Information" {
            let description = param(template, "Description").unwrap_or_default();
            if !description.trim().is_empty() {
                texts.extend(descriptions(description).into_iter().map(|(_, text)| text));
            }
        } else if let Some((_, key, ..)) = LICENCES.iter().find(|l| l.0 == name) {
            texts.extend(param(template, key).map(str::to_string));
        }
    }
    texts
}

/// Wiki texts as plain text, rendered by Commons.
async fn plain_texts(http: &reqwest::Client, wikitexts: &[String]) -> anyhow::Result<HashMap<String, String>> {
    let wiki = Wiki::commons();
    stream::iter(wikitexts.iter().cloned())
        .map(|wikitext: String| {
            let wiki = &wiki;
            async move {
                let html = mediawiki::parse_to_html(http, wiki, &wikitext).await?;
                anyhow::Ok((wikitext, mediawiki::html_to_text(&html).trim().to_string()))
            }
        })
        .buffer_unordered(CONCURRENCY)
        .try_collect()
        .await
}

/// The answer: lists of findings by key, as before.
#[derive(Debug, Default)]
struct Report {
    problem: Map<String, Value>,
    data: Map<String, Value>,
}

impl Report {
    fn of(templates: &[Value], texts: &HashMap<String, String>) -> Value {
        let mut report = Self::default();
        let text = |wikitext: &str| texts.get(wikitext).cloned().unwrap_or_default();
        for template in templates {
            let Some(name) = template_name(template) else { continue };
            if name == "Information" {
                for key in ["Description", "Source", "Date", "Author"] {
                    if let Some(value) = param(template, key) {
                        report.information(key, value, &text);
                    }
                }
            } else if let Some((_, key, q, version, lang)) = LICENCES.iter().find(|l| l.0 == name) {
                let mut licence = json!({"q": q, "version": version, "lang": lang});
                if let Some(attribution) = param(template, key) {
                    licence["attribution_text"] = text(attribution).into();
                }
                report.add_data("license", licence);
            } else if !IGNORED.contains(&name.as_str()) {
                report.add_problem("template", template.to_string());
            }
        }
        // PHP wrote empty lists as `[]`.
        let list_or_map = |map: Map<String, Value>| if map.is_empty() { json!([]) } else { Value::Object(map) };
        json!({"problem": list_or_map(report.problem), "data": list_or_map(report.data)})
    }

    fn information(&mut self, key: &str, wikitext: &str, text: &impl Fn(&str) -> String) {
        static DATE: LazyLock<Regex> = LazyLock::new(|| regex(r"^(\d{3,4})(?:-(\d{1,2})(?:-(\d{1,2}))?)?$"));
        static USER_LINK: LazyLock<Regex> = LazyLock::new(|| regex(r"^\[\[(?::([a-z-]+):)?[Uu]ser:(.+?)\|(.+?)\]\]$"));
        let trimmed = wikitext.trim();
        match key {
            "Date" => match DATE.captures(trimmed) {
                Some(c) => {
                    let mut date = Map::new();
                    for (i, part) in ["year", "month", "day"].iter().enumerate() {
                        if let Some(n) = c.get(i + 1).and_then(|m| m.as_str().parse::<u32>().ok()) {
                            date.insert(part.to_string(), n.into());
                        }
                    }
                    self.add_data("date", Value::Object(date));
                }
                None => self.add_problem("date", wikitext),
            },
            "Source" => match SELF_MADE.contains(&trimmed.to_lowercase().as_str()) {
                true => {
                    self.data.insert("made_by_uploader".to_string(), 1.into());
                }
                false => self.add_problem("source", wikitext),
            },
            "Description" if !trimmed.is_empty() => {
                for (language, wikitext) in descriptions(wikitext) {
                    self.add_data("description", json!({"language": language, "text": text(&wikitext)}));
                }
            }
            "Author" => match USER_LINK.captures(trimmed) {
                Some(c) if c[2].trim().to_lowercase() == c[3].trim().to_lowercase() => {
                    let wiki = c.get(1).map_or("commons", |m| m.as_str());
                    let author = json!({"user_name": &c[2], "source_type": "wikimedia", "wiki": wiki});
                    self.add_data("author", author);
                }
                _ => self.add_problem("author", wikitext),
            },
            _ => {}
        }
    }

    fn add_data(&mut self, key: &str, value: Value) {
        push(&mut self.data, key, value);
    }

    fn add_problem(&mut self, key: &str, value: impl Into<Value>) {
        push(&mut self.problem, key, value.into());
    }
}

fn push(map: &mut Map<String, Value>, key: &str, value: Value) {
    if let Value::Array(list) = map.entry(key).or_insert_with(|| json!([])) {
        list.push(value);
    }
}

fn regex(pattern: &str) -> Regex {
    // The patterns are constants, covered by the tests.
    #[allow(clippy::expect_used)]
    Regex::new(pattern).expect("valid regex")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    const HTML: &str = r#"<div data-mw='{"parts":[{"template":{"target":{"wt":"int:filedesc","function":"int"},"params":{},"i":0}}]}'></div>
<table data-mw='{"parts":["x",{"template":{"target":{"wt":"Information\n","href":"./Template:Information"},"params":{
"description":{"wt":"{{en|1=A &apos;cat&apos;.}} {{de|1=Eine Katze.}}"},"source":{"wt":"self-made"},"date":{"wt":"2005-09-10"},
"author":{"wt":"[[User:Alice|alice]]"}},"i":0}}]}'></table>
<span data-mw="{&quot;parts&quot;:[{&quot;template&quot;:{&quot;target&quot;:{&quot;wt&quot;:&quot;Cc-by-sa-2.0-de&quot;},&quot;params&quot;:{&quot;1&quot;:{&quot;wt&quot;:&quot;''Bob''&quot;}}}},{&quot;template&quot;:{&quot;target&quot;:{&quot;wt&quot;:&quot;Odd&quot;},&quot;params&quot;:{}}}]}"></span>"#;

    #[test]
    fn templates_of_parsoid_html() {
        let templates = templates(HTML);
        let names: Vec<String> = templates.iter().filter_map(template_name).collect();
        assert_eq!(names, ["Int:filedesc", "Information", "Cc-by-sa-2.0-de", "Odd"]);
        assert_eq!(wiki_texts(&templates), ["A 'cat'.", "Eine Katze.", "''Bob''"]);
        let texts = HashMap::from([
            ("A 'cat'.".to_string(), "A 'cat'.".to_string()),
            ("Eine Katze.".to_string(), "Eine Katze.".to_string()),
            ("''Bob''".to_string(), "Bob".to_string()),
        ]);
        let report = Report::of(&templates, &texts);
        assert_eq!(
            report.to_string(),
            r#"{"problem":{"template":["{\"target\":{\"wt\":\"Odd\"},\"params\":{}}"]},"data":{"description":[{"language":"en","text":"A 'cat'."},{"language":"de","text":"Eine Katze."}],"made_by_uploader":1,"date":[{"year":2005,"month":9,"day":10}],"author":[{"user_name":"Alice","source_type":"wikimedia","wiki":"commons"}],"license":[{"q":"Q6905942","version":"2.0","lang":"de","attribution_text":"Bob"}]}}"#
        );
    }

    #[test]
    fn plain_descriptions_are_english() {
        assert_eq!(descriptions("A cat"), [("en".to_string(), "A cat".to_string())]);
        assert_eq!(Report::of(&[], &HashMap::new()).to_string(), r#"{"problem":[],"data":[]}"#);
    }

    #[tokio::test]
    async fn file_is_required() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(get(dir.path(), "/commons2json.php?file=").await.status.as_u16(), 400);
    }
}
