//! Statement values as Widar's actions take them, for `wbcreateclaim` and
//! `wbsetqualifier`. Parsed first: the PHP pasted the parameters into JSON
//! as they were, so any of them could break out of the value.

use crate::wikidata::parse_id;
use serde_json::{Value, json};

const ENTITY: &str = "http://www.wikidata.org/entity/";
const GREGORIAN: &str = "http://www.wikidata.org/entity/Q1985727";
const EARTH: &str = "http://www.wikidata.org/entity/Q2";
/// Coordinates are set to the micro-degree, as the PHP did.
const COORDINATE_PRECISION: f64 = 0.000001;

#[derive(Debug, Clone, PartialEq)]
pub enum ClaimValue {
    Item(u64),
    String(String),
    Monolingual { text: String, language: String },
    Date { time: String, precision: u8 },
    Location { latitude: f64, longitude: f64 },
    Quantity { amount: String, upper: String, lower: String, unit: String },
}

impl ClaimValue {
    /// `Q42`, `q42` or `42`.
    pub fn item(target: &str) -> Result<Self, String> {
        let id = parse_id(target, 'Q').ok_or_else(|| format!("Invalid item '{target}'"))?;
        id.trim_start_matches('Q').parse().map(Self::Item).map_err(|_| format!("Invalid item '{target}'"))
    }

    /// `+1950-01-01T00:00:00Z` (the sign may be missing) and a precision (0-14; 11 is the day).
    pub fn date(date: &str, precision: &str) -> Result<Self, String> {
        let invalid = || format!("Invalid date '{date}' or precision '{precision}'");
        let date = date.trim();
        let time = if date.starts_with(['+', '-']) { date.to_string() } else { format!("+{date}") };
        let precision: u8 = precision.trim().parse().map_err(|_| invalid())?;
        (is_wikibase_time(&time) && precision <= 14).then_some(Self::Date { time, precision }).ok_or_else(invalid)
    }

    pub fn location(latitude: &str, longitude: &str) -> Result<Self, String> {
        let parse = |s: &str, max: f64| s.trim().parse::<f64>().ok().filter(|v| v.is_finite() && v.abs() <= max);
        match (parse(latitude, 90.0), parse(longitude, 360.0)) {
            (Some(latitude), Some(longitude)) => Ok(Self::Location { latitude, longitude }),
            _ => Err(format!("Invalid coordinates '{latitude}', '{longitude}'")),
        }
    }

    /// Bounds default to the amount; the unit is `1` (none), an item, or an item's URI.
    pub fn quantity(amount: &str, upper: &str, lower: &str, unit: &str) -> Result<Self, String> {
        let (upper, lower) = match (upper.trim(), lower.trim()) {
            ("", "") => (amount, amount),
            bounds => bounds,
        };
        let number = |s: &str| decimal(s).ok_or_else(|| format!("Invalid number '{s}'"));
        Ok(Self::Quantity {
            amount: number(amount)?,
            upper: number(upper)?,
            lower: number(lower)?,
            unit: quantity_unit(unit).ok_or_else(|| format!("Invalid unit '{unit}'"))?,
        })
    }

    /// The `value` of the API's `wbcreateclaim`/`wbsetqualifier`.
    pub fn json(&self) -> Value {
        match self {
            Self::Item(id) => json!({"entity-type": "item", "numeric-id": id}),
            Self::String(text) => json!(text),
            Self::Monolingual { text, language } => json!({"text": text, "language": language}),
            Self::Date { time, precision } => json!({
                "time": time, "timezone": 0, "before": 0, "after": 0,
                "precision": precision, "calendarmodel": GREGORIAN,
            }),
            Self::Location { latitude, longitude } => json!({
                "latitude": latitude, "longitude": longitude,
                "precision": COORDINATE_PRECISION, "globe": EARTH,
            }),
            Self::Quantity { amount, upper, lower, unit } => {
                json!({"amount": amount, "unit": unit, "upperBound": upper, "lowerBound": lower})
            }
        }
    }

    /// Whether `entity` (with its claims) already has this value for `property`,
    /// so that adding it again can be skipped. Coordinates never match, as in the PHP.
    pub fn is_on(&self, entity: &Value, property: &str) -> bool {
        let statements = entity["claims"][property].as_array().into_iter().flatten();
        statements.map(|s| &s["mainsnak"]["datavalue"]["value"]).any(|value| self.matches(value))
    }

    fn matches(&self, value: &Value) -> bool {
        match self {
            Self::Item(id) => value["numeric-id"].as_u64() == Some(*id),
            Self::String(text) => value.as_str() == Some(text),
            Self::Monolingual { text, language } => value["text"] == *text && value["language"] == *language,
            Self::Date { time, precision } => {
                value["time"] == *time && value["precision"].as_u64() == Some(u64::from(*precision))
            }
            Self::Location { .. } => false,
            Self::Quantity { amount, unit, .. } => {
                let number = |v: &Value| v.as_str().and_then(|s| s.parse::<f64>().ok());
                number(&value["amount"]).is_some_and(|a| amount.parse::<f64>().is_ok_and(|b| a == b))
                    && value["unit"] == *unit
            }
        }
    }
}

