//! `wdq_image_feed.php`: an RSS feed of images (for a screensaver, say): a
//! random sample of the images of the items of a SPARQL (or WDQ) query.

use crate::mediawiki;
use crate::replicas::select_in;
use crate::sparql;
use crate::state::AppState;
use crate::wdq;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use futures::{StreamExt, TryStreamExt, stream};
use percent_encoding::percent_decode_str;
use rand::seq::SliceRandom;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

const MAX_IMAGES: usize = 500;
/// Files per `imageinfo` request: the API's maximum.
const BATCH: usize = 50;
const CONCURRENCY: usize = 4;
const LOGO: &str =
    "https://upload.wikimedia.org/wikipedia/commons/thumb/6/66/Wikidata-logo-en.svg/200px-Wikidata-logo-en.svg.png";
const DEMO_SPARQL: &str = "SELECT ?item ?image WHERE {\n?item wdt:P31 ?sub0 .\n?sub0 (wdt:P279)* wd:Q12280 .\n\
                           ?item wdt:P177 wd:Q1653 .\n?item wdt:P18 ?image\n}";

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    wdq: String,
    sparql: String,
    max: usize,
    thumbsize: u32,
    doit: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self { wdq: String::new(), sparql: String::new(), max: 50, thumbsize: 300, doit: None }
    }
}

/// An image of an item.
#[derive(Debug, Clone, PartialEq)]
struct Image {
    item: String,
    /// The file name, with spaces.
    file: String,
}

#[derive(Template)]
#[template(path = "tools/wdq_image_feed.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    supported: &'static str,
    demo_url: String,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let (wdq, sparql) = (request.wdq.trim(), request.sparql.trim());
    if request.doit.is_none() || (wdq.is_empty() && sparql.is_empty()) {
        let demo_url = format!("/wdq_image_feed/?sparql={}&doit=1", encode_component(DEMO_SPARQL));
        let page = Page { title: "Image feed", request: &request, supported: wdq::SUPPORTED, demo_url };
        return Ok(render(&page)?.into_response());
    }
    let query = match sparql.is_empty() {
        true => format!("SELECT ?item ?image {{ {} ?item wdt:P18 ?image }}", wdq::pattern(wdq)?),
        false => sparql.to_string(),
    };
    let mut images = images(&wdq::run(&state.sparql, &query).await?);
    images.shuffle(&mut rand::rng());
    images.truncate(request.max.clamp(1, MAX_IMAGES));
    let files: Vec<String> = images.iter().map(|i| i.file.clone()).collect();
    let thumbs = thumbnails(&state.http, &files, request.thumbsize.clamp(50, 2000)).await?;
    let uploaders = uploaders(&state, &files).await?;

    let (name, value) = if sparql.is_empty() { ("wdq", wdq) } else { ("sparql", sparql) };
    let self_url = format!("{}/wdq_image_feed/?{name}={}&doit=1", state.config.public_url, encode_component(value));
    let feed = Feed {
        title: if wdq.is_empty() { "SPARQL query" } else { wdq },
        self_url: &self_url,
        home: &format!("{}/wdq_image_feed/", state.config.public_url),
        timestamp: &chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
    };
    let rss =
        feed.rss(&images, |file| thumbs.get(file).map(String::as_str), |file| uploaders.get(file).map(String::as_str));
    Ok(([(header::CONTENT_TYPE, "application/rss+xml; charset=utf-8")], rss).into_response())
}

/// The items and images: the query's first and second variables.
fn images(json: &Value) -> Vec<Image> {
    let vars: Vec<&str> = json["head"]["vars"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let (Some(item), Some(image)) = (vars.first(), vars.get(1)) else { return vec![] };
    sparql::bindings(json)
        .filter_map(|row| {
            let item = sparql::entity_id(row[*item]["value"].as_str()?)?.to_string();
            let image = row[*image]["value"].as_str()?;
            let file = image.split_once("Special:FilePath/").map_or(image, |(_, name)| name);
            let file = percent_decode_str(file).decode_utf8().ok()?.replace('_', " ");
            Some(Image { item, file })
        })
        .collect()
}

/// Thumbnail URLs by file name.
async fn thumbnails(http: &reqwest::Client, files: &[String], width: u32) -> anyhow::Result<HashMap<String, String>> {
    let wiki = Wiki::commons();
    let width = width.to_string();
    let batches: Vec<String> =
        files.chunks(BATCH).map(|c| c.iter().map(|f| format!("File:{f}")).collect::<Vec<_>>().join("|")).collect();
    let answers: Vec<Value> = stream::iter(batches)
        .map(|titles| {
            let (wiki, width) = (&wiki, &width);
            async move {
                let params = [
                    ("action", "query"),
                    ("prop", "imageinfo"),
                    ("iiprop", "url"),
                    ("iiurlwidth", width.as_str()),
                    ("titles", titles.as_str()),
                ];
                mediawiki::api(http, wiki, &params).await
            }
        })
        .buffer_unordered(CONCURRENCY)
        .try_collect()
        .await?;
    Ok(answers
        .iter()
        .flat_map(|json| json["query"]["pages"].as_array().into_iter().flatten())
        .filter_map(|page| {
            let file = page["title"].as_str()?.strip_prefix("File:")?;
            Some((file.to_string(), page["imageinfo"][0]["thumburl"].as_str()?.to_string()))
        })
        .collect())
}

/// Who uploaded the current version of each file, by file name.
async fn uploaders(state: &AppState, files: &[String]) -> anyhow::Result<HashMap<String, String>> {
    let names: Vec<String> = files.iter().map(|f| f.replace(' ', "_")).collect();
    let mut conn = state.replicas.conn(&Wiki::commons(), &["image", "actor"]).await?;
    let sql = "SELECT img_name, actor_name FROM image JOIN actor ON img_actor=actor_id WHERE img_name IN";
    let rows: Vec<(String, String)> = select_in(&mut conn, sql, "", &[], &names).await?;
    Ok(rows.into_iter().map(|(name, user)| (name.replace('_', " "), user)).collect())
}

struct Feed<'a> {
    title: &'a str,
    self_url: &'a str,
    home: &'a str,
    timestamp: &'a str,
}

