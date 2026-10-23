//! Background work: building the index, coloring files, git blame, the
//! file watcher. Each job sends a message back and asks for a repaint.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui;
use notify::{RecursiveMode, Watcher};
use rubyscope_index::{BuildOptions, Index};

use crate::diff::{self, Branch, FileDiff, RepoStatus};
use crate::git::{self, Blame};
use crate::highlight::{self, LineTokens};

pub enum Msg {
    Indexed {
        index: Arc<Index>,
        took: Duration,
    },
    IndexFailed(String),
    /// Files that changed on disk.
    Changed(Vec<PathBuf>),
    Highlighted {
        path: PathBuf,
        tokens: Option<Arc<Vec<LineTokens>>>,
    },
    Blamed {
        path: PathBuf,
        blame: Option<Arc<Blame>>,
    },
    /// A URL to open (from a GitHub lookup).
    OpenUrl(String),
    /// The folder picked in the folder dialog; none when it was closed.
    PickedFolder(Option<PathBuf>),
    /// Diff mode: the branch, its base and the changed files.
    RepoStatus(Arc<RepoStatus>),
    /// Diff mode: the diffs of an open file, made from `text`.
    FileDiff {
        path: PathBuf,
        text: Arc<str>,
        diff: Arc<FileDiff>,
    },
    /// The branches for the base branch picker.
    Branches(Vec<Branch>),
    /// `HEAD` and the base branch as commits (see `diff::fingerprint`).
    Fingerprint(String),
    /// What the pull button did.
    Pulled(git::Pull),
}

/// What a build starts from.
pub enum From {
    /// The saved index: only changed files are parsed.
    Saved,
    /// The last index: only changed files are parsed.
    Last(Arc<Index>),
    /// Nothing: every file is parsed.
    Nothing,
}

pub struct Worker {
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
    ctx: egui::Context,
    pub building: Arc<AtomicBool>,
    /// Set when a build is asked for while one runs: it runs once more.
    again: Arc<AtomicBool>,
    /// Where the index is saved between starts. None in tests.
    cache_dir: Option<PathBuf>,
    _watcher: Option<notify::RecommendedWatcher>,
}

/// Folders worth watching in a Rails project. `node_modules`, `tmp` and
/// `log` change a lot and hold no Ruby code to browse.
const WATCHED: &[&str] = &["app", "config", "db", "lib", "spec", "test"];

impl Worker {
    pub fn new(ctx: egui::Context) -> Worker {
        let (tx, rx) = channel();
        let cache_dir = if cfg!(test) { None } else { rubyscope_index::cache::default_dir() };
        Worker {
            tx,
            rx,
            ctx,
            building: Arc::new(AtomicBool::new(false)),
            again: Arc::new(AtomicBool::new(false)),
            cache_dir,
            _watcher: None,
        }
    }

    /// A new worker with its own channel. Jobs of the old one still run,
    /// but their messages go nowhere, and the old file watcher stops.
    pub fn restart(&mut self) {
        *self = Worker::new(self.ctx.clone());
    }

