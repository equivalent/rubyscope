//! Git diff mode: what the branch changed since its merge-base with a base
//! branch (as in a pull request), what is not committed yet, and the rows
//! of the code view, with the removed lines ("ghost rows") between the real
//! lines of the file on disk.
//!
//! The code view always shows the file on disk. A [`RowMap`] says which
//! display row shows which file line, and where the ghost rows go. Every
//! file line has exactly one row, so line numbers, go to line, find and
//! clicks keep working with file lines and only the drawing uses rows.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;

use crate::highlight::LineTokens;

/// One hunk of `git diff -U0`: `@@ -old_start,old_count +new_start,new_count @@`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// The text of the removed lines, without the `-`.
    pub removed: Vec<String>,
    /// Syntax colors of the removed lines (empty when not colored).
    pub tokens: Vec<LineTokens>,
}

impl Hunk {
    /// The 0-based line of the new file that the removed lines come
    /// before. With no new lines (`+6,0`), the lines were removed after
    /// line 6, so before 0-based line 6.
    pub fn anchor(&self) -> u32 {
        if self.new_count > 0 { self.new_start.saturating_sub(1) } else { self.new_start }
    }

    /// The first removed line of the old file, 0-based.
    fn old_first(&self) -> u32 {
        if self.old_count > 0 { self.old_start.saturating_sub(1) } else { self.old_start }
    }
}

/// `5,3` or `5` (a count of 1).
fn parse_range(s: &str) -> Option<(u32, u32)> {
    match s.split_once(',') {
        Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

/// `@@ -5,0 +6,2 @@ def ship!` → the hunk, with no lines yet.
pub fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let new = rest.split(' ').next()?;
    let (old_start, old_count) = parse_range(old)?;
    let (new_start, new_count) = parse_range(new)?;
    Some(Hunk { old_start, old_count, new_start, new_count, ..Hunk::default() })
}

/// The hunks of `git diff -U0` for one file, and whether git said the file
/// is binary. The counts in each header say how many `-` and `+` lines
/// follow, so a removed line that starts with `--` is not taken for a
/// file header.
pub fn parse_diff(text: &str) -> (Vec<Hunk>, bool) {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut binary = false;
    let (mut old_left, mut new_left) = (0u32, 0u32);
    for line in text.lines() {
        if old_left > 0
            && let Some(removed) = line.strip_prefix('-')
        {
            old_left -= 1;
            if let Some(h) = hunks.last_mut() {
                h.removed.push(removed.to_string());
            }
            continue;
        }
        if new_left > 0 && line.starts_with('+') {
            new_left -= 1;
            continue;
        }
        if line.starts_with('\\') {
            // `\ No newline at end of file`
            continue;
        }
        if let Some(h) = parse_hunk_header(line) {
            (old_left, new_left) = (h.old_count, h.new_count);
            hunks.push(h);
            continue;
        }
        if line.starts_with("Binary files ") || line == "GIT binary patch" {
            binary = true;
        }
        (old_left, new_left) = (0, 0);
    }
    (hunks, binary)
}

/// The two diffs of one open file.
#[derive(Debug, Clone, Default)]
pub struct FileDiff {
    /// From the merge-base to `HEAD`: what the branch changed.
    pub branch: Vec<Hunk>,
    /// From `HEAD` to the file on disk: what is not committed.
    pub uncommitted: Vec<Hunk>,
    pub binary: bool,
    /// Not in git yet (and not ignored): every line is new.
    pub untracked: bool,
    /// The merge-base and `HEAD` that this diff was made with.
    pub merge_base: Option<String>,
    pub head: Option<String>,
}

/// What kind of change a file line is part of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineMark {
    #[default]
    None,
    /// Added or changed on the branch (committed).
    Branch,
    /// Added or changed, not committed.
    Uncommitted,
}

/// One row of the code view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayRow {
    /// A line of the file on disk (0-based).
    Line(u32),
    /// A removed line: an index into [`RowMap::ghosts`].
    Ghost(u32),
}

#[derive(Debug, Clone)]
pub struct Ghost {
    pub text: String,
    pub tokens: LineTokens,
    /// Removed in the working tree (not committed), not on the branch.
    pub uncommitted: bool,
}

