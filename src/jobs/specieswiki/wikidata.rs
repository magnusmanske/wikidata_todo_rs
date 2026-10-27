//! Wikidata, as the bot reads and edits it. Without a login (a dry run),
//! edits are only printed.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use wikimisc::entity_id::{EntityId, ItemId, PropertyId};
use wikimisc::mediawiki::api::Api;
use wikimisc::retry::RetryPolicy;
use wikimisc::wikibase::{EntityTrait, ItemEntity, LocaleString, SiteLink, Statement};
use wikimisc::wikidata::{CirrusQuery, WikidataError};

pub const SITE: &str = "specieswiki";
/// Search hits to look at: enough to tell one from several, and to find a
/// person among namesakes.
const SEARCH_LIMIT: usize = 50;
/// Between edits, as the bot policy asks.
const EDIT_DELAY_MS: u64 = 2000;
/// Publications of a person to look up.
const MAX_DOIS: usize = 20;
const DOI: PropertyId = PropertyId::new(356);
const MAX_AUTHORS: usize = 50;
/// For reads that hit a rate limit or a server error.
const RETRIES: u32 = 4;

pub struct Wikidata {
    client: wikimisc::wikidata::Wikidata,
    /// Logged in as the bot; `None` in a dry run.
    api: Option<Api>,
    searches: HashMap<String, Hits>,
}

#[derive(Clone)]
struct Hits {
    items: Vec<ItemId>,
    /// Whether these are all there are.
    complete: bool,
}

/// A new item for a Wikispecies page.
pub struct NewItem {
    pub label: LocaleString,
    pub aliases: Vec<LocaleString>,
    pub statements: Vec<Statement>,
    pub page: String,
}

impl Wikidata {
    /// Logs in with the bot's credentials, unless there are none.
    pub async fn new(http: reqwest::Client, credentials: Option<&Path>) -> Result<Self> {
        let client = wikimisc::wikidata::Wikidata::with_client(crate::state::USER_AGENT, http);
        let api = match credentials {
            Some(path) => {
                let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
                let (user, password) = parse_credentials(&text).context("no user and password in the credentials")?;
                let mut api = client.api().await?;
                api.login(user, password).await.context("bot login failed")?;
                api.set_edit_delay(Some(EDIT_DELAY_MS));
                Some(api)
            }
            None => None,
        };
        Ok(Self { client, api, searches: HashMap::new() })
    }

    /// The items of those of the Wikispecies `pages` that have one.
    pub async fn items_for_pages(&self, pages: &[&str]) -> Result<HashMap<String, ItemId>> {
        Ok(patiently(|| self.client.items_for_titles(SITE, pages)).await?)
    }

    pub async fn item_for_page(&self, page: &str) -> Result<Option<ItemId>> {
        Ok(self.items_for_pages(&[page]).await?.into_values().next())
    }

    /// The items a search finds, best match first.
    pub async fn search(&mut self, query: &str) -> Result<Vec<ItemId>> {
        Ok(self.hits(query).await?.items)
    }

    /// All items a search finds; `None` if there are too many to look at.
    pub async fn search_all(&mut self, query: &str) -> Result<Option<Vec<ItemId>>> {
        let hits = self.hits(query).await?;
        Ok(hits.complete.then_some(hits.items))
    }

    /// Remembered until an item is created.
    async fn hits(&mut self, query: &str) -> Result<Hits> {
        if let Some(hits) = self.searches.get(query) {
            return Ok(hits.clone());
        }
        let results = patiently(|| self.client.search(query, 0, SEARCH_LIMIT)).await?;
        let items = results.ids.iter().filter_map(|id| id.as_item()).collect();
        let hits = Hits { items, complete: results.total_hits <= SEARCH_LIMIT as u64 };
        self.searches.insert(query.to_string(), hits.clone());
        Ok(hits)
    }

    /// The authors (P50) of the works with these DOIs; but for works with
    /// more authors than anyone could check.
    pub async fn authors(&mut self, dois: &[String]) -> Result<Vec<ItemId>> {
        let mut works = vec![];
        for doi in dois.iter().take(MAX_DOIS) {
            works.extend(self.search(&CirrusQuery::new("").has_statement(DOI, doi).to_string()).await?);
        }
        if works.is_empty() {
            return Ok(vec![]);
        }
        let works = self.items(&works, &["claims"]).await?;
        let authors = works.iter().map(|work| crate::wikidata::item_values(work, "P50"));
        let authors: HashSet<ItemId> =
            authors.filter(|a| a.len() <= MAX_AUTHORS).flatten().filter_map(|id| id.parse().ok()).collect();
        Ok(authors.into_iter().collect())
    }

    /// The items, as JSON, with only `props` (`claims|sitelinks`).
    pub async fn items(&self, items: &[ItemId], props: &[&str]) -> Result<Vec<Value>> {
        let ids: Vec<EntityId> = items.iter().map(|&q| q.into()).collect();
        Ok(patiently(|| self.client.get_entities(&ids, props, &[])).await?.into_values().collect())
    }

