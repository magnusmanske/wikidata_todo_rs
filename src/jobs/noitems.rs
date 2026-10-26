//! The data of "Important blank items": items without any statement, most
//! sitelinks first, as `<data_dir>/no_items.50K.tab` (`item\tsitelinks`,
//! numeric item IDs).

use super::Writes;
use crate::sparql::entity_id;
use crate::state::AppState;
use anyhow::{Result, bail};
use std::fmt::Write;

const MAX: usize = 50_000;
pub const FILE: &str = "no_items.50K.tab";

pub async fn update(state: &AppState, writes: Writes) -> Result<()> {
    let query = format!(
        "SELECT ?item ?sitelinks {{ ?item wikibase:statements 0 ; wikibase:sitelinks ?sitelinks FILTER(?sitelinks > 1) }}
         ORDER BY DESC(?sitelinks) LIMIT {MAX}"
    );
    let mut csv = state.sparql.heavy_csv(&query).await?;
    let mut out = String::from("item\tsitelinks\n");
    let mut rows = 0;
    for row in csv.records() {
        let row = row?;
        let item = row.get(0).and_then(entity_id).and_then(|id| id.strip_prefix('Q'));
        if let (Some(item), Some(sitelinks)) = (item, row.get(1)) {
            let _ = writeln!(out, "{item}\t{sitelinks}");
            rows += 1;
        }
    }
    // An empty answer means trouble, not "no such items": keep the old file.
    if rows == 0 {
        bail!("the query service returned no items");
    }
    writes.file(&state.config.data_dir.join(FILE), out.as_bytes()).await?;
    println!("{rows} items");
    Ok(())
}
