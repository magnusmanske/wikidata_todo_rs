//! Duplicity's data (`duplicity_p`): articles without a Wikidata item
//! (`no_wd`, refreshed daily per wiki), and items that may be about the same
//! topic (`candidates`, for the Distributed Game).

use super::Writes;
use crate::mediawiki;
use crate::replicas::{linktarget_ids, select_in};
use crate::state::AppState;
use crate::terms::{self, TermType};
use crate::tool_db::Database;
use crate::wiki::Wiki;
use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use std::collections::{BTreeSet, HashMap, HashSet};
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// Articles younger than this may still get an item from their author.
const MIN_AGE_DAYS: i64 = 3;
/// Wikis that are not about topics, or have no items of their own.
const SKIP: &[&str] = &["wikidatawiki", "metawiki", "nostalgiawiki"];
/// "Template:Merge": articles proposed for merging are not candidates.
const MERGE_TEMPLATE_ITEM: &str = "Q6919004";
/// Rows per INSERT or DELETE.
const WRITE_CHUNK: usize = 1000;
/// Articles per round.
const CANDIDATE_BATCH: usize = 500;
/// An article whose title matches more items than this is too ambiguous to ask about.
const MAX_CANDIDATES: usize = 10;

/// Refreshes `no_wd` for `only` (or for all wikis), one wiki at a time.
pub async fn update(state: &AppState, only: &[String], writes: Writes) -> Result<()> {
    let wikis = match only.is_empty() {
        true => wiki_list(&state.http).await?,
        false => only.to_vec(),
    };
    let mut failed = vec![];
    for dbname in &wikis {
        let result = match Wiki::from_dbname(dbname) {
            Ok(wiki) => update_wiki(state, &wiki, writes).await,
            Err(e) => Err(e.into()),
        };
        match result {
            Ok(count) => println!("Done\t{dbname}\t{count}"),
            Err(e) => {
                eprintln!("{dbname}: {e:#}");
                failed.push(dbname.as_str());
            }
        }
    }
    // A few odd wikis always fail; only a broken run should alarm.
    if failed.len() * 2 > wikis.len() {
        bail!("{} of {} wikis failed", failed.len(), wikis.len());
    }
    Ok(())
}

/// The wikis to look at: Wikidata's local interwiki prefixes (the
/// Wikipedias, Commons, Species, …) that are wiki databases.
async fn wiki_list(http: &reqwest::Client) -> Result<Vec<String>> {
    let wikidata = Wiki::wikidata();
    let params = [("action", "query"), ("meta", "siteinfo"), ("siprop", "interwikimap"), ("sifilteriw", "local")];
    let json = mediawiki::api(http, &wikidata, &params).await?;
    let prefixes: HashSet<String> = json["query"]["interwikimap"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|iw| iw["prefix"].as_str())
        .map(|prefix| format!("{}wiki", prefix.replace('-', "_")))
        .collect();
    let json = mediawiki::api(http, &wikidata, &[("action", "sitematrix"), ("smstate", "all")]).await?;
    let mut dbnames = BTreeSet::new();
    collect_dbnames(&json["sitematrix"], &mut dbnames);
    let wikis: Vec<String> =
        dbnames.into_iter().filter(|db| prefixes.contains(db) && !SKIP.contains(&db.as_str())).collect();
    // A failed download must not empty the run.
    if wikis.len() < 300 {
        bail!("the wiki list looks incomplete ({} wikis)", wikis.len());
    }
    Ok(wikis)
}

fn collect_dbnames(json: &serde_json::Value, out: &mut BTreeSet<String>) {
    match json {
        serde_json::Value::Object(map) => {
            if let Some(db) = map.get("dbname").and_then(|v| v.as_str()) {
                out.insert(db.to_string());
            }
            map.values().for_each(|v| collect_dbnames(v, out));
        }
        serde_json::Value::Array(list) => list.iter().for_each(|v| collect_dbnames(v, out)),
        _ => {}
    }
}