/// The rows of the code view in diff mode.
#[derive(Debug, Clone, Default)]
pub struct RowMap {
    pub rows: Vec<DisplayRow>,
    pub ghosts: Vec<Ghost>,
    /// The display row of each file line.
    pub line_row: Vec<u32>,
    /// The change mark of each file line.
    pub marks: Vec<LineMark>,
    /// The changes: runs of changed rows (first and last row, inclusive).
    pub changes: Vec<(u32, u32)>,
    /// Runs of rows of one kind, for the marks next to the scroll bar:
    /// first row, last row, kind, and the change they are part of.
    pub runs: Vec<(u32, u32, RunKind, usize)>,
    /// The most characters in a ghost row.
    pub max_ghost_chars: usize,
}

/// The color of a run of changed rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// Added or changed on the branch: green.
    Added,
    /// Removed on the branch: red.
    Removed,
    /// Not committed (added, changed or removed): amber.
    Uncommitted,
}

impl RowMap {
    /// Builds the rows for a file with `display_lines` lines in the view, of
    /// which `git_lines` are lines for git (the view has one more, empty
    /// line when the file ends with a newline).
    pub fn build(diff: &FileDiff, git_lines: u32, display_lines: u32) -> RowMap {
        let mut map = RowMap {
            rows: Vec::with_capacity(display_lines as usize + 8),
            line_row: vec![0; display_lines as usize],
            marks: vec![LineMark::None; display_lines as usize],
            ..RowMap::default()
        };
        let mut unc: Vec<&Hunk> = diff.uncommitted.iter().collect();
        unc.sort_by_key(|h| h.old_first());
        let added: u32 = unc.iter().map(|h| h.new_count).sum();
        let removed: u32 = unc.iter().map(|h| h.old_count).sum();
        let head_len = (git_lines + removed).saturating_sub(added);

        // The branch diff in HEAD lines: which lines it added, and the
        // removed lines that come before each HEAD line.
        let mut head_added = vec![false; head_len as usize];
        let mut ghosts_before: Vec<Vec<u32>> = vec![Vec::new(); head_len as usize + 1];
        for h in &diff.branch {
            for k in 0..h.new_count {
                if let Some(a) = head_added.get_mut((h.new_start + k).saturating_sub(1) as usize) {
                    *a = true;
                }
            }
            let at = h.anchor().min(head_len) as usize;
            for (i, text) in h.removed.iter().enumerate() {
                ghosts_before[at].push(map.ghosts.len() as u32);
                map.ghosts.push(Ghost {
                    text: text.clone(),
                    tokens: h.tokens.get(i).cloned().unwrap_or_default(),
                    uncommitted: false,
                });
            }
        }

        let (mut h, mut d) = (0u32, 0u32);
        let flush = |map: &mut RowMap, ghosts_before: &mut Vec<Vec<u32>>, h: u32| {
            if let Some(list) = ghosts_before.get_mut(h as usize) {
                for g in std::mem::take(list) {
                    map.rows.push(DisplayRow::Ghost(g));
                }
            }
        };
        let line = |map: &mut RowMap, d: u32, mark: LineMark| {
            if (d as usize) < map.line_row.len() {
                map.line_row[d as usize] = map.rows.len() as u32;
                map.marks[d as usize] = mark;
                map.rows.push(DisplayRow::Line(d));
            }
        };
        let branch_mark = |head_added: &[bool], h: u32| {
            if head_added.get(h as usize).copied().unwrap_or(false) { LineMark::Branch } else { LineMark::None }
        };
        for hunk in unc {
            let first = hunk.old_first().min(head_len);
            while h < first && d < git_lines {
                flush(&mut map, &mut ghosts_before, h);
                line(&mut map, d, branch_mark(&head_added, h));
                h += 1;
                d += 1;
            }
            for (i, text) in hunk.removed.iter().enumerate() {
                flush(&mut map, &mut ghosts_before, h);
                let g = map.ghosts.len() as u32;
                map.ghosts.push(Ghost {
                    text: text.clone(),
                    tokens: hunk.tokens.get(i).cloned().unwrap_or_default(),
                    uncommitted: true,
                });
                map.rows.push(DisplayRow::Ghost(g));
                h += 1;
            }
            for _ in 0..hunk.new_count {
                if d >= git_lines {
                    break;
                }
                line(&mut map, d, LineMark::Uncommitted);
                d += 1;
            }
        }
        while d < git_lines {
            flush(&mut map, &mut ghosts_before, h);
            line(&mut map, d, branch_mark(&head_added, h));
            h += 1;
            d += 1;
        }
        // Lines removed at the end of the file.
        for at in h..=head_len {
            flush(&mut map, &mut ghosts_before, at);
        }
        // The empty line after the last newline.
        while d < display_lines {
            line(&mut map, d, LineMark::None);
            d += 1;
        }

        // A change is a run of rows that are not plain lines.
        let changed = |r: &DisplayRow| match r {
            DisplayRow::Ghost(_) => true,
            DisplayRow::Line(l) => map.marks[*l as usize] != LineMark::None,
        };
        let mut changes: Vec<(u32, u32)> = Vec::new();
        let mut open: Option<u32> = None;
        for (i, r) in map.rows.iter().enumerate() {
            match (changed(r), open) {
                (true, None) => open = Some(i as u32),
                (false, Some(s)) => {
                    changes.push((s, i as u32 - 1));
                    open = None;
                }
                _ => {}
            }
        }
        if let Some(s) = open {
            changes.push((s, map.rows.len() as u32 - 1));
        }
        map.changes = changes;

        let kind = |r: &DisplayRow| match r {
            DisplayRow::Ghost(g) if map.ghosts[*g as usize].uncommitted => Some(RunKind::Uncommitted),
            DisplayRow::Ghost(_) => Some(RunKind::Removed),
            DisplayRow::Line(l) => match map.marks[*l as usize] {
                LineMark::None => None,
                LineMark::Branch => Some(RunKind::Added),
                LineMark::Uncommitted => Some(RunKind::Uncommitted),
            },
        };
        let mut runs: Vec<(u32, u32, RunKind, usize)> = Vec::new();
        let mut change = 0;
        for (i, r) in map.rows.iter().enumerate() {
            let i = i as u32;
            let Some(k) = kind(r) else { continue };
            while map.changes.get(change).is_some_and(|c| c.1 < i) {
                change += 1;
            }
            match runs.last_mut() {
                Some(last) if last.2 == k && last.1 + 1 == i && last.3 == change => last.1 = i,
                _ => runs.push((i, i, k, change)),
            }
        }
        map.runs = runs;
        map.max_ghost_chars = map.ghosts.iter().map(|g| g.text.chars().count()).max().unwrap_or(0);
        map
    }

