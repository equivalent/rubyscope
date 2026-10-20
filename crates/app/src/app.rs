//! The rubyscope window: panels, tabs, navigation and actions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Color32, Pos2, RichText};
use rubyscope_index::model::Span;
use rubyscope_index::{Confidence, FileId, Hover, Index, OutlineKind, Symbol, Target};

use crate::external;
use crate::keys::{self, Action, Keymap};
use crate::picker::{Payload, PickItem, Picked, Picker, PickerKind};
use crate::theme::Theme;
use crate::tree::FileTree;
use crate::view::{self, Clickable, OpenFile};
use crate::worker::{self, Msg, Worker};

#[derive(Debug, Clone, PartialEq, Eq)]
struct NavPoint {
    path: PathBuf,
    line: u32,
}

#[derive(Debug, Clone)]
pub struct ResultRow {
    pub path: PathBuf,
    pub rel: String,
    /// 0-based line.
    pub line: u32,
    pub offset: u32,
    pub text: String,
    pub context: String,
    pub badge: Option<&'static str>,
    /// The method the row is in, for "callers" (the call hierarchy).
    pub caller: Option<Symbol>,
}

pub enum Bottom {
    Results { title: String, rows: Vec<ResultRow> },
    Hierarchy { name: String, ancestors: Vec<String>, descendants: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarTab {
    Files,
    Outline,
}

struct Choose {
    pos: Pos2,
    title: String,
    targets: Vec<Target>,
    /// The frame it opened in: the click that opened it must not close it.
    opened: u64,
}

struct HoverPopup {
    offset: u32,
    pos: Pos2,
    data: Option<Hover>,
}

pub struct App {
    pub root: Option<PathBuf>,
    pub theme: Theme,
    keys: Keymap,
    pub(crate) worker: Worker,
    pub index: Option<Arc<Index>>,
    index_status: String,
    tree: FileTree,
    all_files: Arc<Vec<String>>,
    pub tabs: Vec<OpenFile>,
    pub active: usize,
    history: Vec<NavPoint>,
    history_pos: usize,
    pub show_sidebar: bool,
    sidebar_tab: SidebarTab,
    pub show_blame: bool,
    pub bottom: Option<Bottom>,
    show_bottom: bool,
    pub picker: Option<Picker>,
    file_items: Option<Arc<Vec<PickItem>>>,
    symbol_items: Option<Arc<Vec<PickItem>>>,
    choose: Option<Choose>,
    hover: Option<HoverPopup>,
    /// The right-click menu: offset, position, and the frame it opened in.
    context_menu: Option<(u32, Pos2, u64)>,
    frame_nr: u64,
    pending_changes: Vec<PathBuf>,
    last_change: Option<Instant>,
    welcome_path: String,
    /// True while the folder dialog is open.
    picking_folder: bool,
    toast: Option<(String, f64)>,
    title: String,
    started: Instant,
}

/// Lets the code view ask the index what is clickable in a file.
struct Links<'a> {
    index: &'a Index,
    file: FileId,
}

impl Clickable for Links<'_> {
    fn clickable_at(&self, offset: u32) -> Option<Span> {
        self.index.clickable_at(self.file, offset)
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Where rubyscope keeps open tabs and recent projects. Tests use a temp
/// folder, so they never touch the user's state.
fn state_dir() -> Option<PathBuf> {
    let dir = if cfg!(test) {
        std::env::temp_dir().join(format!("rubyscope-test-state-{}", std::process::id()))
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".local/state/rubyscope")
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn session_path(root: &Path) -> Option<PathBuf> {
    let name: String =
        root.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    Some(state_dir()?.join(format!("session{name}.json")))
}

pub fn recent_projects() -> Vec<String> {
    state_dir()
        .and_then(|d| std::fs::read_to_string(d.join("recent.json")).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn remember_project(root: &Path) {
    let mut list = recent_projects();
    let p = root.display().to_string();
    list.retain(|x| *x != p);
    list.insert(0, p);
    list.truncate(12);
    if let Some(d) = state_dir() {
        let _ = std::fs::write(d.join("recent.json"), serde_json::to_string(&list).unwrap_or_default());
    }
}

/// The first font file fontconfig gives for a pattern. macOS has no
/// fontconfig, so there it reads system fonts that every Mac has.
fn font_file(pattern: &str) -> Option<Vec<u8>> {
    if cfg!(target_os = "macos") {
        let files: &[&str] = match pattern {
            "monospace" => &["/System/Library/Fonts/Menlo.ttc"],
            _ => &["/System/Library/Fonts/Supplemental/Arial Unicode.ttf", "/System/Library/Fonts/Apple Symbols.ttf"],
        };
        return files.iter().find_map(|f| std::fs::read(f).ok());
    }
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
    let file = String::from_utf8_lossy(&out.stdout).to_string();
    (file.ends_with(".ttf") || file.ends_with(".otf")).then(|| std::fs::read(&file).ok()).flatten()
}

/// Uses the system's monospace font for code, with a sans-serif font as a
/// fallback for rare characters. A character no font has is drawn as a box.
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(bytes) = font_file("monospace") {
        fonts.font_data.insert("system-mono".into(), egui::FontData::from_owned(bytes).into());
        if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            list.insert(0, "system-mono".into());
        }
        // Nerd fonts also have arrows and symbols the UI text uses.
        if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            list.push("system-mono".into());
        }
    }
    if let Some(bytes) = font_file("sans-serif") {
        fonts.font_data.insert("system-sans".into(), egui::FontData::from_owned(bytes).into());
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            if let Some(list) = fonts.families.get_mut(&family) {
                list.push("system-sans".into());
            }
        }
    }
    ctx.set_fonts(fonts);
    ctx.set_missing_glyph_policy(egui::epaint::text::MissingGlyphPolicy::Tofu);
}

impl App {
    pub fn new(ctx: &egui::Context, root: Option<PathBuf>, open: Option<(PathBuf, Option<u32>)>) -> App {
        setup_fonts(ctx);
        let theme = Theme::load();
        theme.apply(ctx);
        let mut app = App {
            root: None,
            theme,
            keys: Keymap::load(),
            worker: Worker::new(ctx.clone()),
            index: None,
            index_status: String::new(),
            tree: FileTree::default(),
            all_files: Arc::new(Vec::new()),
            tabs: Vec::new(),
            active: 0,
            history: Vec::new(),
            history_pos: 0,
            show_sidebar: true,
            sidebar_tab: SidebarTab::Files,
            show_blame: false,
            bottom: None,
            show_bottom: true,
            picker: None,
            file_items: None,
            symbol_items: None,
            choose: None,
            hover: None,
            context_menu: None,
            frame_nr: 0,
            pending_changes: Vec::new(),
            last_change: None,
            welcome_path: String::new(),
            picking_folder: false,
            toast: None,
            title: String::new(),
            started: Instant::now(),
        };
        if let Some(root) = root {
            app.open_project(root);
        }
        if let Some((path, line)) = open {
            app.open_path(&path, None, line, true);
        }
        app
    }

