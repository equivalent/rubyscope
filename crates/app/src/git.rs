//! Git blame for the blame panel, and links to GitHub.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Commit {
    pub sha: String,
    pub author: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
    /// The author's time zone, in seconds east of UTC.
    pub tz: i32,
    pub summary: String,
    /// 0 for the newest commit of the file, 1 for the next one, and so on.
    pub rank: u32,
}

impl Commit {
    pub fn is_uncommitted(&self) -> bool {
        self.sha.bytes().all(|b| b == b'0')
    }

    /// `Novak` for `Ana Novak`.
    pub fn last_name(&self) -> &str {
        self.author.split_whitespace().last().unwrap_or("")
    }

    /// The commit date as the author saw it: `Today`, `Yesterday` or
    /// `16/07/2025`.
    pub fn short_date(&self, now: i64) -> String {
        let day = |t: i64| (t + self.tz as i64).div_euclid(86_400);
        let (then, today) = (day(self.time), day(now));
        if then == today {
            return "Today".into();
        }
        if then + 1 == today {
            return "Yesterday".into();
        }
        let (y, m, d) = civil_date(then);
        format!("{d:02}/{m:02}/{y}")
    }
}

/// The year, month and day of a day number (days since 1970-01-01).
fn civil_date(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

/// `+0200` as seconds east of UTC.
fn parse_tz(s: &str) -> i32 {
    let (sign, digits) = match s.as_bytes().first() {
        Some(b'-') => (-1, &s[1..]),
        Some(b'+') => (1, &s[1..]),
        _ => (1, s),
    };
    let n: i32 = digits.parse().unwrap_or(0);
    sign * ((n / 100) * 3600 + (n % 100) * 60)
}

#[derive(Debug, Clone, Default)]
pub struct Blame {
    pub commits: Vec<Commit>,
    /// The commit of each line (0-based), as an index into `commits`.
    pub lines: Vec<u32>,
}

/// Runs `git blame --porcelain` on a file of the repo at `root`.
pub fn blame(root: &Path, rel: &str) -> Option<Blame> {
    let out = Command::new("git").current_dir(root).args(["blame", "--porcelain", "--", rel]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_porcelain(&String::from_utf8_lossy(&out.stdout)))
}

pub fn parse_porcelain(text: &str) -> Blame {
    let mut blame = Blame::default();
    let mut by_sha: std::collections::HashMap<String, u32> = Default::default();
    let mut current: Option<u32> = None;
    for line in text.lines() {
        // A content line ends the headers of one blamed line.
        if line.starts_with('\t') {
            if let Some(c) = current {
                blame.lines.push(c);
            }
            continue;
        }
        let first = line.split(' ').next().unwrap_or("");
        if first.len() == 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            let idx = *by_sha.entry(first.to_string()).or_insert_with(|| {
                blame.commits.push(Commit {
                    sha: first.to_string(),
                    author: String::new(),
                    time: 0,
                    tz: 0,
                    summary: String::new(),
                    rank: 0,
                });
                (blame.commits.len() - 1) as u32
            });
            current = Some(idx);
            continue;
        }
        let Some(c) = current else { continue };
        let commit = &mut blame.commits[c as usize];
        if let Some(a) = line.strip_prefix("author ") {
            commit.author = a.to_string();
        } else if let Some(t) = line.strip_prefix("author-time ") {
            commit.time = t.parse().unwrap_or(0);
        } else if let Some(t) = line.strip_prefix("author-tz ") {
            commit.tz = parse_tz(t);
        } else if let Some(s) = line.strip_prefix("summary ") {
            commit.summary = s.to_string();
        }
    }
    let mut order: Vec<usize> = (0..blame.commits.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(blame.commits[i].time));
    for (rank, i) in order.into_iter().enumerate() {
        blame.commits[i].rank = rank as u32;
    }
    blame
}

