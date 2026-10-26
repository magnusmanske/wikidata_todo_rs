//! Background jobs: `wikidata_todo_rs <command>`. On Toolforge, the jobs
//! framework runs them (see `jobs.yaml`).

use crate::state::AppState;
use anyhow::Result;
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum Command {}

pub async fn run(command: Command, _state: &AppState) -> Result<()> {
    match command {}
}
