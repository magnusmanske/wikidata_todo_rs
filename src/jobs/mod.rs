//! Background jobs: `wikidata_todo_rs <command>`. On Toolforge, the jobs
//! framework runs them (see `jobs.yaml`).

mod duplicity;
mod merge_by_id;
mod noitems;
mod pasleim;
mod user_scripts;

use crate::state::AppState;
use anyhow::Result;
use clap::Subcommand;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Params};

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Duplicity: articles without a Wikidata item, per wiki (daily).
    DuplicityUpdate {
        #[arg(long)]
        dry_run: bool,
        /// Only these wikis (database names), instead of all.
        wikis: Vec<String>,
    },
    /// Duplicity: items that may match those articles, for the Distributed Game (hourly).
    DuplicityCandidates {
        #[arg(long)]
        dry_run: bool,
        /// Batches of random articles to look at.
        #[arg(long, default_value_t = 100)]
        rounds: usize,
    },
    /// Merge game: items sharing an external ID that should be unique (daily).
    MergeByIdUpdate {
        #[arg(long)]
        dry_run: bool,
        /// Only these properties, instead of all.
        properties: Vec<String>,
    },
    /// Merge game: the item pairs on User:Pasleim/projectmerge (daily).
    PasleimUpdate {
        #[arg(long)]
        dry_run: bool,
    },
    /// Important blank items: items without statements, by sitelinks (daily).
    NoitemsUpdate {
        #[arg(long)]
        dry_run: bool,
    },
    /// Which user scripts Wikidata users load in their common.js (daily).
    UserScriptsUpdate {
        #[arg(long)]
        dry_run: bool,
    },
}

pub async fn run(command: Command, state: &AppState) -> Result<()> {
    match command {
        Command::DuplicityUpdate { dry_run, wikis } => duplicity::update(state, &wikis, Writes { dry_run }).await,
        Command::DuplicityCandidates { dry_run, rounds } => {
            duplicity::candidates(state, rounds, Writes { dry_run }).await
        }
        Command::MergeByIdUpdate { dry_run, properties } => {
            merge_by_id::update(state, &properties, Writes { dry_run }).await
        }
        Command::PasleimUpdate { dry_run } => pasleim::update(state, Writes { dry_run }).await,
        Command::NoitemsUpdate { dry_run } => noitems::update(state, Writes { dry_run }).await,
        Command::UserScriptsUpdate { dry_run } => user_scripts::update(state, Writes { dry_run }).await,
    }
}

/// Database and file writes of a job; with `dry_run`, only printed.
#[derive(Debug, Clone, Copy)]
pub struct Writes {
    dry_run: bool,
}

impl Writes {
    pub async fn exec<P: Into<Params> + Send>(self, conn: &mut Conn, sql: &str, params: P) -> Result<()> {
        if self.dry_run {
            let sql = sql.split_whitespace().collect::<Vec<_>>().join(" ");
            println!("[dry run] {}", sql.chars().take(150).collect::<String>());
            return Ok(());
        }
        conn.exec_drop(sql, params).await?;
        Ok(())
    }

    /// Replaces `path` with `content` at once: readers never see half a file.
    pub async fn file(self, path: &std::path::Path, content: &[u8]) -> Result<()> {
        if self.dry_run {
            println!("[dry run] would write {} bytes to {}", content.len(), path.display());
            return Ok(());
        }
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, content).await?;
        tokio::fs::rename(&tmp, path).await?;
        Ok(())
    }
}