/// Brings `no_wd` for one wiki up to date; returns how many articles it lists.
async fn update_wiki(state: &AppState, wiki: &Wiki, writes: Writes) -> Result<usize> {
    let mut tool = state.tool_db.conn(Database::Duplicity).await?;
    let current = articles_without_item(state, wiki, &mut tool).await?;
    let existing: Vec<(u64, Vec<u8>)> = tool.exec("SELECT id, title FROM no_wd WHERE wiki=?", (wiki.dbname(),)).await?;
    let existing: HashMap<String, u64> =
        existing.into_iter().map(|(id, title)| (String::from_utf8_lossy(&title).into_owned(), id)).collect();

    let new: Vec<&(String, String)> = current.iter().filter(|(title, _)| !existing.contains_key(title)).collect();
    for chunk in new.chunks(WRITE_CHUNK) {
        let rows = vec!["(?,?,?,0,RAND())"; chunk.len()].join(",");
        let sql = format!("INSERT IGNORE INTO no_wd (wiki,title,creation_date,tmp_flag,random) VALUES {rows}");
        let params: Vec<String> = chunk
            .iter()
            .flat_map(|(title, created)| [wiki.dbname().to_string(), title.clone(), created.clone()])
            .collect();
        writes.exec(&mut tool, &sql, params).await?;
    }

    let current_titles: HashSet<&str> = current.iter().map(|(title, _)| title.as_str()).collect();
    let gone: Vec<u64> =
        existing.iter().filter(|(title, _)| !current_titles.contains(title.as_str())).map(|(_, id)| *id).collect();
    forget(&mut tool, &gone, writes).await?;

    let today = Utc::now().format("%Y%m%d").to_string();
    let sql = "INSERT IGNORE INTO stats (wiki,date,count) VALUES (?,?,?)";
    writes.exec(&mut tool, sql, (wiki.dbname(), today, current.len())).await?;

    drop_linked_candidates(state, wiki, &mut tool, writes).await?;
    Ok(current.len())
}

/// Removes `no_wd` rows, with their candidates.
pub(super) async fn forget(tool: &mut Conn, ids: &[u64], writes: Writes) -> Result<()> {
    for chunk in ids.chunks(WRITE_CHUNK) {
        let list = placeholders(chunk.len());
        writes.exec(tool, &format!("DELETE FROM candidates WHERE no_wd_id IN ({list})"), chunk.to_vec()).await?;
        writes.exec(tool, &format!("DELETE FROM no_wd WHERE id IN ({list})"), chunk.to_vec()).await?;
    }
    Ok(())
}

fn placeholders(n: usize) -> String {
    crate::replicas::placeholders(n)
}

/// `(title with spaces, creation timestamp)` of the wiki's articles that
/// have no item, are older than a few days, and are not marked for deletion
/// (the wiki's `bad_cats`) or merging.
async fn articles_without_item(state: &AppState, wiki: &Wiki, tool: &mut Conn) -> Result<Vec<(String, String)>> {
    let mut conn = state.replicas.conn(wiki, &["page", "page_props", "revision"]).await?;
    let sql = "SELECT page_id, page_title FROM page WHERE page_namespace=0 AND page_is_redirect=0 AND page_len>0
        AND NOT EXISTS (SELECT 1 FROM page_props WHERE pp_page=page_id AND pp_propname='wikibase_item')";
    let pages: Vec<(u64, Vec<u8>)> = conn.query(sql).await.context("cannot list articles without item")?;
    if pages.is_empty() {
        return Ok(vec![]);
    }
    let excluded = excluded_pages(state, wiki, tool).await?;
    let pages: HashMap<u64, Vec<u8>> = pages.into_iter().filter(|(id, _)| !excluded.contains(id)).collect();

    // The first revision gives the creation time.
    let ids: Vec<u64> = pages.keys().copied().collect();
    let sql = "SELECT rev_page, MIN(rev_timestamp) FROM revision WHERE rev_page IN";
    let created: Vec<(u64, Vec<u8>)> = select_in(&mut conn, sql, "GROUP BY rev_page", &[], &ids).await?;
    let cutoff = (Utc::now() - Duration::days(MIN_AGE_DAYS)).format("%Y%m%d%H%M%S").to_string();
    Ok(created
        .into_iter()
        .filter_map(|(id, ts)| {
            let ts = String::from_utf8_lossy(&ts).into_owned();
            let title = pages.get(&id)?;
            (ts < cutoff).then(|| (String::from_utf8_lossy(title).replace('_', " "), ts))
        })
        .collect())
}

