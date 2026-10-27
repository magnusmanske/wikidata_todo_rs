//! The Wikispecies bot: gives Wikispecies pages without an item one on
//! Wikidata, by linking a matching item, or by creating one. How it reads
//! pages: [`page`] (taxa, publications, …, from the HTML) and [`wikitext`]
//! (people, serials).

mod page;
mod people;
mod ranks;
mod wikidata;
mod wikitext;

use super::Writes;
use crate::state::AppState;
use crate::tool_db::Database;
use crate::wiki::Wiki;
use anyhow::{Context, Result};
use page::{PageKind, Taxon};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;
use wikidata::{NewItem, SITE, Wikidata};
use wikimisc::date::Date;
use wikimisc::entity_id::{ItemId, PropertyId};
use wikimisc::http::HttpRetryPolicy;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::statements::{external_id_snak, imported_from_revision, item_snak, statement, string_snak, time_snak};
use wikimisc::wikibase::{LocaleString, Snak};
use wikimisc::wikidata::CirrusQuery;
use wikitext::{Person, WikitextKind};

const WIKISPECIES: ItemId = ItemId::new(13679);
const HOST: &str = "species.wikimedia.org";
const SUMMARY: &str = "Item for a Wikispecies page";
/// How many missing ancestors of a taxon get an item too.
const MAX_PARENTS: usize = 5;
/// For page requests that hit a rate limit or a server error.
const RETRIES: u32 = 5;
/// Pages whose items are looked up at once.
const BATCH: usize = 50;

const INSTANCE_OF: PropertyId = PropertyId::new(31);
const TAXON_NAME: PropertyId = PropertyId::new(225);
const TAXON_RANK: PropertyId = PropertyId::new(105);
const PARENT_TAXON: PropertyId = PropertyId::new(171);
const IPNI_PUBLICATION: PropertyId = PropertyId::new(2008);
const BPH: PropertyId = PropertyId::new(4569);
const ISSN: PropertyId = PropertyId::new(236);
const AUTHOR_ABBREVIATION: PropertyId = PropertyId::new(428);
const OCCUPATION: PropertyId = PropertyId::new(106);
const BORN: PropertyId = PropertyId::new(569);
const DIED: PropertyId = PropertyId::new(570);
const FLORUIT: PropertyId = PropertyId::new(1317);

const TAXON: ItemId = ItemId::new(16521);
const EXTINCT_TAXON: ItemId = ItemId::new(98961713);
const HUMAN: ItemId = ItemId::new(5);
const SCHOLARLY_WORK: ItemId = ItemId::new(47461344);
const SCIENTIFIC_JOURNAL: ItemId = ItemId::new(5633421);
const DISAMBIGUATION: ItemId = ItemId::new(4167410);

/// `A. Smith`: too likely the same as some `Anna Smith` to create.
static INITIAL_FIRST: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^[A-Z]\.").ok());
static MIDDLE_INITIAL: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r" [A-Z]\. ").ok());

/// Goes through `titles`, or, without any, all Wikispecies pages that Duplicity lists.
pub async fn run(state: &AppState, titles: &[String], writes: Writes) -> Result<()> {
    let credentials = (!writes.dry_run).then_some(state.config.bot_credentials.as_path());
    let mut bot = Bot { state, wikidata: Wikidata::new(state.http.clone(), credentials).await?, writes };
    let titles = match titles.is_empty() {
        true => bot.candidates().await?,
        false => titles.to_vec(),
    };
    for chunk in titles.chunks(BATCH) {
        let pages: Vec<&str> = chunk.iter().map(String::as_str).collect();
        let linked = match bot.wikidata.items_for_pages(&pages).await {
            Ok(linked) => linked,
            Err(e) => {
                eprintln!("{e:#}");
                continue;
            }
        };
        for title in chunk {
            let result = match linked.contains_key(title) {
                true => bot.forget(title).await,
                false => bot.process(title, 0).await.map(|_| ()),
            };
            if let Err(e) = result {
                eprintln!("{title}: {e:#}");
            }
        }
    }
    Ok(())
}

struct Bot<'a> {
    state: &'a AppState,
    wikidata: Wikidata,
    writes: Writes,
}

/// A Wikispecies page, as the bot reads it.
struct Page {
    /// After following a redirect.
    title: String,
    revision: u64,
    html: String,
    wikitext: String,
}

