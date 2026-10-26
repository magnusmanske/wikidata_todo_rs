//! Edits on Wikidata as the logged-in user, as the PHP `MW_OAuth` made them:
//! retried while the database lags, with a pause after each edit, and the
//! calling tool's hashtag in the summary.

use super::claims::ClaimValue;
use crate::oauth::{OAuth, Token};
use serde_json::{Map, Value, json};
use std::time::Duration;

const API: &str = "https://www.wikidata.org/w/api.php";
/// `maxlag` of edits, and the seconds to wait when the API says it lags.
const MAXLAG: u64 = 5;
const MAXLAG_TRIES: usize = 5;
/// The login check is repeated while the API gives no usable answer.
const USERINFO_TRIES: u32 = 3;
const USERINFO_DELAY: Duration = Duration::from_millis(300);
/// Hashtags that become change tags instead of a hashtag in the summary.
const TAGS: &[&str] = &["distributed-game"];

/// The pause after an edit, so that no tool edits faster than the PHP let it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    None,
    Edit,
    Create,
}

/// Why an action failed; `transient` failures are worth repeating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
    pub transient: bool,
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self { message: message.to_string(), transient: false }
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self { message, transient: false }
    }
}

/// Where a statement value goes: a new statement on an entity, or a qualifier of a statement.
#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    Entity(&'a str),
    Qualifier(&'a str),
}

pub struct Editor<'a> {
    oauth: &'a OAuth,
    token: Option<Token>,
    hashtag: Option<String>,
    csrf: Option<String>,
    http: &'a reqwest::Client,
    /// The API's answer to the last edit, which callers read (`res`), e.g. for a new item's ID.
    pub last: Option<Value>,
    /// What the API said went wrong (`error2`).
    pub error: String,
}

impl<'a> Editor<'a> {
    pub fn new(oauth: &'a OAuth, http: &'a reqwest::Client, token: Option<Token>, hashtag: &str) -> Self {
        let hashtag = Some(hashtag.trim()).filter(|h| is_hashtag(h)).map(str::to_string);
        Self { oauth, token, hashtag, csrf: None, http, last: None, error: String::new() }
    }

    /// One API request, repeated (up to a point) while the database lags.
    /// `None` if the API did not answer at all.
    async fn call(&self, params: &[(&str, &str)]) -> Option<Value> {
        let Some(token) = &self.token else {
            // Without a login, what the API would answer.
            return Some(json!({"error": {"code": "mwoauth-invalid-authorization", "info": "Not logged in"}}));
        };
        let reading = params.contains(&("meta", "userinfo"));
        let mut maxlag = if reading { 99999 } else { MAXLAG };
        for _ in 0..MAXLAG_TRIES {
            let lag = maxlag.to_string();
            let mut all = params.to_vec();
            all.push(("maxlag", &lag));
            let answer = match self.oauth.api(API, Some(token), &all).await {
                Ok(answer) => answer,
                Err(e) => {
                    tracing::warn!("Widar API request: {e:#}");
                    return None;
                }
            };
            if answer["error"]["code"] != "maxlag" {
                return Some(answer);
            }
            tokio::time::sleep(Duration::from_secs(MAXLAG)).await;
            // As the PHP: next time, accept the lag there is.
            maxlag = answer["error"]["lag"].as_f64().map_or(MAXLAG, |lag| lag.max(0.0).ceil() as u64) + MAXLAG;
        }
        None
    }

    /// Who the user is (`query.userinfo`). A failed request says nothing
    /// about the login, so it is repeated: a blip must not look like being logged out.
    pub async fn userinfo(&self, uiprop: Option<&str>) -> Option<Value> {
        let mut params = vec![("action", "query"), ("meta", "userinfo")];
        params.extend(uiprop.map(|p| ("uiprop", p)));
        let mut answer = None;
        for attempt in 0..USERINFO_TRIES {
            if attempt > 0 {
                tokio::time::sleep(USERINFO_DELAY * attempt).await;
            }
            answer = self.call(&params).await;
            if answer.as_ref().is_some_and(|a| a["query"].get("userinfo").is_some() || is_logged_out(a)) {
                break;
            }
        }
        answer
    }

    /// Fails unless the user is logged in; `authorize` is where to log in.
    pub async fn ensure_auth(&self, authorize: &str) -> Result<(), Failure> {
        let answer = self.userinfo(None).await.unwrap_or_default();
        if is_logged_out(&answer) {
            return Err(Failure::from(format!(
                "You haven't authorized this application yet! Go <a target=\"_blank\" href=\"{}\">here</a> to do that, then reload this page.",
                wikimisc::escape::html(authorize)
            )));
        }
        let userinfo = &answer["query"]["userinfo"];
        if userinfo.is_null() {
            return Err(Failure {
                message: "The Wikidata API did not answer the login check; this is temporary, please try again."
                    .to_string(),
                transient: true,
            });
        }
        match userinfo.get("anon") {
            Some(_) => Err("Not logged in. (How did that happen?)".into()),
            None => Ok(()),
        }
    }