impl Feed<'_> {
    fn rss<'f>(
        &self,
        images: &[Image],
        thumb: impl Fn(&str) -> Option<&'f str>,
        uploader: impl Fn(&str) -> Option<&'f str>,
    ) -> String {
        let (title, home, ts) = (xml(self.title), xml(self.home), self.timestamp);
        let mut rss = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\">\n<channel>\n\
             <atom:link href=\"{}\" rel=\"self\" type=\"application/rss+xml\" />\n\
             <title>{title} - www.wikidata.org</title>\n<link>{home}</link>\n\
             <description>Query {title} for www.wikidata.org</description>\n<language>en-us</language>\n\
             <copyright>Feed: CC-0; Images: see description page</copyright>\n\
             <pubDate>{ts}</pubDate>\n<lastBuildDate>{ts}</lastBuildDate>\n<generator>wikidata-todo</generator>\n\
             <docs>{home}</docs>\n\
             <image>\n<url>{LOGO}</url>\n<title>{title} - www.wikidata.org</title>\n<link>{home}</link>\n</image>\n",
            xml(self.self_url)
        );
        let commons = Wiki::commons();
        for image in images {
            let url = commons.page_url(&format!("File:{}", image.file));
            let mut html = String::new();
            if let Some(thumb) = thumb(&image.file) {
                let _ = write!(html, "<a href=\"{}\"><img border=\"0\" src=\"{}\" /></a><br/>", xml(&url), xml(thumb));
            }
            if let Some(user) = uploader(&image.file) {
                let user_url = commons.page_url(&format!("User:{user}"));
                let _ = write!(html, "Uploaded by <a href=\"{}\">{}</a><br/>", xml(&user_url), xml(user));
            }
            let item = &image.item;
            let _ =
                write!(html, "Used by Wikidata item <a href=\"https://www.wikidata.org/wiki/{item}\">{item}</a><br/>");
            let _ = write!(
                rss,
                "<item>\n<title>{}</title>\n<pubDate>{ts}</pubDate>\n<link>{url}</link>\n\
                 <guid isPermaLink=\"false\">{url} - {ts}</guid>\n<description>{}</description>\n</item>\n",
                xml(&image.file),
                xml(&html),
                url = xml(&url),
            );
        }
        rss.push_str("</channel>\n</rss>\n");
        rss
    }
}

/// Text (or an attribute value) for XML.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;
    use serde_json::json;

    #[test]
    fn images_from_results() {
        let json = json!({"head": {"vars": ["q", "img"]}, "results": {"bindings": [
            {"q": {"value": "http://www.wikidata.org/entity/Q1"},
             "img": {"value": "http://commons.wikimedia.org/wiki/Special:FilePath/A%20b%26c.jpg"}},
            {"q": {"value": "nonsense"}, "img": {"value": "x"}},
        ]}});
        assert_eq!(images(&json), [Image { item: "Q1".into(), file: "A b&c.jpg".into() }]);
    }

    #[test]
    fn rss_items() {
        let feed = Feed { title: "a&b", self_url: "https://h/?x=1&y", home: "https://h/", timestamp: "T" };
        let images = [Image { item: "Q1".into(), file: "A b.jpg".into() }];
        let rss = feed.rss(&images, |_| Some("https://t/a.jpg?x&y"), |_| None);
        assert!(rss.contains("<title>a&amp;b - www.wikidata.org</title>"));
        assert!(rss.contains("<atom:link href=\"https://h/?x=1&amp;y\""));
        assert!(rss.contains("<link>https://commons.wikimedia.org/wiki/File:A_b.jpg</link>"));
        assert!(rss.contains("&lt;img border=&quot;0&quot; src=&quot;https://t/a.jpg?x&amp;amp;y&quot; /&gt;"));
        assert!(rss.ends_with("</channel>\n</rss>\n"));
    }

    #[tokio::test]
    async fn form_and_bad_wdq() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/wdq_image_feed/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains("name=\"sparql\""));
        assert_eq!(get(dir.path(), "/wdq_image_feed/?wdq=web%5Bx%5D&doit=1").await.status.as_u16(), 400);
    }
}