    /// The file line a row shows, or for a ghost row, the next real line
    /// (the last line at the end of the file).
    pub fn line_of_row(&self, row: u32) -> u32 {
        let row = row.min(self.rows.len().saturating_sub(1) as u32);
        self.rows[row as usize..]
            .iter()
            .find_map(|r| match r {
                DisplayRow::Line(l) => Some(*l),
                DisplayRow::Ghost(_) => None,
            })
            .unwrap_or(self.line_row.len().saturating_sub(1) as u32)
    }
}

/// The status of one changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// The path in the project.
    pub rel: String,
    /// `A` added, `M` modified, `D` deleted, `?` untracked.
    pub status: char,
    pub added: u32,
    pub removed: u32,
}

/// What diff mode knows about the whole repository.
#[derive(Debug, Clone, Default)]
pub struct RepoStatus {
    /// Why there is no diff (not a git repository).
    pub error: Option<String>,
    /// The base branch, and whether it exists.
    pub base: String,
    pub base_missing: bool,
    /// The branch rubyscope picks when none was chosen.
    pub default_base: String,
    /// The checked-out branch (or a short commit when detached).
    pub head_name: String,
    pub head: Option<String>,
    pub merge_base: Option<String>,
    /// Changed on the branch since the merge-base.
    pub branch_files: Vec<ChangedFile>,
    /// Changed in the working tree since `HEAD`.
    pub uncommitted_files: Vec<ChangedFile>,
}

/// The marks of a file in the file tree, the tabs and the pickers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileMark {
    /// `A` or `M` (or `D`) on the branch.
    pub branch: Option<char>,
    pub uncommitted: bool,
}

impl RepoStatus {
    /// Lines added and removed on the branch, over all files.
    pub fn branch_totals(&self) -> (u32, u32) {
        self.branch_files.iter().fold((0, 0), |(a, d), f| (a + f.added, d + f.removed))
    }

