//! Lines appended to data files by anonymous requests (game decisions, …).

use anyhow::{Result, bail};
use std::path::Path;
use tokio::io::AsyncWriteExt;

/// Larger files take no more lines: requests must not fill the disk.
const MAX_BYTES: u64 = 20 * 1024 * 1024;

/// Appends `line` to the file at `path` (created if need be), unless it is there already.
pub async fn line_once(path: &Path, line: &str) -> Result<()> {
    let existing = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    if existing.lines().any(|l| l == line) {
        return Ok(());
    }
    if existing.len() as u64 >= MAX_BYTES {
        bail!("{} is full", path.display());
    }
    let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(path).await?;
    // One write per line, so that concurrent appends do not interleave;
    // flushed, as tokio only hands the write to a background thread.
    file.write_all(format!("{line}\n").as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn appends_each_line_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.log");
        for line in ["a", "b", "a"] {
            line_once(&path, line).await.unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nb\n");
    }
}