/// `3 months ago`, `2 days ago`, `just now`.
pub fn relative_time(then: i64, now: i64) -> String {
    let secs = (now - then).max(0);
    let (n, unit) = match secs {
        s if s < 60 => return "just now".into(),
        s if s < 3600 => (s / 60, "minute"),
        s if s < 86_400 => (s / 3600, "hour"),
        s if s < 86_400 * 30 => (s / 86_400, "day"),
        s if s < 86_400 * 365 => (s / (86_400 * 30), "month"),
        s => (s / (86_400 * 365), "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// `https://github.com/owner/repo` from the `origin` remote.
pub fn github_url(root: &Path) -> Option<String> {
    let out = Command::new("git").current_dir(root).args(["remote", "get-url", "origin"]).output().ok()?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    parse_github_remote(&url)
}

pub fn parse_github_remote(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    Some(format!("https://github.com/{rest}"))
}

/// The PR that a commit came from, through the `gh` CLI. Falls back to
/// the commit page.
pub fn commit_or_pr_url(root: &Path, sha: &str) -> Option<String> {
    let base = github_url(root)?;
    let repo = base.trim_start_matches("https://github.com/");
    let pr = Command::new("gh")
        .current_dir(root)
        .args(["api", &format!("repos/{repo}/commits/{sha}/pulls"), "--jq", ".[0].html_url"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|u| u.starts_with("https://"));
    Some(pr.unwrap_or_else(|| format!("{base}/commit/{sha}")))
}

/// What `git pull` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pull {
    UpToDate,
    /// HEAD moved by this many commits.
    Pulled(u32),
    /// The pull stopped on conflicts in these files. `rebase` is true when
    /// a rebase stopped, false for a merge. Nothing is fixed or aborted.
    Conflicts {
        files: Vec<String>,
        rebase: bool,
    },
    /// Git refused or failed: its message.
    Failed(String),
}

impl Pull {
    /// The message for the toast.
    pub fn message(&self) -> String {
        match self {
            Pull::UpToDate => "Already up to date".into(),
            Pull::Pulled(1) => "Pulled 1 commit".into(),
            Pull::Pulled(n) => format!("Pulled {n} commits"),
            Pull::Conflicts { files, rebase } => {
                let what = if *rebase { "rebase" } else { "merge" };
                let n = files.len();
                let mut list = files.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
                if n > 3 {
                    list.push_str(&format!(" and {} more", n - 3));
                }
                format!(
                    "The pull stopped on {what} conflicts in {n} file{}: {list}. Fix them in git, or run git {what} --abort.",
                    if n == 1 { "" } else { "s" }
                )
            }
            Pull::Failed(e) => format!("Pull failed: {e}"),
        }
    }
}

fn git_out(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").current_dir(root).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Runs `git pull` for the checked-out branch. It never asks for input,
/// and it does not try to fix conflicts: it says which files have them.
/// With no `pull.rebase` or `pull.ff` setting, it merges, as git did
/// before it started to ask.
pub fn pull(root: &Path) -> Pull {
    let before = git_out(root, &["rev-parse", "HEAD"]);
    let mut cmd = Command::new("git");
    cmd.current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .stdin(std::process::Stdio::null())
        .args(["pull", "--no-edit"]);
    let configured = ["pull.rebase", "pull.ff"].iter().any(|k| git_out(root, &["config", "--get", k]).is_some());
    if !configured {
        cmd.arg("--no-rebase");
    }
    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => return Pull::Failed(format!("could not run git: {e}")),
    };
    let conflicts = git_out(root, &["diff", "--name-only", "--diff-filter=U"])
        .map(|s| s.lines().map(String::from).collect::<Vec<_>>())
        .unwrap_or_default();
    if !conflicts.is_empty() {
        let in_merge = git_out(root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_some();
        return Pull::Conflicts { files: conflicts, rebase: !in_merge };
    }
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let lines: Vec<&str> = err
            .lines()
            .map(|l| l.trim().trim_start_matches("fatal: ").trim_start_matches("error: "))
            .filter(|l| !l.is_empty() && !l.starts_with("hint:"))
            .collect();
        let msg = if lines.is_empty() { "git pull did not work".to_string() } else { lines.join(" ") };
        return Pull::Failed(msg);
    }
    let after = git_out(root, &["rev-parse", "HEAD"]);
    match (before, after) {
        (Some(b), Some(a)) if a != b => {
            let n = git_out(root, &["rev-list", "--count", &format!("{b}..{a}")]).and_then(|n| n.parse().ok());
            Pull::Pulled(n.unwrap_or(1))
        }
        _ => Pull::UpToDate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain() {
        let text = "\
1111111111111111111111111111111111111111 1 1 2
author Ana
author-time 1700000000
summary First commit
filename a.rb
\tline one
1111111111111111111111111111111111111111 2 2
\tline two
2222222222222222222222222222222222222222 3 3 1
author Tomas
author-time 1760000000
author-tz +0200
summary Second
filename a.rb
\tline three
";
        let b = parse_porcelain(text);
        assert_eq!(b.lines, vec![0, 0, 1]);
        assert_eq!(b.commits[1].author, "Tomas");
        assert_eq!(b.commits[0].summary, "First commit");
        assert_eq!(b.commits[1].tz, 7200);
        // The newer commit comes first.
        assert_eq!((b.commits[0].rank, b.commits[1].rank), (1, 0));
    }

    #[test]
    fn short_dates() {
        let c = |time: i64, tz: i32| Commit {
            sha: String::new(),
            author: "Ana Novak".into(),
            time,
            tz,
            summary: String::new(),
            rank: 0,
        };
        // 2025-07-16 10:00 UTC.
        let t = 1_752_660_000;
        assert_eq!(c(t, 0).short_date(t + 3600), "Today");
        assert_eq!(c(t, 0).short_date(t + 86_400), "Yesterday");
        assert_eq!(c(t, 0).short_date(t + 86_400 * 3), "16/07/2025");
        // 23:30 in UTC is the next day for an author in UTC+2.
        assert_eq!(c(1_752_708_600, 7200).short_date(t + 86_400 * 3), "17/07/2025");
        assert_eq!(c(951_782_400, 0).short_date(t), "29/02/2000");
        assert_eq!(c(t, 0).last_name(), "Novak");
        assert_eq!(parse_tz("-0530"), -(5 * 3600 + 30 * 60));
    }

    #[test]
    fn github_remotes() {
        assert_eq!(
            parse_github_remote("git@github.com:acme/shop.git").as_deref(),
            Some("https://github.com/acme/shop")
        );
        assert_eq!(parse_github_remote("https://github.com/a/b").as_deref(), Some("https://github.com/a/b"));
        assert_eq!(parse_github_remote("https://gitlab.com/a/b"), None);
    }

    #[test]
    fn relative_times() {
        assert_eq!(relative_time(0, 30), "just now");
        assert_eq!(relative_time(0, 86_400 * 3), "3 days ago");
        assert_eq!(relative_time(0, 86_400 * 400), "1 year ago");
    }

    /// A remote, a clone of it with `order.rb`, and a second clone to push
    /// new commits from.
    fn pull_fixture(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("rubyscope-pull-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |cwd: &Path, args: &[&str]| {
            let out = Command::new("git").current_dir(cwd).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        run(&dir, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        for clone in ["mine", "theirs"] {
            run(&dir, &["clone", "-q", "remote.git", clone]);
            let c = dir.join(clone);
            for (k, v) in
                [("user.name", "Test Author"), ("user.email", "test@example.com"), ("commit.gpgsign", "false")]
            {
                run(&c, &["config", k, v]);
            }
            if clone == "mine" {
                std::fs::write(c.join("order.rb"), "class Order\nend\n").unwrap();
                run(&c, &["add", "."]);
                run(&c, &["commit", "-q", "-m", "Order"]);
                run(&c, &["push", "-q", "-u", "origin", "main"]);
            }
        }
        let theirs = dir.join("theirs");
        run(&theirs, &["pull", "-q", "origin", "main"]);
        run(&theirs, &["branch", "-q", "--set-upstream-to", "origin/main"]);
        (dir.join("mine"), theirs)
    }

    fn commit_and_push(repo: &Path, text: &str) {
        std::fs::write(repo.join("order.rb"), text).unwrap();
        for args in [&["commit", "-q", "-am", "Change"][..], &["push", "-q"][..]] {
            let out = Command::new("git").current_dir(repo).args(args).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        }
    }

    #[test]
    fn pull_brings_new_commits() {
        let (mine, theirs) = pull_fixture("new");
        assert_eq!(pull(&mine), Pull::UpToDate);
        commit_and_push(&theirs, "class Order\n  has_many :refunds\nend\n");
        assert_eq!(pull(&mine), Pull::Pulled(1));
        assert!(std::fs::read_to_string(mine.join("order.rb")).unwrap().contains("refunds"));
        assert_eq!(pull(&mine).message(), "Already up to date");
    }

    /// Both clones change the same line; `mine` pulls with `pull.rebase`
    /// set to `rebase`.
    fn pull_into_a_conflict(name: &str, rebase: &str) -> (std::path::PathBuf, Pull) {
        let (mine, theirs) = pull_fixture(name);
        commit_and_push(&theirs, "class Order\n  has_many :refunds\nend\n");
        std::fs::write(mine.join("order.rb"), "class Order\n  has_many :items\nend\n").unwrap();
        for args in [&["config", "pull.rebase", rebase][..], &["commit", "-q", "-am", "Items"][..]] {
            let out = Command::new("git").current_dir(&mine).args(args).output().unwrap();
            assert!(out.status.success());
        }
        let result = pull(&mine);
        (mine, result)
    }

    #[test]
    fn pull_reports_merge_conflicts_and_leaves_them() {
        let (mine, result) = pull_into_a_conflict("merge", "false");
        assert_eq!(result, Pull::Conflicts { files: vec!["order.rb".into()], rebase: false });
        assert!(result.message().contains("merge conflicts in 1 file: order.rb"), "{}", result.message());
        assert!(result.message().contains("git merge --abort"), "{}", result.message());
        // The conflict is still there for the user to fix.
        assert!(std::fs::read_to_string(mine.join("order.rb")).unwrap().contains("<<<<<<<"));
    }

    #[test]
    fn pull_reports_rebase_conflicts() {
        let (_, result) = pull_into_a_conflict("rebase", "true");
        assert_eq!(result, Pull::Conflicts { files: vec!["order.rb".into()], rebase: true });
        assert!(result.message().contains("git rebase --abort"), "{}", result.message());
    }

    #[test]
    fn pull_without_an_upstream_says_why() {
        let (mine, _) = pull_fixture("upstream");
        let out = Command::new("git").current_dir(&mine).args(["checkout", "-q", "-b", "feature/x"]).output().unwrap();
        assert!(out.status.success());
        match pull(&mine) {
            Pull::Failed(msg) => assert!(msg.contains("no tracking information"), "{msg}"),
            other => panic!("{other:?}"),
        }
    }
}