    pub fn marks(&self) -> HashMap<String, FileMark> {
        let mut m: HashMap<String, FileMark> = HashMap::new();
        for f in &self.branch_files {
            m.entry(f.rel.clone()).or_default().branch = Some(f.status);
        }
        for f in &self.uncommitted_files {
            m.entry(f.rel.clone()).or_default().uncommitted = true;
        }
        m
    }

    /// Every changed file that is still on disk, sorted: the order that
    /// `]` goes through the files.
    pub fn changed_paths(&self) -> Vec<String> {
        let set: BTreeSet<&str> = self
            .branch_files
            .iter()
            .chain(&self.uncommitted_files)
            .filter(|f| f.status != 'D')
            .map(|f| f.rel.as_str())
            .collect();
        set.into_iter().map(str::to_string).collect()
    }

    /// The label in the top bar: `main…feature/order-refunds`.
    pub fn range_label(&self) -> String {
        format!("{}…{}", self.base, self.head_name)
    }
}

fn git_command(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(root)
        // Diff mode only reads: `git status` must not write the index.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["--literal-pathspecs", "-c", "core.quotepath=off"]);
    c
}

/// Runs git in `root`. Returns its output when it succeeds.
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = git_command(root).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_line(root: &Path, args: &[&str]) -> Option<String> {
    git(root, args).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

const DIFF: &[&str] = &["diff", "--no-color", "--no-ext-diff", "--no-textconv", "--no-renames", "-U0"];

/// Computes both diffs of one file. `text` is the file as the view shows it.
pub fn file_diff(root: &Path, rel: &str, text: &str, merge_base: Option<&str>, head: Option<&str>) -> FileDiff {
    let mut diff =
        FileDiff { merge_base: merge_base.map(String::from), head: head.map(String::from), ..Default::default() };
    let tracked =
        git_command(root).args(["ls-files", "--error-unmatch", "--", rel]).output().is_ok_and(|o| o.status.success());
    if !tracked {
        let untracked =
            git(root, &["ls-files", "--others", "--exclude-standard", "--", rel]).is_some_and(|s| !s.trim().is_empty());
        if untracked {
            diff.untracked = true;
            let n = git_line_count(text);
            if n > 0 {
                diff.uncommitted =
                    vec![Hunk { old_start: 0, old_count: 0, new_start: 1, new_count: n, ..Default::default() }];
            }
        }
        return diff;
    }
    let run = |from: &str, to: Option<&str>| -> (Vec<Hunk>, bool) {
        let mut args: Vec<&str> = DIFF.to_vec();
        args.push(from);
        args.extend(to);
        args.extend(["--", rel]);
        git(root, &args).map(|t| parse_diff(&t)).unwrap_or_default()
    };
    if head.is_some() {
        let (hunks, binary) = run("HEAD", None);
        diff.uncommitted = hunks;
        diff.binary |= binary;
    } else {
        // No commit yet: everything is new.
        let n = git_line_count(text);
        if n > 0 {
            diff.uncommitted =
                vec![Hunk { old_start: 0, old_count: 0, new_start: 1, new_count: n, ..Default::default() }];
        }
    }
    if let (Some(mb), Some(_)) = (merge_base, head) {
        let (hunks, binary) = run(mb, Some("HEAD"));
        diff.branch = hunks;
        diff.binary |= binary;
    }
    diff
}

/// The number of lines git counts: a last line without a newline counts,
/// the empty part after the last newline does not.
pub fn git_line_count(text: &str) -> u32 {
    let n = text.matches('\n').count() as u32;
    if text.is_empty() || text.ends_with('\n') { n } else { n + 1 }
}

/// `HEAD` and the base branch as commits. When this changes, the diffs are
/// made again.
pub fn fingerprint(root: &Path, base: &str) -> String {
    let head = git_line(root, &["rev-parse", "-q", "--verify", "HEAD"]).unwrap_or_default();
    let base = git_line(root, &["rev-parse", "-q", "--verify", &format!("{base}^{{commit}}")]).unwrap_or_default();
    format!("{head} {base}")
}

fn ref_exists(root: &Path, name: &str) -> bool {
    git_line(root, &["rev-parse", "-q", "--verify", &format!("{name}^{{commit}}")]).is_some()
}

/// `main`, else `master`, else what `origin/HEAD` points to.
pub fn default_base(root: &Path) -> Option<String> {
    for name in ["main", "master"] {
        if git_line(root, &["rev-parse", "-q", "--verify", &format!("refs/heads/{name}")]).is_some() {
            return Some(name.to_string());
        }
    }
    if let Some(r) = git_line(root, &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"]) {
        return Some(r);
    }
    ["origin/main", "origin/master"].into_iter().find(|n| ref_exists(root, n)).map(String::from)
}

/// Everything diff mode needs to know about the repository. `base` is the
/// branch the user chose, or none for the default.
pub fn repo_status(root: &Path, base: Option<&str>) -> RepoStatus {
    let mut st = RepoStatus::default();
    if git_line(root, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        st.error = Some("not a git repository".into());
        return st;
    }
    let prefix = git_line(root, &["rev-parse", "--show-prefix"]).unwrap_or_default();
    st.head = git_line(root, &["rev-parse", "-q", "--verify", "HEAD"]);
    st.head_name = match git_line(root, &["symbolic-ref", "-q", "--short", "HEAD"]) {
        Some(b) => b,
        None => st.head.as_deref().map(|s| s[..s.len().min(8)].to_string()).unwrap_or_else(|| "HEAD".into()),
    };
    st.default_base = default_base(root).unwrap_or_default();
    st.base = base.filter(|b| !b.is_empty()).map(String::from).unwrap_or_else(|| st.default_base.clone());
    if st.base.is_empty() || !ref_exists(root, &st.base) {
        st.base_missing = true;
    } else if st.head.is_some() {
        st.merge_base = git_line(root, &["merge-base", &st.base, "HEAD"]);
    }
    if let Some(mb) = st.merge_base.clone() {
        let names = git(root, &["diff", "--no-renames", "--relative", "-z", "--name-status", &mb, "HEAD"]);
        let nums = git(root, &["diff", "--no-renames", "--relative", "-z", "--numstat", &mb, "HEAD"]);
        st.branch_files = join_counts(parse_name_status(&names.unwrap_or_default()), &nums.unwrap_or_default());
    }
    let status = git(root, &["status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all", "--", "."]);
    let mut unc = parse_porcelain_status(&status.unwrap_or_default(), &prefix);
    if st.head.is_some() {
        let nums = git(root, &["diff", "--no-renames", "--relative", "-z", "--numstat", "HEAD"]).unwrap_or_default();
        unc = join_counts(unc, &nums);
    }
    for f in unc.iter_mut().filter(|f| f.status == '?' || (st.head.is_none() && f.status != 'D')) {
        let text = std::fs::read(root.join(&f.rel)).ok().filter(|b| b.len() < 4 << 20);
        f.added = text.map(|b| git_line_count(&String::from_utf8_lossy(&b))).unwrap_or(0);
    }
    st.uncommitted_files = unc;
    st
}

/// `git diff --name-status -z`: `M\0path\0A\0path\0`.
pub fn parse_name_status(text: &str) -> Vec<ChangedFile> {
    let mut out = Vec::new();
    let mut parts = text.split('\0').filter(|p| !p.is_empty());
    while let Some(code) = parts.next() {
        let status = code.chars().next().unwrap_or('M');
        // A rename or copy (when renames are on) has two paths: take the new one.
        let path = if matches!(status, 'R' | 'C') {
            parts.next();
            parts.next()
        } else {
            parts.next()
        };
        let Some(path) = path else { break };
        let status = match status {
            'A' | 'D' => status,
            _ => 'M',
        };
        out.push(ChangedFile { rel: path.to_string(), status, added: 0, removed: 0 });
    }
    out
}

/// `git status --porcelain=v1 -z`: `XY path\0`, with paths from the top of
/// the repository; `prefix` is the project folder in the repository.
pub fn parse_porcelain_status(text: &str, prefix: &str) -> Vec<ChangedFile> {
    let mut out = Vec::new();
    let mut parts = text.split('\0').filter(|p| !p.is_empty());
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let (xy, path) = entry.split_at(3);
        let xy: Vec<char> = xy.chars().take(2).collect();
        let (x, y) = (xy[0], xy[1]);
        if matches!(x, 'R' | 'C') {
            // The old path follows.
            parts.next();
        }
        let Some(rel) = path.strip_prefix(prefix) else { continue };
        let status = if x == '?' {
            '?'
        } else if x == 'D' || y == 'D' {
            'D'
        } else if x == 'A' {
            'A'
        } else {
            'M'
        };
        out.push(ChangedFile { rel: rel.to_string(), status, added: 0, removed: 0 });
    }
    out
}

