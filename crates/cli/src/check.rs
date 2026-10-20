//! `rubyscope-cli check`: compares the index with the real locations that
//! `probe/rails_probe.rb` got from booted Rails.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rubyscope_index::{FileId, Index};
use serde_json::Value;

#[derive(Default)]
struct Tally {
    total: usize,
    exact: usize,
    /// The probe's line does not name the constant or method, so Ruby made
    /// it at runtime (`define_method` in a loop, `class_eval` with a string,
    /// `Struct.new` members...). These are not counted in `total`.
    runtime: usize,
    /// The name is in the index, but not at the probe's file and line.
    elsewhere: usize,
    missing: Vec<String>,
    wrong_place: Vec<String>,
}

impl Tally {
    fn print(&self, title: &str, show: usize) {
        let pct = |n: usize| if self.total == 0 { 0.0 } else { 100.0 * n as f64 / self.total as f64 };
        println!(
            "{title}: {} total, {} exact ({:.1}%), {} at another place ({:.1}%), {} missing ({:.1}%), {} made at runtime (not counted)",
            self.total,
            self.exact,
            pct(self.exact),
            self.elsewhere,
            pct(self.elsewhere),
            self.missing.len(),
            pct(self.missing.len()),
            self.runtime
        );
        for m in self.missing.iter().take(show) {
            println!("  missing      {m}");
        }
        for m in self.wrong_place.iter().take(show) {
            println!("  other place  {m}");
        }
    }
}

/// Maps the probe's absolute paths to index files. Paths are compared after
/// `canonicalize`, because the Ruby install can be reached through a symlink.
struct Files<'a> {
    index: &'a Index,
    by_canonical: HashMap<PathBuf, FileId>,
    cache: HashMap<String, Option<FileId>>,
    lines: HashMap<FileId, Vec<String>>,
}

impl<'a> Files<'a> {
    fn new(index: &'a Index) -> Self {
        let by_canonical = index
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path.canonicalize().unwrap_or_else(|_| f.path.clone()), i as FileId))
            .collect();
        Self { index, by_canonical, cache: HashMap::new(), lines: HashMap::new() }
    }

    fn get(&mut self, path: &str) -> Option<FileId> {
        if let Some(v) = self.cache.get(path) {
            return *v;
        }
        let p = Path::new(path);
        let id = p.canonicalize().ok().and_then(|c| self.by_canonical.get(&c).copied());
        self.cache.insert(path.to_string(), id);
        id
    }

    /// True when the 1-based `line` of the file contains `name`.
    fn line_names(&mut self, file: FileId, line: u64, name: &str) -> bool {
        let index = self.index;
        let lines = self.lines.entry(file).or_insert_with(|| {
            std::fs::read_to_string(&index.files[file as usize].path)
                .map(|s| s.lines().map(str::to_string).collect())
                .unwrap_or_default()
        });
        let name = name.trim_end_matches(['=', '?', '!']);
        lines.get(line as usize - 1).is_some_and(|l| l.contains(name))
    }

    /// The 1-based line of a byte offset.
    fn line(&self, file: FileId, offset: u32) -> u64 {
        self.index.files[file as usize].raw.line_col(offset).0 as u64 + 1
    }
}

pub fn check(index: &Index, probe_path: &Path, show: usize) -> Result<()> {
    let probe: Value = serde_json::from_slice(&std::fs::read(probe_path)?)?;
    let root = probe["root"].as_str().unwrap_or_default().to_string();
    let mut files = Files::new(index);
    let in_project = |path: &str| path.starts_with(&format!("{root}/"));

    let mut consts = [Tally::default(), Tally::default()];
    if let Some(map) = probe["constants"].as_object() {
        for (name, loc) in map {
            let (Some(path), Some(line)) = (loc[0].as_str(), loc[1].as_u64()) else { continue };
            let Some(file) = files.get(path) else { continue };
            let t = &mut consts[usize::from(!in_project(path))];
            let last = name.rsplit("::").next().unwrap_or(name);
            if !files.line_names(file, line, last) {
                t.runtime += 1;
                continue;
            }
            t.total += 1;
            let shown = format!("{name}  {}:{line}", index.display_path(file));
            match index.consts.get(name) {
                None => t.missing.push(shown),
                Some(defs) => {
                    if defs.iter().any(|d| d.loc.file == file && files.line(file, d.loc.span.start) == line) {
                        t.exact += 1;
                    } else {
                        t.elsewhere += 1;
                        t.wrong_place.push(shown);
                    }
                }
            }
        }
    }

    let mut methods = [Tally::default(), Tally::default()];
    if let Some(list) = probe["methods"].as_array() {
        for m in list {
            let (Some(owner), Some(singleton), Some(name), Some(path), Some(line)) =
                (m[0].as_str(), m[1].as_bool(), m[2].as_str(), m[3].as_str(), m[4].as_u64())
            else {
                continue;
            };
            let Some(file) = files.get(path) else { continue };
            let t = &mut methods[usize::from(!in_project(path))];
            if !files.line_names(file, line, name) {
                t.runtime += 1;
                continue;
            }
            t.total += 1;
            let sep = if singleton { "." } else { "#" };
            let shown = format!("{owner}{sep}{name}  {}:{line}", index.display_path(file));
            let at_line = |id: &u32| {
                let d = &index.methods[*id as usize];
                if d.loc.file == file && files.line(file, d.loc.span.start) == line {
                    return true;
                }
                // Ruby reports an alias at the line of the original method.
                let Some(orig) = &d.alias_of else { return false };
                index.find_method(&d.owner, d.singleton, orig).is_some_and(|ids| {
                    ids.iter().any(|o| {
                        let od = &index.methods[*o as usize];
                        od.loc.file == file && files.line(file, od.loc.span.start) == line
                    })
                })
            };
            // What a click does: Ruby's method lookup through the ancestors.
            match index.find_method(owner, singleton, name) {
                Some(ids) if ids.iter().any(at_line) => t.exact += 1,
                Some(_) => {
                    t.elsewhere += 1;
                    t.wrong_place.push(shown);
                }
                None => t.missing.push(shown),
            }
        }
    }

    consts[0].print("constants in the project", show);
    consts[1].print("constants in gems and stdlib", show);
    methods[0].print("methods in the project", show);
    methods[1].print("methods in gems and stdlib", show);
    Ok(())
}
