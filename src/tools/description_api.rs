//! `description_api.php`: an automatic one-sentence description of an item,
//! in English, as JSON(P). People: "Douglas Adams was a British playwright,
//! screenwriter, and novelist."; anything else: its classes and makers.
//! Labels come in the requested language where there is one.

use crate::state::AppState;
use crate::web::{AppError, Params, api_json};
use crate::wikidata::{self, parse_id};
use axum::extract::State;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Label languages after the requested one; `mul` is the label for all languages.
const LANGUAGES: &[&str] = &["mul", "en", "de", "fr", "es", "it", "pl", "nl"];
/// Statements describing other items: what they are, then who made them.
const GENERIC: &[&[&str]] = &[&["P279", "P31", "P60", "P105"], &["P175", "P86", "P170", "P57", "P50", "P61"]];

const NATIONALITY: &[(&str, &str)] = &[
    ("Ecuador", "Ecuadorian"),
    ("Ghana", "Ghanaian"),
    ("Russia", "Russian"),
    ("Argentina", "Argentine"),
    ("Australia", "Australian"),
    ("Austria", "Austrian"),
    ("Belgium", "Belgian"),
    ("Brazil", "Brazilian"),
    ("Canada", "Canadian"),
    ("Chile", "Chilean"),
    ("China", "Chinese"),
    ("Denmark", "Danish"),
    ("Finland", "Finnish"),
    ("Faroe Islands", "Faroese"),
    ("Netherlands", "Dutch"),
    ("Puerto Rico", "Puerto Rican"),
    ("France", "French"),
    ("Luxembourg", "Luxembourgish"),
    ("Germany", "German"),
    ("Greece", "Greek"),
    ("Holland", "Dutch"),
    ("Hungary", "Hungarian"),
    ("Iceland", "Icelander"),
    ("India", "Indian"),
    ("Iran", "Iranian"),
    ("Iraq", "Iraqi"),
    ("Ireland", "Irish"),
    ("Israel", "Israeli"),
    ("Indonesia", "Indonesian"),
    ("Italy", "Italian"),
    ("Japan", "Japanese"),
    ("Jamaica", "Jamaican"),
    ("Jordan", "Jordanian"),
    ("Mexico", "Mexican"),
    ("Nepal", "Nepalese"),
    ("New Zealand", "New Zealander"),
    ("Norway", "Norwegian"),
    ("Pakistan", "Pakistani"),
    ("Paraguay", "Paraguayan"),
    ("Peru", "Peruvian"),
    ("Poland", "Polish"),
    ("Romania", "Romanian"),
    ("Scotland", "Scottish"),
    ("South Africa", "South African"),
    ("Spain", "Spanish"),
    ("Switzerland", "Swiss"),
    ("Syria", "Syrian"),
    ("Thailand", "Thai"),
    ("Turkey", "Turkish"),
    ("USA", "American"),
    ("Uruguay", "Uruguayan"),
    ("Venezuela", "Venezuelan"),
    ("Wales", "Welsh"),
    ("United Kingdom", "British"),
    ("United States of America", "US-American"),
    ("Sweden", "Swedish"),
];

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    q: String,
    #[serde(default)]
    mode: String,
    #[serde(default = "default_language")]
    lang: String,
    callback: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

#[derive(Debug, Default, Serialize)]
struct Answer {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    short_desc: Option<String>,
}

/// How items appear in the description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `Douglas Adams`
    Text,
    /// `[[Q42|Douglas Adams]]`
    Wikidata,
    /// `Douglas Adams (Q42)`
    WithId,
}

impl Mode {
    fn parse(mode: &str) -> Self {
        match mode {
            "" | "text" => Self::Text,
            "wikidata" => Self::Wikidata,
            _ => Self::WithId,
        }
    }
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let Some(q) = parse_id(&request.q, 'Q') else {
        let answer = Answer { status: "No valid item passed".to_string(), ..Default::default() };
        return Ok(api_json(answer, request.callback));
    };
    let Some(item) = wikidata::entity(&state.http, &q, "labels|claims").await? else {
        let answer = Answer { status: format!("No such item: {q}"), q: Some(q), ..Default::default() };
        return Ok(api_json(answer, request.callback));
    };
    let mut languages = vec![request.lang.as_str()];
    languages.extend(LANGUAGES);
    let mut describer =
        Describer { id: &q, item: &item, others: HashMap::new(), languages, mode: Mode::parse(&request.mode) };
    describer.others = wikidata::entities(&state.http, &describer.needed(), "labels").await?;
    let answer = Answer {
        status: "OK".to_string(),
        label: Some(describer.label(&q)),
        short_desc: Some(describer.describe()),
        q: Some(q.clone()),
        language: Some(request.lang.clone()),
    };
    Ok(api_json(answer, request.callback))
}