impl Bot<'_> {
    /// A connection for each use: a run takes hours, and idle connections get closed.
    async fn db(&self) -> Result<Conn> {
        self.state.tool_db.conn(Database::Duplicity).await
    }

    async fn candidates(&mut self) -> Result<Vec<String>> {
        let rows: Vec<Vec<u8>> =
            self.db().await?.exec("SELECT title FROM no_wd WHERE wiki=? ORDER BY RAND()", (SITE,)).await?;
        Ok(rows.into_iter().map(|t| String::from_utf8_lossy(&t).into_owned()).collect())
    }

    /// Gives the page an item, matched or new; returns it, if it has one now.
    async fn process(&mut self, title: &str, depth: usize) -> Result<Option<ItemId>> {
        let Some(page) = fetch(&self.state.http, title).await? else { return Ok(None) };
        let text_kind = wikitext::classify(&page.title, &page.wikitext);
        if text_kind == WikitextKind::Disputed {
            return Ok(None);
        }
        match page::classify(&page.title, &page.html) {
            PageKind::WrittenWork { title, ipni, abbreviation } => {
                let aliases = abbreviation.map(|a| LocaleString::new("en".to_string(), a)).into_iter().collect();
                let mut snaks = vec![item_snak(INSTANCE_OF, SCHOLARLY_WORK)];
                let label = LocaleString::new("en".to_string(), title);
                let Some(ipni) = ipni else { return self.create(&page, label, aliases, snaks).await };
                snaks.push(external_id_snak(IPNI_PUBLICATION, ipni.as_str()));
                let query = CirrusQuery::new("").has_statement(IPNI_PUBLICATION, &ipni);
                self.match_or_create(&page, query, label, aliases, snaks).await
            }
            PageKind::InvalidName => {
                self.forget(&page.title).await?;
                Ok(None)
            }
            PageKind::Source => {
                let label = LocaleString::new("en", &page.title);
                self.create(&page, label, vec![], vec![item_snak(INSTANCE_OF, SCHOLARLY_WORK)]).await
            }
            PageKind::Journal { bph } => {
                let query = CirrusQuery::new("").has_statement(BPH, &bph);
                let snaks = vec![item_snak(INSTANCE_OF, SCIENTIFIC_JOURNAL), external_id_snak(BPH, bph)];
                self.match_or_create(&page, query, LocaleString::new("en", &page.title), vec![], snaks).await
            }
            PageKind::Disambiguation => {
                let query = CirrusQuery::new(&format!("\"{}\"", page.title)).has_statement(INSTANCE_OF, DISAMBIGUATION);
                let snaks = vec![item_snak(INSTANCE_OF, DISAMBIGUATION)];
                self.match_or_create(&page, query, LocaleString::new("mul", &page.title), vec![], snaks).await
            }
            PageKind::Taxon(taxon) => self.taxon(&page, taxon, depth).await,
            PageKind::UnknownRank(rank) => {
                println!("{}: unknown rank {rank}", page.title);
                self.other(&page, text_kind).await
            }
            PageKind::Other => self.other(&page, text_kind).await,
        }
    }

    /// Links the one item the search finds, or, if none, creates one.
    async fn match_or_create(
        &mut self,
        page: &Page,
        query: CirrusQuery,
        label: LocaleString,
        aliases: Vec<LocaleString>,
        snaks: Vec<Snak>,
    ) -> Result<Option<ItemId>> {
        match self.wikidata.search(&query.to_string()).await?.as_slice() {
            [] => self.create(page, label, aliases, snaks).await,
            [item] => self.link(*item, page).await,
            _ => skipped(page, "several items match"),
        }
    }

    async fn taxon(&mut self, page: &Page, taxon: Taxon, depth: usize) -> Result<Option<ItemId>> {
        let by_name = CirrusQuery::new("").has_statement(TAXON_NAME, &taxon.name);
        let found = self.wikidata.search(&by_name.to_string()).await?;
        // A title other than the name (`Cuspidaria (Plantae)`, a subgenus)
        // means homonyms: then the item must have the same parent.
        let homonym = page.title != taxon.name;
        if let [item] = found.as_slice()
            && !homonym
        {
            return self.link(*item, page).await;
        }
        let parent = match &taxon.parent {
            Some(parent) => self.parent(parent, depth).await?,
            None => None,
        };
        let in_parent = match parent {
            Some(parent) if !found.is_empty() => {
                self.wikidata.search(&by_name.has_statement(PARENT_TAXON, parent).to_string()).await?
            }
            _ => vec![],
        };
        match (in_parent.as_slice(), found.as_slice()) {
            ([item], _) => self.link(*item, page).await,
            (_, []) => {
                let class = if taxon.extinct { EXTINCT_TAXON } else { TAXON };
                let mut snaks = vec![
                    item_snak(INSTANCE_OF, class),
                    string_snak(TAXON_NAME, taxon.name.as_str()),
                    item_snak(TAXON_RANK, taxon.rank_item.parse()?),
                ];
                snaks.extend(parent.map(|parent| item_snak(PARENT_TAXON, parent)));
                self.create(page, LocaleString::new("mul", &taxon.name), vec![], snaks).await
            }
            _ => skipped(page, "several items have that taxon name"),
        }
    }

    /// The item of a parent taxon's page; made, if it has none.
    async fn parent(&mut self, title: &str, depth: usize) -> Result<Option<ItemId>> {
        if let Some(item) = self.wikidata.item_for_page(title).await? {
            return Ok(Some(item));
        }
        if depth >= MAX_PARENTS {
            return Ok(None);
        }
        Box::pin(self.process(title, depth + 1)).await
    }

    /// Pages that are not taxa, by what their wikitext says.
    async fn other(&mut self, page: &Page, kind: WikitextKind) -> Result<Option<ItemId>> {
        // Some taxon pages have no Taxonavigation; their title is the name.
        let query = CirrusQuery::new("").has_statement(TAXON_NAME, &page.title);
        if let [item] = self.wikidata.search(&query.to_string()).await?.as_slice() {
            return self.link(*item, page).await;
        }
        match kind {
            WikitextKind::Issn(issn) => {
                let query = CirrusQuery::new("").has_statement(ISSN, issn);
                match self.wikidata.search(&query.to_string()).await?.as_slice() {
                    [item] => self.link(*item, page).await,
                    _ => Ok(None),
                }
            }
            WikitextKind::Person(person) => self.person(page, person).await,
            WikitextKind::Disputed | WikitextKind::Other => Ok(None),
        }
    }

    async fn person(&mut self, page: &Page, person: Person) -> Result<Option<ItemId>> {
        // Without a disambiguation: `Yu-Fan Wang (botanist)`.
        let label = page.title.split(" (").next().unwrap_or_default();
        let initial_first = INITIAL_FIRST.as_ref().is_some_and(|r| r.is_match(label));
        if initial_first || label.contains('/') || !label.contains(' ') {
            return skipped(page, "not a full name");
        }
        // Their abbreviation as botanical author identifies them.
        if let Some(ipni) = &person.ipni {
            let query = CirrusQuery::new("").has_statement(AUTHOR_ABBREVIATION, ipni);
            match self.wikidata.search(&query.to_string()).await?.as_slice() {
                [] => {}
                [item] => return self.link(*item, page).await,
                _ => return skipped(page, "several people have that author abbreviation"),
            }
        }
        let name = people::Name::new(label, person.sort_key.as_deref());
        // The one author of their publications with their name.
        let authors = self.wikidata.authors(&people::dois(&page.html)).await?;
        let authors = self.wikidata.items(&authors, &["labels", "aliases", "claims"]).await?;
        match ids(authors.iter().filter(|item| people::is(item, &name, &person))).as_slice() {
            [] => {}
            [item] => return self.link(*item, page).await,
            _ => return skipped(page, "several authors of their publications may be them"),
        }
        // Everyone else the name may belong to, but for those whose dates rule them out.
        let mut found = vec![];
        for text in name_searches(label) {
            let query = CirrusQuery::new(&text).has_statement(INSTANCE_OF, HUMAN);
            let Some(items) = self.wikidata.search_all(&query.to_string()).await? else {
                return skipped(page, "too many people have such a name");
            };
            found.extend(items);
        }
        found.sort();
        found.dedup();
        let found = match found.is_empty() {
            true => vec![],
            false => self.wikidata.items(&found, &["labels", "aliases", "claims"]).await?,
        };
        let candidates: Vec<&Value> = found.iter().filter(|item| people::may_be(item, &name, &person)).collect();
        if candidates.is_empty() {
            let snaks = person_snaks(&person)?;
            return self.create(page, LocaleString::new("mul", label), vec![], snaks).await;
        }
        // The one born and died in the same years.
        let same_years = |item: &&Value| {
            let has = |property, year: Option<i64>| year.is_some_and(|y| people::years(item, property).contains(&y));
            has("P569", person.born) && has("P570", person.died)
        };
        match ids(candidates.into_iter().filter(same_years)).as_slice() {
            [item] => self.link(*item, page).await,
            _ => skipped(page, "someone may have that name already"),
        }
    }

    /// Creates an item for the page, with the statements `snaks` make,
    /// referenced to the page's revision.
    async fn create(
        &mut self,
        page: &Page,
        label: LocaleString,
        aliases: Vec<LocaleString>,
        snaks: Vec<Snak>,
    ) -> Result<Option<ItemId>> {
        let reference = imported_from_revision(Some(WIKISPECIES), HOST, &page.title, page.revision);
        let statements = snaks.into_iter().map(|snak| statement(snak, vec![reference.clone()])).collect();
        let item = NewItem { label, aliases, statements, page: page.title.clone() };
        let item = match self.wikidata.create(item, SUMMARY).await {
            Ok(Some(item)) => item,
            Ok(None) => return Ok(None),
            // Perhaps someone else gave the page an item meanwhile.
            Err(e) => self.wikidata.item_for_page(&page.title).await?.ok_or(e)?,
        };
        println!("{}: created {item}", page.title);
        self.forget(&page.title).await?;
        Ok(Some(item))
    }

    /// Links the item to the page, unless it has a Wikispecies page already,
    /// or had this bot's link once (and someone removed it).
    async fn link(&mut self, item: ItemId, page: &Page) -> Result<Option<ItemId>> {
        match self.wikidata.page_of(item).await? {
            Some(linked) if linked == page.title => {
                self.forget(&page.title).await?;
                return Ok(Some(item));
            }
            Some(linked) => return skipped(page, &format!("{item} has {linked}")),
            None => {}
        }
        let added: Option<u8> =
            self.db().await?.exec_first("SELECT 1 FROM added_wikispecies WHERE q=?", (item.get(),)).await?;
        if added.is_some() {
            return skipped(page, &format!("{item} had a link before"));
        }
        self.wikidata.link(item, &page.title, SUMMARY).await?;
        println!("{}: linked {item}", page.title);
        self.writes
            .exec(&mut self.db().await?, "INSERT IGNORE INTO added_wikispecies (q) VALUES (?)", (item.get(),))
            .await?;
        self.forget(&page.title).await?;
        Ok(Some(item))
    }

    /// Takes the page off Duplicity's list.
    async fn forget(&mut self, title: &str) -> Result<()> {
        let mut db = self.db().await?;
        let ids: Vec<u64> = db.exec("SELECT id FROM no_wd WHERE wiki=? AND title=?", (SITE, title)).await?;
        super::duplicity::forget(&mut db, &ids, self.writes).await
    }
}

