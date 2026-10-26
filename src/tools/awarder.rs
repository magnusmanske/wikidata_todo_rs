//! `awarder.php`: "award received" (P166) statements for QuickStatements,
//! two ways. For a person's item: the awards their articles link to, to pick
//! from. For an award: its recipients, from a wikitext list of years and
//! linked names.

use crate::cache::TimedCache;
use crate::replicas::{db_title, display_title, linktarget_ids, select_in};
use crate::sparql;
use crate::state::AppState;
use crate::terms;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use crate::wikidata::parse_id;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use futures::{StreamExt, stream};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;

/// "award"
const AWARD: &str = "Q618779";
const POSITION: &str = "Q4164871";
const DEGREE: &str = "Q189533";
const HUMAN: &str = "Q5";
/// "mul" holds the labels common to many languages.
const LANGUAGES: &[&str] = &["en", "mul", "de", "es", "fr", "it", "ru"];
const CONCURRENCY: usize = 4;
/// Linked recipients read from the wikitext, at most.
const MAX_RECIPIENTS: usize = 5000;

/// All awards: their item numbers. Fetching them takes a while.
static AWARDS: TimedCache<HashSet<u64>> = TimedCache::new(Duration::from_secs(24 * 3600));

/// The form fields, as PHP's `$_REQUEST`: `use_<award>` and `year_<award>` vary.
type Request = HashMap<String, String>;

struct Award {
    id: String,
    label: String,
    /// Articles about the person that link to it.
    count: usize,
    /// The person's item links to it already.
    in_item: bool,
}

struct Missing {
    title: String,
    year: u16,
    search: String,
}

enum Results {
    /// Awards to pick from for a person.
    Awards { person: String, awards: Vec<Award> },
    /// Statements, and the recipients without an item.
    Recipients { award: String, statements: String, missing: Vec<Missing> },
    /// Statements for the picked awards.
    Picked { statements: String },
}

#[derive(Template)]
#[template(path = "tools/awarder.html")]
struct Page {
    title: &'static str,
    q: String,
    wiki: String,
    q_award: String,
    wikitext: String,
    results: Option<Results>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let field = |name: &str| request.get(name).map(|v| v.trim().to_string()).unwrap_or_default();
    let item = |name: &str| -> Result<Option<String>, AppError> {
        match field(name) {
            v if v.is_empty() => Ok(None),
            v => parse_id(&v, 'Q').map(Some).ok_or_else(|| AppError::BadRequest(format!("Not an item: {v}"))),
        }
    };
    let (person, award) = (item("q")?, item("q_award")?);
    let mut page = Page {
        title: "Awarder",
        q: person.clone().unwrap_or_default(),
        wiki: field("wiki").to_lowercase(),
        q_award: award.clone().unwrap_or_default(),
        wikitext: field("wikitext"),
        results: None,
        error: None,
    };
    if request.contains_key("generate")
        && let Some(person) = &person
    {
        page.results = Some(Results::Picked { statements: picked_statements(person, &request) });
    } else if let Some(award) = award
        && !page.wiki.is_empty()
    {
        let wiki = Wiki::from_dbname(&page.wiki)?;
        let (statements, missing) = recipients(&state, &wiki, &award, &page.wikitext).await?;
        page.results = Some(Results::Recipients { award, statements, missing });
    } else if let Some(person) = person {
        let awards = awards_of(&state, &person).await?;
        match awards.is_empty() {
            true => page.error = Some(format!("Could not find any awards for {person}.")),
            false => page.results = Some(Results::Awards { person, awards }),
        }
    }
    render(&page)
}

/// A QuickStatements line; with the year, as a "point in time" qualifier.
fn statement(person: &str, award: &str, year: Option<u16>) -> String {
    match year {
        Some(year) => format!("{person}\tP166\t{award}\tP585\t+{year:04}-01-01T00:00:00Z/9"),
        None => format!("{person}\tP166\t{award}"),
    }
}

