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
    WikiFlix,
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
            Self::WikiFlix => "wikiflix_p",
        }
    }
}

pub enum ToolDb {
    ToolsDb(ToolsDbPool),
    /// For development: a server given by URL, with the databases under their full names.
    Url {
        pool: CustomPool,
        user: String,
    },
}

impl std::fmt::Debug for ToolDb {
    // Not derived: the pools must not print their connection options.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToolsDb(pool) => write!(f, "ToolDb({})", pool.user()),
            Self::Url { pool, .. } => write!(f, "ToolDb({})", pool.server()),
        }
    }
}

impl ToolDb {
    pub fn new(url: Option<&str>, credentials: &Path) -> Result<Self> {
        let pool = ToolsDbPool::builder()
            .my_cnf_path(credentials)
            .build()
            .with_context(|| format!("cannot read the database credentials from {}", credentials.display()))?;
        match url {
            Some(url) => {
                let custom = CustomPool::builder_from_url(url).context("invalid tool_db_url")?.build()?;
                Ok(Self::Url { pool: custom, user: pool.user().to_string() })
            }
            None => Ok(Self::ToolsDb(pool)),
        }
    }

    pub async fn conn(&self, database: Database) -> Result<Conn> {
        let conn = match self {
            Self::ToolsDb(pool) => pool.connect(&format!("{}__{}", pool.user(), database.name())).await,
            Self::Url { pool, user } => match pool.connect().await {
                Ok(mut conn) => {
                    conn.query_drop(format!("USE `{user}__{}`", database.name())).await?;
                    Ok(conn)
                }
                Err(e) => Err(e),
            },
        };
        conn.with_context(|| format!("cannot connect to the tool database {}", database.name()))
    }
}
