//! `rubyscope://` links: open a file at a line from a browser or a chat.
//!
//!     rubyscope://open?file=/abs/path/app/models/order.rb&line=12
//!     rubyscope://open?repo=shop&path=app/models/order.rb&line=12
//!
//! The `repo=` form finds the file in `~/git/<repo>` on this machine, so one
//! link works on every computer, whatever its home folder is.
//!
//! A link comes from a web page, so it is data. Nothing in it is run, and it
//! is never given to a shell. The line must be digits, the file must exist,
//! and a `repo=` path must stay in its repo, else the link is refused.

use std::path::{Path, PathBuf};

/// True when the argument is a `rubyscope:` link and not a file name.
pub fn is_link(arg: &str) -> bool {
    arg.get(..10).is_some_and(|s| s.eq_ignore_ascii_case("rubyscope:"))
}

/// A `rubyscope://open?…` link → the file and its 0-based line. `home` is
/// the folder that has `git/<repo>` in it. The error is a message for the
/// user.
pub fn parse_link(url: &str, home: &Path) -> Result<(PathBuf, Option<u32>), String> {
    let rest = url
        .get(..12)
        .filter(|s| s.eq_ignore_ascii_case("rubyscope://"))
        .map(|_| &url[12..])
        .ok_or_else(|| format!("not a rubyscope:// link: {url}"))?;
    let rest = rest.split_once('#').map_or(rest, |(r, _)| r);
    let (action, query) = rest.split_once('?').unwrap_or((rest, ""));
    if action.trim_end_matches('/') != "open" {
        return Err(format!("unknown link action (only open is known): {action}"));
    }
    let q = Query::parse(query)?;
    let (file, repo, rel, line) = (q.get("file"), q.get("repo"), q.get("path"), q.get("line"));

    let line = match line {
        "" => None,
        l if (1..=7).contains(&l.len()) && l.bytes().all(|b| b.is_ascii_digit()) => {
            Some(l.parse::<u32>().map_err(|_| format!("line is not a number: {l}"))?.saturating_sub(1))
        }
        l => return Err(format!("line is not a number: {l}")),
    };

    let mut root = None;
    let file = if !repo.is_empty() {
        let good_name = repo.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
        if !good_name || repo == "." || repo == ".." {
            return Err(format!("bad repo name: {repo}"));
        }
        let dir = home.join("git").join(repo);
        if !dir.is_dir() {
            return Err(format!("no folder ~/git/{repo} on this machine"));
        }
        if rel.is_empty() {
            return Err("the link has repo= but no path=".into());
        }
        if rel.starts_with('/') {
            return Err(format!("path must be relative to the repo: {rel}"));
        }
        root = Some(dir.canonicalize().map_err(|e| format!("cannot read ~/git/{repo}: {e}"))?);
        dir.join(rel)
    } else if file.is_empty() {
        return Err("the link has no file= (or repo= and path=)".into());
    } else {
        PathBuf::from(file)
    };

    if !file.is_absolute() {
        return Err(format!("file must be an absolute path: {}", file.display()));
    }
    let resolved = file.canonicalize().map_err(|_| format!("no such file: {}", file.display()))?;
    if !resolved.is_file() {
        return Err(format!("not a file: {}", file.display()));
    }
    if let Some(root) = root
        && !resolved.starts_with(&root)
    {
        return Err(format!("path leaves the repo: {rel}"));
    }
    Ok((resolved, line))
}

/// `file:///srv/code/a%20b.rb` → `/srv/code/a b.rb`. A file manager may pass
/// a file this way to the launcher entry (`%u`).
pub fn file_uri(arg: &str) -> Option<PathBuf> {
    let rest = arg.strip_prefix("file://")?;
    // `file://localhost/path`; any other host is not a local file.
    let path = if rest.starts_with('/') { rest } else { rest.strip_prefix("localhost")? };
    decode(path, false).ok().map(PathBuf::from)
}

/// The query of a link. The first value of a key wins, as in the
/// `nvim://` handler.
struct Query(Vec<(String, String)>);

impl Query {
    fn parse(query: &str) -> Result<Query, String> {
        let mut pairs = Vec::new();
        for part in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            pairs.push((decode(k, true)?, decode(v, true)?));
        }
        Ok(Query(pairs))
    }

    fn get(&self, key: &str) -> &str {
        self.0.iter().find(|(k, v)| k == key && !v.is_empty()).map_or("", |(_, v)| v.as_str())
    }
}

