//! Widar (`api.php`, and `widar/index.php`): edits Wikidata as the logged-in
//! user, for the pages and apps of this site, which call it with `action`
//! and read `{"error": "OK", …}`. One login, in one cookie, for the whole site;
//! the apps' own APIs (`duplicity/api.php`, `relator/api.php`, …) pass on
//! what they do not know to [`respond`]. Without an action, a page shows who
//! is logged in.

mod claims;
mod editor;

use crate::oauth::Session;
use crate::state::AppState;
use crate::web::{Params, is_same_origin, render};
use crate::wiki::encode_component;
use askama::Template;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Json, Response};
use claims::ClaimValue;
use editor::{Editor, Failure, Target};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

const SESSION: Session = Session { name: "wdt_oauth", path: "/" };
/// Wikidata entities per `set_claims` request: each edit takes over a second.
const MAX_IDS: usize = 100;
/// The actions [`respond`] answers.
const ACTIONS: &[&str] = &[
    "authorize",
    "logout",
    "get_rights",
    "generic",
    "set_claims",
    "set_string",
    "set_monolang",
    "set_location",
    "set_date",
    "set_quantity",
    "set_label",
    "set_desc",
    "set_alias",
    "set_sitelink",
    "create_item_from_page",
    "create_blank_item",
    "add_source",
    "merge_items",
    "create_redirect",
    "remove_claim",
];
/// Wiki database names whose language code is not a label language.
const LABEL_LANGUAGES: &[(&str, &str)] = &[
    ("als", "gsw"),
    ("bat_smg", "sgs"),
    ("be_x_old", "be-tarask"),
    ("bh", "bho"),
    ("commons", "en"),
    ("fiu_vro", "vro"),
    ("mediawiki", "en"),
    ("meta", "en"),
    ("no", "nb"),
    ("roa_rup", "rup"),
    ("simple", "en"),
    ("species", "en"),
    ("wikidata", "en"),
    ("zh_classical", "lzh"),
    ("zh_min_nan", "nan"),
    ("zh_yue", "yue"),
];

/// How a login started at one endpoint comes back: MediaWiki calls
/// `callback` (as registered with the OAuth consumer), which then forwards
/// the user to `forward`. `authorize` is where to start one.
pub struct Login {
    pub callback: String,
    pub forward: String,
    pub authorize: String,
}

#[derive(Template)]
#[template(path = "tools/widar.html")]
struct Page {
    title: &'static str,
    user: Option<String>,
    message: Option<String>,
    script: String,
    back: Option<String>,
}

/// `/api.php` and `/widar/index.php`; `path` is where to go after logging in.
pub async fn handler(
    State(state): State<Arc<AppState>>,
    uri: Uri,
    headers: HeaderMap,
    Params(params): Params<HashMap<String, String>>,
) -> Response {
    let script = uri.path();
    let path = match params.get("path").map(|p| p.trim()) {
        Some(path) if !path.is_empty() => path,
        _ if script == "/widar/index.php" => "widar/index.php",
        _ => "",
    };
    let authorize = match path {
        "" => format!("{script}?action=authorize"),
        path => format!("{script}?action=authorize&path={}", encode_component(path)),
    };
    let login = Login {
        callback: format!("{}/api.php?path={}", state.config.public_url, encode_component(path)),
        forward: local_path(path),
        authorize,
    };
    if let Some(response) = respond(&state, &headers, &params, &login).await {
        return response;
    }
    match params.get("action").map(String::as_str).unwrap_or_default() {
        "" => status_page(&state, &headers, script).await,
        action => answer(Err(format!("Unknown action '{action}'").into()), None, String::new(), Extras::default()),
    }
}

