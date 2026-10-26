//! The query language of WikiData Query (WDQ, `wdq.wmflabs.org`), long gone,
//! translated to SPARQL, so that old links and habits keep working. Only its
//! common forms: `claim[P]`, `claim[P:Q]`, `claim[P:(tree[Q][][P])]`,
//! `noclaim[…]`, `tree[Q][P][P]`, `link[wiki]`, `nolink[wiki]`, `items[Q,…]`,
//! joined with `and`. Several values in one clause (`claim[31:5,279]`) mean
//! any of them.

use crate::sparql::{Entities, Sparql};
use crate::web::AppError;
use crate::wiki::Wiki;
use crate::wikidata::parse_id;
use serde_json::Value;

/// For the pages that accept WDQ.
pub const SUPPORTED: &str = "claim[P], claim[P:Q], claim[P:(tree[Q][][P])], noclaim[…], tree[Q][P][P], link[wiki], nolink[wiki], items[Q,…], joined with “and”";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot translate the WDQ query ({0}); supported are {SUPPORTED}")]
pub struct WdqError(String);

/// A user's query: SPARQL as it is, WDQ translated (see [`to_sparql`]).
/// WDQ never has braces; SPARQL always has.
pub fn sparql_or_wdq(query: &str) -> Result<String, WdqError> {
    match query.contains('{') {
        true => Ok(query.trim().to_string()),
        false => to_sparql(query),
    }
}

/// The result of a user's query, SPARQL or WDQ. A failure is most likely
/// the query's, so it is the user's to see.
pub async fn run(sparql: &Sparql, query: &str) -> Result<Value, AppError> {
    let query = sparql_or_wdq(query)?;
    sparql.json(&query).await.map_err(|e| {
        tracing::info!("user query failed: {e:#}");
        AppError::BadRequest("The query failed. Please check it on https://query.wikidata.org/".to_string())
    })
}

/// The first `limit` entities of a user's query, SPARQL or WDQ, read row by
/// row; with the number of rows that had one.
pub async fn entities(sparql: &Sparql, query: &str, limit: usize) -> Result<Entities, AppError> {
    let query = sparql_or_wdq(query)?;
    sparql.entities(&query, None, limit).await.map_err(|e| {
        tracing::info!("user query failed: {e:#}");
        AppError::BadRequest("The query failed. Please check it on https://query.wikidata.org/".to_string())
    })
}

impl From<WdqError> for AppError {
    fn from(e: WdqError) -> Self {
        Self::BadRequest(e.to_string())
    }
}

/// `SELECT DISTINCT ?item { … }` for a WDQ query.
pub fn to_sparql(wdq: &str) -> Result<String, WdqError> {
    Ok(format!("SELECT DISTINCT ?item {{ {} }}", pattern(wdq)?))
}

/// The SPARQL graph pattern for a WDQ query; it binds `?item`.
pub fn pattern(wdq: &str) -> Result<String, WdqError> {
    let mut translator = Translator::default();
    let (mut positive, mut negative) = (vec![], vec![]);
    for clause in split_top(wdq, Separator::And)? {
        let (name, args) = call(clause)?;
        match name.as_str() {
            "claim" => positive.push(translator.claims(&args)?),
            "noclaim" => negative.push(translator.claims(&args)?),
            "tree" => positive.push(translator.tree("?item", &args)?),
            "link" => positive.push(translator.links(&args)?),
            "nolink" => negative.push(translator.links(&args)?),
            "items" => positive.push(items(&args)?),
            _ => return Err(WdqError(format!("unknown clause “{name}”"))),
        }
    }
    if positive.is_empty() {
        return Err(WdqError("there must be a clause that is not negated".to_string()));
    }
    // MINUS removes what the positive clauses found, so it comes last.
    positive.extend(negative.into_iter().map(|p| format!("MINUS {{ {p} }}")));
    Ok(positive.join(" "))
}

#[derive(Default)]
struct Translator {
    variables: usize,
}

impl Translator {
    fn variable(&mut self) -> String {
        self.variables += 1;
        format!("?v{}", self.variables)
    }