struct Describer<'a> {
    id: &'a str,
    item: &'a Value,
    /// The entities the description names, with their labels.
    others: HashMap<String, Value>,
    languages: Vec<&'a str>,
    mode: Mode,
}

impl Describer<'_> {
    /// The entities whose labels the description needs.
    fn needed(&self) -> Vec<String> {
        let mut ids: Vec<String> = match self.is_human() {
            true => ["P106", "P21", "P27"].iter().flat_map(|p| self.targets(p)).collect(),
            false => GENERIC
                .iter()
                .flat_map(|group| group.iter())
                .filter(|p| self.has(p))
                .flat_map(|p| std::iter::once(p.to_string()).chain(self.targets(p)))
                .collect(),
        };
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    fn describe(&self) -> String {
        match self.is_human() {
            true => self.describe_person(),
            false => self.describe_thing(),
        }
    }

    fn describe_person(&self) -> String {
        let mut objects = self.targets("P106");
        if objects.is_empty() {
            objects = self.targets("P21").into_iter().last().into_iter().collect();
        }
        if objects.is_empty() {
            objects = vec!["Q5".to_string()];
        }
        let mut words: Vec<String> = self.targets("P27").iter().map(|country| self.nationality(country)).collect();
        words.push(collapse(objects.iter().map(|o| self.link(o, None)).collect()));
        let object = words.join(" ");
        let verb = if self.has("P570") { "was" } else { "is" };
        let starts_with_vowel = object.trim().chars().next().is_some_and(|c| "aeiouAEIOU".contains(c));
        let article = if starts_with_vowel { "an" } else { "a" };
        format!("{} {verb} {article} {object}.", self.link(self.id, None))
    }

    /// Groups of "property values" clauses, `;` between groups.
    fn describe_thing(&self) -> String {
        let groups: Vec<String> = GENERIC
            .iter()
            .map(|group| {
                let clauses: Vec<String> = group.iter().filter_map(|p| self.clause(p)).collect();
                clauses.join(", ")
            })
            .filter(|clause| !clause.is_empty())
            .collect();
        match groups.is_empty() {
            true => String::new(),
            false => format!("{}.", groups.join("; ")),
        }
    }

    /// `director Alice and Bob`; for "instance of", just the classes.
    fn clause(&self, property: &str) -> Option<String> {
        let targets = self.targets(property);
        if targets.is_empty() {
            return None;
        }
        let objects = collapse(targets.iter().map(|t| self.link(t, None)).collect());
        Some(match property {
            "P31" => objects,
            _ => format!("{} {objects}", self.label(property)),
        })
    }

    /// The country as an adjective ("British"), if known.
    fn nationality(&self, country: &str) -> String {
        let label = self.label(country);
        let adjective = NATIONALITY.iter().find(|(name, _)| *name == label).map(|(_, adjective)| *adjective);
        self.link(country, adjective)
    }

    fn is_human(&self) -> bool {
        self.targets("P31").iter().any(|q| q == "Q5")
    }

    fn has(&self, property: &str) -> bool {
        self.item["claims"][property].as_array().is_some_and(|claims| !claims.is_empty())
    }

    /// The items the item's statements for `property` point to, each once.
    fn targets(&self, property: &str) -> Vec<String> {
        let mut targets: Vec<String> = vec![];
        let ids = self.item["claims"][property]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|claim| claim["mainsnak"]["datavalue"]["value"]["id"].as_str());
        for id in ids {
            if !targets.iter().any(|t| t == id) {
                targets.push(id.to_string());
            }
        }
        targets
    }

    /// The label in the first language that has one, else in any, else the ID.
    fn label(&self, id: &str) -> String {
        let entity = if id == self.id { Some(self.item) } else { self.others.get(id) };
        let Some(entity) = entity else { return id.to_string() };
        wikimisc::labels::json_term_or_any(entity, "labels", &self.languages)
            .map_or_else(|| id.to_string(), str::to_string)
    }

    fn link(&self, id: &str, text: Option<&str>) -> String {
        let text = text.map_or_else(|| self.label(id).trim().to_string(), str::to_string);
        let text = if text.is_empty() { format!("({id})") } else { text };
        match self.mode {
            Mode::Text => text,
            Mode::Wikidata => format!("[[{id}|{text}]]"),
            Mode::WithId => format!("{text} ({id})"),
        }
    }
}

