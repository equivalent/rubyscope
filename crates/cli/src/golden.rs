//! `rubyscope-cli golden`: real click points in a project and where they
//! must go.
//!
//! Each line is `FILE:LINE:COL => TARGET:LINE`, with an optional `# note`.
//! TARGET uses the stable path form (`app/models/x.rb`, `gem:rails/...`,
//! `stdlib:...`). A click passes when its first target is TARGET:LINE.

use std::path::Path;

use anyhow::{Context, Result};
use rubyscope_index::Index;

use crate::parse_position;

/// Returns true when every click passes.
pub fn run(index: &Index, golden: &Path) -> Result<bool> {
    let text = std::fs::read_to_string(golden).with_context(|| format!("reading {}", golden.display()))?;
    let (mut pass, mut fail) = (0, 0);
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split(" # ").next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((click, expected)) = line.split_once("=>") else {
            println!("line {}: no `=>`", n + 1);
            fail += 1;
            continue;
        };
        let (click, expected) = (click.trim(), expected.trim());
        let got = parse_position(index, click)
            .ok()
            .and_then(|(file, offset)| index.definition_at(file, offset))
            .and_then(|r| {
                let t = r.targets.first()?;
                let line = index.files[t.loc.file as usize].raw.line_col(t.loc.span.start).0 + 1;
                Some(format!("{}:{line}", index.stable_path(t.loc.file)))
            });
        if got.as_deref() == Some(expected) {
            pass += 1;
        } else {
            fail += 1;
            println!("FAIL {click}\n     expected {expected}\n     got      {}", got.as_deref().unwrap_or("nothing"));
        }
    }
    println!("{pass} passed, {fail} failed");
    Ok(fail == 0)
}