/// Answers Widar's actions, and the end of a login; `None` for anything else.
pub async fn respond(
    state: &AppState,
    headers: &HeaderMap,
    params: &HashMap<String, String>,
    login: &Login,
) -> Option<Response> {
    let fields = Fields(params);
    let get = |key: &str| fields.get(key);
    if !get("oauth_verifier").is_empty() && !get("oauth_token").is_empty() {
        return Some(login_callback(state, headers, get("oauth_verifier"), get("oauth_token"), login).await);
    }
    let action = get("action");
    if !ACTIONS.contains(&action) {
        return None;
    }
    let Some(oauth) = &state.oauth else {
        return Some(match action {
            "authorize" => failure_page(StatusCode::SERVICE_UNAVAILABLE, NO_LOGIN, None),
            _ => answer(Err(NO_LOGIN.into()), None, String::new(), Extras::default()),
        });
    };
    let jar = state.cookies(headers);
    if action == "authorize" {
        return Some(match oauth.initiate(&login.callback).await {
            Ok((request_token, url)) => {
                (SESSION.with_request_token(jar, &request_token), redirect(&url)).into_response()
            }
            Err(e) => {
                tracing::warn!("OAuth authorization failed: {e:#}");
                failure_page(StatusCode::BAD_GATEWAY, "Wikimedia's login did not answer; please try again.", None)
            }
        });
    }
    // Changes only from this site: other sites could otherwise act in the user's name.
    if action != "get_rights" && !is_same_origin(headers) {
        let failure = Failure::from("Requests that change something must come from this site");
        return Some(answer(Err(failure), None, String::new(), Extras::default()));
    }
    if action == "logout" {
        let jar = SESSION.without_tokens(jar);
        return Some(match params.contains_key("botmode") {
            true => (jar, answer(Ok(()), None, String::new(), Extras::default())).into_response(),
            false => (jar, redirect(&login.forward)).into_response(),
        });
    }
    let mut editor = Editor::new(oauth, &state.http, SESSION.access_token(&jar), get("tool_hashtag"));
    let mut extras = Extras::default();
    let outcome = act(&mut editor, action, fields, &login.authorize, &mut extras).await;
    Some(answer(outcome, editor.last, editor.error, extras))
}

/// The request's parameters; missing ones are empty, as for PHP's `getRequest`.
#[derive(Clone, Copy)]
struct Fields<'a>(&'a HashMap<String, String>);

impl<'a> Fields<'a> {
    fn get(self, key: &str) -> &'a str {
        self.0.get(key).map_or("", String::as_str)
    }
}

const NO_LOGIN: &str = "Logging in is not available: this server has no OAuth consumer.";

/// What the JSON answer has besides the outcome.
#[derive(Default)]
struct Extras {
    /// `get_rights`: the user's rights.
    result: Option<Value>,
    /// `generic`: PHP's `json_last_error()` code of the request's JSON.
    jle: u8,
    /// A new item, which the item creator (`creator.html`) reads (the PHP never said).
    q: Option<String>,
}

/// PHP's `json_last_error()` for a syntax error.
const JSON_ERROR_SYNTAX: u8 = 4;