    /// `P`, `P:Q` or `P:(tree…)`, comma-separated: any of them.
    fn claims(&mut self, args: &[&str]) -> Result<String, WdqError> {
        let arg = single(args)?;
        let patterns = split_top(arg, Separator::Comma)?
            .into_iter()
            .map(|claim| {
                let (property, value) = claim.split_once(':').unwrap_or((claim, ""));
                let property = id(property, 'P')?;
                let value = value.trim();
                if value.is_empty() {
                    return Ok(format!("?item p:{property} [] ."));
                }
                if let Some(inner) = value.strip_prefix('(').and_then(|v| v.strip_suffix(')')) {
                    let (name, args) = call(inner)?;
                    if name != "tree" {
                        return Err(WdqError(format!("only a tree can be a claim's value, not “{name}”")));
                    }
                    let variable = self.variable();
                    let tree = self.tree(&variable, &args)?;
                    return Ok(format!("?item p:{property}/ps:{property} {variable} . {tree}"));
                }
                Ok(format!("?item p:{property}/ps:{property} wd:{} .", id(value, 'Q')?))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(union(patterns))
    }

    /// `tree[roots][forward properties][reverse properties]`: the roots and
    /// all items reached from them along those properties, in `variable`.
    fn tree(&mut self, variable: &str, args: &[&str]) -> Result<String, WdqError> {
        let list = |i: usize, prefix: char| -> Result<Vec<String>, WdqError> {
            let arg = args.get(i).copied().unwrap_or_default();
            split_top(arg, Separator::Comma)?.into_iter().map(|s| id(s, prefix)).collect()
        };
        if args.is_empty() || args.len() > 3 {
            return Err(WdqError("a tree needs [roots][forward properties][reverse properties]".to_string()));
        }
        let roots = list(0, 'Q')?;
        let (forward, reverse) = (list(1, 'P')?, list(2, 'P')?);
        let root = match roots.as_slice() {
            [] => return Err(WdqError("a tree needs a root".to_string())),
            [root] => format!("wd:{root}"),
            _ => {
                let root = self.variable();
                let values = roots.iter().map(|q| format!("wd:{q}")).collect::<Vec<_>>().join(" ");
                return Ok(format!("VALUES {root} {{ {values} }} {}", path(&root, variable, &forward, &reverse)));
            }
        };
        Ok(path(&root, variable, &forward, &reverse))
    }

    /// Sitelinks to any of the wikis (`enwiki`, …; none: any wiki).
    fn links(&mut self, args: &[&str]) -> Result<String, WdqError> {
        let wikis = split_top(single(args)?, Separator::Comma)?;
        if wikis.is_empty() {
            return Ok("[] schema:about ?item ; schema:isPartOf [] .".to_string());
        }
        let patterns = wikis
            .into_iter()
            .map(|dbname| {
                let wiki = Wiki::from_dbname(dbname).map_err(|e| WdqError(e.to_string()))?;
                Ok(format!("[] schema:about ?item ; schema:isPartOf <https://{}/> .", wiki.host()))
            })
            .collect::<Result<Vec<_>, WdqError>>()?;
        Ok(union(patterns))
    }
}

/// `root` and what it reaches along the properties, into `variable`.
fn path(root: &str, variable: &str, forward: &[String], reverse: &[String]) -> String {
    fn steps(properties: &[String], inverse: &str) -> Vec<String> {
        properties.iter().map(|p| format!("{inverse}wdt:{p}")).collect()
    }
    match (forward.is_empty(), reverse.is_empty()) {
        (true, true) => format!("BIND({root} AS {variable})"),
        // The usual direction, which the query service handles best.
        (true, false) => format!("{variable} ({})* {root} .", steps(reverse, "").join("|")),
        _ => format!("{root} ({})* {variable} .", [steps(forward, ""), steps(reverse, "^")].concat().join("|")),
    }
}

fn items(args: &[&str]) -> Result<String, WdqError> {
    let ids = split_top(single(args)?, Separator::Comma)?
        .into_iter()
        .map(|s| id(s, 'Q').map(|q| format!("wd:{q}")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("VALUES ?item {{ {} }}", ids.join(" ")))
}

fn union(patterns: Vec<String>) -> String {
    match patterns.as_slice() {
        [single] => single.clone(),
        _ => patterns.iter().map(|p| format!("{{ {p} }}")).collect::<Vec<_>>().join(" UNION "),
    }
}

fn id(s: &str, prefix: char) -> Result<String, WdqError> {
    parse_id(s, prefix).ok_or_else(|| WdqError(format!("“{}” is not a {prefix} number", s.trim())))
}

fn single<'a>(args: &[&'a str]) -> Result<&'a str, WdqError> {
    match args {
        [arg] => Ok(arg),
        _ => Err(WdqError("a clause has one [list]".to_string())),
    }
}