/// `+1950-01-01T00:00:00Z`: sign, year (up to 16 digits), month, day, and a zero time.
fn is_wikibase_time(time: &str) -> bool {
    let Some((year, rest)) = time.get(1..).and_then(|t| t.split_once('-')) else { return false };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let two_digits = |s: &str| s.len() == 2 && digits(s);
    let mut parts = rest.splitn(3, ['-', 'T']);
    let (month, day, clock) = (parts.next(), parts.next(), parts.next().and_then(|c| c.strip_suffix('Z')));
    year.len() <= 16
        && digits(year)
        && month.is_some_and(two_digits)
        && day.is_some_and(two_digits)
        && clock.is_some_and(|c| c.split(':').count() == 3 && c.split(':').all(two_digits))
}

/// A decimal number with a sign, as Wikibase writes amounts: `5` → `+5`.
fn decimal(s: &str) -> Option<String> {
    let s = s.trim();
    let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, "0"));
    let digits = |s: &str| !s.is_empty() && s.len() < 100 && s.bytes().all(|b| b.is_ascii_digit());
    (digits(whole) && digits(fraction)).then(|| if s.starts_with(['+', '-']) { s.to_string() } else { format!("+{s}") })
}

fn quantity_unit(unit: &str) -> Option<String> {
    match unit.trim() {
        "" | "1" => Some("1".to_string()),
        unit => {
            let item = parse_id(unit.strip_prefix(ENTITY).unwrap_or(unit), 'Q')?;
            Some(format!("{ENTITY}{item}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_as_json() {
        assert_eq!(ClaimValue::item("q42").unwrap().json(), json!({"entity-type": "item", "numeric-id": 42}));
        assert!(ClaimValue::item("42}, \"x\": {").is_err());
        let date = ClaimValue::date("1950-01-02T00:00:00Z", "11").unwrap();
        assert_eq!(date.json()["time"], "+1950-01-02T00:00:00Z");
        assert_eq!(date.json()["precision"], 11);
        assert!(ClaimValue::date("+1950-01-02T00:00:00Z\",\"x", "11").is_err());
        assert!(ClaimValue::date("+1950-01-02T00:00:00Z", "15").is_err());
        assert!(ClaimValue::date("1950", "9").is_err());
        let location = ClaimValue::location("51.5", "-0.12").unwrap().json();
        assert_eq!((location["latitude"].as_f64(), location["longitude"].as_f64()), (Some(51.5), Some(-0.12)));
        assert!(ClaimValue::location("91", "0").is_err());
        assert!(ClaimValue::location("1,\"x\":2", "0").is_err());
    }

    #[test]
    fn quantities() {
        let q = ClaimValue::quantity("5", "", "", "1").unwrap().json();
        assert_eq!(q, json!({"amount": "+5", "unit": "1", "upperBound": "+5", "lowerBound": "+5"}));
        let q = ClaimValue::quantity("-1.5", "-1", "-2", "Q11573").unwrap().json();
        assert_eq!(q["unit"], "http://www.wikidata.org/entity/Q11573");
        assert_eq!((q["upperBound"].as_str(), q["lowerBound"].as_str()), (Some("-1"), Some("-2")));
        assert!(ClaimValue::quantity("5e3", "", "", "1").is_err());
        assert!(ClaimValue::quantity("5", "", "", "kg").is_err());
    }

    #[test]
    fn existing_values() {
        let entity = json!({"claims": {
            "P31": [{"mainsnak": {"datavalue": {"value": {"entity-type": "item", "numeric-id": 5, "id": "Q5"}}}}],
            "P1477": [{"mainsnak": {"datavalue": {"value": {"text": "Ada", "language": "en"}}}}],
            "P1082": [{"mainsnak": {"datavalue": {"value": {"amount": "+5.0", "unit": "1"}}}}],
            "P569": [{"mainsnak": {"datavalue": {"value": {"time": "+1815-12-10T00:00:00Z", "precision": 11}}}}],
            "P214": [{"mainsnak": {"snaktype": "novalue"}}],
        }});
        assert!(ClaimValue::item("Q5").unwrap().is_on(&entity, "P31"));
        assert!(!ClaimValue::item("Q6").unwrap().is_on(&entity, "P31"));
        assert!(!ClaimValue::item("Q5").unwrap().is_on(&entity, "P279"));
        let name = ClaimValue::Monolingual { text: "Ada".into(), language: "en".into() };
        assert!(name.is_on(&entity, "P1477"));
        assert!(ClaimValue::quantity("5", "", "", "1").unwrap().is_on(&entity, "P1082"));
        assert!(ClaimValue::date("+1815-12-10T00:00:00Z", "11").unwrap().is_on(&entity, "P569"));
        assert!(!ClaimValue::date("+1815-12-10T00:00:00Z", "9").unwrap().is_on(&entity, "P569"));
        assert!(!ClaimValue::String("x".into()).is_on(&entity, "P214"));
    }
}