/// Adds the counts of `git diff --numstat -z` (`3\t1\tpath\0`; `-\t-\t` for
/// a binary file) to the files.
pub fn join_counts(mut files: Vec<ChangedFile>, numstat: &str) -> Vec<ChangedFile> {
    let mut counts: HashMap<&str, (u32, u32)> = HashMap::new();
    for entry in numstat.split('\0').filter(|p| !p.is_empty()) {
        let mut f = entry.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (f.next(), f.next(), f.next()) else { continue };
        counts.insert(path, (a.parse().unwrap_or(0), d.parse().unwrap_or(0)));
    }
    for f in &mut files {
        if let Some((a, d)) = counts.get(f.rel.as_str()) {
            (f.added, f.removed) = (*a, *d);
        }
    }
    files
}

/// A branch for the base branch picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// `main` or `origin/main`.
    pub name: String,
    pub remote: bool,
    /// The time of its last commit, in seconds since the epoch.
    pub time: i64,
    /// The checked-out branch.
    pub is_head: bool,
    /// The author of its last commit.
    pub author: String,
}

pub const BRANCH_FORMAT: &str = "%(refname)%09%(refname:short)%09%(committerdate:unix)%09%(HEAD)%09%(authorname)";

/// The local and remote branches, local first, each newest first.
pub fn branches(root: &Path) -> Vec<Branch> {
    let format = format!("--format={BRANCH_FORMAT}");
    git(root, &["for-each-ref", &format, "refs/heads", "refs/remotes"]).map(|t| parse_branches(&t)).unwrap_or_default()
}

