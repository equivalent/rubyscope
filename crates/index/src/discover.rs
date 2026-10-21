//! Finds the Ruby files to index: the project itself, its gems (from
//! `Gemfile.lock`) and the Ruby standard library.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Origin {
    Project,
    Gem { name: String, version: String },
    Stdlib,
}

impl Origin {
    pub fn is_project(&self) -> bool {
        matches!(self, Origin::Project)
    }
}

/// A folder of Ruby code and where it comes from.
#[derive(Debug, Clone)]
pub struct Source {
    pub root: PathBuf,
    pub origin: Origin,
    /// Sub folders to scan. Empty means the whole root.
    pub dirs: Vec<PathBuf>,
    /// True for an installed gem or the standard library: no file is added
    /// or removed, so the saved list of its files can be used.
    pub fixed: bool,
}

/// Where Ruby keeps gems and the standard library for this project.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RubyEnv {
    pub gem_path: Vec<PathBuf>,
    pub rubylibdir: Option<PathBuf>,
}

impl RubyEnv {
    /// Asks the Ruby that the project uses (through mise, rbenv or PATH).
    pub fn detect(project: &Path) -> Result<Self> {
        let out = Command::new(ruby_for(project))
            .current_dir(project)
            .args(["-rjson", "-e", "print JSON.dump(gem_path: Gem.path, rubylibdir: RbConfig::CONFIG['rubylibdir'])"])
            .output()
            .context("running ruby to find the gem path")?;
        anyhow::ensure!(out.status.success(), "ruby failed: {}", String::from_utf8_lossy(&out.stderr));
        Ok(serde_json::from_slice(&out.stdout)?)
    }
}

/// The Ruby of the project folder. `ruby` from PATH is not enough: mise
/// and rbenv choose the Ruby when a shell goes into a folder, so PATH can
/// hold the Ruby of another folder, whose gem folder does not have the
/// project's gems.
fn ruby_for(project: &Path) -> PathBuf {
    for manager in ["mise", "rbenv"] {
        let Ok(out) = Command::new(manager).current_dir(project).args(["which", "ruby"]).output() else { continue };
        let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if out.status.success() && path.is_file() {
            return path;
        }
    }
    PathBuf::from("ruby")
}

pub const RUBY_EXTENSIONS: &[&str] = &["rb", "rake", "ru", "gemspec", "jbuilder", "rbs"];
const RUBY_FILE_NAMES: &[&str] = &["Gemfile", "Rakefile", "Guardfile", "Capfile"];

/// How a file is read: as Ruby, as an ERB template, or as plain text.
pub fn file_kind(path: &Path) -> crate::model::FileKind {
    use crate::model::FileKind;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".erb") {
        FileKind::Erb
    } else if is_ruby_file(path) {
        FileKind::Ruby
    } else {
        FileKind::Text
    }
}

/// Every file of a project, relative to its root, respecting `.gitignore`.
pub fn list_all_files(root: &Path) -> Vec<String> {
    let mut walk = ignore::WalkBuilder::new(root);
    walk.hidden(false).parents(true).filter_entry(|e| e.file_name() != ".git");
    let mut out: Vec<String> = walk
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| e.path().strip_prefix(root).ok().map(|p| p.display().to_string()))
        .collect();
    out.sort();
    out
}

/// Files that are not Ruby but that clicks can go to: templates, locale
/// files and Stimulus controllers.
pub fn is_linked_text_file(rel: &str) -> bool {
    let is_erb = rel.ends_with(".erb");
    let in_views = rel.starts_with("app/views/") && !rel.ends_with(".rb");
    let locale = rel.starts_with("config/locales/") && (rel.ends_with(".yml") || rel.ends_with(".yaml"));
    let stimulus = (rel.starts_with("app/javascript/") || rel.starts_with("app/components/"))
        && [".js", ".ts", ".jsx", ".tsx"].iter().any(|e| rel.ends_with(&format!("_controller{e}")));
    is_erb || in_views || locale || stimulus
}

pub fn is_ruby_file(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        // RBS files are type signatures, not Ruby. They are not indexed yet.
        return ext != "rbs" && RUBY_EXTENSIONS.contains(&ext);
    }
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| RUBY_FILE_NAMES.contains(&n))
}

/// Lists the Ruby files of a source. Project files respect `.gitignore`.
pub fn list_files(source: &Source) -> Vec<PathBuf> {
    let dirs: Vec<PathBuf> = if source.dirs.is_empty() { vec![source.root.clone()] } else { source.dirs.clone() };
    let mut files = Vec::new();
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        let mut walk = ignore::WalkBuilder::new(dir);
        walk.hidden(true).parents(false);
        if !source.origin.is_project() {
            walk.git_ignore(false).git_global(false).git_exclude(false).ignore(false);
        }
        for entry in walk.build().flatten() {
            if entry.file_type().is_some_and(|t| t.is_file()) && is_ruby_file(entry.path()) {
                files.push(entry.into_path());
            }
        }
    }
    files
}