fn picked_statements(person: &str, request: &Request) -> String {
    let awards = request.get("aq").map(String::as_str).unwrap_or_default();
    awards
        .split(',')
        .filter_map(|a| parse_id(a, 'Q'))
        .filter(|a| request.contains_key(&format!("use_{}", number(a))))
        .map(|a| {
            let year = request.get(&format!("year_{}", number(&a))).and_then(|y| parse_year(y));
            statement(person, &a, year)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `42` for `Q42`.
fn number(id: &str) -> &str {
    id.trim_start_matches('Q')
}

fn parse_year(s: &str) -> Option<u16> {
    let s = s.trim();
    (s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

/// The awards that the person's articles link to, most linked first.
async fn awards_of(state: &AppState, person: &str) -> anyhow::Result<Vec<Award>> {
    let all = AWARDS.get_or_fetch(async || all_awards(&state.sparql).await).await?;
    let wikidata = Wiki::wikidata();
    let mut conn = state.replicas.conn(&wikidata, &["wb_items_per_site"]).await?;
    let sitelinks: Vec<(String, String)> = conn
        .exec("SELECT ips_site_id, ips_site_page FROM wb_items_per_site WHERE ips_item_id=?", (number(person),))
        .await?;
    let linked: Vec<Vec<u64>> = stream::iter(sitelinks)
        .map(|(site, page)| async move {
            linked_items(state, &site, &page).await.unwrap_or_else(|e| {
                tracing::warn!("awarder: cannot read the links of {page} on {site}: {e:#}");
                vec![]
            })
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for item in linked.into_iter().flatten().filter(|i| all.contains(i)) {
        *counts.entry(item).or_default() += 1;
    }
    let ids: Vec<String> = counts.keys().map(|n| format!("Q{n}")).collect();
    let not_awards = positions_and_degrees(&state.sparql, &ids).await?;
    let ids: Vec<String> = ids.into_iter().filter(|id| !not_awards.contains(id)).collect();
    let labels = terms::labels(&state.replicas, &ids, LANGUAGES).await?;
    let in_item = item_links(state, person).await?;
    let mut awards: Vec<Award> = ids
        .into_iter()
        .map(|id| Award {
            label: labels.get(&id).cloned().unwrap_or_else(|| id.clone()),
            count: number(&id).parse().ok().and_then(|n: u64| counts.get(&n).copied()).unwrap_or_default(),
            in_item: in_item.contains(&id),
            id,
        })
        .collect();
    awards.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
    Ok(awards)
}

/// Those of `ids` that are positions ("professor", "Pope") or academic
/// degrees: Wikidata files them under "award", but nobody receives them as
/// one. (Too slow to exclude from the list of all awards at once.)
async fn positions_and_degrees(sparql: &sparql::Sparql, ids: &[String]) -> anyhow::Result<HashSet<String>> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let values: Vec<String> = ids.iter().map(|id| format!("wd:{id}")).collect();
    let query = format!(
        "SELECT DISTINCT ?q {{ VALUES ?q {{ {} }} VALUES ?not {{ wd:{POSITION} wd:{DEGREE} }} ?q wdt:P31/wdt:P279* ?not }}",
        values.join(" ")
    );
    Ok(sparql.entities(&query, Some("q")).await?.into_iter().collect())
}

/// The items of the articles that an article links to, as numbers.
async fn linked_items(state: &AppState, site: &str, page: &str) -> anyhow::Result<Vec<u64>> {
    let Ok(wiki) = Wiki::from_dbname(site) else { return Ok(vec![]) };
    let mut conn = state.replicas.conn(&wiki, &["page", "pagelinks", "linktarget"]).await?;
    let titles: Vec<String> = conn
        .exec(
            "SELECT DISTINCT lt_title FROM page JOIN pagelinks ON pl_from=page_id JOIN linktarget ON pl_target_id=lt_id
             WHERE page_namespace=0 AND page_title=? AND lt_namespace=0",
            (db_title(page),),
        )
        .await?;
    let titles: Vec<String> = titles.iter().map(|t| display_title(t)).collect();
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["wb_items_per_site"]).await?;
    let sql = "SELECT DISTINCT ips_item_id FROM wb_items_per_site WHERE ips_site_id=? AND ips_site_page IN";
    select_in(&mut conn, sql, "", &[site.into()], &titles).await
}

/// The items that the item links to.
async fn item_links(state: &AppState, item: &str) -> anyhow::Result<HashSet<String>> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page", "pagelinks", "linktarget"]).await?;
    let links: Vec<String> = conn
        .exec(
            "SELECT DISTINCT lt_title FROM page JOIN pagelinks ON pl_from=page_id JOIN linktarget ON pl_target_id=lt_id
             WHERE page_namespace=0 AND page_title=? AND lt_namespace=0",
            (item,),
        )
        .await?;
    Ok(links.into_iter().collect())
}

/// Items that are awards. (Not their classes: "subclass of" chains lead from
/// "award" to occupations such as "biologist", even to buildings and wars.)
async fn all_awards(sparql: &sparql::Sparql) -> anyhow::Result<HashSet<u64>> {
    let query = format!("SELECT ?q {{ ?q wdt:P31/wdt:P279* wd:{AWARD} }}");
    let mut csv = sparql.csv(&query).await?;
    let mut awards = HashSet::new();
    for record in csv.records() {
        let record = record?;
        if let Some(n) = record.get(0).and_then(sparql::entity_id).and_then(|id| id.strip_prefix('Q')?.parse().ok()) {
            awards.insert(n);
        }
    }
    Ok(awards)
}

/// Lines `* 1999 [[Recipient]], [[Another]]` as `(year, title)`.
fn parse_recipients(wikitext: &str) -> Vec<(u16, String)> {
    let mut recipients = vec![];
    for line in wikitext.lines() {
        let rest = line.trim_start();
        let after_bullets = rest.trim_start_matches(['|', '*', '#']);
        if after_bullets.len() == rest.len() {
            continue;
        }
        let after_bullets = after_bullets.trim_start();
        let Some(year) = after_bullets.get(..4).and_then(parse_year) else { continue };
        let mut text = after_bullets.get(4..).unwrap_or_default();
        while let Some((_, after)) = text.split_once("[[") {
            let Some((link, after)) = after.split_once("]]") else { break };
            let target = link.split(['|', '#']).next().unwrap_or_default();
            if !target.trim().is_empty() {
                recipients.push((year, db_title(target)));
            }
            text = after;
        }
    }
    recipients
}

/// Statements for the recipients in the wikitext that Wikidata lacks; and
/// the recipients without an item (of a human).
async fn recipients(
    state: &AppState,
    wiki: &Wiki,
    award: &str,
    wikitext: &str,
) -> Result<(String, Vec<Missing>), AppError> {
    let mut recipients = parse_recipients(wikitext);
    if recipients.is_empty() {
        return Err(AppError::BadRequest("No lines with a year and linked recipients found.".into()));
    }
    recipients.truncate(MAX_RECIPIENTS);
    let has_year = existing_recipients(&state.sparql, award).await?;
    let mut per_year: HashMap<u16, usize> = HashMap::new();
    for (year, _) in &recipients {
        *per_year.entry(*year).or_default() += 1;
    }
    let titles: Vec<String> = recipients.iter().map(|(_, t)| t.clone()).collect();
    let targets = redirect_targets(state, wiki, &titles).await?;
    let target_titles: Vec<String> =
        titles.iter().map(|t| display_title(targets.get(t).unwrap_or(t))).collect::<HashSet<_>>().into_iter().collect();
    let items = human_items(state, wiki, &target_titles).await?;

    let mut statements = vec![];
    let mut missing = vec![];
    let mut done = HashSet::new();
    for (year, title) in recipients {
        let target = display_title(targets.get(&title).unwrap_or(&title));
        let item = items.get(&target).cloned().or_else(|| {
            // The only recipient of a year, and Wikidata knows one for that year: that's them.
            (per_year.get(&year) == Some(&1))
                .then(|| has_year.iter().find(|(_, years)| years.contains(&year)).map(|(q, _)| q.clone()))
                .flatten()
        });
        match item {
            None => missing.push(Missing {
                search: title.split(" (").next().unwrap_or_default().replace('_', " "),
                title: display_title(&title),
                year,
            }),
            Some(item) if has_year.get(&item).is_some_and(|years| years.contains(&year)) => {}
            Some(item) => {
                if done.insert((item.clone(), year)) {
                    statements.push(statement(&item, award, Some(year)));
                }
            }
        }
    }
    Ok((statements.join("\n"), missing))
}

/// The award's recipients on Wikidata, with the years they received it in.
async fn existing_recipients(sparql: &sparql::Sparql, award: &str) -> anyhow::Result<BTreeMap<String, HashSet<u16>>> {
    let query = format!("SELECT ?q ?date {{ ?q p:P166 ?s . ?s ps:P166 wd:{award} OPTIONAL {{ ?s pq:P585 ?date }} }}");
    let json = sparql.json(&query).await?;
    let mut recipients: BTreeMap<String, HashSet<u16>> = BTreeMap::new();
    for row in sparql::bindings(&json) {
        let Some(q) = row["q"]["value"].as_str().and_then(sparql::entity_id) else { continue };
        let years = recipients.entry(q.to_string()).or_default();
        if let Some(year) = row["date"]["value"].as_str().and_then(|d| d.get(..4)).and_then(parse_year) {
            years.insert(year);
        }
    }
    Ok(recipients)
}

/// Redirects among `titles` (database form), to their targets.
async fn redirect_targets(state: &AppState, wiki: &Wiki, titles: &[String]) -> anyhow::Result<HashMap<String, String>> {
    let mut conn = state.replicas.conn(wiki, &["page", "redirect"]).await?;
    let sql = "SELECT page_title, rd_title FROM page JOIN redirect ON rd_from=page_id
        WHERE page_namespace=0 AND rd_namespace=0 AND page_title IN";
    Ok(select_in(&mut conn, sql, "", &[], titles).await?.into_iter().collect())
}

/// The items of `titles` (with spaces) on `wiki` that link to "human".
async fn human_items(state: &AppState, wiki: &Wiki, titles: &[String]) -> anyhow::Result<HashMap<String, String>> {
    let items = crate::pagepile::items_for_titles(&state.replicas, wiki.dbname(), titles).await?;
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page", "pagelinks", "linktarget"]).await?;
    let Some(human) = linktarget_ids(&mut conn, 0, &[HUMAN]).await?.first().copied() else { return Ok(HashMap::new()) };
    let ids: Vec<String> = items.values().cloned().collect();
    let sql = "SELECT page_title FROM page JOIN pagelinks ON pl_from=page_id
        WHERE pl_target_id=? AND page_namespace=0 AND page_title IN";
    let humans: HashSet<String> = select_in(&mut conn, sql, "", &[human.into()], &ids).await?.into_iter().collect();
    Ok(items.into_iter().filter(|(_, q)| humans.contains(q)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn recipients_from_wikitext() {
        let text = "== Recipients ==\n* 1999 [[Jane Doe]], [[john smith (physicist)|John Smith]]\n# 2001: [[A#b]]\n1999 [[No bullet]]\n* 19x9 [[Bad]]";
        assert_eq!(
            parse_recipients(text),
            [(1999, "Jane_Doe".into()), (1999, "John_smith_(physicist)".into()), (2001, "A".into())]
        );
    }

    #[test]
    fn statements() {
        assert_eq!(statement("Q1", "Q2", Some(1999)), "Q1\tP166\tQ2\tP585\t+1999-01-01T00:00:00Z/9");
        let request: Request = [("aq", "2,3,4"), ("use_2", "on"), ("use_4", "on"), ("year_4", "2001"), ("year_2", "x")]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .into();
        assert_eq!(picked_statements("Q1", &request), "Q1\tP166\tQ2\nQ1\tP166\tQ4\tP585\t+2001-01-01T00:00:00Z/9");
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/awarder/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("name=\"wikitext\""));
        assert_eq!(get(dir.path(), "/awarder/?q=abc").await.status.as_u16(), 400);
        let generated = get(dir.path(), "/awarder/?q=Q1&generate=1&aq=5&use_5=on").await;
        assert!(generated.body.contains("Q1\tP166\tQ5"));
    }
}
