//! `lazarus host logs`: the tail of the daemon's captured structured log.

use std::fs;

use anyhow::{Context, Result};
use lazarus_hostd::runtime::DataPaths;

use crate::host::discovery;

pub fn run(tail: usize) -> Result<()> {
    let paths = DataPaths::resolve()?;
    let path = discovery::log_path(&paths);
    if !path.exists() {
        println!("no Host log yet at {}", path.display());
        return Ok(());
    }
    let content =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    for line in tail_lines(&content, tail) {
        println!("{line}");
    }
    Ok(())
}

/// The trailing `tail` non-trimming lines, oldest first.
fn tail_lines(content: &str, tail: usize) -> Vec<&str> {
    let total = content.lines().count();
    content.lines().skip(total.saturating_sub(tail)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_returns_only_the_requested_trailing_lines_in_order() {
        let content = "l1\nl2\nl3\nl4";
        assert_eq!(tail_lines(content, 2), vec!["l3", "l4"]);
        assert_eq!(tail_lines(content, 10), vec!["l1", "l2", "l3", "l4"]);
        assert_eq!(tail_lines("", 5), Vec::<&str>::new());
        assert_eq!(tail_lines("only", 0), Vec::<&str>::new());
    }
}