/// The project, every gem in `Gemfile.lock` that is installed, and the
/// standard library.
pub fn sources(project: &Path, env: &RubyEnv) -> Vec<Source> {
    let mut out = vec![Source { root: project.to_path_buf(), origin: Origin::Project, dirs: Vec::new(), fixed: false }];
    let lock = project.join("Gemfile.lock");
    if let Ok(text) = std::fs::read_to_string(&lock) {
        let mut names = HashSet::new();
        for gem in parse_lockfile(&text) {
            if names.contains(&gem.name) {
                continue;
            }
            if let Some(dir) = gem.locate(project, env) {
                names.insert(gem.name.clone());
                let dirs = ["lib", "app"].iter().map(|d| dir.join(d)).collect();
                let fixed = !matches!(gem.source, GemSource::Path { .. });
                let origin = Origin::Gem { name: gem.name, version: gem.version };
                out.push(Source { root: dir, origin, dirs, fixed });
            }
        }
    }
    if let Some(lib) = &env.rubylibdir {
        // Only the top level of the standard library; architecture folders
        // such as `x86_64-linux` hold compiled files.
        out.push(Source { root: lib.clone(), origin: Origin::Stdlib, dirs: Vec::new(), fixed: true });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GemSource {
    Rubygems,
    Git { remote: String, revision: String },
    Path { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedGem {
    pub name: String,
    /// The version as written in the lock file, with a platform suffix if
    /// there is one (`1.18.0-x86_64-linux-gnu`).
    pub version: String,
    pub source: GemSource,
}

impl LockedGem {
    /// Finds the folder where the gem is installed.
    pub fn locate(&self, project: &Path, env: &RubyEnv) -> Option<PathBuf> {
        match &self.source {
            GemSource::Rubygems => env
                .gem_path
                .iter()
                .map(|p| p.join("gems").join(format!("{}-{}", self.name, self.version)))
                .find(|d| d.is_dir()),
            GemSource::Git { remote, revision } => {
                let repo = remote.trim_end_matches('/').rsplit('/').next()?.trim_end_matches(".git");
                let short = &revision[..revision.len().min(12)];
                let dir = env
                    .gem_path
                    .iter()
                    .map(|p| p.join("bundler").join("gems").join(format!("{repo}-{short}")))
                    .find(|d| d.is_dir())?;
                Some(gem_dir_in_repo(&dir, &self.name).unwrap_or(dir))
            }
            GemSource::Path { path } => {
                let dir = project.join(path);
                let dir = gem_dir_in_repo(&dir, &self.name).unwrap_or(dir);
                dir.is_dir().then_some(dir)
            }
        }
    }
}

/// A git or path source can hold many gems (for example the Rails repo).
/// The gem's folder is the one with `<name>.gemspec`.
fn gem_dir_in_repo(repo: &Path, name: &str) -> Option<PathBuf> {
    let spec = format!("{name}.gemspec");
    if repo.join(&spec).is_file() {
        return Some(repo.to_path_buf());
    }
    std::fs::read_dir(repo).ok()?.flatten().map(|e| e.path()).find(|d| d.join(&spec).is_file())
}

/// Reads the gems from a `Gemfile.lock`. Only the top-level specs count
/// (4 spaces of indent); dependency lines have 6.
pub fn parse_lockfile(text: &str) -> Vec<LockedGem> {
    let mut gems = Vec::new();
    let mut seen = HashSet::new();
    let mut section = "";
    let mut remote = String::new();
    let mut revision = String::new();
    for line in text.lines() {
        if !line.starts_with(' ') {
            section = line.trim();
            remote.clear();
            revision.clear();
            continue;
        }
        let trimmed = line.trim();
        if let Some(r) = trimmed.strip_prefix("remote: ") {
            remote = r.to_string();
            continue;
        }
        if let Some(r) = trimmed.strip_prefix("revision: ") {
            revision = r.to_string();
            continue;
        }
        let is_spec = line.starts_with("    ") && !line.starts_with("      ");
        if !is_spec || !matches!(section, "GEM" | "GIT" | "PATH") {
            continue;
        }
        let Some((name, rest)) = trimmed.split_once(" (") else { continue };
        let version = rest.trim_end_matches(')').to_string();
        let source = match section {
            "GIT" => GemSource::Git { remote: remote.clone(), revision: revision.clone() },
            "PATH" => GemSource::Path { path: remote.clone() },
            _ => GemSource::Rubygems,
        };
        // The same gem can be listed once per platform. Keep each variant;
        // `locate` picks the one that is installed.
        if seen.insert((name.to_string(), version.clone())) {
            gems.push(LockedGem { name: name.to_string(), version, source });
        }
    }
    // Put plain versions before platform ones, so `sources` tries them first.
    gems.sort_by_key(|g| (g.name.clone(), g.version.contains('-')));
    gems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gems_from_all_sections() {
        let lock = "\
GIT
  remote: https://github.com/yshmarov/action_push_native.git
  revision: a1b2c3d0123456789
  specs:
    action_push_native (0.1.0)
      rails (>= 7)

GEM
  remote: https://rubygems.org/
  specs:
    nokogiri (1.18.0-x86_64-linux-gnu)
      racc (~> 1.4)
    rails (8.1.4)
      actioncable (= 8.1.4)

PLATFORMS
  x86_64-linux
";
        let gems = parse_lockfile(lock);
        let names: Vec<_> = gems.iter().map(|g| (g.name.as_str(), g.version.as_str())).collect();
        assert_eq!(
            names,
            vec![("action_push_native", "0.1.0"), ("nokogiri", "1.18.0-x86_64-linux-gnu"), ("rails", "8.1.4")]
        );
        assert!(matches!(&gems[0].source, GemSource::Git { revision, .. } if revision.starts_with("a1b2c3d")));
    }
}