/// One action, with its parameters (`get`), as `Widar::process_request`.
async fn act(
    editor: &mut Editor<'_>,
    action: &str,
    fields: Fields<'_>,
    authorize: &str,
    extras: &mut Extras,
) -> Result<(), Failure> {
    let get = |key: &str| fields.get(key);
    if action == "get_rights" {
        extras.result = editor.userinfo(Some("blockinfo|groups|rights")).await;
        return Ok(());
    }
    editor.ensure_auth(authorize).await?;
    let done = |ok: bool, failure: String| if ok { Ok(()) } else { Err(Failure::from(failure)) };
    let (id, prop, claim) = (get("id").trim(), get("prop").trim(), get("claim").trim());
    match action {
        "set_label" | "set_desc" => {
            let (q, language) = (get("q").trim(), get("lang").trim());
            if q.is_empty() || language.is_empty() {
                return Err("Needs q, lang, label".into());
            }
            let (api, what) = match action {
                "set_label" => ("wbsetlabel", "label"),
                _ => ("wbsetdescription", "description"),
            };
            done(editor.set_term(api, q, language, get("label")).await, format!("Problem setting {what}"))
        }
        "set_alias" => {
            let (q, language, label) = (get("q").trim(), get("lang").trim(), get("label"));
            let mode = match get("mode").trim() {
                "" => "add",
                mode => mode,
            };
            if q.is_empty() || language.is_empty() || label.is_empty() || !["add", "set", "remove"].contains(&mode) {
                return Err("Needs q, lang, label [, mode=add/set/remove]".into());
            }
            done(editor.set_alias(q, language, label, mode).await, "Problem setting alias".to_string())
        }
        "set_sitelink" => {
            let (q, site, title) = (get("q").trim(), get("site").trim(), get("title").trim());
            if q.is_empty() || site.is_empty() || title.is_empty() {
                return Err("Needs q, site, and title".into());
            }
            done(editor.set_sitelink(q, site, title).await, "Problem creating sitelink".to_string())
        }
        "create_blank_item" | "create_item_from_page" => {
            let (site, page) = (get("site").trim(), get("page").trim());
            let (data, failure) = match action {
                "create_blank_item" => (json!({}), "Problem creating blank item"),
                _ if site.is_empty() || page.is_empty() => return Err("Needs site and page".into()),
                _ => (item_from_page(site, page), "Problem creating item"),
            };
            done(editor.create_item(&data).await, failure.to_string())?;
            extras.q = editor.last.as_ref().and_then(|a| a["entity"]["id"].as_str()).map(str::to_string);
            Ok(())
        }
        "remove_claim" => {
            let baserev = get("baserev").trim();
            if id.is_empty() {
                return Err("Needs id".into());
            }
            done(editor.remove_claim(id, baserev).await, format!("Problem removing claim {id}/{baserev}"))
        }
        "add_source" => {
            let (statement, snaks) = (get("statement").trim(), get("snaks").trim());
            if statement.is_empty() || snaks.is_empty() {
                return Err("Needs statement and snaks".into());
            }
            done(editor.set_source(statement, snaks).await, format!("Problem setting source {statement} / {snaks}"))
        }
        "merge_items" | "create_redirect" => {
            let (from, to) = (get("from").trim(), get("to").trim());
            if from.is_empty() || to.is_empty() {
                return Err("Needs from and to".into());
            }
            match action {
                "merge_items" => {
                    done(editor.merge_items(from, to).await, format!("Problem merging item '{from}' into '{to}'"))
                }
                _ => {
                    done(editor.create_redirect(from, to).await, format!("Problem creating redirect '{from}'=>'{to}'"))
                }
            }
        }
        "generic" => {
            let json = get("json");
            let Ok(Value::Object(request)) = serde_json::from_str::<Value>(json) else {
                extras.jle = JSON_ERROR_SYNTAX;
                return Err("generic_action JSON parsing error".into());
            };
            let summary = get("summary");
            done(editor.generic(request, summary).await, format!("generic_action failed: {json}/{summary}"))
        }
        "set_claims" => {
            let (target, qualifier) = (get("target").trim(), get("qualifier_claim").trim());
            let ids: Vec<&str> = get("ids").split(',').map(str::trim).filter(|id| !id.is_empty()).collect();
            if prop.is_empty() || target.is_empty() || (ids.is_empty() && qualifier.is_empty()) {
                return Err("set_claim parameters incomplete".into());
            }
            if ids.len() > MAX_IDS {
                return Err(format!("set_claims takes at most {MAX_IDS} ids at a time").into());
            }
            let value = ClaimValue::item(target)?;
            let property = property(prop)?;
            let summary = get("summary");
            // With a qualifier, the ids do not matter (the PHP set it once per id).
            let targets: Vec<Target> = match qualifier {
                "" => ids.iter().copied().map(Target::Entity).collect(),
                claim => vec![Target::Qualifier(claim)],
            };
            for (entity, id) in targets.into_iter().zip(ids.iter().chain(std::iter::repeat(&""))) {
                if !editor.set_claim(entity, &property, &value, summary).await {
                    return Err(format!("set_claims failed: {id}/{prop}/{target}/{qualifier}").into());
                }
            }
            Ok(())
        }
        "set_string" | "set_monolang" | "set_location" | "set_date" | "set_quantity" => {
            // `set_date` and `set_quantity` name the qualified statement `claim`, the others `qualifier_claim`.
            let qualifier = match action {
                "set_date" | "set_quantity" => claim,
                _ => get("qualifier_claim").trim(),
            };
            let target = match (qualifier, id) {
                ("", "") => return Err(format!("{action} parameters incomplete").into()),
                ("", id) => Target::Entity(id),
                (claim, _) => Target::Qualifier(claim),
            };
            let value = claim_value(action, fields).ok_or_else(|| format!("{action} parameters incomplete"))??;
            let property = property(prop)?;
            let failed = format!("{action} failed: {id}/{prop}/{qualifier}");
            done(editor.set_claim(target, &property, &value, get("summary")).await, failed)
        }
        _ => Err(format!("Unknown action '{action}'").into()),
    }
}

/// The value of a `set_*` action; `None` if a parameter is missing.
fn claim_value(action: &str, fields: Fields<'_>) -> Option<Result<ClaimValue, String>> {
    let get = |key: &str| fields.get(key);
    let required = |key: &str| Some(fields.get(key)).filter(|v| !v.trim().is_empty());
    Some(match action {
        "set_string" => Ok(ClaimValue::String(required("text")?.to_string())),
        "set_monolang" => Ok(ClaimValue::Monolingual {
            text: required("text")?.to_string(),
            language: required("language")?.trim().to_string(),
        }),
        "set_location" => ClaimValue::location(required("lat")?, required("lon")?),
        "set_date" => ClaimValue::date(required("date")?, required("prec")?),
        _ => ClaimValue::quantity(required("amount")?, get("upper"), get("lower"), get("unit")),
    })
}