/// Searches for a name: without middle initials, and, if it has hyphens,
/// also written together (`Xin-Yue Shen`, `Xinyue Shen`).
fn name_searches(name: &str) -> Vec<String> {
    let name = MIDDLE_INITIAL.as_ref().map_or(name.into(), |r| r.replace_all(name, " ")).into_owned();
    let together = name.replace('-', "");
    if together == name { vec![name] } else { vec![name, together] }
}

/// The IDs of the items.
fn ids<'a>(items: impl Iterator<Item = &'a Value>) -> Vec<ItemId> {
    items.filter_map(|item| item["id"].as_str()?.parse().ok()).collect()
}

fn skipped(page: &Page, reason: &str) -> Result<Option<ItemId>> {
    println!("{}: skipped, {reason}", page.title);
    Ok(None)
}

/// The page's HTML and wikitext; `None` if there is no such page.
async fn fetch(http: &reqwest::Client, title: &str) -> Result<Option<Page>> {
    let wiki = Wiki::from_dbname(SITE)?;
    let params = [
        ("action", "parse"),
        ("page", title),
        ("prop", "text|wikitext|revid"),
        ("redirects", "1"),
        ("format", "json"),
        ("formatversion", "2"),
    ];
    // Parsing is costly, and the wiki limits it: wait as long as it asks.
    let request = http.get(wiki.api_url()).query(&params);
    let json: Value = HttpRetryPolicy::new(RETRIES).with_jitter().fetch_json(request).await?;
    if json["error"]["code"] == "missingtitle" {
        return Ok(None);
    }
    let parse = &json["parse"];
    let text = |key: &str| parse[key].as_str().map(str::to_string).with_context(|| format!("no {key} for {title}"));
    Ok(Some(Page {
        title: text("title")?,
        revision: parse["revid"].as_u64().context("no revision")?,
        html: text("text")?,
        wikitext: text("wikitext")?,
    }))
}