/// `name[arg][arg]…` as the lower-case name and the args.
fn call(s: &str) -> Result<(String, Vec<&str>), WdqError> {
    let s = s.trim();
    let open = s.find('[').ok_or_else(|| WdqError(format!("“{s}” is not a clause")))?;
    let name = s.get(..open).unwrap_or_default().trim().to_lowercase();
    let mut rest = s.get(open..).unwrap_or_default();
    let mut args = vec![];
    while !rest.is_empty() {
        let close = closing(rest).ok_or_else(|| WdqError(format!("unbalanced brackets in “{s}”")))?;
        args.push(rest.get(1..close).unwrap_or_default());
        rest = rest.get(close + 1..).unwrap_or_default().trim_start();
    }
    Ok((name, args))
}

/// Where the bracket that `s` starts with closes.
fn closing(s: &str) -> Option<usize> {
    if !s.starts_with('[') {
        return None;
    }
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return Some(i);
        }
    }
    None
}

#[derive(Clone, Copy)]
enum Separator {
    Comma,
    /// The word `and`, between spaces.
    And,
}

/// `s` split at separators outside brackets and parentheses; empty parts dropped.
fn split_top(s: &str, separator: Separator) -> Result<Vec<&str>, WdqError> {
    let mut parts = vec![];
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in s.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return Err(WdqError(format!("unbalanced brackets in “{s}”")));
        }
        let (at_separator, width) = match separator {
            Separator::Comma => (c == ',', 1),
            Separator::And => {
                let word = s.get(i..i + 3).is_some_and(|w| w.eq_ignore_ascii_case("and"));
                let spaced = s.get(..i).is_some_and(|b| b.ends_with(char::is_whitespace))
                    && s.get(i + 3..).is_some_and(|a| a.starts_with(char::is_whitespace));
                (word && spaced, 3)
            }
        };
        if depth == 0 && at_separator && i >= start {
            parts.push(s.get(start..i).unwrap_or_default());
            start = i + width;
        }
    }
    if depth != 0 {
        return Err(WdqError(format!("unbalanced brackets in “{s}”")));
    }
    parts.push(s.get(start..).unwrap_or_default());
    Ok(parts.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims() {
        assert_eq!(pattern("claim[31:5]").unwrap(), "?item p:P31/ps:P31 wd:Q5 .");
        assert_eq!(pattern("CLAIM[P31]").unwrap(), "?item p:P31 [] .");
        assert_eq!(pattern("claim[31:5,279]").unwrap(), "{ ?item p:P31/ps:P31 wd:Q5 . } UNION { ?item p:P279 [] . }");
        assert_eq!(
            pattern("claim[31:(tree[838948][][279])]").unwrap(),
            "?item p:P31/ps:P31 ?v1 . ?v1 (wdt:P279)* wd:Q838948 ."
        );
    }

    #[test]
    fn combined() {
        assert_eq!(
            pattern("link[dewiki] and noclaim[31,279]").unwrap(),
            "[] schema:about ?item ; schema:isPartOf <https://de.wikipedia.org/> . \
             MINUS { { ?item p:P31 [] . } UNION { ?item p:P279 [] . } }"
        );
        assert_eq!(
            to_sparql("noclaim[18] AND items[1, Q2]").unwrap(),
            "SELECT DISTINCT ?item { VALUES ?item { wd:Q1 wd:Q2 } MINUS { ?item p:P18 [] . } }"
        );
        // "and" inside a word or brackets is no separator.
        assert!(pattern("claim[31:5] andclaim[1]").is_err());
    }

    #[test]
    fn trees() {
        assert_eq!(pattern("tree[146481][][361,279]").unwrap(), "?item (wdt:P361|wdt:P279)* wd:Q146481 .");
        assert_eq!(pattern("tree[1][2][3]").unwrap(), "wd:Q1 (wdt:P2|^wdt:P3)* ?item .");
        assert_eq!(pattern("tree[1,2][2]").unwrap(), "VALUES ?v1 { wd:Q1 wd:Q2 } ?v1 (wdt:P2)* ?item .");
        assert_eq!(pattern("tree[5]").unwrap(), "BIND(wd:Q5 AS ?item)");
    }

    #[test]
    fn errors() {
        for bad in
            ["", "claim[31", "claim[x]", "noclaim[31]", "web[x]", "link[nowiki_x]", "claim[31:(items[1])]", "a)("]
        {
            assert!(pattern(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sparql_passes() {
        assert_eq!(sparql_or_wdq(" SELECT ?q { ?q wdt:P31 wd:Q5 } ").unwrap(), "SELECT ?q { ?q wdt:P31 wd:Q5 }");
        assert!(sparql_or_wdq("claim[31:5]").unwrap().starts_with("SELECT DISTINCT ?item"));
    }
}