/// Pages in the wiki's `bad_cats`, or using its "Merge" template.
async fn excluded_pages(state: &AppState, wiki: &Wiki, tool: &mut Conn) -> Result<HashSet<u64>> {
    let mut excluded = HashSet::new();
    let categories: Vec<String> = tool.exec("SELECT cat FROM bad_cats WHERE wiki=?", (wiki.dbname(),)).await?;
    let categories: Vec<String> = categories.iter().map(|c| crate::replicas::db_title(c)).collect();
    if !categories.is_empty() {
        let mut conn = state.replicas.conn(wiki, &["categorylinks", "linktarget"]).await?;
        let titles: Vec<&str> = categories.iter().map(String::as_str).collect();
        let targets = linktarget_ids(&mut conn, 14, &titles).await?;
        let sql = "SELECT cl_from FROM categorylinks WHERE cl_target_id IN";
        excluded.extend(select_in::<u64, _>(&mut conn, sql, "", &[], &targets).await?);
    }
    if let Some(template) = merge_template(state, wiki).await? {
        let mut conn = state.replicas.conn(wiki, &["templatelinks", "linktarget"]).await?;
        let targets = linktarget_ids(&mut conn, 10, &[template.as_str()]).await?;
        let sql = "SELECT tl_from FROM templatelinks WHERE tl_target_id IN";
        excluded.extend(select_in::<u64, _>(&mut conn, sql, "", &[], &targets).await?);
    }
    Ok(excluded)
}

/// The wiki's "Merge" template, as a database title without namespace.
async fn merge_template(state: &AppState, wiki: &Wiki) -> Result<Option<String>> {
    let Some(entity) = crate::wikidata::entity(&state.http, MERGE_TEMPLATE_ITEM, "sitelinks").await? else {
        return Ok(None);
    };
    let title = entity["sitelinks"][wiki.dbname()]["title"].as_str();
    Ok(title.and_then(|t| t.split_once(':')).map(|(_, name)| crate::replicas::db_title(name)))
}

/// Unchecked candidates whose item meanwhile links to an article on the wiki are moot.
async fn drop_linked_candidates(state: &AppState, wiki: &Wiki, tool: &mut Conn, writes: Writes) -> Result<()> {
    let sql = "SELECT DISTINCT q FROM candidates JOIN no_wd ON no_wd.id=no_wd_id WHERE wiki=? AND checked=0";
    let items: Vec<u64> = tool.exec(sql, (wiki.dbname(),)).await?;
    let linked = items_with_sitelink(state, wiki.dbname(), &items).await?;
    for chunk in linked.chunks(WRITE_CHUNK) {
        let sql = format!(
            "DELETE candidates FROM candidates JOIN no_wd ON no_wd.id=no_wd_id
             WHERE wiki=? AND checked=0 AND q IN ({})",
            placeholders(chunk.len())
        );
        let mut params = vec![wikimisc::mysql_async::Value::from(wiki.dbname())];
        params.extend(chunk.iter().map(|q| (*q).into()));
        writes.exec(tool, &sql, params).await?;
    }
    Ok(())
}

/// Those of `items` (numeric IDs) with a sitelink to `site`.
async fn items_with_sitelink(state: &AppState, site: &str, items: &[u64]) -> Result<Vec<u64>> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["wb_items_per_site"]).await?;
    let sql = "SELECT DISTINCT ips_item_id FROM wb_items_per_site WHERE ips_site_id=? AND ips_item_id IN";
    select_in(&mut conn, sql, "", &[site.into()], items).await
}

/// Finds candidate items for random `no_wd` articles: items with the
/// article's title (without a bracketed suffix) as label or alias, that do
/// not link to that wiki yet.
pub async fn candidates(state: &AppState, rounds: usize, writes: Writes) -> Result<()> {
    let mut tool = state.tool_db.conn(Database::Duplicity).await?;
    let mut added = 0;
    for _ in 0..rounds {
        added += candidate_round(state, &mut tool, writes).await?;
    }
    // Too many candidates for one article: too ambiguous for the game.
    let sql = "SELECT no_wd_id FROM candidates WHERE checked=0 GROUP BY no_wd_id HAVING COUNT(*)>?";
    let ambiguous: Vec<u64> = tool.exec(sql, (MAX_CANDIDATES,)).await?;
    for chunk in ambiguous.chunks(WRITE_CHUNK) {
        let sql = format!("DELETE FROM candidates WHERE checked=0 AND no_wd_id IN ({})", placeholders(chunk.len()));
        writes.exec(&mut tool, &sql, chunk.to_vec()).await?;
    }
    println!("{added} candidates added; {} ambiguous articles dropped", ambiguous.len());
    Ok(())
}