    /// The edit token, fetched once per request.
    async fn csrf(&mut self) -> Option<String> {
        if self.csrf.is_none() {
            let answer = self.call(&[("action", "query"), ("meta", "tokens")]).await;
            let token = answer.as_ref().and_then(|a| a["query"]["tokens"]["csrftoken"].as_str());
            match token {
                Some(token) => self.csrf = Some(token.to_string()),
                None => self.error = format!("Bad API response: {}", answer.unwrap_or_default()),
            }
        }
        self.csrf.clone()
    }

    /// An edit (`params` with `action`), with token, bot flag and summary;
    /// whether it worked. The answer is kept in [`Self::last`].
    async fn edit(&mut self, params: Vec<(String, String)>, summary: &str, pause: Pause) -> bool {
        let Some(token) = self.csrf().await else { return false };
        let mut params = params;
        params.extend([("token".to_string(), token), ("bot".to_string(), "1".to_string())]);
        self.add_hashtag(&mut params, summary);
        let fields: Vec<(&str, &str)> = params.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let answer = self.call(&fields).await;
        let error = match &answer {
            None => Some("No answer from the Wikidata API".to_string()),
            Some(answer) => answer.get("error").map(|e| e["info"].as_str().unwrap_or("Unknown API error").to_string()),
        };
        self.last = answer;
        if let Some(error) = error {
            self.error = error;
            return false;
        }
        match pause {
            Pause::None => {}
            Pause::Edit => tokio::time::sleep(Duration::from_secs(1)).await,
            Pause::Create => tokio::time::sleep(Duration::from_secs(2)).await,
        }
        true
    }

    /// The tool's hashtag: a change tag if there is one by that name, else `#tag` in the summary.
    fn add_hashtag(&self, params: &mut Vec<(String, String)>, summary: &str) {
        let given = params.iter().position(|(k, _)| k == "summary").map(|i| params.remove(i).1);
        let mut summary = Some(summary.trim()).filter(|s| !s.is_empty()).map(str::to_string).or(given);
        match &self.hashtag {
            Some(tag) if TAGS.contains(&tag.as_str()) => match params.iter_mut().find(|(k, _)| k == "tags") {
                Some((_, tags)) => tags.push_str(&format!("|{tag}")),
                None => params.push(("tags".to_string(), tag.clone())),
            },
            Some(tag) => summary = Some(summary.map_or_else(|| format!("#{tag}"), |s| format!("{s} #{tag}"))),
            None => {}
        }
        params.extend(summary.map(|s| ("summary".to_string(), s)));
    }

    /// `wbsetlabel` or `wbsetdescription`.
    pub async fn set_term(&mut self, action: &str, id: &str, language: &str, value: &str) -> bool {
        let params = owned(&[("action", action), ("id", id), ("language", language), ("value", value)]);
        self.edit(params, "", Pause::Edit).await
    }

    /// `mode` is `add`, `set` or `remove`.
    pub async fn set_alias(&mut self, id: &str, language: &str, text: &str, mode: &str) -> bool {
        let params = owned(&[("action", "wbsetaliases"), ("id", id), ("language", language), (mode, text)]);
        self.edit(params, "", Pause::Edit).await
    }

    pub async fn set_sitelink(&mut self, id: &str, site: &str, title: &str) -> bool {
        let params = owned(&[("action", "wbsetsitelink"), ("id", id), ("linksite", site), ("linktitle", title)]);
        self.edit(params, "", Pause::Edit).await
    }

    pub async fn create_item(&mut self, data: &Value) -> bool {
        let data = data.to_string();
        let params = owned(&[("action", "wbeditentity"), ("new", "item"), ("data", &data)]);
        self.edit(params, "", Pause::Create).await
    }

    pub async fn remove_claim(&mut self, id: &str, baserev: &str) -> bool {
        let mut params = owned(&[("action", "wbremoveclaims"), ("claim", id)]);
        if !baserev.is_empty() {
            params.push(("baserevid".to_string(), baserev.to_string()));
        }
        self.edit(params, "", Pause::Edit).await
    }

    /// Adds a reference; one that is there already counts as success.
    pub async fn set_source(&mut self, statement: &str, snaks: &str) -> bool {
        let params = owned(&[("action", "wbsetreference"), ("statement", statement), ("snaks", snaks)]);
        self.edit(params, "", Pause::Edit).await
            || self.last.as_ref().is_some_and(|a| a["error"]["code"] == "modification-failed")
    }

    pub async fn create_redirect(&mut self, from: &str, to: &str) -> bool {
        let params = owned(&[("action", "wbcreateredirect"), ("from", from), ("to", to)]);
        let hashtag = self.hashtag.take(); // The API takes no summary here.
        let done = self.edit(params, "", Pause::Create).await;
        self.hashtag = hashtag;
        done
    }

