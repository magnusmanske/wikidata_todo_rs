//! Whether an item may be the person a Wikispecies page is about: by name,
//! and by dates that do not contradict the page's.

use super::wikitext::Person;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// The youngest age at which someone publishes.
const MIN_AGE_PUBLISHING: i64 = 15;
/// How long after their death someone's work may still come out.
const MAX_POSTHUMOUS_YEARS: i64 = 5;
/// Years in which sources disagree about a birth or death.
const DATE_TOLERANCE: i64 = 1;

static DOI: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"href="https?://(?:dx\.)?doi\.org/(10\.[^"\s]+)""#).ok());

/// A person's name, split into family name and given names, as words
/// without case and accents; and the given names as written (`tianli` of
/// `Tian-Li`).
#[derive(Debug)]
pub struct Name {
    family: Vec<String>,
    given: Vec<String>,
    given_names: Vec<String>,
}

impl Name {
    /// The family name is the page's sort key (`Pease, Eleanor`) if the name
    /// ends with it, else the last word (`Moreno-Bernal`).
    pub fn new(name: &str, sort_key: Option<&str>) -> Self {
        let all = words(name);
        let family = sort_key
            .and_then(|key| key.split(',').next())
            .map(words)
            .filter(|family| !family.is_empty() && all.ends_with(family))
            .unwrap_or_else(|| name.split_whitespace().last().map(words).unwrap_or_default());
        let given = all.iter().filter(|w| !family.contains(w)).cloned().collect();
        let given_names = given_names(&family, name);
        Self { family, given, given_names }
    }

    /// Whether `label` may name the same person: it has the family name
    /// (or a part of it, but for particles: `Moreno` of `Moreno-Bernal`),
    /// and its given names, if any, agree with these in one word or initial
    /// (or all written together: `Tianli`, `Tian-Li`), in any order.
    pub fn may_be(&self, label: &str) -> bool {
        let label = words(label);
        let parts: Vec<&String> = self.family.iter().filter(|f| f.chars().count() > 2).collect();
        let family = match parts.is_empty() {
            true => self.family.iter().all(|f| label.contains(f)),
            false => parts.iter().any(|f| label.contains(f)),
        };
        if !family {
            return false;
        }
        let given: Vec<&String> = label.iter().filter(|w| !self.family.contains(w)).collect();
        if given.is_empty() || self.given.is_empty() {
            return true;
        }
        self.given.iter().any(|a| given.iter().any(|b| agree(a, b)))
            || self.given.concat() == given.iter().map(|s| s.as_str()).collect::<String>()
    }

    /// Whether `label` is this name: the whole family name, the first given
    /// name (or its initial), and no given name that is not theirs.
    pub fn is(&self, label: &str) -> bool {
        let words = words(label);
        if !self.family.iter().all(|f| words.contains(f)) {
            return false;
        }
        let given = given_names(&self.family, label);
        match (given.first(), self.given_names.first()) {
            (Some(first), Some(own)) => {
                agree(first, own) && given.iter().all(|g| self.given_names.iter().any(|own| agree(g, own)))
            }
            _ => false,
        }
    }
}

/// The names in `text` but for the `family` name, each as one word
/// (`tianli` of `Tian-Li`).
fn given_names(family: &[String], text: &str) -> Vec<String> {
    let whole = family.concat();
    let names = text.split_whitespace().map(|name| words(name).concat());
    names.filter(|n| !n.is_empty() && *n != whole && !family.contains(n)).collect()
}

/// Whether two given names agree: the same, or one is the other's initial.
fn agree(a: &str, b: &str) -> bool {
    a == b || (a.len() == 1 && b.starts_with(a)) || (b.len() == 1 && a.starts_with(b))
}

