//! Which scripts Wikidata users load in their `common.js` (`userscripts_p`):
//! `user_page` (the common.js pages), `scripts`, and `script_use` between them.

use super::Writes;
use crate::mediawiki;
use crate::state::AppState;
use crate::tool_db::Database;
use crate::wiki::{Wiki, upper_first};
use anyhow::Result;
use futures::{StreamExt, stream};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

fn regex(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

static BLOCK_COMMENT: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"(?s)/\*.*?\*/"));
static LINE_COMMENT: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"(?m)(^|;)\s*//.*$"));
static LOAD: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r#"(?:importScript|mw\.loader\.load)\s*\(\s*['"](.+?)['"]"#));
static INDEX_PHP_TITLE: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"/w/index\.php\?title=([^&]+)"));

pub async fn update(state: &AppState, writes: Writes) -> Result<()> {
    let mut tool = state.tool_db.conn(Database::UserScripts).await?;
    let known: Vec<(u64, String, String)> = tool.query("SELECT id, page, last_touched FROM user_page").await?;
    let known: HashMap<String, (u64, String)> =
        known.into_iter().map(|(id, page, touched)| (page, (id, touched))).collect();

    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["page"]).await?;
    let sql = "SELECT page_title, page_touched FROM page
        WHERE page_namespace=2 AND page_title LIKE '%/common.js' AND page_is_redirect=0";
    let pages: Vec<(Vec<u8>, Vec<u8>)> = conn.query(sql).await?;
    let changed: Vec<(String, String)> = pages
        .into_iter()
        .map(|(title, touched)| {
            (normalize(&String::from_utf8_lossy(&title)), String::from_utf8_lossy(&touched).into_owned())
        })
        .filter(|(page, touched)| known.get(page).is_none_or(|(_, last)| touched > last))
        .collect();

    let wiki = Wiki::wikidata();
    let mut texts = stream::iter(changed)
        .map(|(page, touched)| {
            let wiki = &wiki;
            async move {
                let text = mediawiki::page_wikitext(&state.http, wiki, &format!("User:{page}")).await;
                (page, touched, text)
            }
        })
        .buffer_unordered(4);
    let mut updated = 0;
    while let Some((page, touched, text)) = texts.next().await {
        let text = match text {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("User:{page}: {e:#}");
                continue;
            }
        };
        if writes.dry_run {
            println!("[dry run] User:{page}: {:?}", scripts(&text));
            continue;
        }
        store(&mut tool, &page, &touched, &scripts(&text)).await?;
        updated += 1;
    }
    println!("{updated} user pages updated");
    Ok(())
}

/// Records the scripts a page loads, replacing what it loaded before.
async fn store(tool: &mut Conn, page: &str, touched: &str, scripts: &[(String, bool)]) -> Result<()> {
    tool.exec_drop("INSERT IGNORE INTO user_page (page, last_touched) VALUES (?, '')", (page,)).await?;
    let page_id: Option<u64> = tool.exec_first("SELECT id FROM user_page WHERE page=?", (page,)).await?;
    let Some(page_id) = page_id else { return Ok(()) };
    tool.exec_drop("DELETE FROM script_use WHERE user_page_id=?", (page_id,)).await?;
    for (path, on_wiki) in scripts {
        tool.exec_drop("INSERT IGNORE INTO scripts (path, on_wiki) VALUES (?, ?)", (path, on_wiki)).await?;
        let script_id: Option<u64> = tool.exec_first("SELECT id FROM scripts WHERE path=?", (path,)).await?;
        if let Some(script_id) = script_id {
            tool.exec_drop(
                "INSERT IGNORE INTO script_use (script_id, user_page_id) VALUES (?, ?)",
                (script_id, page_id),
            )
            .await?;
        }
    }
    tool.exec_drop("UPDATE user_page SET last_touched=? WHERE id=?", (touched, page_id)).await?;
    Ok(())
}

/// The scripts a common.js loads, outside comments: `(path, on_wiki)`. On-wiki
/// scripts by page title; others by URL path (without scheme).
fn scripts(js: &str) -> Vec<(String, bool)> {
    let (Some(block), Some(line), Some(load)) = (BLOCK_COMMENT.as_ref(), LINE_COMMENT.as_ref(), LOAD.as_ref()) else {
        return vec![];
    };
    let js = block.replace_all(js, "");
    let js = line.replace_all(&js, "$1");
    let mut out: Vec<(String, bool)> =
        load.captures_iter(&js).filter_map(|c| Some(script(c.get(1)?.as_str()))).collect();
    out.sort();
    out.dedup();
    out
}

fn script(path: &str) -> (String, bool) {
    let path = path.trim_start_matches("https:").trim_start_matches("http:");
    let path = path.strip_prefix("//www.wikidata.org").unwrap_or(path);
    if let Some(title) = INDEX_PHP_TITLE.as_ref().and_then(|r| r.captures(path)).and_then(|c| c.get(1)) {
        let title = percent_encoding::percent_decode_str(title.as_str()).decode_utf8_lossy();
        return (normalize(&title), true);
    }
    match path.starts_with('/') {
        true => (path.to_string(), false),
        false => (normalize(path), true),
    }
}

/// A page title as stored: spaces, first letter upper case.
fn normalize(title: &str) -> String {
    upper_first(title.replace('_', " ").trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_outside_comments() {
        let js = "importScript('User:Magnus_Manske/duplicate_item.js');\n\
            // importScript('User:X/old.js');\n\
            /* mw.loader.load('//x.org/y.js'); */\n\
            mw.loader.load(\"//www.wikidata.org/w/index.php?title=User:A/b.js&action=raw&ctype=text/javascript\"); // note\n\
            mw.loader.load('https://tools-static.wmflabs.org/x/y.js');";
        assert_eq!(
            scripts(js),
            [
                ("//tools-static.wmflabs.org/x/y.js".to_string(), false),
                ("User:A/b.js".to_string(), true),
                ("User:Magnus Manske/duplicate item.js".to_string(), true),
            ]
        );
    }
}
