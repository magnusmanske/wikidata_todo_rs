//! What a Wikispecies page is about, from its wikitext: for pages the HTML
//! does not tell about (see [`super::page`]), people and ISSN pages.

use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WikitextKind {
    /// Marked `{{disputed}}`: left alone.
    Disputed,
    /// A serial, by its ISSN (`ISSN 1435-3393`).
    Issn(String),
    Person(Person),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Person {
    /// Standard form of the name as a botanical author (P428).
    pub ipni: Option<String>,
    pub born: Option<i64>,
    pub died: Option<i64>,
    /// "flourished", the year someone is known to have been active.
    pub floruit: Option<i64>,
    /// Items of the occupations (P106) their categories name.
    pub occupations: Vec<&'static str>,
    /// `Family, Given`, as the page sorts.
    pub sort_key: Option<String>,
    /// The year of their first publication on the page, or their floruit.
    pub active_since: Option<i64>,
}

/// Categories of people by occupation, and the occupations' items.
const OCCUPATIONS: &[(&str, &str)] = &[
    ("Acarologists", "Q19798999"),
    ("Annelidologists", "Q38687772"),
    ("Arachnologists", "Q17344952"),
    ("Botanists", "Q2374149"),
    ("Carcinologists", "Q16868721"),
    ("Cnidariologists", "Q5137366"),
    ("Entomologists", "Q3055126"),
    ("Helminthologists", "Q27497422"),
    ("Herpetologists", "Q16271064"),
    ("Ichthyologists", "Q4205432"),
    ("Lepidopterists", "Q497294"),
    ("Lichenologists", "Q15924544"),
    ("Malacologists", "Q16271261"),
    ("Mammalogists", "Q16831394"),
    ("Myriapodologists", "Q27924982"),
    ("Nematologists", "Q21491495"),
    ("Ornithologists", "Q1225716"),
    ("Ostracodologists", "Q21484199"),
    ("Palaeontologists", "Q1662561"),
    ("Paleobotanists", "Q30059961"),
    ("Phycologists", "Q12358881"),
    ("Spongiologists", "Q26936818"),
    ("Zoologists", "Q350979"),
];

fn regex(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

static ISSN_TITLE: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"^ISSN (\d{4}-\d{3}[\dX])$"));
/// Serials and publications, which may look like people otherwise.
static SOURCE: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"(?i)\[\[Category:(?:Sources|ISSN|Journals|Series identifiers)\b"));
/// Only people have these.
static PERSON: LazyLock<Option<Regex>> = LazyLock::new(|| {
    regex(r"(?i)\{\{Tax(?:on|a)[_ ]authored|\[\[Category:Taxon[_ ]authorities|\[\[Category:[a-z]+?ists(?:\]\]|\|)")
});
static CATEGORY: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"\[\[Category:([^\]|]+)"));
static IPNI: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"(?i)\{\{IPNI standard form\|([^}|]+)"));
static SORT_KEY: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"\{\{DEFAULTSORT:([^}]+)\}\}"));
/// A publication, by a template named after authors and year: `{{Han et al., 2009a}}`.
static PUBLICATION_YEAR: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"\{\{[^{}|\n]+?,? (\d{4})[a-z]?\}\}"));
/// `'''Name''' (dates)` at the start of a line.
static LEAD_DATES: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"(?m)^'''[^'\n]+''' *\(([^)\n]+)\)"));
static LINK: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"\[\[(?:[^\]|]*\|)?([^\]]*)\]\]"));
static FLORUIT: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"^fl\. ?(\d{4})$"));
/// A year, possibly with month and day: `1833`, `August 8, 1833`, `8 Aug. 1833`.
static DATE: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^(?:(?:\d{1,2} )?[A-Z][a-z]+\.? (?:\d{1,2}, )?)?(\d{4})$"));

pub fn classify(title: &str, wikitext: &str) -> WikitextKind {
    let has = |r: &Option<Regex>| r.as_ref().is_some_and(|r| r.is_match(wikitext));
    if wikitext.contains("{{disputed}}") {
        return WikitextKind::Disputed;
    }
    if let Some(c) = ISSN_TITLE.as_ref().and_then(|r| r.captures(title)) {
        return WikitextKind::Issn(c[1].to_string());
    }
    if has(&SOURCE) || !has(&PERSON) {
        return WikitextKind::Other;
    }
    let first = |r: &Option<Regex>| r.as_ref()?.captures(wikitext).map(|c| c[1].trim().to_string());
    let (born, died, floruit) = lead_dates(wikitext).unwrap_or_default();
    let published = PUBLICATION_YEAR.as_ref().into_iter().flat_map(|r| r.captures_iter(wikitext));
    let published = published.filter_map(|c| c[1].parse::<i64>().ok()).min();
    WikitextKind::Person(Person {
        ipni: first(&IPNI).filter(|i| !i.is_empty()),
        born,
        died,
        floruit,
        occupations: occupations(wikitext),
        sort_key: first(&SORT_KEY),
        active_since: published.into_iter().chain(floruit).min(),
    })
}