fn property(prop: &str) -> Result<String, Failure> {
    crate::wikidata::parse_id(prop, 'P').ok_or_else(|| format!("Invalid property '{prop}'").into())
}

/// A new item for a wiki page: its sitelink, and the page title (without
/// a disambiguation in brackets) as label in the wiki's language.
fn item_from_page(site: &str, page: &str) -> Value {
    let page = page.replace(' ', "_");
    let mut data = json!({"sitelinks": {site: {"site": site, "title": page}}});
    let language = site.strip_suffix("wikiquote").or_else(|| site.strip_suffix("wiki")).filter(|l| !l.is_empty());
    if let Some(language) = language {
        let language = match LABEL_LANGUAGES.iter().find(|(wiki, _)| *wiki == language) {
            Some((_, label_language)) => label_language.to_string(),
            None => language.replace('_', "-"),
        };
        let label = without_disambiguation(&page.replace('_', " ")).to_string();
        data["labels"] = json!({&language: {"language": language, "value": label}});
    }
    data
}

/// `Paris (Texas)` → `Paris`.
fn without_disambiguation(title: &str) -> Cow<'_, str> {
    static DISAMBIGUATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+\(.+\)$").expect("valid regex"));
    DISAMBIGUATION.replace(title, "")
}

/// The JSON answer, as `Widar::render_reponse`.
fn answer(outcome: Result<(), Failure>, last: Option<Value>, error2: String, extras: Extras) -> Response {
    let mut out = Map::new();
    let error = outcome.as_ref().err().map_or_else(|| "OK".to_string(), |f| f.message.clone());
    out.insert("error".to_string(), error.into());
    out.insert("status".to_string(), "OK".into());
    out.insert("data".to_string(), json!([]));
    if let Err(failure) = &outcome {
        out.insert("transient".to_string(), failure.transient.into());
    }
    out.insert("error2".to_string(), error2.into());
    out.insert("jle".to_string(), extras.jle.into());
    out.insert("res".to_string(), last.unwrap_or_else(|| json!("")));
    out.insert("result".to_string(), extras.result.unwrap_or_else(|| json!("")));
    if let Some(q) = extras.q {
        out.insert("q".to_string(), q.into());
    }
    Json(Value::Object(out)).into_response()
}

/// MediaWiki sends the user back with the approved request token.
async fn login_callback(state: &AppState, headers: &HeaderMap, verifier: &str, token: &str, login: &Login) -> Response {
    let Some(oauth) = &state.oauth else {
        return failure_page(StatusCode::SERVICE_UNAVAILABLE, NO_LOGIN, None);
    };
    let jar = state.cookies(headers);
    let Some(request) = SESSION.request_token(&jar).filter(|t| t.key == token) else {
        // A revisited callback URL, when logged in already, is no problem.
        if SESSION.access_token(&jar).is_some() {
            return redirect(&login.forward);
        }
        let message = "This login attempt has expired (no authorization in progress). Please log in again.";
        return failure_page(StatusCode::BAD_REQUEST, message, Some(&login.forward));
    };
    match oauth.access_token(&request, verifier).await {
        Ok(access) => (SESSION.with_access_token(jar, &access), redirect(&login.forward)).into_response(),
        Err(e) => {
            tracing::warn!("OAuth login failed: {e:#}");
            let message = "The login failed. Please try logging in again.";
            failure_page(StatusCode::BAD_GATEWAY, message, Some(&login.forward))
        }
    }
}

/// Who is logged in, with a link to log in or out.
async fn status_page(state: &AppState, headers: &HeaderMap, script: &str) -> Response {
    let token = SESSION.access_token(&state.cookies(headers));
    let user = match (&state.oauth, token) {
        (Some(oauth), Some(token)) => {
            let editor = Editor::new(oauth, &state.http, Some(token), "");
            let answer = editor.userinfo(None).await.unwrap_or_default();
            answer["query"]["userinfo"]
                .as_object()
                .filter(|u| !u.contains_key("anon"))
                .and_then(|u| u.get("name")?.as_str().map(str::to_string))
        }
        _ => None,
    };
    let message = state.oauth.is_none().then(|| NO_LOGIN.to_string());
    let page = Page { title: "WiDaR", user, message, script: script.to_string(), back: None };
    match render(&page) {
        Ok(html) => html.into_response(),
        Err(e) => e.into_response(),
    }
}