/// Lowercase words, without accents.
fn words(text: &str) -> Vec<String> {
    let plain: String = text.nfkd().filter(|c| !is_combining_mark(*c)).collect::<String>().to_lowercase();
    plain.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// Whether the item (`labels`, `aliases`, `claims`) may be the person:
/// a name that may be theirs, and no date that rules them out.
pub fn may_be(item: &Value, name: &Name, person: &Person) -> bool {
    names(item).any(|label| name.may_be(label)) && !dates_differ(item, person)
}

/// As [`may_be`], but the item must have their name.
pub fn is(item: &Value, name: &Name, person: &Person) -> bool {
    names(item).any(|label| name.is(label)) && !dates_differ(item, person)
}

fn names(item: &Value) -> impl Iterator<Item = &str> {
    let labels = item["labels"].as_object().into_iter().flat_map(|l| l.values());
    let aliases = item["aliases"]
        .as_object()
        .into_iter()
        .flat_map(|a| a.values())
        .flat_map(|a| a.as_array().into_iter().flatten());
    labels.chain(aliases).filter_map(|term| term["value"].as_str())
}

/// Whether the item's years of birth or death contradict the page: other
/// years than the page's, or a life that does not fit its publications.
fn dates_differ(item: &Value, person: &Person) -> bool {
    let (born, died) = (years(item, "P569"), years(item, "P570"));
    let other = |page: Option<i64>, item: &[i64]| {
        page.is_some_and(|p| !item.is_empty() && item.iter().all(|y| (y - p).abs() > DATE_TOLERANCE))
    };
    let active = person.active_since;
    other(person.born, &born)
        || other(person.died, &died)
        || active.is_some_and(|a| born.iter().any(|&b| b > a - MIN_AGE_PUBLISHING))
        || active.is_some_and(|a| died.iter().any(|&d| d < a - MAX_POSTHUMOUS_YEARS))
}

/// The years of the item's `property` dates, of year precision or better.
pub fn years(item: &Value, property: &str) -> Vec<i64> {
    let values = crate::wikidata::values(item, property);
    let years = values.iter().filter(|v| v["precision"].as_u64().is_none_or(|p| p >= 9));
    years.filter_map(|v| v["time"].as_str()?.get(1..)?.split('-').next()?.parse().ok()).collect()
}

/// The DOIs the page links to: of the person's publications.
pub fn dois(html: &str) -> Vec<String> {
    let Some(doi) = DOI.as_ref() else { return vec![] };
    let mut dois: Vec<String> = doi
        .captures_iter(html)
        .map(|c| percent_encoding::percent_decode_str(&c[1]).decode_utf8_lossy().replace("&amp;", "&"))
        .map(|d| d.trim_end_matches('.').to_uppercase())
        .collect();
    dois.sort();
    dois.dedup();
    dois
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_that_may_be_the_same() {
        let name = Name::new("Tian-Li Wei", Some("Wei, Tian-Li"));
        assert!(name.may_be("Wei Tianli"));
        assert!(name.may_be("T. Wei"));
        assert!(name.may_be("Wei"));
        assert!(!name.may_be("Wei Zhang"));
        assert!(!name.may_be("Tian-Li Wu"));
        let name = Name::new("Brayan Esteban Bolaños-Molina", Some("Bolaños-Molina, Brayan Esteban"));
        assert!(name.may_be("Bolanos-Molina, B. E."));
        // The sort key must be the end of the name: here it is a typo.
        let name = Name::new("Jorge W. Moreno-Bernal", Some("Moreno-Bernala, Jorge W."));
        assert!(name.may_be("Jorge Moreno"));
        assert!(!name.may_be("Jorge Bernala"));
        let name = Name::new("Maria Grazia De Simoni", Some("De Simoni, Maria Grazia"));
        assert!(name.may_be("M. G. De Simoni") && !name.may_be("Maria De Luca"));
        assert!(!Name::new("Luke Milan", None).may_be("Milan Lukeš"));
    }

    #[test]
    fn names_that_are_the_same() {
        let name = Name::new("Tian-Li Wei", Some("Wei, Tian-Li"));
        assert!(name.is("Tian-Li Wei") && name.is("Wei Tianli") && name.is("T. Wei"));
        assert!(!name.is("Tian Wei") && !name.is("Wei") && !name.is("Tian-Li Wu"));
        let name = Name::new("Brayan Esteban Bolaños-Molina", Some("Bolaños-Molina, Brayan Esteban"));
        assert!(
            name.is("Brayan E Bolaños-Molina") && name.is("Brayan Bolaños-Molina") && name.is("B. E. Bolanos-Molina")
        );
        assert!(
            !name.is("Esteban Bolaños-Molina")
                && !name.is("Brayan Bolaños")
                && !name.is("Brayan Carlos Bolaños-Molina")
        );
    }

    #[test]
    fn dates_that_rule_someone_out() {
        let item = |born: &str, died: &str| {
            let date = |t: &str| json!([{"rank": "normal", "mainsnak": {"snaktype": "value", "datavalue": {"value": {"time": t, "precision": 9}}}}]);
            let mut claims = serde_json::Map::new();
            if !born.is_empty() {
                claims.insert("P569".into(), date(born));
            }
            if !died.is_empty() {
                claims.insert("P570".into(), date(died));
            }
            json!({"labels": {"en": {"value": "Eleanor Pease"}}, "claims": claims})
        };
        let name = Name::new("Eleanor Pease", None);
        let publishing = Person { active_since: Some(2022), ..Default::default() };
        assert!(!may_be(&item("+1889-00-00T00:00:00Z", "+1978-00-00T00:00:00Z"), &name, &publishing));
        assert!(!may_be(&item("+2015-00-00T00:00:00Z", ""), &name, &publishing));
        assert!(may_be(&item("+1980-00-00T00:00:00Z", ""), &name, &publishing));
        assert!(may_be(&item("", "+2019-00-00T00:00:00Z"), &name, &publishing));
        let dated = Person { born: Some(1915), died: Some(1984), ..Default::default() };
        assert!(may_be(&item("+1916-00-00T00:00:00Z", ""), &name, &dated));
        assert!(!may_be(&item("+1920-00-00T00:00:00Z", ""), &name, &dated));
        assert!(may_be(&item("", ""), &name, &dated));
    }

    #[test]
    fn years_of_dates() {
        let item = json!({"claims": {"P569": [
            {"rank": "normal", "mainsnak": {"snaktype": "value", "datavalue": {"value": {"time": "+1833-08-08T00:00:00Z", "precision": 11}}}},
            {"rank": "normal", "mainsnak": {"snaktype": "value", "datavalue": {"value": {"time": "+1800-00-00T00:00:00Z", "precision": 7}}}},
            {"rank": "deprecated", "mainsnak": {"snaktype": "value", "datavalue": {"value": {"time": "+1834-00-00T00:00:00Z"}}}}
        ]}});
        assert_eq!(years(&item, "P569"), [1833]);
        assert!(years(&item, "P570").is_empty());
    }

    #[test]
    fn dois_of_publications() {
        let html = r#"<a href="https://doi.org/10.1080/02724634.2026.2666240">x</a> <a href="https://doi.org/10.1002%2Fspp2.1475">y</a>
            <a href="http://dx.doi.org/10.1080/02724634.2026.2666240">again</a>"#;
        assert_eq!(dois(html), ["10.1002/SPP2.1475", "10.1080/02724634.2026.2666240"]);
    }
}