    pub async fn merge_items(&mut self, from: &str, to: &str) -> bool {
        let params = owned(&[
            ("action", "wbmergeitems"),
            ("fromid", from),
            ("toid", to),
            ("ignoreconflicts", "description|sitelink"),
        ]);
        self.edit(params, "", Pause::None).await
    }

    /// Any API action, given as a JSON object with `action`.
    pub async fn generic(&mut self, request: Map<String, Value>, summary: &str) -> bool {
        if !request.contains_key("action") {
            self.error = format!("No action in {}", Value::Object(request));
            return false;
        }
        let creates = request["action"] == "wbeditentity" && request.contains_key("new");
        let params = request
            .into_iter()
            .filter(|(k, _)| !["token", "format", "bot", "maxlag"].contains(&k.as_str()))
            .map(|(k, v)| match v {
                Value::String(s) => (k, s),
                other => (k, other.to_string()),
            })
            .collect();
        self.edit(params, summary, if creates { Pause::Create } else { Pause::Edit }).await
    }

    /// A statement or qualifier; one the entity has already is not added again.
    pub async fn set_claim(&mut self, target: Target<'_>, property: &str, value: &ClaimValue, summary: &str) -> bool {
        if let Target::Entity(id) = target
            && self.has_claim(id, property, value).await
        {
            return true;
        }
        let value = value.json().to_string();
        let mut params = owned(&[("snaktype", "value"), ("property", property), ("value", &value)]);
        params.extend(owned(&match target {
            Target::Entity(id) => [("action", "wbcreateclaim"), ("entity", id)],
            Target::Qualifier(claim) => [("action", "wbsetqualifier"), ("claim", claim)],
        }));
        self.edit(params, summary, Pause::Edit).await
    }

    async fn has_claim(&self, id: &str, property: &str, value: &ClaimValue) -> bool {
        match crate::wikidata::entity(self.http, id, "claims").await {
            Ok(Some(entity)) => value.is_on(&entity, property),
            _ => false,
        }
    }
}

/// An API answer that says the OAuth token identifies nobody.
fn is_logged_out(answer: &Value) -> bool {
    let code = answer["error"]["code"].as_str().unwrap_or_default();
    code.starts_with("mwoauth") || code.contains("invalid-authorization")
}

/// Client-side tools send their name (`undefined` if they have none).
fn is_hashtag(tag: &str) -> bool {
    (1..=64).contains(&tag.len())
        && tag != "undefined"
        && tag.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

fn owned(params: &[(&str, &str)]) -> Vec<(String, String)> {
    params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::Consumer;

    fn editor(hashtag: &str) -> (OAuth, reqwest::Client, String) {
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("oauth.ini");
        std::fs::write(&ini, "consumerKey=k\nconsumerSecret=s\nagent=a\n").unwrap();
        let http = reqwest::Client::new();
        (OAuth::new(Consumer::from_ini(&ini).unwrap(), http.clone()), http, hashtag.to_string())
    }

    fn tagged(hashtag: &str, params: &[(&str, &str)], summary: &str) -> Vec<(String, String)> {
        let (oauth, http, tag) = editor(hashtag);
        let editor = Editor::new(&oauth, &http, None, &tag);
        let mut params = owned(params);
        editor.add_hashtag(&mut params, summary);
        params
    }

    #[test]
    fn hashtags() {
        assert_eq!(tagged("relator2", &[], "Added"), owned(&[("summary", "Added #relator2")]));
        assert_eq!(tagged("relator2", &[], ""), owned(&[("summary", "#relator2")]));
        assert_eq!(tagged("", &[], ""), owned(&[]));
        assert_eq!(tagged("undefined", &[("summary", "Own")], ""), owned(&[("summary", "Own")]));
        assert_eq!(tagged("x", &[("summary", "Own")], ""), owned(&[("summary", "Own #x")]));
        assert_eq!(tagged("evil tag&", &[], "S"), owned(&[("summary", "S")]));
        assert_eq!(tagged("distributed-game", &[], "S"), owned(&[("tags", "distributed-game"), ("summary", "S")]));
        assert_eq!(tagged("distributed-game", &[("tags", "a")], ""), owned(&[("tags", "a|distributed-game")]));
    }

    #[tokio::test]
    async fn without_login() {
        let (oauth, http, tag) = editor("");
        let mut editor = Editor::new(&oauth, &http, None, &tag);
        let failure = editor.ensure_auth("/api.php?action=authorize&path=a\"b").await.unwrap_err();
        assert!(!failure.transient);
        assert!(failure.message.contains("href=\"/api.php?action=authorize&amp;path=a&quot;b\""));
        assert!(!editor.set_term("wbsetlabel", "Q1", "en", "x").await);
        assert!(editor.error.starts_with("Bad API response"));
        assert!(!editor.generic(Map::new(), "").await);
        assert_eq!(editor.error, "No action in {}");
    }
}