fn failure_page(status: StatusCode, message: &str, back: Option<&str>) -> Response {
    let page = Page {
        title: "WiDaR",
        user: None,
        message: Some(message.to_string()),
        script: "/api.php".to_string(),
        back: back.map(str::to_string),
    };
    match render(&page) {
        Ok(html) => (status, html).into_response(),
        Err(e) => e.into_response(),
    }
}

/// A site path for user input such as `sourcerer_game/`, which can never
/// lead to another site (`//evil.org`).
fn local_path(path: &str) -> String {
    const UNSAFE: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>');
    let path = path.trim().trim_start_matches(['/', '\\']);
    format!("/{}", percent_encoding::utf8_percent_encode(path, UNSAFE))
}

/// A redirect that cannot fail on odd characters (axum's would panic).
pub fn redirect(location: &str) -> Response {
    let location = HeaderValue::from_str(location).unwrap_or(HeaderValue::from_static("/"));
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::{get, send};
    use axum::body::Body;
    use axum::http::Request;

    #[test]
    fn items_from_pages() {
        let data = item_from_page("enwiki", "Paris (Texas)");
        assert_eq!(data["sitelinks"]["enwiki"], json!({"site": "enwiki", "title": "Paris_(Texas)"}));
        assert_eq!(data["labels"]["en"], json!({"language": "en", "value": "Paris"}));
        assert_eq!(item_from_page("be_x_oldwiki", "A")["labels"]["be-tarask"]["value"], "A");
        assert_eq!(item_from_page("roa_tarawiki", "A")["labels"]["roa-tara"]["value"], "A");
        assert_eq!(item_from_page("dewikiquote", "B")["labels"]["de"]["value"], "B");
        assert!(item_from_page("dewikisource", "B").get("labels").is_none());
        assert_eq!(without_disambiguation("A b (c) (d)"), "A b");
        assert_eq!(without_disambiguation("(c)"), "(c)");
        assert_eq!(without_disambiguation("A ()"), "A ()");
        assert_eq!(without_disambiguation("A"), "A");
    }

    #[test]
    fn forwards_stay_on_this_site() {
        assert_eq!(local_path("sourcerer_game.php"), "/sourcerer_game.php");
        assert_eq!(local_path("//evil.org"), "/evil.org");
        assert_eq!(local_path("/\\evil.org"), "/evil.org");
        assert_eq!(local_path("a\r\nb c"), "/a%0D%0Ab%20c");
        assert_eq!(local_path(""), "/");
        assert_eq!(redirect("/a\nb").headers()["location"], "/");
    }

    #[tokio::test]
    async fn answers_without_oauth() {
        let dir = tempfile::tempdir().unwrap();
        let rights = get(dir.path(), "/api.php?action=get_rights&botmode=1").await;
        assert_eq!(rights.headers["content-type"], "application/json");
        let json: Value = serde_json::from_str(&rights.body).unwrap();
        assert_eq!(json["error"], NO_LOGIN);
        assert_eq!(json["transient"], false);
        assert_eq!(json["result"], "");
        let page = get(dir.path(), "/widar/index.php").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains(NO_LOGIN));
        let unknown = get(dir.path(), "/api.php?action=nonsense").await;
        let json: Value = serde_json::from_str(&unknown.body).unwrap();
        assert_eq!(json["error"], "Unknown action 'nonsense'");
    }

    #[tokio::test]
    async fn edits_only_from_this_site() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("none.ini"), "consumerKey=k\nconsumerSecret=s\nagent=a\n").unwrap();
        let request = Request::post("/api.php")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("sec-fetch-site", "cross-site")
            .body(Body::from("action=set_label&q=Q1&lang=en&label=x"))
            .unwrap();
        let response = send(dir.path(), request).await;
        let json: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(json["error"], "Requests that change something must come from this site");
        // Not logged in: no request to Wikidata is made.
        let request = Request::post("/api.php")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from("action=set_label&q=Q1&lang=en&label=x&path=duplicity/"))
            .unwrap();
        let json: Value = serde_json::from_str(&send(dir.path(), request).await.body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("/api.php?action=authorize&amp;path=duplicity%2F"));
        let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["error", "status", "data", "transient", "error2", "jle", "res", "result"]);
    }
}