/// `a`, `a and b`, `a, b, and c`.
fn collapse(mut list: Vec<String>) -> String {
    match list.len() {
        2 => list.join(" and "),
        n if n > 2 => {
            if let Some(last) = list.last_mut() {
                *last = format!("and {last}");
            }
            list.join(", ")
        }
        _ => list.join(""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;
    use serde_json::json;

    fn claims(pairs: &[(&str, &str)]) -> Value {
        let mut claims = serde_json::Map::new();
        for (property, target) in pairs {
            let claim = json!({"mainsnak": {"snaktype": "value", "datavalue": {"value": {"id": target}}}});
            claims.entry(property.to_string()).or_insert_with(|| json!([])).as_array_mut().unwrap().push(claim);
        }
        Value::Object(claims)
    }

    fn labelled(id: &str, label: &str) -> (String, Value) {
        (id.to_string(), json!({"labels": {"en": {"language": "en", "value": label}}}))
    }

    fn describe(item: Value, others: &[(&str, &str)], mode: Mode) -> String {
        let others = others.iter().map(|(id, label)| labelled(id, label)).collect();
        Describer { id: "Q42", item: &item, others, languages: vec!["en"], mode }.describe()
    }

    #[test]
    fn lists() {
        let list = |items: &[&str]| collapse(items.iter().map(|s| s.to_string()).collect());
        assert_eq!(list(&[]), "");
        assert_eq!(list(&["a"]), "a");
        assert_eq!(list(&["a", "b"]), "a and b");
        assert_eq!(list(&["a", "b", "c"]), "a, b, and c");
    }

    #[test]
    fn people() {
        let item = json!({"labels": {"en": {"value": "Douglas Adams"}}, "claims": claims(&[
            ("P31", "Q5"), ("P27", "Q145"), ("P106", "Q1"), ("P106", "Q2"), ("P106", "Q1"), ("P570", "Q9"),
        ])});
        let others = [("Q145", "United Kingdom"), ("Q1", "playwright"), ("Q2", "novelist")];
        assert_eq!(describe(item.clone(), &others, Mode::Text), "Douglas Adams was a British playwright and novelist.");
        assert_eq!(
            describe(item, &others, Mode::Wikidata),
            "[[Q42|Douglas Adams]] was a [[Q145|British]] [[Q1|playwright]] and [[Q2|novelist]]."
        );
        let item = json!({"labels": {}, "claims": claims(&[("P31", "Q5"), ("P21", "Q6581072"), ("P27", "Q1")])});
        let others = [("Q1", "Atlantis"), ("Q6581072", "female")];
        assert_eq!(describe(item, &others, Mode::WithId), "Q42 (Q42) is an Atlantis (Q1) female (Q6581072).");
    }

    #[test]
    fn other_things() {
        let item = json!({"claims": claims(&[("P31", "Q11424"), ("P57", "Q1"), ("P57", "Q2"), ("P50", "Q3")])});
        let others = [("Q11424", "film"), ("P57", "director"), ("Q1", "A"), ("Q2", "B"), ("P50", "author")];
        assert_eq!(describe(item, &others, Mode::Text), "film; director A and B, author Q3.");
        assert_eq!(describe(json!({"claims": {}}), &[], Mode::Text), "");
    }

    #[tokio::test]
    async fn invalid_item() {
        let dir = tempfile::tempdir().unwrap();
        let response = get(dir.path(), "/description_api.php?q=nonsense&callback=cb").await;
        assert_eq!(response.status.as_u16(), 200);
        assert_eq!(response.body, r#"cb({"status":"No valid item passed"});"#);
    }
}
