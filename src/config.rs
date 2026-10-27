//! Typed configuration, loaded from a JSON file that is never committed.
//! See `config.json.template` for the shape.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use wikimisc::telemetry::TelemetryConfig;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The PHP tool's `public_html`, whose static files are served as they are.
    /// Relative paths are relative to the config file.
    pub public_html: PathBuf,
    /// The tool's home: data files the tools read, and the jobs write.
    pub data_dir: PathBuf,
    /// Database credentials (`replica.my.cnf`, as Toolforge provides it). With
    /// `local='true'` in it, connections go to SSH tunnels instead.
    #[serde(default = "default_replica_credentials")]
    pub replica_credentials: PathBuf,
    /// For development: a `mysql://` URL (without database) to use instead of ToolsDB.
    #[serde(default)]
    pub tool_db_url: Option<String>,
    /// For development: whose databases to use (e.g. `s51211`), if not the
    /// credentials' own. Other tools' `_p` databases are readable to all.
    #[serde(default)]
    pub tool_db_owner: Option<String>,
    /// Where this service is reachable, for OAuth callbacks.
    #[serde(default = "default_public_url")]
    pub public_url: String,
    /// Encrypts the login cookies: 64 bytes, hex. Without one, a random key
    /// is used, so logins last only until a restart, and only on one replica.
    #[serde(default)]
    pub cookie_key: Option<String>,
    /// The OAuth consumer (`consumerKey`, `consumerSecret`, `agent`) for
    /// editing as the user; without the file, logging in is unavailable.
    #[serde(default = "default_oauth")]
    pub oauth: PathBuf,
    /// The Wikispecies bot's Wikidata login: `user=` and (bot) `password=` lines.
    #[serde(default = "default_bot_credentials")]
    pub bot_credentials: PathBuf,
    /// Service dashboard push; the token may also come from `$DASHBOARD_TOKEN`.
    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

fn default_replica_credentials() -> PathBuf {
    "replica.my.cnf".into()
}

fn default_public_url() -> String {
    "https://wikidata-todo.toolforge.org".to_string()
}

fn default_oauth() -> PathBuf {
    "oauth.ini".into()
}

fn default_bot_credentials() -> PathBuf {
    "reinheitsgebot.conf".into()
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("cannot read config file {}", path.display()))?;
        let mut config: Self =
            serde_json::from_str(&text).with_context(|| format!("invalid config file {}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        config.public_html = dir.join(&config.public_html);
        config.data_dir = dir.join(&config.data_dir);
        config.replica_credentials = dir.join(&config.replica_credentials);
        config.oauth = dir.join(&config.oauth);
        config.bot_credentials = dir.join(&config.bot_credentials);
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loads `json` as a config file; also returns the directory it was in.
    fn load(json: &str) -> (Result<Config>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, json).unwrap();
        (Config::load(&path), dir)
    }

    #[test]
    fn relative_paths_are_relative_to_the_config_file() {
        let (config, dir) = load(r#"{"public_html": "php/public_html", "data_dir": "."}"#);
        let config = config.unwrap();
        assert_eq!(config.public_html, dir.path().join("php/public_html"));
        assert_eq!(config.replica_credentials, dir.path().join("replica.my.cnf"));
        assert_eq!(config.oauth, dir.path().join("oauth.ini"));
        assert!(config.telemetry.token.is_none());
    }

    #[test]
    fn absolute_paths_stay() {
        let (config, _dir) = load(r#"{"public_html": "/data/project/x/public_html", "data_dir": "/data/project/x"}"#);
        assert_eq!(config.unwrap().public_html, Path::new("/data/project/x/public_html"));
    }

    #[test]
    fn typos_and_omissions_are_errors() {
        assert!(load(r#"{"public_html": "x", "data_dir": "x", "telemtry": {}}"#).0.is_err());
        assert!(load(r#"{"public_html": "x"}"#).0.is_err());
    }
}
