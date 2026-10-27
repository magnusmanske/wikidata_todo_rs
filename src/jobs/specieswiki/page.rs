//! What a Wikispecies page is about, from its HTML: a taxon (with its rank
//! and parent, from the Taxonavigation section), a publication, a journal,
//! a disambiguation page, …

use super::ranks::{SUBGENUS, rank_item};
use crate::mediawiki::html_to_text;
use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageKind {
    /// A publication, with its IPNI publication ID if any.
    WrittenWork {
        title: String,
        ipni: Option<String>,
        abbreviation: Option<String>,
    },
    /// A name Wikispecies marks as invalid: not for Wikidata.
    InvalidName,
    /// A reference (with authors), of unknown kind.
    Source,
    /// A journal, with its Botanico-Periodicum-Huntianum ID.
    Journal {
        bph: String,
    },
    Disambiguation,
    Taxon(Taxon),
    /// A taxon whose rank is not known here.
    UnknownRank(String),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taxon {
    /// As Wikidata has it (P225): a subgenus without its genus.
    pub name: String,
    /// As the page has it, lower case.
    pub rank: String,
    pub rank_item: &'static str,
    /// The parent taxon's page title.
    pub parent: Option<String>,
    pub extinct: bool,
}

fn regex(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

static IPNI: LazyLock<Option<Regex>> = LazyLock::new(|| {
    regex(r#"<a rel="nofollow" class="external text" href="https://www\.ipni\.org/p/(.+?)">IPNI</a>"#)
});
static WORK_TITLE: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"<li>Title: <i><b>(.+?)</b></i></li>"));
static ABBREVIATION: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"<li>Abbreviation: <i><b>(.+?)</b></i></li>"));
static BPH: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"<li>BPH Index: (\d+)</li>"));
/// What separates the lines of the Taxonavigation section.
static LINE_BREAK: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"(?i)<br\s*/?>|</?(?:p|td|tr|th|tbody|table)\b[^>]*>|\n"));
static SELF_LINK: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r#"<a class="mw-selflink selflink">(.+?)</a>"#));
/// A name in bold or italics: `<b>…</b>`, `<i>…</i>`.
static EMPHASIZED: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"<([bi])>(.+?)</([bi])>"));
static LINK_TITLE: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r#"<a href="/wiki/[^"]*"[^>]*?\stitle="([^"]+)""#));
/// A rank, and its taxon on the next line.
static RANK_AT_LINE_END: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"(?:\(\d+\))?:[ \t]*\n"));
/// An abbreviated name with its full form: `<abbr title="Hexamita">H</abbr>.`
static ABBR: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r#"<abbr title="([^"]+)">[^<]*</abbr>\.?"#));
static BRACKETS: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"\s*\(.*\)\s*"));
/// `Genus (Subgenus)`, or `Subgenus`.
static SUBGENUS_NAME: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^(?:[A-Z]\S* \(([A-Z][a-z]+)\)|([A-Z][a-z]+))$"));
static ABBREVIATED_GENUS: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"^([A-Z])\. (.+)$"));

