//! The tool's own databases on ToolsDB (`<user>__duplicity_p` and others).

use anyhow::{Context, Result};
use std::path::Path;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::toolforge::pool::{CustomPool, ToolsDbPool};

/// The tool's databases, without the `<user>__` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Database {
    Duplicity,
    MergeById,
    Monitor,
    Sourcerer,
    UserScripts,
    WikidataCache,
}

impl Database {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Duplicity => "duplicity_p",
            Self::MergeById => "wikidata_merge_by_id_p",
            Self::Monitor => "monitor_p",
            Self::Sourcerer => "sourcerer_p",
            Self::UserScripts => "userscripts_p",
            Self::WikidataCache => "wikidata_cache_p",
        }
    }
}

pub struct ToolDb {
    pool: Pool,
    /// The database name prefix: the tool's database user.
    owner: String,
}

enum Pool {
    ToolsDb(ToolsDbPool),
    /// For development: a server given by URL.
    Url(CustomPool),
}

impl std::fmt::Debug for ToolDb {
    // Not derived: the pools must not print their connection options.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ToolDb({})", self.owner)
    }
}

impl ToolDb {
    pub fn new(url: Option<&str>, owner: Option<&str>, credentials: &Path) -> Result<Self> {
        let tools_db = ToolsDbPool::builder()
            .my_cnf_path(credentials)
            .build()
            .with_context(|| format!("cannot read the database credentials from {}", credentials.display()))?;
        let owner = owner.unwrap_or(tools_db.user()).to_string();
        let pool = match url {
            Some(url) => Pool::Url(CustomPool::builder_from_url(url).context("invalid tool_db_url")?.build()?),
            None => Pool::ToolsDb(tools_db),
        };
        Ok(Self { pool, owner })
    }

    pub async fn conn(&self, database: Database) -> Result<Conn> {
        let name = format!("{}__{}", self.owner, database.name());
        let conn = match &self.pool {
            Pool::ToolsDb(pool) => pool.connect(&name).await,
            Pool::Url(pool) => match pool.connect().await {
                Ok(mut conn) => conn.query_drop(format!("USE `{name}`")).await.map(|()| conn).map_err(Into::into),
                Err(e) => Err(e),
            },
        };
        conn.with_context(|| format!("cannot connect to the tool database {name}"))
    }
}