    /// The Wikispecies page an item links to.
    pub async fn page_of(&self, item: ItemId) -> Result<Option<String>> {
        let items = self.items(&[item], &["sitelinks"]).await?;
        Ok(items.first().and_then(|i| i["sitelinks"][SITE]["title"].as_str()).map(str::to_string))
    }

    /// Creates the item; `None` in a dry run.
    pub async fn create(&mut self, item: NewItem, summary: &str) -> Result<Option<ItemId>> {
        let data = item_json(item);
        let Some(api) = &mut self.api else {
            println!("[dry run] create {data}");
            return Ok(None);
        };
        let token = api.get_edit_token().await?;
        let params = [("action", "wbeditentity"), ("new", "item"), ("data", &data.to_string())];
        let json = edit(api, &params, token, summary).await?;
        let id = json["entity"]["id"].as_str().context("no ID for the new item")?;
        self.searches.clear();
        Ok(Some(id.parse()?))
    }

    /// Links the item to the Wikispecies page.
    pub async fn link(&mut self, item: ItemId, page: &str, summary: &str) -> Result<()> {
        let Some(api) = &mut self.api else {
            println!("[dry run] link {item} to {page}");
            return Ok(());
        };
        let token = api.get_edit_token().await?;
        let id = item.to_string();
        let params = [("action", "wbsetsitelink"), ("id", &id), ("linksite", SITE), ("linktitle", page)];
        edit(api, &params, token, summary).await?;
        Ok(())
    }
}

/// Tries again after rate limits and server errors.
async fn patiently<T, F, Fut>(op: F) -> Result<T, WikidataError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, WikidataError>>,
{
    let transient = |e: &WikidataError| match e {
        WikidataError::Http(e) => {
            e.is_timeout() || e.is_connect() || e.status().is_some_and(|s| s.as_u16() == 429 || s.is_server_error())
        }
        WikidataError::Api { code, .. } => code == "cirrussearch-too-busy-error",
        _ => false,
    };
    RetryPolicy::new(RETRIES).with_jitter().run_if("Wikidata API", transient, op).await
}

/// `wbeditentity` data for a new item.
fn item_json(item: NewItem) -> Value {
    let sitelinks = vec![SiteLink::new(SITE.to_string(), item.page, vec![])];
    let entity =
        ItemEntity::new(String::new(), vec![item.label], vec![], item.aliases, item.statements, Some(sitelinks), false);
    let mut json = entity.to_json();
    // Not for new items.
    if let Some(object) = json.as_object_mut() {
        object.remove("type");
        object.remove("id");
    }
    json
}

async fn edit(api: &mut Api, params: &[(&str, &str)], token: String, summary: &str) -> Result<Value> {
    let mut params: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    for (key, value) in [("token", token.as_str()), ("bot", "1"), ("summary", summary)] {
        params.insert(key.to_string(), value.to_string());
    }
    let json = api.post_query_api_json_mut(&params).await?;
    if let Some(error) = json.get("error") {
        bail!("{}: {}", error["code"].as_str().unwrap_or("error"), error["info"].as_str().unwrap_or_default());
    }
    Ok(json)
}

/// User and (bot) password from `key=value` lines (`user`/`username`,
/// `pass`/`password`, any case), ignoring stray direction marks.
fn parse_credentials(text: &str) -> Option<(String, String)> {
    let (mut user, mut password) = (None, None);
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim_matches(|c: char| c.is_whitespace() || matches!(c, '\u{200e}' | '\u{200f}' | '"'));
        match key.trim().to_lowercase().as_str() {
            "user" | "username" => user = Some(value.to_string()),
            "pass" | "password" => password = Some(value.to_string()),
            _ => {}
        }
    }
    Some((user.filter(|u| !u.is_empty())?, password.filter(|p| !p.is_empty())?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wikimisc::entity_id::PropertyId;
    use wikimisc::statements::{item_snak, statement};

    #[test]
    fn credentials() {
        let text = "[bot]\nUser=Bot@species\u{200e}\npassword = secret \n";
        assert_eq!(parse_credentials(text), Some(("Bot@species".to_string(), "secret".to_string())));
        assert_eq!(parse_credentials("user=x\n"), None);
    }

    #[test]
    fn new_item_data() {
        let item = NewItem {
            label: LocaleString::new("mul", "Herpa"),
            aliases: vec![],
            statements: vec![statement(item_snak(PropertyId::new(31), ItemId::new(16521)), vec![])],
            page: "Herpa (Walker)".to_string(),
        };
        let json = item_json(item);
        assert_eq!(json["labels"]["mul"]["value"], "Herpa");
        assert_eq!(json["sitelinks"][SITE]["title"], "Herpa (Walker)");
        assert_eq!(json["claims"]["P31"][0]["mainsnak"]["datavalue"]["value"]["id"], "Q16521");
        assert!(json.get("type").is_none() && json.get("id").is_none());
    }
}