fn person_snaks(person: &Person) -> Result<Vec<Snak>> {
    let mut snaks = vec![item_snak(INSTANCE_OF, HUMAN)];
    for occupation in &person.occupations {
        snaks.push(item_snak(OCCUPATION, occupation.parse()?));
    }
    if let Some(ipni) = &person.ipni {
        snaks.push(string_snak(AUTHOR_ABBREVIATION, ipni.as_str()));
    }
    for (property, year) in [(BORN, person.born), (DIED, person.died), (FLORUIT, person.floruit)] {
        if let Some(year) = year {
            snaks.push(time_snak(property, &Date::new(year, None, None)?));
        }
    }
    Ok(snaks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_for_names() {
        assert_eq!(name_searches("Xin-Yue Shen"), ["Xin-Yue Shen", "XinYue Shen"]);
        assert_eq!(name_searches("Laura A. Frost"), ["Laura Frost"]);
    }

    #[test]
    fn statements_for_people() {
        let person = Person {
            ipni: Some("L.F.Han".to_string()),
            born: Some(1965),
            occupations: vec!["Q15924544"],
            ..Default::default()
        };
        let snaks = person_snaks(&person).unwrap();
        let properties: Vec<&str> = snaks.iter().map(|s| s.property()).collect();
        assert_eq!(properties, ["P31", "P106", "P428", "P569"]);
    }
}