async fn candidate_round(state: &AppState, tool: &mut Conn, writes: Writes) -> Result<usize> {
    let sql = format!("SELECT id, wiki, title FROM no_wd WHERE random>? ORDER BY random LIMIT {CANDIDATE_BATCH}");
    let rows: Vec<(u64, Vec<u8>, Vec<u8>)> = tool.exec(sql, (rand::random::<f64>(),)).await?;
    let articles: Vec<(u64, String, String)> = rows
        .into_iter()
        .map(|(id, wiki, title)| {
            let title = String::from_utf8_lossy(&title).into_owned();
            (id, String::from_utf8_lossy(&wiki).into_owned(), label_of(&title).to_string())
        })
        .collect();
    let texts: Vec<String> = articles.iter().map(|(.., label)| label.clone()).collect();
    let mut by_text: HashMap<String, Vec<u64>> = HashMap::new();
    for (item, text) in
        terms::items_with_texts(&state.replicas, &texts, &[TermType::Label, TermType::Alias], None).await?
    {
        if let Ok(q) = item[1..].parse() {
            by_text.entry(text).or_default().push(q);
        }
    }

    // Per wiki, the matching items that already link there.
    let mut wiki_items: HashMap<&str, Vec<u64>> = HashMap::new();
    for (_, wiki, label) in &articles {
        wiki_items.entry(wiki).or_default().extend(by_text.get(label).into_iter().flatten());
    }
    let mut linked: HashSet<(&str, u64)> = HashSet::new();
    for (wiki, items) in wiki_items {
        linked.extend(items_with_sitelink(state, wiki, &items).await?.into_iter().map(|q| (wiki, q)));
    }

    let pairs: Vec<(u64, u64)> = articles
        .iter()
        .flat_map(|(id, wiki, label)| {
            let mut items: Vec<u64> = by_text.get(label).cloned().unwrap_or_default();
            items.sort_unstable();
            items.dedup();
            items.retain(|q| !linked.contains(&(wiki.as_str(), *q)));
            let items = if items.len() > MAX_CANDIDATES { vec![] } else { items };
            items.into_iter().map(|q| (*id, q))
        })
        .collect();
    insert_new_candidates(tool, &pairs, writes).await
}

/// Inserts those `(no_wd_id, q)` pairs that are not there yet. (Not with
/// INSERT IGNORE: that uses up an ID for every pair already there.)
async fn insert_new_candidates(tool: &mut Conn, pairs: &[(u64, u64)], writes: Writes) -> Result<usize> {
    let ids: Vec<u64> = pairs.iter().map(|(id, _)| *id).collect::<BTreeSet<_>>().into_iter().collect();
    let sql = "SELECT no_wd_id, q FROM candidates WHERE no_wd_id IN";
    let existing: HashSet<(u64, u64)> = select_in(tool, sql, "", &[], &ids).await?.into_iter().collect();
    let new: Vec<&(u64, u64)> = pairs.iter().filter(|p| !existing.contains(p)).collect();
    for chunk in new.chunks(WRITE_CHUNK) {
        let sql = format!("INSERT INTO candidates (no_wd_id,q) VALUES {}", vec!["(?,?)"; chunk.len()].join(","));
        let params: Vec<u64> = chunk.iter().flat_map(|(id, q)| [*id, *q]).collect();
        writes.exec(tool, &sql, params).await?;
    }
    Ok(new.len())
}

/// The likely label of an article: its title without a bracketed suffix.
fn label_of(title: &str) -> &str {
    match title.rfind(" (") {
        Some(pos) if title.ends_with(')') => &title[..pos],
        _ => title,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_of_titles() {
        assert_eq!(label_of("Paris (Texas)"), "Paris");
        assert_eq!(label_of("Paris"), "Paris");
        assert_eq!(label_of("A (b) c"), "A (b) c");
    }

    #[test]
    fn dbnames_from_the_sitematrix() {
        let json = serde_json::json!({"count": 2, "0": {"site": [{"dbname": "enwiki"}, {"dbname": "enwiktionary"}]},
            "specials": [{"dbname": "commonswiki"}]});
        let mut out = BTreeSet::new();
        collect_dbnames(&json, &mut out);
        assert_eq!(out.into_iter().collect::<Vec<_>>(), ["commonswiki", "enwiki", "enwiktionary"]);
    }
}