/// Kingdoms whose names follow the botanical code.
const BOTANICAL_KINGDOMS: &[&str] = &[r#"title="Plantae""#, r#"title="Fungi""#, r#"title="Chromista""#];

/// What the page `title` with this HTML is about. The checks come in the
/// order the PHP bot made them. (It took titles like `Herpa (Walker)` as
/// a taxon and an ancestor, which fails for subgenera, `Genus (Subgenus)`,
/// and for authors; their Taxonavigation says what they are.)
pub fn classify(title: &str, html: &str) -> PageKind {
    let title = title.replace('_', " ");
    // Probably not a taxon.
    if title.contains(',') {
        return PageKind::Other;
    }
    let first = |r: &Option<Regex>| r.as_ref().and_then(|r| r.captures(html)).map(|c| c[1].to_string());
    let ipni = first(&IPNI);
    let abbreviation = first(&ABBREVIATION);
    if let Some(work) = first(&WORK_TITLE).or_else(|| ipni.as_ref().map(|_| title.clone())) {
        return PageKind::WrittenWork { title: work, ipni, abbreviation };
    }
    if html.contains("The name of this taxon appears to be <b>invalid</b> under the relevant nomenclatural code") {
        return PageKind::InvalidName;
    }
    if html.contains("<li>Authors:") {
        return PageKind::Source;
    }
    if let Some(bph) = first(&BPH) {
        return PageKind::Journal { bph };
    }
    if html.contains(r#"<a href="/wiki/File:Disambig.svg""#) {
        return PageKind::Disambiguation;
    }
    match taxonavigation(&title, html) {
        Some(Navigation::Taxon(taxon)) => PageKind::Taxon(taxon),
        Some(Navigation::UnknownRank(rank)) => PageKind::UnknownRank(rank),
        None => PageKind::Other,
    }
}

enum Navigation {
    Taxon(Taxon),
    UnknownRank(String),
}

/// The taxon of the page, from its Taxonavigation section: one taxon per
/// line, `Rank: link`, from the top down to this page's taxon, which links
/// to itself (or, unlinked, is the title in bold or italics). The line
/// before it is the parent. Extinct if a dagger marks it or one of its ancestors.
fn taxonavigation(title: &str, html: &str) -> Option<Navigation> {
    if !html.contains("Taxonavigation") {
        return None;
    }
    let section = section(html, "Taxonavigation");
    let botanical = BOTANICAL_KINGDOMS.iter().any(|k| section.contains(k));
    let section = RANK_AT_LINE_END.as_ref()?.replace_all(section, ": ");
    let mut parent: Option<String> = None;
    let mut extinct_ancestor = false;
    for line in LINE_BREAK.as_ref()?.split(&section) {
        let line = line.trim();
        let Some(start) = line.find('<') else { continue };
        let (prefix, rest) = line.split_at(start);
        let rank =
            prefix.trim_matches(|c: char| c.is_whitespace() || matches!(c, ':' | 'ː' | '"' | '†')).to_lowercase();
        let own = SELF_LINK.as_ref()?.captures(rest).map(|c| (c[1].to_string(), true));
        let own = own.or_else(|| {
            let emphasized = EMPHASIZED.as_ref()?.captures(rest)?;
            (!rest.contains("<a ")).then(|| (emphasized[2].to_string(), false))
        });
        if let Some((name, linked)) = own {
            if rank.is_empty() || !rank.chars().all(|c| c.is_alphabetic() || c == ' ') {
                return None;
            }
            let rank_item = rank_item(&rank, botanical);
            let name = match rank_item {
                Some(SUBGENUS) => subgenus_name(&name),
                _ => Some(clean_name(&name, parent.as_deref(), title)),
            };
            // An unlinked name could be any taxon; it must be the page's.
            let Some(name) = name.filter(|name| linked || name == title) else { continue };
            let Some(rank_item) = rank_item else {
                return Some(Navigation::UnknownRank(rank));
            };
            let extinct = extinct_ancestor || line.contains('†');
            return Some(Navigation::Taxon(Taxon { name, rank, rank_item, parent, extinct }));
        }
        if let Some(c) = LINK_TITLE.as_ref()?.captures(rest) {
            parent = Some(wikimisc::escape::unescape_html(&c[1]).into_owned());
            extinct_ancestor |= line.contains('†');
        }
    }
    None
}

/// The HTML from the heading with this ID to the next heading; without such
/// a heading, from the start (some pages begin with their Taxonavigation).
fn section<'a>(html: &'a str, id: &str) -> &'a str {
    let body = match html.find(&format!(r#"id="{id}""#)) {
        Some(start) => {
            let rest = &html[start..];
            rest.find("</h2>").map_or(rest, |end| &rest[end..])
        }
        None => html,
    };
    body.find(r#"<div class="mw-heading"#).map_or(body, |end| &body[..end])
}

/// A taxon name as the page writes it, made plain: no markup, quotes,
/// daggers, authors in brackets; abbreviations written out (from `<abbr>`,
/// the page title, or the parent's genus).
fn clean_name(html: &str, parent: Option<&str>, title: &str) -> String {
    let html = ABBR.as_ref().map_or(html.into(), |r| r.replace_all(html, "$1"));
    let text = html_to_text(&html).replace(['\u{a0}', '†', '"'], " ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = match BRACKETS.as_ref() {
        Some(r) => r.replace_all(&text, " ").trim().to_string(),
        None => text,
    };
    if abbreviates(&text, title) {
        return title.to_string();
    }
    let expanded = ABBREVIATED_GENUS.as_ref().and_then(|r| r.captures(&text)).and_then(|c| {
        let genus = parent?.split_whitespace().next()?;
        genus.starts_with(&c[1]).then(|| format!("{genus} {}", &c[2]))
    });
    expanded.unwrap_or(text)
}

/// `Subgenus`, of `Genus (Subgenus)` (or of itself).
fn subgenus_name(html: &str) -> Option<String> {
    let text = html_to_text(html).replace('\u{a0}', " ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let c = SUBGENUS_NAME.as_ref()?.captures(&text)?;
    Some(c.get(1).or(c.get(2))?.as_str().to_string())
}

/// Whether `name` is `full` with some words cut to their initial
/// (`I. c. var. ciliaris` for `Inula ciliaris var. ciliaris`).
fn abbreviates(name: &str, full: &str) -> bool {
    let (short, long): (Vec<&str>, Vec<&str>) = (name.split(' ').collect(), full.split(' ').collect());
    short.len() == long.len()
        && short != long
        && short.iter().zip(&long).all(|(s, l)| {
            s == l || s.strip_suffix('.').is_some_and(|i| i.chars().count() == 1 && l.starts_with(i) && *l != *s)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taxon(name: &str, rank: &str, item: &'static str, parent: Option<&str>, extinct: bool) -> PageKind {
        PageKind::Taxon(Taxon {
            name: name.to_string(),
            rank: rank.to_string(),
            rank_item: item,
            parent: parent.map(str::to_string),
            extinct,
        })
    }

    const HEADING: &str = r#"<div class="mw-heading mw-heading2"><h2 id="Taxonavigation">Taxonavigation</h2></div>"#;
    const NEXT: &str = r#"<div class="mw-heading mw-heading2"><h2 id="Name">Name</h2></div><p>† <i>Other</i></p>"#;

    fn page(navigation: &str) -> String {
        format!("{HEADING}\n{navigation}\n{NEXT}")
    }

    #[test]
    fn taxa() {
        // Plants: a section is the botanical rank; the parent comes from the link title.
        let html = page(
            r#"<p>Regnum: <a href="/wiki/Plantae" title="Plantae">Plantae</a><br />
Genus: <i><a href="/wiki/Acer" title="Acer">Acer</a></i><br />
Section: <a href="/wiki/Acer_sect._Platanoidea" title="Acer sect. Platanoidea"><i>Acer</i> sect. <i>Platanoidea</i></a> <br />
Species: <i><a class="mw-selflink selflink">Acer hueberi</a></i>
</p>"#,
        );
        assert_eq!(
            classify("Acer hueberi", &html),
            taxon("Acer hueberi", "species", "Q7432", Some("Acer sect. Platanoidea"), false)
        );
        // Quotes, no colon, a dagger on the taxon's own line only.
        let html = page(
            r#"<p>Classis: "<a href="/wiki/Microgenomatia" title="Microgenomatia">Microgenomatia</a>" <br />
Ordo: † "<a class="mw-selflink selflink">Levybacterales</a>" <br />
Familiae:
<i><a href="/wiki/Sevelisomataceae" title="Sevelisomataceae">Sevelisomataceae</a></i></p>"#,
        );
        assert_eq!(
            classify("Levybacterales", &html),
            taxon("Levybacterales", "ordo", "Q36602", Some("Microgenomatia"), true)
        );
        let html = page(
            r#"<p>Genus: <i><a href="/wiki/Impatiens" title="Impatiens">Impatiens</a></i><br />
Species <i><a class="mw-selflink selflink">I. meeboldii</a></i></p>"#,
        );
        assert_eq!(
            classify("Impatiens meeboldii", &html),
            taxon("Impatiens meeboldii", "species", "Q7432", Some("Impatiens"), false)
        );
        // In bold or italics, unlinked (an author in brackets); it must be the title.
        let html =
            page(r#"<p>Genus: <a href="/wiki/X" title="X">X</a><br />Species: <i><b>X y (Smith, 1900)</b></i></p>"#);
        assert_eq!(classify("X y", &html), taxon("X y", "species", "Q7432", Some("X"), false));
        let html = page(r#"<p>Genus: † <i><a href="/wiki/X" title="X">X</a></i><br /></p><p>Species: †<i>X z</i></p>"#);
        assert_eq!(classify("X z", &html), taxon("X z", "species", "Q7432", Some("X"), true));
        assert_eq!(classify("X q", &html), PageKind::Other);
        // A homonym, with its kingdom in brackets; a subgenus.
        let html = page(
            r#"<p>Tribus: <a href="/wiki/Bignonieae" title="Bignonieae">Bignonieae</a> <br />
Genus: <i><a class="mw-selflink selflink">Cuspidaria</a></i> <br /></p>"#,
        );
        assert_eq!(
            classify("Cuspidaria (Plantae)", &html),
            taxon("Cuspidaria", "genus", "Q34740", Some("Bignonieae"), false)
        );
        let html = page(
            r#"<p>Genus: <i><a href="/wiki/Chrysolina" title="Chrysolina">Chrysolina</a></i><br />
Subgenus: <i><a class="mw-selflink selflink">Chrysolina (Colaphoptera)</a></i> <br /></p>"#,
        );
        assert_eq!(
            classify("Chrysolina (Colaphoptera)", &html),
            taxon("Colaphoptera", "subgenus", "Q3238261", Some("Chrysolina"), false)
        );
    }

    #[test]
    fn abbreviations_and_layouts() {
        // No heading; the rank on its own line; an <abbr>.
        let html = r#"<table><tr><th>Taxonavigation: <a href="/wiki/Distomatida" title="Distomatida">Distomatida</a></th></tr></table>
<p>Genus: † <i><a href="/wiki/Hexamita" class="mw-redirect" title="Hexamita">Hexamita</a></i><br />
Species:
<i><a class="mw-selflink selflink"><abbr title="Hexamita">H</abbr>.&#160;intestinalis</a></i><br /></p>
<div class="mw-heading mw-heading2"><h2 id="Name">Name</h2></div>"#;
        assert_eq!(
            classify("Hexamita intestinalis", html),
            taxon("Hexamita intestinalis", "species", "Q7432", Some("Hexamita"), true)
        );
        let html = page(
            r#"<p>Species: <a href="/wiki/Inula_ciliaris" title="Inula ciliaris">I. ciliaris</a><br />
Varietas: <i><a class="mw-selflink selflink">I. c. var. ciliaris</a></i></p>"#,
        );
        assert_eq!(
            classify("Inula ciliaris var. ciliaris", &html),
            taxon("Inula ciliaris var. ciliaris", "varietas", "Q767728", Some("Inula ciliaris"), false)
        );
        assert!(abbreviates("I. c. var. ciliaris", "Inula ciliaris var. ciliaris"));
        assert!(!abbreviates("Inula ciliaris", "Inula ciliaris"));
        assert!(!abbreviates("X. ciliaris", "Inula ciliaris"));
    }

    #[test]
    fn other_kinds() {
        let html = page(
            r#"<p>Genus: <a href="/wiki/X" title="X">X</a><br />Grex: <i><a class="mw-selflink selflink">X y</a></i></p>"#,
        );
        assert_eq!(classify("X y", &html), PageKind::UnknownRank("grex".to_string()));
        assert_eq!(classify("Some person", "<p>A botanist.</p>"), PageKind::Other);
        assert_eq!(classify("Syzygium of Sumatra, the free-petalled species", &page("")), PageKind::Other);
        assert_eq!(
            classify("X", r#"<a href="/wiki/File:Disambig.svg" class="mw-file-description">"#),
            PageKind::Disambiguation
        );
        assert_eq!(
            classify("J. Bot.", "<ul><li>BPH Index: 4711</li></ul>"),
            PageKind::Journal { bph: "4711".to_string() }
        );
        let work = r#"<ul><li>Title: <i><b>Flora Sinensis</b></i></li><li>Abbreviation: <i><b>Fl. Sin.</b></i></li></ul>
<a rel="nofollow" class="external text" href="https://www.ipni.org/p/123-2">IPNI</a>"#;
        assert_eq!(
            classify("Fl. Sin.", work),
            PageKind::WrittenWork {
                title: "Flora Sinensis".to_string(),
                ipni: Some("123-2".to_string()),
                abbreviation: Some("Fl. Sin.".to_string())
            }
        );
    }
}