    fn spawn(&self, job: impl FnOnce(&Sender<Msg>) + Send + 'static) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            job(&tx);
            ctx.request_repaint();
        });
    }

    /// Builds the index in the background. When a build is running, it
    /// runs once more when it is done, from the index it made. After a
    /// build, the index is saved.
    pub fn build(&self, root: &Path, from: From) {
        if self.building.swap(true, Ordering::SeqCst) {
            self.again.store(true, Ordering::SeqCst);
            return;
        }
        let root = root.to_path_buf();
        let (building, again, cache_dir) = (self.building.clone(), self.again.clone(), self.cache_dir.clone());
        let ctx = self.ctx.clone();
        self.spawn(move |tx| {
            let opts = BuildOptions::default();
            let mut from = from;
            loop {
                again.store(false, Ordering::SeqCst);
                let started = Instant::now();
                let result = match (&from, &cache_dir) {
                    (From::Last(last), _) => last.rebuild(&opts),
                    (From::Saved, Some(dir)) => Index::load(&root, &opts, dir),
                    _ => Index::build(&root, &opts),
                };
                match result {
                    Ok(index) => {
                        let index = Arc::new(index);
                        let took = started.elapsed();
                        let s = &index.stats;
                        eprintln!(
                            "rubyscope: indexed {} files in {:.2} s, {} from the saved index",
                            s.files,
                            took.as_secs_f64(),
                            s.reused
                        );
                        let _ = tx.send(Msg::Indexed { index: index.clone(), took });
                        ctx.request_repaint();
                        if let Some(dir) = &cache_dir
                            && !index.stats.same_as_saved
                            && let Err(e) = index.save(dir)
                        {
                            eprintln!("rubyscope: could not save the index: {e}");
                        }
                        from = From::Last(index);
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::IndexFailed(e.to_string()));
                    }
                }
                building.store(false, Ordering::SeqCst);
                if !again.swap(false, Ordering::SeqCst) || building.swap(true, Ordering::SeqCst) {
                    break;
                }
            }
        });
    }

    pub fn highlight(&self, path: PathBuf, text: Arc<str>) {
        self.spawn(move |tx| {
            let tokens = highlight::highlight(&path, &text).map(Arc::new);
            let _ = tx.send(Msg::Highlighted { path, tokens });
        });
    }

    pub fn blame(&self, root: PathBuf, path: PathBuf, rel: String) {
        self.spawn(move |tx| {
            let blame = git::blame(&root, &rel).map(Arc::new);
            let _ = tx.send(Msg::Blamed { path, blame });
        });
    }

    /// Diff mode: the status of the repository against a base branch.
    pub fn repo_status(&self, root: PathBuf, base: Option<String>) {
        self.spawn(move |tx| {
            let status = diff::repo_status(&root, base.as_deref());
            let _ = tx.send(Msg::RepoStatus(Arc::new(status)));
        });
    }

    /// Diff mode: the branch diff and the uncommitted diff of one file. The
    /// removed lines get syntax colors too.
    pub fn file_diff(
        &self,
        root: PathBuf,
        path: PathBuf,
        rel: String,
        text: Arc<str>,
        merge_base: Option<String>,
        head: Option<String>,
    ) {
        self.spawn(move |tx| {
            let mut d = diff::file_diff(&root, &rel, &text, merge_base.as_deref(), head.as_deref());
            for h in d.branch.iter_mut().chain(d.uncommitted.iter_mut()) {
                if !h.removed.is_empty() {
                    h.tokens = highlight::highlight(&path, &h.removed.join("\n")).unwrap_or_default();
                }
            }
            let _ = tx.send(Msg::FileDiff { path, text, diff: Arc::new(d) });
        });
    }

    pub fn branches(&self, root: PathBuf) {
        self.spawn(move |tx| {
            let _ = tx.send(Msg::Branches(diff::branches(&root)));
        });
    }

    pub fn fingerprint(&self, root: PathBuf, base: String) {
        self.spawn(move |tx| {
            let _ = tx.send(Msg::Fingerprint(diff::fingerprint(&root, &base)));
        });
    }

    /// `git pull` for the pull button.
    pub fn pull(&self, root: PathBuf) {
        self.spawn(move |tx| {
            let _ = tx.send(Msg::Pulled(git::pull(&root)));
        });
    }

    /// Looks up the PR of a commit and opens it (or the commit page).
    pub fn open_commit(&self, root: PathBuf, sha: String) {
        self.spawn(move |tx| {
            if let Some(url) = git::commit_or_pr_url(&root, &sha) {
                let _ = tx.send(Msg::OpenUrl(url));
            }
        });
    }

    /// Shows the system folder dialog. It blocks, so it runs on its own
    /// thread.
    pub fn pick_folder(&self, start: Option<PathBuf>) {
        self.spawn(move |tx| {
            let mut dialog = rfd::FileDialog::new().set_title("Open a project folder");
            if let Some(dir) = start {
                dialog = dialog.set_directory(dir);
            }
            let _ = tx.send(Msg::PickedFolder(dialog.pick_folder()));
        });
    }

    /// Watches the project's code folders for changes.
    pub fn watch(&mut self, root: &Path) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let base = root.to_path_buf();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            let paths: Vec<PathBuf> = event.paths.into_iter().filter(|p| !is_noise(&base, p)).collect();
            if !paths.is_empty() {
                let _ = tx.send(Msg::Changed(paths));
                ctx.request_repaint();
            }
        });
        let Ok(mut watcher) = watcher else { return };
        for dir in WATCHED {
            let p = root.join(dir);
            if p.is_dir() {
                let _ = watcher.watch(&p, RecursiveMode::Recursive);
            }
        }
        for file in ["Gemfile.lock", "Gemfile"] {
            let p = root.join(file);
            if p.is_file() {
                let _ = watcher.watch(&p, RecursiveMode::NonRecursive);
            }
        }
        self._watcher = Some(watcher);
    }
}

/// Editor swap files, and folders inside the project that change often.
fn is_noise(root: &Path, p: &Path) -> bool {
    let rel = p.strip_prefix(root).unwrap_or(p);
    let name = rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let busy_dir =
        rel.components().any(|c| matches!(c.as_os_str().to_str(), Some(".git" | "tmp" | "log" | "node_modules")));
    busy_dir || name.ends_with('~') || name.ends_with(".swp") || name.ends_with(".swx") || name.starts_with(".#")
}
