//! The "Pasleim project merge" game's data: pairs of items listed on the
//! subpages of User:Pasleim/projectmerge, as `public_html/pasleim_projectmerge.json`.

use super::Writes;
use crate::mediawiki;
use crate::state::AppState;
use crate::wiki::Wiki;
use anyhow::{Result, bail};
use regex::Regex;
use std::collections::BTreeSet;
use std::sync::LazyLock;

const PREFIX: &str = "Pasleim/projectmerge/";
const NS_USER: &str = "2";
/// Pages per request.
const BATCH: usize = 50;

/// A list row naming two items: `# [[Q1]] … [[Q2]]`.
static ROW: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?m)^#\s*\[\[Q(\d+)\]\].+?\[\[Q(\d+)\]\]").ok());

pub async fn update(state: &AppState, writes: Writes) -> Result<()> {
    let wiki = Wiki::wikidata();
    let titles = subpages(&state.http, &wiki).await?;
    let mut pairs = BTreeSet::new();
    for batch in titles.chunks(BATCH) {
        for text in wikitexts(&state.http, &wiki, batch).await? {
            pairs.extend(self::pairs(&text));
        }
    }
    // An empty list means trouble, not "no pairs": keep the old file.
    if pairs.is_empty() {
        bail!("no pairs found on {} pages", titles.len());
    }
    let json: Vec<[u64; 2]> = pairs.iter().map(|(a, b)| [*a, *b]).collect();
    let path = state.config.public_html.join("pasleim_projectmerge.json");
    writes.file(&path, serde_json::to_string(&json)?.as_bytes()).await?;
    println!("{} pairs from {} pages", json.len(), titles.len());
    Ok(())
}

/// The wikitexts of up to 50 pages, in one request (the API allows no more).
async fn wikitexts(http: &reqwest::Client, wiki: &Wiki, titles: &[String]) -> Result<Vec<String>> {
    let titles = titles.join("|");
    let params =
        [("action", "query"), ("prop", "revisions"), ("rvprop", "content"), ("rvslots", "main"), ("titles", &titles)];
    let json = api(http, wiki, &params).await?;
    let pages = json["query"]["pages"].as_array().into_iter().flatten();
    Ok(pages.filter_map(|p| p["revisions"][0]["slots"]["main"]["content"].as_str().map(str::to_string)).collect())
}

async fn subpages(http: &reqwest::Client, wiki: &Wiki) -> Result<Vec<String>> {
    let mut titles = vec![];
    let mut cont: Option<String> = None;
    loop {
        let mut params = vec![
            ("action", "query"),
            ("list", "allpages"),
            ("apnamespace", NS_USER),
            ("apprefix", PREFIX),
            ("aplimit", "max"),
        ];
        if let Some(c) = &cont {
            params.push(("apcontinue", c));
        }
        let json = api(http, wiki, &params).await?;
        let pages = json["query"]["allpages"].as_array().into_iter().flatten();
        titles.extend(pages.filter_map(|p| p["title"].as_str().map(str::to_string)));
        cont = json["continue"]["apcontinue"].as_str().map(str::to_string);
        if cont.is_none() {
            return Ok(titles);
        }
    }
}

/// An API request; an answer with an error is an error.
async fn api(http: &reqwest::Client, wiki: &Wiki, params: &[(&str, &str)]) -> Result<serde_json::Value> {
    let json = mediawiki::api_patiently(http, wiki, params).await?;
    if let Some(error) = json.get("error") {
        bail!("API error: {error}");
    }
    Ok(json)
}

/// The item pairs in a page's wikitext, each as `(smaller, larger)` number.
fn pairs(text: &str) -> Vec<(u64, u64)> {
    let Some(row) = ROW.as_ref() else { return vec![] };
    row.captures_iter(text)
        .filter_map(|c| {
            let a: u64 = c.get(1)?.as_str().parse().ok()?;
            let b: u64 = c.get(2)?.as_str().parse().ok()?;
            (a != b).then(|| (a.min(b), a.max(b)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_from_rows() {
        let text = "Intro [[Q1]] [[Q2]]\n# [[Q20]] (x) – [[Q10]]\n#[[Q5]]→[[Q6]] and [[Q7]]\n# [[Q3]] [[Q3]]";
        assert_eq!(pairs(text), [(10, 20), (5, 6)]);
    }
}
