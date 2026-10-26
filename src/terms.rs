//! Labels, descriptions and aliases of Wikidata items and properties, from
//! the term store (the `wbt_*` tables on their own database server; the
//! copies in the main Wikidata database are frozen).

use crate::replicas::{Replicas, placeholders, select_in};
use crate::wiki::Wiki;
use anyhow::Result;
use std::collections::HashMap;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Params, Value};

const TABLES: &[&str] = &["wbt_item_terms", "wbt_property_terms", "wbt_term_in_lang", "wbt_text_in_lang", "wbt_text"];

/// The fallback languages most tools here use, after the user's own. `mul`
/// holds the labels common to many languages (e.g. most names of people).
pub const DEFAULT_LANGUAGES: &[&str] = &["en", "mul", "de", "fr", "es", "it", "nl"];

/// Term types, as `wbt_type` numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TermType {
    Label = 1,
    Description = 2,
    Alias = 3,
}

impl TermType {
    fn from_id(id: u8) -> Option<Self> {
        match id {
            1 => Some(Self::Label),
            2 => Some(Self::Description),
            3 => Some(Self::Alias),
            _ => None,
        }
    }
}

/// One term of an entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// `Q42` or `P31`.
    pub entity: String,
    pub term_type: TermType,
    pub language: String,
    pub text: String,
}

pub async fn conn(replicas: &Replicas) -> Result<Conn> {
    replicas.conn(&Wiki::wikidata(), TABLES).await
}

/// The label of each of `ids` (items and properties) in the first of
/// `languages` that has one. Entities without such a label are left out.
pub async fn labels(replicas: &Replicas, ids: &[String], languages: &[&str]) -> Result<HashMap<String, String>> {
    let terms = terms(replicas, ids, &[TermType::Label], languages).await?;
    let rank = |language: &str| languages.iter().position(|l| *l == language).unwrap_or(usize::MAX);
    let mut best: HashMap<String, (usize, String)> = HashMap::new();
    for term in terms {
        let r = rank(&term.language);
        match best.get(&term.entity) {
            Some((existing, _)) if *existing <= r => {}
            _ => {
                best.insert(term.entity, (r, term.text));
            }
        }
    }
    Ok(best.into_iter().map(|(id, (_, label))| (id, label)).collect())
}

/// All terms of `types` of `ids` (`Q…` and `P…`), in `languages` (all, if empty).
pub async fn terms(replicas: &Replicas, ids: &[String], types: &[TermType], languages: &[&str]) -> Result<Vec<Term>> {
    let (items, properties): (Vec<&String>, Vec<&String>) = ids.iter().partition(|id| id.starts_with('Q'));
    let numbers = |ids: Vec<&String>| ids.iter().filter_map(|id| id[1..].parse::<u64>().ok()).collect::<Vec<_>>();
    let mut conn = conn(replicas).await?;
    let mut terms = vec![];
    for (prefix, table, column, term_column, numbers) in [
        ('Q', "wbt_item_terms", "wbit_item_id", "wbit_term_in_lang_id", numbers(items)),
        ('P', "wbt_property_terms", "wbpt_property_id", "wbpt_term_in_lang_id", numbers(properties)),
    ] {
        if numbers.is_empty() {
            continue;
        }
        let mut params: Vec<Value> = types.iter().map(|t| Value::from(*t as u8)).collect();
        let mut sql = format!(
            "SELECT {column}, wbtl_type_id, wbxl_language, wbx_text FROM {table}
             JOIN wbt_term_in_lang ON {term_column}=wbtl_id
             JOIN wbt_text_in_lang ON wbtl_text_in_lang_id=wbxl_id
             JOIN wbt_text ON wbxl_text_id=wbx_id
             WHERE wbtl_type_id IN ({})",
            placeholders(types.len())
        );
        if !languages.is_empty() {
            sql += &format!(" AND wbxl_language IN ({})", placeholders(languages.len()));
            params.extend(languages.iter().map(|l| Value::from(*l)));
        }
        sql += &format!(" AND {column} IN");
        let rows: Vec<(u64, u8, Vec<u8>, Vec<u8>)> = select_in(&mut conn, &sql, "", &params, &numbers).await?;
        terms.extend(rows.into_iter().filter_map(|(id, kind, language, text)| {
            Some(Term {
                entity: format!("{prefix}{id}"),
                term_type: TermType::from_id(kind)?,
                language: String::from_utf8_lossy(&language).into_owned(),
                text: String::from_utf8_lossy(&text).into_owned(),
            })
        }));
    }
    Ok(terms)
}