/// Undoes `%XX` (and `+` for a space, in a query). A `%` without two hex
/// digits stays as it is. Text that is not UTF-8 is refused.
fn decode(s: &str, plus_is_space: bool) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' if plus_is_space => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).map_err(|_| format!("the link is not UTF-8 text: {s}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up home folder with `git/shop/app/models/order.rb` and a file
    /// outside the repo.
    fn fake_home(name: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("rubyscope-link-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("git/shop/app/models")).unwrap();
        std::fs::create_dir_all(home.join("git/shop/app/my models")).unwrap();
        std::fs::write(home.join("git/shop/app/models/order.rb"), "class Order\nend\n").unwrap();
        std::fs::write(home.join("git/shop/app/my models/line item.rb"), "class LineItem\nend\n").unwrap();
        std::fs::write(home.join("secret.rb"), "x = 1\n").unwrap();
        home.canonicalize().unwrap()
    }

    #[test]
    fn opens_an_absolute_file_at_a_line() {
        let home = fake_home("abs");
        let order = home.join("git/shop/app/models/order.rb");
        let url = format!("rubyscope://open?file={}&line=12", order.display());
        assert_eq!(parse_link(&url, &home), Ok((order.clone(), Some(11))));
        // No line: open the file at the top.
        let url = format!("rubyscope://open?file={}", order.display());
        assert_eq!(parse_link(&url, &home), Ok((order, None)));
    }

    #[test]
    fn decodes_the_query() {
        let home = fake_home("decode");
        let item = home.join("git/shop/app/my models/line item.rb");
        let encoded = item.display().to_string().replace(' ', "%20");
        let url = format!("rubyscope://open?file={encoded}&line=2");
        assert_eq!(parse_link(&url, &home), Ok((item.clone(), Some(1))));
        let url = "rubyscope://open?repo=shop&path=app/my+models/line%20item.rb&line=1#top";
        assert_eq!(parse_link(url, &home), Ok((item, Some(0))));
    }

    #[test]
    fn finds_a_repo_file_in_home_git() {
        let home = fake_home("repo");
        let url = "rubyscope://open?repo=shop&path=app/models/order.rb&line=181";
        assert_eq!(parse_link(url, &home), Ok((home.join("git/shop/app/models/order.rb"), Some(180))));
        // repo= wins over file=, as in the nvim:// handler.
        let url = "rubyscope://open?file=/nowhere.rb&repo=shop&path=app/models/order.rb";
        assert!(parse_link(url, &home).is_ok());
    }

    #[test]
    fn refuses_a_bad_line() {
        let home = fake_home("line");
        let order = home.join("git/shop/app/models/order.rb");
        for line in ["12a", "-1", "1.5", "12345678", "%31%32x", "1;rm"] {
            let url = format!("rubyscope://open?file={}&line={line}", order.display());
            let err = parse_link(&url, &home).unwrap_err();
            assert!(err.starts_with("line is not a number"), "{line}: {err}");
        }
    }

    #[test]
    fn refuses_a_missing_file() {
        let home = fake_home("missing");
        let url = format!("rubyscope://open?file={}/git/shop/app/models/gone.rb&line=1", home.display());
        assert!(parse_link(&url, &home).unwrap_err().starts_with("no such file"));
        let url = "rubyscope://open?repo=shop&path=app/models/gone.rb";
        assert!(parse_link(url, &home).unwrap_err().starts_with("no such file"));
        let url = "rubyscope://open?repo=nope&path=app/models/order.rb";
        assert_eq!(parse_link(url, &home).unwrap_err(), "no folder ~/git/nope on this machine");
        // A folder is not a file.
        let url = "rubyscope://open?repo=shop&path=app/models";
        assert!(parse_link(url, &home).unwrap_err().starts_with("not a file"));
    }

    #[test]
    fn refuses_a_path_out_of_the_repo() {
        let home = fake_home("escape");
        for path in ["../../secret.rb", "app/../../../secret.rb", "app%2F..%2F..%2F..%2Fsecret.rb"] {
            let url = format!("rubyscope://open?repo=shop&path={path}");
            assert_eq!(
                parse_link(&url, &home).unwrap_err(),
                format!("path leaves the repo: {}", path.replace("%2F", "/"))
            );
        }
        let url = "rubyscope://open?repo=shop&path=/etc/hostname";
        assert!(parse_link(url, &home).unwrap_err().starts_with("path must be relative"));
        // A link in the repo that points out of it.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(home.join("secret.rb"), home.join("git/shop/app/models/link.rb")).unwrap();
            let url = "rubyscope://open?repo=shop&path=app/models/link.rb";
            assert_eq!(parse_link(url, &home).unwrap_err(), "path leaves the repo: app/models/link.rb");
        }
    }

    #[test]
    fn refuses_other_links() {
        let home = fake_home("other");
        let order = home.join("git/shop/app/models/order.rb");
        for repo in ["..", ".", "a/b", "a b", ""] {
            let url = format!("rubyscope://open?repo={repo}&path=x.rb");
            assert!(parse_link(&url, &home).is_err(), "{repo}");
        }
        assert!(parse_link("rubyscope://open?repo=..&path=x.rb", &home).unwrap_err().starts_with("bad repo name"));
        let url = format!("nvim://open?file={}", order.display());
        assert!(parse_link(&url, &home).unwrap_err().starts_with("not a rubyscope:// link"));
        let url = format!("rubyscope://run?file={}", order.display());
        assert!(parse_link(&url, &home).unwrap_err().starts_with("unknown link action"));
        assert!(
            parse_link("rubyscope://open?file=app/models/order.rb", &home).unwrap_err().starts_with("file must be")
        );
        assert!(parse_link("rubyscope://open?line=3", &home).unwrap_err().starts_with("the link has no file="));
        assert!(parse_link("rubyscope://open?file=%FF", &home).unwrap_err().starts_with("the link is not UTF-8"));
    }

    #[test]
    fn tells_links_from_files() {
        assert!(is_link("rubyscope://open?file=/a.rb"));
        assert!(is_link("RubyScope://open"));
        assert!(!is_link("app/models/order.rb:12"));
        assert!(!is_link("rubyscope"));
        assert_eq!(file_uri("file:///srv/a%20b.rb"), Some(PathBuf::from("/srv/a b.rb")));
        assert_eq!(file_uri("file://localhost/srv/a.rb"), Some(PathBuf::from("/srv/a.rb")));
        assert_eq!(file_uri("file://other/srv/a.rb"), None);
        assert_eq!(file_uri("/srv/a.rb"), None);
    }
}