fn occupations(wikitext: &str) -> Vec<&'static str> {
    let Some(category) = CATEGORY.as_ref() else { return vec![] };
    let categories: Vec<String> =
        category.captures_iter(wikitext).map(|c| c[1].trim().replace('_', " ").to_lowercase()).collect();
    OCCUPATIONS.iter().filter(|(name, _)| categories.contains(&name.to_lowercase())).map(|(_, item)| *item).collect()
}

/// Birth, death, and floruit years from the bracket after the name in bold
/// that pages begin with: `(1915–1984)`, `(1936– )`, `(fl. 1994)`. Anything
/// vague (`c. 1800`, `1800s`, `?`) gives none.
fn lead_dates(wikitext: &str) -> Option<(Option<i64>, Option<i64>, Option<i64>)> {
    let dates = &LEAD_DATES.as_ref()?.captures(wikitext)?[1];
    let dates = LINK.as_ref()?.replace_all(dates, "$1");
    let dates = dates.split_whitespace().collect::<Vec<_>>().join(" ");
    let year = |c: regex::Captures| c[1].parse::<i64>().ok();
    if let Some(c) = FLORUIT.as_ref()?.captures(&dates) {
        return Some((None, None, year(c)));
    }
    let (from, to) = dates.split_once(['–', '—', '-'])?;
    let date = |s: &str| -> Option<Option<i64>> {
        match s.trim() {
            "" => Some(None),
            s => DATE.as_ref()?.captures(s).map(year),
        }
    };
    let (born, died) = (date(from)?, date(to)?);
    let this_year = i64::from(chrono::Datelike::year(&chrono::Utc::now()));
    let plausible = |y: Option<i64>| y.is_none_or(|y| (1500..=this_year).contains(&y));
    let ordered = match (born, died) {
        (Some(b), Some(d)) => b < d && d - b <= 110,
        (None, None) => false,
        _ => true,
    };
    (plausible(born) && plausible(died) && ordered).then_some((born, died, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(wikitext: &str) -> Person {
        match classify("Liu-Fu Han", wikitext) {
            WikitextKind::Person(p) => p,
            other => panic!("not a person: {other:?}"),
        }
    }

    #[test]
    fn people() {
        let p = person(
            "'''Liu-Fu Han''' (1965– ), Chinese lichenologist.\n{{IPNI standard form|L.F.Han}}\n\
             {{Taxa authored}}\n{{Han et al., 2015b}}\n{{Han, Guo & Wei, 2009a}}\n{{DEFAULTSORT:Han, Liu Fu}}\n\
             [[Category:Taxon authorities]]\n[[Category:Lichenologists]]\n[[Category:Mycologists]]",
        );
        let expected = Person {
            ipni: Some("L.F.Han".to_string()),
            born: Some(1965),
            occupations: vec!["Q15924544"],
            sort_key: Some("Han, Liu Fu".to_string()),
            active_since: Some(2009),
            ..Default::default()
        };
        assert_eq!(p, expected);
        let p = person("'''A B''' ([[fl.]] 1994), lichenologist.\n{{IPNI standard form}}\n[[Category:Botanists|B]]");
        assert_eq!((p.ipni, p.floruit, p.occupations), (None, Some(1994), vec!["Q2374149"]));
        let p = person("'''A B''' (August 8, 1833 – October 2, 1865), explorer.\n[[Category:Taxon authorities]]");
        assert_eq!((p.born, p.died), (Some(1833), Some(1865)));
        let p = person("'''A B''' ([[Floruit|fl.]] 1901)\n[[Category:Taxon_authorities]]");
        assert_eq!(p.floruit, Some(1901));
        for vague in ["(c. 1800–1870)", "(1800s)", "(1870–1850)", "(born 1950)", "(–)"] {
            let p = person(&format!("'''A B''' {vague}\n[[Category:Entomologists]]"));
            assert_eq!((p.born, p.died, p.floruit), (None, None, None), "{vague}");
        }
    }

    #[test]
    fn other_pages() {
        assert_eq!(
            classify("ISSN 1673-274X", "{{Authority control}}\n[[Category:ISSN]]"),
            WikitextKind::Issn("1673-274X".to_string())
        );
        assert_eq!(classify("X", "'''X''' {{disputed}}"), WikitextKind::Disputed);
        // Journals have authority control, as people do; but no person has only that.
        assert_eq!(classify("J. Bot.", "{{Authority control}}\n[[Category:Journals]]"), WikitextKind::Other);
        assert_eq!(classify("Some Body", "{{Authority control}}"), WikitextKind::Other);
        assert_eq!(classify("Some Body", "{{Taxa authored}}\n[[Category:Sources]]"), WikitextKind::Other);
    }
}