/// Reads `git for-each-ref` with [`BRANCH_FORMAT`]. `origin/HEAD` is only a
/// pointer to another branch, so it is left out.
pub fn parse_branches(text: &str) -> Vec<Branch> {
    let mut out: Vec<Branch> = text
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let (full, short, time) = (f.next()?, f.next()?, f.next()?);
            let (head, author) = (f.next().unwrap_or(""), f.next().unwrap_or(""));
            if full.ends_with("/HEAD") {
                return None;
            }
            Some(Branch {
                name: short.to_string(),
                remote: full.starts_with("refs/remotes/"),
                time: time.parse().unwrap_or(0),
                is_head: head.trim() == "*",
                author: author.trim().to_string(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.remote.cmp(&b.remote).then(b.time.cmp(&a.time)).then(a.name.cmp(&b.name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(os: u32, oc: u32, ns: u32, nc: u32, removed: &[&str]) -> Hunk {
        Hunk {
            old_start: os,
            old_count: oc,
            new_start: ns,
            new_count: nc,
            removed: removed.iter().map(|s| s.to_string()).collect(),
            tokens: Vec::new(),
        }
    }

    #[test]
    fn hunk_headers() {
        assert_eq!(parse_hunk_header("@@ -5,0 +6,2 @@ def ship!"), Some(hunk(5, 0, 6, 2, &[])));
        assert_eq!(parse_hunk_header("@@ -3 +3 @@"), Some(hunk(3, 1, 3, 1, &[])));
        assert_eq!(parse_hunk_header("@@ -10,2 +9,0 @@"), Some(hunk(10, 2, 9, 0, &[])));
        assert_eq!(parse_hunk_header("@@ -0,0 +1,4 @@"), Some(hunk(0, 0, 1, 4, &[])));
        assert_eq!(parse_hunk_header("+++ b/app/models/order.rb"), None);
        assert_eq!(parse_hunk_header("@@ -x +1 @@"), None);
    }

    #[test]
    fn parses_a_diff_with_removed_lines() {
        let text = "\
diff --git a/app/models/order.rb b/app/models/order.rb
index 1111111..2222222 100644
--- a/app/models/order.rb
+++ b/app/models/order.rb
@@ -4,0 +5 @@ class Order < ApplicationRecord
+  has_many :refunds
@@ -7 +8 @@ class Order < ApplicationRecord
-  enum :status, { cart: 0 }
+  enum :status, { cart: 0, refunded: 3 }
@@ -12,2 +12,0 @@ def total_cents
---- a line that starts with dashes
-  # old comment
";
        let (hunks, binary) = parse_diff(text);
        assert!(!binary);
        assert_eq!(hunks.len(), 3);
        assert_eq!(hunks[0], hunk(4, 0, 5, 1, &[]));
        assert_eq!(hunks[1].removed, vec!["  enum :status, { cart: 0 }"]);
        assert_eq!(hunks[2].removed, vec!["--- a line that starts with dashes", "  # old comment"]);
        assert_eq!(hunks[2].anchor(), 12);
    }

    #[test]
    fn no_newline_at_end_and_binary() {
        let text = "\
@@ -3 +3 @@
-end
\\ No newline at end of file
+end
";
        let (hunks, _) = parse_diff(text);
        assert_eq!(hunks, vec![hunk(3, 1, 3, 1, &["end"])]);
        let (hunks, binary) =
            parse_diff("diff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n");
        assert!(binary && hunks.is_empty());
    }

    #[test]
    fn line_counts_like_git() {
        assert_eq!(git_line_count(""), 0);
        assert_eq!(git_line_count("a\nb\n"), 2);
        assert_eq!(git_line_count("a\nb"), 2);
    }

    fn rows(map: &RowMap) -> String {
        map.rows
            .iter()
            .map(|r| match r {
                DisplayRow::Line(l) => {
                    let m = match map.marks[*l as usize] {
                        LineMark::None => "",
                        LineMark::Branch => "+",
                        LineMark::Uncommitted => "~",
                    };
                    format!("{m}{}", l + 1)
                }
                DisplayRow::Ghost(g) => {
                    let g = &map.ghosts[*g as usize];
                    format!("{}{}", if g.uncommitted { "u-" } else { "-" }, g.text)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn rows_without_changes_are_the_lines() {
        let map = RowMap::build(&FileDiff::default(), 3, 4);
        assert_eq!(rows(&map), "1 2 3 4");
        assert!(map.changes.is_empty());
        assert_eq!(map.line_row, vec![0, 1, 2, 3]);
    }

    #[test]
    fn branch_changes_add_ghost_rows() {
        // Base: a b c d. HEAD: a B c d e (b changed, e added at the end).
        let diff = FileDiff { branch: vec![hunk(2, 1, 2, 1, &["b"]), hunk(4, 0, 5, 1, &[])], ..Default::default() };
        let map = RowMap::build(&diff, 5, 6);
        assert_eq!(rows(&map), "1 -b +2 3 4 +5 6");
        assert_eq!(map.line_row, vec![0, 2, 3, 4, 5, 6]);
        assert_eq!(map.changes, vec![(1, 2), (5, 5)]);
        assert_eq!(map.line_of_row(1), 1, "a ghost row belongs to the next line");
    }

    #[test]
    fn lines_removed_at_the_end_and_the_top() {
        // Base: x a b y. HEAD: a b. (x removed at the top, y at the end.)
        let diff = FileDiff { branch: vec![hunk(1, 1, 0, 0, &["x"]), hunk(4, 1, 2, 0, &["y"])], ..Default::default() };
        let map = RowMap::build(&diff, 2, 3);
        assert_eq!(rows(&map), "-x 1 2 -y 3");
        assert_eq!(map.line_of_row(3), 2);
    }

    #[test]
    fn uncommitted_changes_on_top_of_the_branch() {
        // Base: a b c. HEAD: a B c N (b changed, N added).
        // Disk: a B c2 N (c changed in the working tree).
        let diff = FileDiff {
            branch: vec![hunk(2, 1, 2, 1, &["b"]), hunk(3, 0, 4, 1, &[])],
            uncommitted: vec![hunk(3, 1, 3, 1, &["c"])],
            ..Default::default()
        };
        let map = RowMap::build(&diff, 4, 5);
        assert_eq!(rows(&map), "1 -b +2 u-c ~3 +4 5");
        assert_eq!(map.changes, vec![(1, 5)]);
        use RunKind::*;
        assert_eq!(map.runs, vec![(1, 1, Removed, 0), (2, 2, Added, 0), (3, 4, Uncommitted, 0), (5, 5, Added, 0)]);
    }

    #[test]
    fn branch_ghosts_before_a_line_removed_in_the_working_tree() {
        // Base: a b c. HEAD: a c (b removed). Disk: a (c removed, not committed).
        let diff = FileDiff {
            branch: vec![hunk(2, 1, 1, 0, &["b"])],
            uncommitted: vec![hunk(2, 1, 1, 0, &["c"])],
            ..Default::default()
        };
        let map = RowMap::build(&diff, 1, 2);
        assert_eq!(rows(&map), "1 -b u-c 2");
    }

    #[test]
    fn untracked_file_is_all_new() {
        let diff = FileDiff { untracked: true, uncommitted: vec![hunk(0, 0, 1, 2, &[])], ..Default::default() };
        let map = RowMap::build(&diff, 2, 3);
        assert_eq!(rows(&map), "~1 ~2 3");
        assert_eq!(map.changes, vec![(0, 1)]);
    }

    #[test]
    fn a_diff_that_does_not_fit_the_text_does_not_panic() {
        let diff = FileDiff {
            branch: vec![hunk(1, 3, 40, 9, &["a", "b", "c"])],
            uncommitted: vec![hunk(90, 2, 70, 5, &["x", "y"])],
            ..Default::default()
        };
        let map = RowMap::build(&diff, 3, 4);
        let lines: Vec<u32> = map
            .rows
            .iter()
            .filter_map(|r| match r {
                DisplayRow::Line(l) => Some(*l),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec![0, 1, 2, 3], "every line has one row, in order");
    }

    #[test]
    fn name_status_and_numstat() {
        let files = parse_name_status("M\0app/models/order.rb\0A\0app/models/refund.rb\0D\0old.rb\0");
        let files = join_counts(files, "3\t1\tapp/models/order.rb\09\t0\tapp/models/refund.rb\0-\t-\tlogo.png\0");
        assert_eq!(
            files,
            vec![
                ChangedFile { rel: "app/models/order.rb".into(), status: 'M', added: 3, removed: 1 },
                ChangedFile { rel: "app/models/refund.rb".into(), status: 'A', added: 9, removed: 0 },
                ChangedFile { rel: "old.rb".into(), status: 'D', added: 0, removed: 0 },
            ]
        );
        // A rename (when renames are on) keeps the new path.
        let r = parse_name_status("R100\0old/name.rb\0new/name.rb\0");
        assert_eq!(r[0].rel, "new/name.rb");
    }

    #[test]
    fn porcelain_status() {
        let text = " M shop/app/models/order.rb\0?? shop/app/models/refund.rb\0A  shop/db/new.rb\0 D shop/gone.rb\0 M other/x.rb\0";
        let files = parse_porcelain_status(text, "shop/");
        let got: Vec<(&str, char)> = files.iter().map(|f| (f.rel.as_str(), f.status)).collect();
        assert_eq!(
            got,
            vec![("app/models/order.rb", 'M'), ("app/models/refund.rb", '?'), ("db/new.rb", 'A'), ("gone.rb", 'D'),]
        );
    }

    #[test]
    fn branch_list() {
        let text = "\
refs/heads/main\tmain\t1760000000\t \tLeo Ortiz
refs/heads/feature/order-refunds\tfeature/order-refunds\t1761000000\t*\tAna Novak
refs/remotes/origin/HEAD\torigin\t1760000000\t
refs/remotes/origin/main\torigin/main\t1760000000\t
refs/remotes/origin/release/2.4\torigin/release/2.4\t1750000000\t
";
        let b = parse_branches(text);
        let names: Vec<&str> = b.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["feature/order-refunds", "main", "origin/main", "origin/release/2.4"]);
        assert!(b[0].is_head && !b[0].remote);
        assert_eq!(b[0].author, "Ana Novak");
        assert!(b[2].remote);
        assert_eq!(b[2].author, "");
    }

    #[test]
    fn status_counts_and_order() {
        let st = RepoStatus {
            base: "main".into(),
            head_name: "feature/order-refunds".into(),
            branch_files: vec![
                ChangedFile { rel: "b.rb".into(), status: 'M', added: 3, removed: 1 },
                ChangedFile { rel: "gone.rb".into(), status: 'D', added: 0, removed: 4 },
            ],
            uncommitted_files: vec![ChangedFile { rel: "a.rb".into(), status: '?', added: 2, removed: 0 }],
            ..Default::default()
        };
        assert_eq!(st.branch_totals(), (3, 5));
        assert_eq!(st.changed_paths(), vec!["a.rb", "b.rb"]);
        assert_eq!(st.range_label(), "main…feature/order-refunds");
        let m = st.marks();
        assert_eq!(m["b.rb"], FileMark { branch: Some('M'), uncommitted: false });
        assert!(m["a.rb"].uncommitted);
    }
}