/// Items with a term of `types` that is exactly one of `texts`, as `(item, text)`;
/// in `language` only, if given.
pub async fn items_with_texts(
    replicas: &Replicas,
    texts: &[String],
    types: &[TermType],
    language: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let mut conn = conn(replicas).await?;
    let mut params: Vec<Value> = types.iter().map(|t| Value::from(*t as u8)).collect();
    let mut sql = format!(
        "SELECT DISTINCT wbit_item_id, wbx_text FROM wbt_text
         JOIN wbt_text_in_lang ON wbxl_text_id=wbx_id
         JOIN wbt_term_in_lang ON wbtl_text_in_lang_id=wbxl_id
         JOIN wbt_item_terms ON wbit_term_in_lang_id=wbtl_id
         WHERE wbtl_type_id IN ({})",
        placeholders(types.len())
    );
    if let Some(language) = language {
        sql += " AND wbxl_language=?";
        params.push(language.into());
    }
    sql += " AND wbx_text IN";
    let rows: Vec<(u64, Vec<u8>)> = select_in(&mut conn, &sql, "", &params, texts).await?;
    Ok(rows.into_iter().map(|(id, text)| (format!("Q{id}"), String::from_utf8_lossy(&text).into_owned())).collect())
}

/// Items with a term of `types` in `language` matching the SQL `LIKE` pattern, at most `limit`.
pub async fn items_with_text_like(
    replicas: &Replicas,
    pattern: &str,
    types: &[TermType],
    language: &str,
    limit: usize,
) -> Result<Vec<(String, String)>> {
    let mut conn = conn(replicas).await?;
    let mut params: Vec<Value> = vec![pattern.into(), language.into()];
    params.extend(types.iter().map(|t| Value::from(*t as u8)));
    let sql = format!(
        "SELECT DISTINCT wbit_item_id, wbx_text FROM wbt_text
         JOIN wbt_text_in_lang ON wbxl_text_id=wbx_id
         JOIN wbt_term_in_lang ON wbtl_text_in_lang_id=wbxl_id
         JOIN wbt_item_terms ON wbit_term_in_lang_id=wbtl_id
         WHERE wbx_text LIKE ? AND wbxl_language=? AND wbtl_type_id IN ({}) LIMIT {limit}",
        placeholders(types.len())
    );
    let rows: Vec<(u64, Vec<u8>)> = conn.exec(sql, Params::Positional(params)).await?;
    Ok(rows.into_iter().map(|(id, text)| (format!("Q{id}"), String::from_utf8_lossy(&text).into_owned())).collect())
}

/// `languages`, with `preferred` (comma-separated codes) first.
pub fn languages_with<'a>(preferred: &'a str, languages: &[&'a str]) -> Vec<&'a str> {
    let mut out: Vec<&str> = preferred.split(',').map(str::trim).filter(|l| crate::wiki::is_language_code(l)).collect();
    for l in languages {
        if !out.contains(l) {
            out.push(l);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_languages_first() {
        assert_eq!(languages_with("de, fr", &["en", "de"]), ["de", "fr", "en"]);
        assert_eq!(languages_with("", &["en"]), ["en"]);
        assert_eq!(languages_with("x;y", &["en"]), ["en"]);
    }
}