    /// Opens a project folder: lists its files, restores its tabs and starts
    /// the index and the file watcher.
    pub fn open_project(&mut self, root: PathBuf) {
        let root = root.canonicalize().unwrap_or(root);
        remember_project(&root);
        let files = rubyscope_index::discover::list_all_files(&root);
        self.tree = FileTree::new(&files);
        self.all_files = Arc::new(files);
        self.file_items = None;
        self.symbol_items = None;
        self.index_status = "indexing…".into();
        self.worker.build(&root, worker::From::Saved);
        self.worker.watch(&root);
        self.root = Some(root.clone());
        if let Some(saved) = session_path(&root).and_then(|p| std::fs::read_to_string(p).ok())
            && let Ok(tabs) = serde_json::from_str::<Vec<(String, u32)>>(&saved)
        {
            for (rel, line) in tabs {
                let path = root.join(&rel);
                if path.is_file() {
                    self.open_path(&path, None, Some(line), false);
                }
            }
        }
    }

    /// Opens a project with an index that is already built (tests).
    #[cfg(test)]
    pub fn open_prebuilt(&mut self, root: PathBuf, index: Arc<Index>) {
        self.tree = FileTree::new(&index.all_files);
        self.all_files = Arc::new(index.all_files.clone());
        self.root = Some(root);
        self.index = Some(index);
        self.index_status = "ready".into();
    }

    fn save_session(&self) {
        let Some(root) = &self.root else { return };
        let tabs: Vec<(String, u32)> = self.tabs.iter().map(|t| (t.rel.clone(), t.top_line)).collect();
        if let Some(p) = session_path(root) {
            let _ = std::fs::write(p, serde_json::to_string(&tabs).unwrap_or_default());
        }
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), self.started.elapsed().as_secs_f64()));
    }

    pub fn current(&self) -> Option<&OpenFile> {
        self.tabs.get(self.active)
    }

    fn current_file_id(&self) -> Option<FileId> {
        let index = self.index.as_ref()?;
        index.file_id(&self.current()?.path)
    }

    fn rel_of(&self, path: &Path) -> String {
        match &self.root {
            Some(root) => path.strip_prefix(root).unwrap_or(path).display().to_string(),
            None => path.display().to_string(),
        }
    }

    fn here(&self) -> Option<NavPoint> {
        let t = self.current()?;
        let line = t.cursor.map(|c| t.line_of(c)).unwrap_or(t.top_line + 6);
        Some(NavPoint { path: t.path.clone(), line })
    }

    fn push_history(&mut self, p: NavPoint) {
        if !self.history.is_empty() {
            self.history.truncate(self.history_pos + 1);
        }
        if self.history.last() != Some(&p) {
            self.history.push(p);
        }
        self.history_pos = self.history.len().saturating_sub(1);
    }

    /// Opens a file (or switches to its tab) and moves to an offset or a
    /// 0-based line.
    pub fn open_path(&mut self, path: &Path, offset: Option<u32>, line: Option<u32>, record: bool) {
        if record && let Some(here) = self.here() {
            self.push_history(here);
        }
        let idx = match self.tabs.iter().position(|t| t.path == path) {
            Some(i) => i,
            None => {
                let Ok(text) = std::fs::read_to_string(path) else {
                    self.toast(format!("Cannot read {}", path.display()));
                    return;
                };
                let rel = self.rel_of(path);
                let file = OpenFile::new(path.to_path_buf(), rel, text);
                self.worker.highlight(file.path.clone(), file.text.clone());
                let insert_at = (self.active + 1).min(self.tabs.len());
                self.tabs.insert(insert_at, file);
                insert_at
            }
        };
        self.active = idx;
        let time = self.started.elapsed().as_secs_f64();
        let show_blame = self.show_blame;
        let root = self.root.clone();
        let tab = &mut self.tabs[idx];
        let target_line = offset.map(|o| tab.line_of(o)).or(line);
        if let Some(l) = target_line {
            tab.scroll_to_line = Some(l);
            tab.flash = Some((l, time));
            tab.cursor = Some(offset.unwrap_or_else(|| tab.line_start(l)));
        }
        let rel = tab.rel.clone();
        if show_blame && !tab.blame_requested {
            tab.blame_requested = true;
            if let Some(root) = root {
                self.worker.blame(root, tab.path.clone(), rel.clone());
            }
        }
        if self.root.as_ref().is_some_and(|r| path.starts_with(r)) {
            self.tree.reveal(&rel);
        }
        if record {
            let line = target_line.unwrap_or(0);
            self.push_history(NavPoint { path: path.to_path_buf(), line });
        }
        self.save_session();
    }

    fn go_back(&mut self) {
        if self.history_pos == 0 || self.history.is_empty() {
            return;
        }
        self.history_pos -= 1;
        let p = self.history[self.history_pos].clone();
        self.open_path(&p.path, None, Some(p.line), false);
    }

    fn go_forward(&mut self) {
        if self.history_pos + 1 >= self.history.len() {
            return;
        }
        self.history_pos += 1;
        let p = self.history[self.history_pos].clone();
        self.open_path(&p.path, None, Some(p.line), false);
    }

    fn go_to_target(&mut self, t: &Target) {
        let Some(index) = self.index.clone() else { return };
        let path = index.files[t.loc.file as usize].path.clone();
        let at = if t.loc.name_span.is_empty() && t.loc.name_span.start == 0 {
            t.loc.span.start
        } else {
            t.loc.name_span.start
        };
        self.open_path(&path, Some(at), None, true);
    }

    /// Goes to the targets of a click: straight to one, or a list to choose.
    fn go_to_targets(&mut self, title: String, targets: Vec<Target>, pos: Pos2) {
        match targets.len() {
            0 => self.toast("No definition found"),
            1 => self.go_to_target(&targets[0]),
            _ => {
                // A class opened in many files: go to its main file.
                let same = targets.iter().all(|t| t.label == targets[0].label);
                if same
                    && targets[0].confidence == Confidence::Exact
                    && targets[0].detail.as_deref() != Some("translation (en)")
                {
                    let first = targets[0].clone();
                    self.go_to_target(&first);
                    if targets.len() > 1 {
                        self.toast(format!(
                            "{} is defined in {} places (right-click → all definitions)",
                            first.label,
                            targets.len()
                        ));
                    }
                } else {
                    self.choose = Some(Choose { pos, title, targets, opened: self.frame_nr });
                }
            }
        }
    }

    fn definition_at_cursor(&mut self, all: bool, pos: Pos2) {
        let (Some(index), Some(file)) = (self.index.clone(), self.current_file_id()) else { return };
        let Some(cursor) = self.current().and_then(|t| t.cursor) else { return };
        let Some(res) = index.definition_at(file, cursor) else {
            self.toast("Nothing to go to here");
            return;
        };
        let title = self
            .current()
            .map(|t| t.text[res.span.start as usize..res.span.end as usize].to_string())
            .unwrap_or_default();
        if all && res.targets.len() > 1 {
            self.choose = Some(Choose { pos, title, targets: res.targets, opened: self.frame_nr });
        } else {
            self.go_to_targets(title, res.targets, pos);
        }
    }

    /// Shows the folder dialog, once at a time.
    fn pick_folder(&mut self) {
        if self.picking_folder {
            return;
        }
        self.picking_folder = true;
        let start = self
            .root
            .as_ref()
            .and_then(|r| r.parent().map(Path::to_path_buf))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("git")).filter(|p| p.is_dir()));
        self.worker.pick_folder(start);
    }

    /// Opens a folder: in this window from the start screen, else in a new
    /// window (one window per project).
    fn open_folder(&mut self, folder: PathBuf) {
        if self.root.is_none() {
            self.open_project(folder);
            return;
        }
        if self.root.as_deref() == Some(folder.as_path()) {
            return;
        }
        let spawned = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg(&folder).spawn());
        if let Err(e) = spawned {
            self.toast(format!("Could not open a new window: {e}"));
        }
    }

    /// Goes from a controller action to its view, or from a view to its
    /// controller action.
    fn related_at_cursor(&mut self, pos: Pos2) {
        let (Some(index), Some(file)) = (self.index.clone(), self.current_file_id()) else {
            self.toast("The index is not ready yet");
            return;
        };
        let Some(t) = self.current() else { return };
        let offset = t.cursor.unwrap_or_else(|| t.line_start(t.top_line));
        match index.related_at(file, offset) {
            Some((label, targets)) => self.go_to_targets(label.to_string(), targets, pos),
            None => self.toast("No view or controller action here"),
        }
    }

    fn find_usages(&mut self) {
        let (Some(index), Some(file)) = (self.index.clone(), self.current_file_id()) else {
            self.toast("The index is not ready yet");
            return;
        };
        let Some(cursor) = self.current().and_then(|t| t.cursor) else { return };
        let Some(sym) = index.symbol_at(file, cursor) else {
            self.toast("No symbol at the cursor");
            return;
        };
        self.show_usages(&index, &sym, "Usages of");
    }

    /// Fills the results panel with the usages of a symbol. For a method,
    /// each row can show its own callers, one level up the call hierarchy.
    fn show_usages(&mut self, index: &Index, sym: &Symbol, what: &str) {
        let usages = index.usages(sym);
        let mut texts: HashMap<FileId, Option<String>> = HashMap::new();
        let rows = usages
            .iter()
            .map(|u| {
                let text = texts.entry(u.file).or_insert_with(|| index.read_text(u.file));
                let line = index.files[u.file as usize].raw.line_of(u.span.start);
                let preview =
                    text.as_deref().and_then(|t| t.lines().nth(line as usize)).unwrap_or("").trim().to_string();
                ResultRow {
                    path: index.files[u.file as usize].path.clone(),
                    rel: index.display_path(u.file),
                    line,
                    offset: u.span.start,
                    text: preview,
                    context: u.context.clone(),
                    badge: (u.confidence != Confidence::Exact).then(|| u.confidence.label()),
                    caller: index.method_symbol_at(u.file, u.span.start),
                }
            })
            .collect::<Vec<_>>();
        let title = format!("{what} {} ({})", sym.label(), rows.len());
        self.bottom = Some(Bottom::Results { title, rows });
        self.show_bottom = true;
    }

    fn type_hierarchy(&mut self) {
        let (Some(index), Some(file)) = (self.index.clone(), self.current_file_id()) else { return };
        let cursor = self.current().and_then(|t| t.cursor).unwrap_or(0);
        let name = match index.symbol_at(file, cursor) {
            Some(Symbol::Const { name }) => Some(name),
            Some(Symbol::Method { owner, .. }) => Some(owner),
            None => index.breadcrumb(file, cursor).first().map(|(n, _)| n.clone()),
        };
        let Some(name) = name else {
            self.toast("No class or module at the cursor");
            return;
        };
        let h = index.hierarchy(&name);
        self.bottom = Some(Bottom::Hierarchy { name, ancestors: h.ancestors, descendants: h.descendants });
        self.show_bottom = true;
    }

    fn search_project(&mut self, query: &str) {
        let Some(index) = self.index.clone() else {
            self.toast("The index is not ready yet");
            return;
        };
        match index.search(query, false, false, 2000) {
            Ok(hits) => {
                let root = self.root.clone().unwrap_or_default();
                let rows: Vec<ResultRow> = hits
                    .into_iter()
                    .map(|h| ResultRow {
                        path: root.join(&h.rel),
                        rel: h.rel.clone(),
                        line: h.line,
                        offset: u32::MAX,
                        text: h.text.trim().to_string(),
                        context: String::new(),
                        badge: None,
                        caller: None,
                    })
                    .collect();
                let title = format!("Search \"{query}\" ({} hits)", rows.len());
                self.bottom = Some(Bottom::Results { title, rows });
                self.show_bottom = true;
            }
            Err(e) => self.toast(e),
        }
    }

    fn file_items(&mut self) -> Arc<Vec<PickItem>> {
        if let Some(items) = &self.file_items {
            return items.clone();
        }
        let items: Vec<PickItem> = self
            .all_files
            .iter()
            .map(|rel| PickItem {
                label: rel.clone(),
                detail: String::new(),
                payload: Payload::File(rel.clone()),
                bonus: if rel.ends_with(".rb") { 2 } else { 0 },
            })
            .collect();
        let items = Arc::new(items);
        self.file_items = Some(items.clone());
        items
    }

    fn symbol_items(&mut self) -> Option<Arc<Vec<PickItem>>> {
        if let Some(items) = &self.symbol_items {
            return Some(items.clone());
        }
        let index = self.index.clone()?;
        let items: Vec<PickItem> = index
            .symbol_entries()
            .into_iter()
            .map(|e| {
                let path = index.files[e.loc.file as usize].path.clone();
                let where_ = index.display_path(e.loc.file);
                PickItem {
                    label: e.label,
                    detail: format!("{} · {where_}", e.detail),
                    payload: Payload::Loc(path, e.loc.name_span.start),
                    bonus: if e.project { 30 } else { 0 },
                }
            })
            .collect();
        let items = Arc::new(items);
        self.symbol_items = Some(items.clone());
        Some(items)
    }

    fn command_items(&self) -> Arc<Vec<PickItem>> {
        Arc::new(
            Action::ALL
                .iter()
                .filter(|a| **a != Action::ClosePopup && **a != Action::Commands)
                .map(|a| PickItem {
                    label: a.title().to_string(),
                    detail: self.keys.shortcut(*a).map(|s| s.label()).unwrap_or_default(),
                    payload: Payload::Action(*a),
                    bonus: 0,
                })
                .collect(),
        )
    }

    pub fn run_action(&mut self, action: Action, ctx: &egui::Context) {
        let pointer = ctx.input(|i| i.pointer.latest_pos()).unwrap_or(Pos2::new(200.0, 200.0));
        match action {
            Action::OpenFolder => self.pick_folder(),
            Action::GoToFile => {
                let items = self.file_items();
                self.picker = Some(Picker::new(PickerKind::Files, items));
            }
            Action::GoToSymbol => match self.symbol_items() {
                Some(items) => self.picker = Some(Picker::new(PickerKind::Symbols, items)),
                None => self.toast("The index is not ready yet"),
            },
            Action::Commands => self.picker = Some(Picker::new(PickerKind::Commands, self.command_items())),
            Action::GoToLine => self.picker = Some(Picker::new(PickerKind::Line, Arc::new(Vec::new()))),
            Action::SearchProject => {
                let mut p = Picker::new(PickerKind::Search, Arc::new(Vec::new()));
                p.query =
                    self.current().and_then(|t| t.selected_text()).filter(|s| !s.contains('\n')).unwrap_or_default();
                self.picker = Some(p);
            }
            Action::FindInFile => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.find.open = true;
                    t.find.focus = true;
                    if let Some(sel) = t.selected_text().filter(|s| !s.contains('\n')) {
                        t.find.query = sel;
                        t.update_find();
                    }
                }
            }
            Action::CloseTab => {
                if !self.tabs.is_empty() {
                    self.tabs.remove(self.active);
                    self.active = self.active.min(self.tabs.len().saturating_sub(1));
                    self.save_session();
                }
            }
            Action::NextTab => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + 1) % self.tabs.len();
                }
            }
            Action::PrevTab => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
                }
            }
            Action::ClosePopup => {
                if self.picker.is_some() {
                    self.picker = None;
                } else if self.choose.is_some() || self.context_menu.is_some() || self.hover.is_some() {
                    self.choose = None;
                    self.context_menu = None;
                    self.hover = None;
                } else if let Some(t) = self.tabs.get_mut(self.active).filter(|t| t.find.open) {
                    t.find.open = false;
                    t.find.matches.clear();
                } else if self.bottom.is_some() && self.show_bottom {
                    self.show_bottom = false;
                }
            }
            Action::ToggleBlame => {
                self.show_blame = !self.show_blame;
                if self.show_blame
                    && let (Some(root), Some(t)) = (self.root.clone(), self.tabs.get_mut(self.active))
                    && !t.blame_requested
                {
                    t.blame_requested = true;
                    self.worker.blame(root, t.path.clone(), t.rel.clone());
                }
            }
            Action::ToggleSidebar => self.show_sidebar = !self.show_sidebar,
            Action::ToggleBottomPanel => self.show_bottom = !self.show_bottom,
            Action::Back => self.go_back(),
            Action::Forward => self.go_forward(),
            Action::FindUsages => self.find_usages(),
            Action::TypeHierarchy => self.type_hierarchy(),
            Action::GoToRelated => self.related_at_cursor(pointer),
            Action::FileOutline => {
                self.show_sidebar = true;
                self.sidebar_tab = SidebarTab::Outline;
            }
            Action::OpenInEditor => {
                if let Some(here) = self.here() {
                    external::open_in_editor(&here.path, here.line + 1);
                }
            }
            Action::CopyPath => {
                if let Some(t) = self.current() {
                    let line = t.cursor.map(|c| t.line_of(c)).unwrap_or(t.top_line) + 1;
                    let text = format!("{}:{line}", t.rel);
                    ctx.copy_text(text.clone());
                    self.toast(format!("Copied {text}"));
                }
            }
            Action::OpenOnGitHub => {
                let (Some(root), Some(t)) = (self.root.clone(), self.current()) else { return };
                let line = t.cursor.map(|c| t.line_of(c)).unwrap_or(t.top_line) + 1;
                match (crate::git::github_url(&root), external::head_sha(&root)) {
                    (Some(base), Some(sha)) => external::open_url(&format!("{base}/blob/{sha}/{}#L{line}", t.rel)),
                    _ => self.toast("This project has no GitHub remote"),
                }
            }
            Action::Reindex => {
                if let Some(root) = self.root.clone() {
                    self.index_status = "indexing…".into();
                    self.worker.build(&root, worker::From::Nothing);
                }
            }
        }
        let _ = pointer;
    }

    fn handle_messages(&mut self) {
        while let Ok(msg) = self.worker.rx.try_recv() {
            match msg {
                Msg::Indexed { index, took } => {
                    let s = &index.stats;
                    self.index_status = format!("{} files · {:.1} s", s.files, took.as_secs_f64());
                    self.all_files = Arc::new(index.all_files.clone());
                    self.file_items = None;
                    self.symbol_items = None;
                    // Freeing a big index takes a while; do it off this thread.
                    if let Some(old) = self.index.replace(index) {
                        std::thread::spawn(move || drop(old));
                    }
                }
                Msg::IndexFailed(e) => self.index_status = format!("index failed: {e}"),
                Msg::Changed(paths) => {
                    self.pending_changes.extend(paths);
                    self.last_change = Some(Instant::now());
                }
                Msg::Highlighted { path, tokens } => {
                    for t in self.tabs.iter_mut().filter(|t| t.path == path) {
                        t.highlights = tokens.clone();
                    }
                }
                Msg::Blamed { path, blame } => {
                    for t in self.tabs.iter_mut().filter(|t| t.path == path) {
                        t.blame = blame.clone();
                    }
                }
                Msg::OpenUrl(url) => external::open_url(&url),
                Msg::PickedFolder(folder) => {
                    self.picking_folder = false;
                    if let Some(folder) = folder {
                        self.open_folder(folder);
                    }
                }
            }
        }
        // Files changed on disk: reload open tabs, and index again after a
        // short quiet time.
        if let Some(at) = self.last_change
            && at.elapsed().as_millis() > 800
        {
            self.last_change = None;
            let changed: Vec<PathBuf> = std::mem::take(&mut self.pending_changes);
            for t in self.tabs.iter_mut() {
                if changed.contains(&t.path)
                    && let Ok(text) = std::fs::read_to_string(&t.path)
                    && *text != *t.text
                {
                    let mut fresh = OpenFile::new(t.path.clone(), t.rel.clone(), text);
                    fresh.cursor = t.cursor;
                    fresh.scroll_to_line = Some(t.top_line + 6);
                    self.worker.highlight(fresh.path.clone(), fresh.text.clone());
                    *t = fresh;
                }
            }
            if let Some(root) = self.root.clone() {
                self.index_status = "indexing…".into();
                let from = self.index.clone().map_or(worker::From::Saved, worker::From::Last);
                self.worker.build(&root, from);
            }
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        let mut open = None;
        let mut browse = false;
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme.bg)).show(ui, |ui| {
            ui.add_space(80.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("rubyscope").size(32.0).color(theme.fg_bright));
                ui.label(
                    RichText::new(format!("Read Ruby code. {}+click to go to a definition.", keys::CMD))
                        .color(theme.fg_dim),
                );
                ui.add_space(24.0);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 590.0).max(0.0) / 2.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.welcome_path)
                            .hint_text("~/git/project")
                            .desired_width(420.0),
                    );
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Open").clicked() || enter {
                        open = Some(self.welcome_path.clone());
                    }
                    if ui.button("Browse…").on_hover_text(format!("Choose a folder ({}+O)", keys::CMD)).clicked() {
                        browse = true;
                    }
                });
                ui.add_space(24.0);
                for p in recent_projects() {
                    if ui.link(RichText::new(&p).color(theme.accent)).clicked() {
                        open = Some(p);
                    }
                }
            });
        });
        if browse {
            self.pick_folder();
        }
        if let Some(p) = open {
            let expanded = match p.strip_prefix("~/") {
                Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
                None => PathBuf::from(p),
            };
            if expanded.is_dir() {
                self.open_project(expanded);
            } else {
                self.toast("That folder does not exist");
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        egui::Panel::top("toolbar").frame(egui::Frame::NONE.fill(theme.bg_dark).inner_margin(6.0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let back = ui
                    .add_enabled(self.history_pos > 0, egui::Button::new("←"))
                    .on_hover_text("Back (mouse back button)");
                let fwd = ui
                    .add_enabled(self.history_pos + 1 < self.history.len(), egui::Button::new("→"))
                    .on_hover_text("Forward (mouse forward button)");
                if back.clicked() {
                    self.go_back();
                }
                if fwd.clicked() {
                    self.go_forward();
                }
                ui.separator();
                // Breadcrumb: the classes and method around the cursor.
                let crumbs = match (self.index.clone(), self.current_file_id(), self.current()) {
                    (Some(index), Some(file), Some(t)) => {
                        let at = t.cursor.unwrap_or_else(|| t.line_start(t.top_line + 2));
                        index.breadcrumb(file, at).into_iter().map(|(l, loc)| (l, Some(loc))).collect::<Vec<_>>()
                    }
                    _ => Vec::new(),
                };
                if let Some(t) = self.current() {
                    ui.label(RichText::new(&t.rel).color(theme.fg_dim));
                }
                let mut go = None;
                for (label, loc) in crumbs {
                    ui.label(RichText::new("›").color(theme.muted));
                    if ui.link(RichText::new(label).color(theme.fg)).clicked() {
                        go = loc;
                    }
                }
                if let (Some(loc), Some(index)) = (go, self.index.clone()) {
                    let path = index.files[loc.file as usize].path.clone();
                    self.open_path(&path, Some(loc.name_span.start), None, true);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Commands").on_hover_text(format!("{}+K", keys::CMD)).clicked() {
                        self.picker = Some(Picker::new(PickerKind::Commands, self.command_items()));
                    }
                    let busy = self.worker.building.load(std::sync::atomic::Ordering::SeqCst);
                    if busy {
                        ui.spinner();
                    }
                    let name = self
                        .root
                        .as_ref()
                        .and_then(|r| r.file_name())
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    ui.label(RichText::new(format!("{name} · {}", self.index_status)).color(theme.fg_dim).small());
                });
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(260.0)
            .frame(egui::Frame::NONE.fill(theme.bg_dark).inner_margin(6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Files, "Files");
                    ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Outline, "Outline");
                });
                ui.separator();
                match self.sidebar_tab {
                    SidebarTab::Files => {
                        let current = self.current().map(|t| t.rel.clone());
                        if let Some(rel) = self.tree.ui(ui, &theme, current.as_deref())
                            && let Some(root) = self.root.clone()
                        {
                            self.open_path(&root.join(rel), None, None, true);
                        }
                    }
                    SidebarTab::Outline => {
                        let (Some(index), Some(file)) = (self.index.clone(), self.current_file_id()) else {
                            ui.label(RichText::new("No outline").color(theme.fg_dim));
                            return;
                        };
                        let mut go = None;
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            for item in index.outline(file) {
                                let (icon, color) = match item.kind {
                                    OutlineKind::Class => ("C", theme.yellow),
                                    OutlineKind::Module => ("M", theme.cyan),
                                    OutlineKind::Method => ("m", theme.blue),
                                    OutlineKind::SingletonMethod => ("s", theme.blue),
                                    OutlineKind::Constant => ("k", theme.orange),
                                    OutlineKind::Generated => ("g", theme.fg_dim),
                                };
                                let r = ui
                                    .horizontal(|ui| {
                                        ui.add_space(item.depth as f32 * 12.0);
                                        ui.label(RichText::new(icon).color(color).monospace());
                                        ui.add(
                                            egui::Label::new(RichText::new(&item.label).color(theme.fg))
                                                .sense(egui::Sense::click())
                                                .truncate(),
                                        )
                                    })
                                    .inner;
                                if r.clicked() {
                                    go = Some(item.name_span.start);
                                }
                            }
                        });
                        if let (Some(at), Some(path)) = (go, self.current().map(|t| t.path.clone())) {
                            self.open_path(&path, Some(at), None, true);
                        }
                    }
                }
            });
    }

    fn bottom_panel(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        let mut close = false;
        let mut go: Option<(PathBuf, u32, u32)> = None;
        let mut go_const: Option<String> = None;
        let mut callers_of: Option<Symbol> = None;
        egui::Panel::bottom("results")
            .resizable(true)
            .default_size(220.0)
            .frame(egui::Frame::NONE.fill(theme.bg_dark).inner_margin(6.0))
            .show(ui, |ui| match &self.bottom {
                Some(Bottom::Results { title, rows }) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(title).color(theme.fg_bright).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            close = ui.small_button("✕").clicked();
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(
                        ui,
                        20.0,
                        rows.len(),
                        |ui, range| {
                            for row in &rows[range] {
                                let r = ui
                                    .horizontal(|ui| {
                                        let r = ui.add(
                                            egui::Label::new(
                                                RichText::new(format!("{}:{}", row.rel, row.line + 1))
                                                    .color(theme.accent)
                                                    .monospace(),
                                            )
                                            .sense(egui::Sense::click()),
                                        );
                                        if let Some(b) = row.badge {
                                            ui.label(RichText::new(b).color(theme.yellow).small());
                                        }
                                        if let Some(c) = &row.caller
                                            && ui
                                                .small_button("callers")
                                                .on_hover_text(format!("Who calls {}", c.label()))
                                                .clicked()
                                        {
                                            callers_of = Some(c.clone());
                                        }
                                        if !row.context.is_empty() {
                                            ui.label(RichText::new(&row.context).color(theme.fg_dim).small());
                                        }
                                        let t = ui.add(
                                            egui::Label::new(RichText::new(&row.text).color(theme.fg).monospace())
                                                .sense(egui::Sense::click())
                                                .truncate(),
                                        );
                                        r.union(t)
                                    })
                                    .inner;
                                if r.clicked() {
                                    go = Some((row.path.clone(), row.offset, row.line));
                                }
                            }
                        },
                    );
                }
                Some(Bottom::Hierarchy { name, ancestors, descendants }) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Type hierarchy of {name}")).color(theme.fg_bright).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            close = ui.small_button("✕").clicked();
                        });
                    });
                    ui.separator();
                    ui.columns(2, |cols| {
                        cols[0].label(RichText::new("Ancestors (method lookup order)").color(theme.fg_dim));
                        egui::ScrollArea::vertical().id_salt("anc").auto_shrink([false, false]).show(
                            &mut cols[0],
                            |ui| {
                                for a in ancestors {
                                    if ui.link(RichText::new(a).color(theme.fg)).clicked() {
                                        go_const = Some(a.clone());
                                    }
                                }
                            },
                        );
                        cols[1].label(RichText::new("Subclasses and includers").color(theme.fg_dim));
                        egui::ScrollArea::vertical().id_salt("desc").auto_shrink([false, false]).show(
                            &mut cols[1],
                            |ui| {
                                if descendants.is_empty() {
                                    ui.label(RichText::new("none").color(theme.fg_dim));
                                }
                                for d in descendants {
                                    if ui.link(RichText::new(d).color(theme.fg)).clicked() {
                                        go_const = Some(d.clone());
                                    }
                                }
                            },
                        );
                    });
                }
                None => {}
            });
        if close {
            self.show_bottom = false;
        }
        if let (Some(sym), Some(index)) = (callers_of, self.index.clone()) {
            self.show_usages(&index, &sym, "Callers of");
        }
        if let Some((path, offset, line)) = go {
            if offset == u32::MAX {
                self.open_path(&path, None, Some(line), true);
            } else {
                self.open_path(&path, Some(offset), None, true);
            }
        }
        if let (Some(name), Some(index)) = (go_const, self.index.clone()) {
            match index.const_targets(&name, Confidence::Exact).first() {
                Some(t) => {
                    let t = t.clone();
                    self.go_to_target(&t);
                }
                None => self.toast(format!("{name} has no definition in the index")),
            }
        }
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        let mut close = None;
        egui::ScrollArea::horizontal().id_salt("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, t) in self.tabs.iter().enumerate() {
                    let name = t.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let active = i == self.active;
                    let text = RichText::new(&name).color(if active { theme.fg_bright } else { theme.fg_dim });
                    let r = ui.selectable_label(active, text).on_hover_text(&t.rel);
                    if r.clicked() {
                        self.active = i;
                    }
                    if r.middle_clicked() {
                        close = Some(i);
                    }
                    if active && ui.small_button("✕").clicked() {
                        close = Some(i);
                    }
                }
            });
        });
        if let Some(i) = close {
            self.tabs.remove(i);
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
            self.save_session();
        }
    }

    fn find_bar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        if !t.find.open {
            return;
        }
        let mut jump = None;
        ui.horizontal(|ui| {
            let label = ui.label(RichText::new("Find").color(theme.fg_dim));
            let edit = ui
                .add(egui::TextEdit::singleline(&mut t.find.query).hint_text("text in this file").desired_width(280.0))
                .labelled_by(label.id);
            if t.find.focus {
                edit.request_focus();
                t.find.focus = false;
            }
            if edit.changed() {
                t.update_find();
                jump = t.find.matches.get(t.find.current).map(|m| m.0);
            }
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let back = ui.input(|i| i.modifiers.shift);
                let n = t.find.matches.len();
                if n > 0 {
                    t.find.current = if back { (t.find.current + n - 1) % n } else { (t.find.current + 1) % n };
                    jump = Some(t.find.matches[t.find.current].0);
                }
                edit.request_focus();
            }
            let n = t.find.matches.len();
            ui.label(
                RichText::new(if n == 0 { "no matches".to_string() } else { format!("{} of {n}", t.find.current + 1) })
                    .color(theme.fg_dim),
            );
            if ui.small_button("✕").clicked() {
                t.find.open = false;
                t.find.matches.clear();
            }
        });
        if let Some(off) = jump {
            let line = t.line_of(off);
            t.scroll_to_line = Some(line);
            t.cursor = Some(off);
        }
    }

    fn overlays(&mut self, ctx: &egui::Context) {
        let theme = self.theme.clone();
        // The picker.
        if let Some(mut picker) = self.picker.take() {
            match picker.ui(ctx, &theme) {
                None => self.picker = Some(picker),
                Some(Err(())) => {}
                Some(Ok(Picked::Line(line))) => {
                    if let Some(path) = self.current().map(|t| t.path.clone()) {
                        self.open_path(&path, None, Some(line), true);
                    }
                }
                Some(Ok(Picked::Search(q))) => self.search_project(&q),
                Some(Ok(Picked::Item(item))) => match item.payload {
                    Payload::File(rel) => {
                        if let Some(root) = self.root.clone() {
                            self.open_path(&root.join(rel), None, None, true);
                        }
                    }
                    Payload::Loc(path, at) => self.open_path(&path, Some(at), None, true),
                    Payload::Action(a) => self.run_action(a, ctx),
                },
            }
        }
        // The list to choose a definition from.
        if let Some(choose) = &self.choose {
            let mut picked = None;
            let mut close = false;
            let index = self.index.clone();
            let area = egui::Area::new(egui::Id::unique("choose"))
                .order(egui::Order::Foreground)
                .fixed_pos(choose.pos + egui::vec2(8.0, 14.0))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).fill(theme.bg_light).show(ui, |ui| {
                        ui.set_max_width(760.0);
                        ui.label(
                            RichText::new(format!("Choose a definition of {}", choose.title))
                                .color(theme.fg_dim)
                                .small(),
                        );
                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            for (i, t) in choose.targets.iter().enumerate() {
                                let place = index
                                    .as_ref()
                                    .map(|ix| {
                                        format!(
                                            "{}:{}",
                                            ix.display_path(t.loc.file),
                                            ix.files[t.loc.file as usize].raw.line_of(t.loc.span.start) + 1
                                        )
                                    })
                                    .unwrap_or_default();
                                let r = ui
                                    .horizontal(|ui| {
                                        let r = ui.add(
                                            egui::Label::new(RichText::new(&t.label).color(theme.fg_bright))
                                                .sense(egui::Sense::click()),
                                        );
                                        if let Some(d) = &t.detail {
                                            ui.label(RichText::new(d).color(theme.fg_dim).small());
                                        }
                                        if t.confidence != Confidence::Exact {
                                            ui.label(RichText::new(t.confidence.label()).color(theme.yellow).small());
                                        }
                                        let p = ui.add(
                                            egui::Label::new(RichText::new(place).color(theme.accent).small())
                                                .sense(egui::Sense::click()),
                                        );
                                        r.union(p)
                                    })
                                    .inner;
                                if r.clicked() {
                                    picked = Some(i);
                                }
                            }
                        });
                    });
                });
            if ctx.input(|i| i.pointer.any_click())
                && !area.response.hovered()
                && picked.is_none()
                && choose.opened != self.frame_nr
            {
                close = true;
            }
            if let Some(i) = picked {
                let t = choose.targets[i].clone();
                self.choose = None;
                self.go_to_target(&t);
            } else if close {
                self.choose = None;
            }
        }
        // The hover popup.
        if let Some(h) = &self.hover
            && let Some(data) = &h.data
        {
            egui::Area::new(egui::Id::unique("hover"))
                .order(egui::Order::Tooltip)
                .fixed_pos(h.pos + egui::vec2(12.0, 18.0))
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).fill(theme.bg_light).show(ui, |ui| {
                        ui.set_max_width(620.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&data.title).color(theme.fg_bright).monospace().strong());
                            if data.confidence != Confidence::Exact {
                                ui.label(RichText::new(data.confidence.label()).color(theme.yellow).small());
                            }
                        });
                        if let Some(sig) = &data.signature {
                            ui.label(RichText::new(sig).color(theme.blue).monospace());
                        }
                        if let Some(r) = &data.receiver {
                            ui.label(RichText::new(r).color(theme.fg_dim).small());
                        }
                        ui.label(RichText::new(&data.location).color(theme.accent).small());
                        if let Some(doc) = &data.doc {
                            ui.separator();
                            let short: String = doc.lines().take(14).collect::<Vec<_>>().join("\n");
                            ui.label(RichText::new(short).color(theme.fg));
                        }
                        if data.more > 0 {
                            ui.label(
                                RichText::new(format!("+{} more definitions", data.more)).color(theme.fg_dim).small(),
                            );
                        }
                    });
                });
        }
        // The right-click menu.
        if let Some((offset, pos, opened)) = self.context_menu {
            let mut action: Option<&'static str> = None;
            let related = match (&self.index, self.current_file_id()) {
                (Some(index), Some(file)) => index.related_at(file, offset),
                _ => None,
            };
            let area = egui::Area::new(egui::Id::unique("context")).order(egui::Order::Foreground).fixed_pos(pos).show(
                ctx,
                |ui| {
                    egui::Frame::menu(ui.style()).fill(theme.bg_light).show(ui, |ui| {
                        ui.set_min_width(220.0);
                        if let Some((label, _)) = related
                            && ui.button(label).clicked()
                        {
                            action = Some("related");
                        }
                        for (id, label) in [
                            ("def", "Go to definition"),
                            ("all", "All definitions"),
                            ("usages", "Find usages"),
                            ("hierarchy", "Type hierarchy"),
                            ("copy_name", "Copy name"),
                            ("copy_path", "Copy path:line"),
                            ("editor", "Open in editor"),
                            ("github", "Open on GitHub"),
                        ] {
                            if ui.button(label).clicked() {
                                action = Some(id);
                            }
                        }
                    });
                },
            );
            let clicked_outside =
                ctx.input(|i| i.pointer.any_click()) && !area.response.hovered() && opened != self.frame_nr;
            if let Some(a) = action {
                self.context_menu = None;
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.cursor = Some(offset);
                }
                match a {
                    "def" => self.definition_at_cursor(false, pos),
                    "all" => self.definition_at_cursor(true, pos),
                    "usages" => self.find_usages(),
                    "hierarchy" => self.type_hierarchy(),
                    "related" => self.related_at_cursor(pos),
                    "copy_name" => {
                        if let Some(t) = self.current() {
                            let (a, b) = view::word_at(&t.text, offset as usize);
                            ctx.copy_text(t.text[a..b].to_string());
                        }
                    }
                    "copy_path" => self.run_action(Action::CopyPath, ctx),
                    "editor" => self.run_action(Action::OpenInEditor, ctx),
                    "github" => self.run_action(Action::OpenOnGitHub, ctx),
                    _ => {}
                }
            } else if clicked_outside {
                self.context_menu = None;
            }
        }
        // A short message at the bottom.
        if let Some((text, at)) = &self.toast {
            let age = self.started.elapsed().as_secs_f64() - at;
            if age > 3.5 {
                self.toast = None;
            } else {
                let screen = ctx.content_rect();
                egui::Area::new(egui::Id::unique("toast"))
                    .order(egui::Order::Tooltip)
                    .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -30.0))
                    .interactable(false)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).fill(theme.bg_light).show(ui, |ui| {
                            ui.set_max_width(screen.width() * 0.7);
                            ui.label(RichText::new(text).color(theme.fg_bright));
                        });
                    });
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let project = self.root.as_ref().and_then(|r| r.file_name()).map(|n| n.to_string_lossy().to_string());
        let title = match (project, self.current()) {
            (Some(p), Some(t)) => format!("rubyscope — {p} — {}", t.rel),
            (Some(p), None) => format!("rubyscope — {p}"),
            _ => "rubyscope".to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// One frame of the whole window.
    pub fn frame_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.frame_nr += 1;
        self.handle_messages();
        self.update_title(&ctx);

        // Keys. The picker and the find bar get Enter, arrows and typing.
        for action in self.keys.pressed(&ctx) {
            self.run_action(action, &ctx);
        }
        let (back, fwd) = ctx.input(|i| {
            (
                i.pointer.button_pressed(egui::PointerButton::Extra1),
                i.pointer.button_pressed(egui::PointerButton::Extra2),
            )
        });
        if back {
            self.go_back();
        }
        if fwd {
            self.go_forward();
        }
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)))
            && let Some(text) = self.current().and_then(|t| t.selected_text())
        {
            ctx.copy_text(text);
        }

        if self.root.is_none() {
            self.welcome(ui);
            self.overlays(&ctx);
            return;
        }
        self.toolbar(ui);
        if self.show_sidebar {
            self.sidebar(ui);
        }
        if self.show_bottom && self.bottom.is_some() {
            self.bottom_panel(ui);
        }
        let theme = self.theme.clone();
        let mut events = view::ViewEvents::default();
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme.bg)).show(ui, |ui| {
            if self.tabs.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(120.0);
                    ui.label(
                        RichText::new(format!(
                            "{c}+P to open a file · {c}+T to find a class or method · {c}+K for all commands",
                            c = keys::CMD
                        ))
                        .color(theme.fg_dim),
                    );
                });
                return;
            }
            self.tab_bar(ui);
            self.find_bar(ui);
            let index = self.index.clone();
            let file_id = self.current_file_id();
            let show_blame = self.show_blame;
            let t = &mut self.tabs[self.active];
            let links = match (&index, file_id) {
                (Some(ix), Some(f)) => Some(Links { index: ix, file: f }),
                _ => None,
            };
            events =
                view::code_view(ui, t, &theme, links.as_ref().map(|l| l as &dyn Clickable), show_blame, now_secs());
        });

        // What happened in the code view.
        if let Some((offset, pos)) = events.goto {
            self.hover = None;
            if let Some(t) = self.tabs.get_mut(self.active) {
                t.cursor = Some(offset);
            }
            self.definition_at_cursor(false, pos);
        }
        if let Some(offset) = events.context {
            let pos = ctx.input(|i| i.pointer.latest_pos()).unwrap_or_default();
            self.context_menu = Some((offset, pos, self.frame_nr));
            self.hover = None;
        }
        let popup_open = self.choose.is_some() || self.context_menu.is_some() || self.picker.is_some();
        match events.hover.filter(|_| !popup_open) {
            Some((offset, pos)) => {
                let same = self.hover.as_ref().is_some_and(|h| h.offset == offset);
                if !same {
                    let data = match (self.index.clone(), self.current_file_id()) {
                        (Some(ix), Some(f)) => ix.hover(f, offset),
                        _ => None,
                    };
                    self.hover = Some(HoverPopup { offset, pos, data });
                }
            }
            None => self.hover = None,
        }
        if let (Some(sha), Some(root)) = (events.blame_click, self.root.clone()) {
            self.toast("Opening the commit on GitHub…");
            self.worker.open_commit(root, sha);
        }
        self.overlays(&ctx);
        let _ = Color32::TRANSPARENT;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame_ui(ui);
    }
}
