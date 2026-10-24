//! The Rubyscope window: panels, tabs, navigation and actions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, RichText};
use rubyscope_index::model::Span;
use rubyscope_index::{Confidence, FileId, Hover, Index, OutlineKind, Symbol, Target};

use crate::closing::{Closing, Step};
use crate::diff::{Branch, FileMark, RepoStatus};
use crate::external;
use crate::git::Pull;
use crate::keys::{self, Action, Keymap};
use crate::memory::{self, Memory};
use crate::picker::{Payload, PickItem, Picked, Picker, PickerKind};
use crate::projects::{self, Project};
use crate::theme::Theme;
use crate::tree::{self, FileTree};
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
    /// The changed files, in diff mode only.
    Changes,
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

/// Which tabs a tab's right-click menu closes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabClose {
    This,
    Left,
    Right,
    All,
}

pub struct App {
    pub root: Option<PathBuf>,
    pub theme: Theme,
    keys: Keymap,
    pub(crate) worker: Worker,
    pub index: Option<Arc<Index>>,
    index_status: String,
    /// The last index: Ruby files, of them in the project, and seconds.
    /// Shown when `index_status` is empty.
    indexed: Option<(usize, usize, f64)>,
    memory: Memory,
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
    /// Start screen: the project being dragged to a new place.
    welcome_drag: Option<usize>,
    /// Start screen: the project being renamed, its new name, and true
    /// once the name field has the keys.
    renaming: Option<(usize, String, bool)>,
    /// Start screen: the icon of each project, None when it has none.
    project_icons: HashMap<String, Option<egui::TextureHandle>>,
    brand_logo: Option<egui::TextureHandle>,
    /// True while the folder dialog is open.
    picking_folder: bool,
    /// Where the folder from the dialog opens: a new window, or this one.
    picked_to_new_window: bool,
    /// The message at the bottom: the text, when it showed, and for how
    /// many seconds.
    toast: Option<(String, f64, f64)>,
    /// A `git pull` from the pull button runs.
    pub pulling: bool,
    title: String,
    started: Instant,
    /// Diff mode: the branch and the working tree against a base branch.
    pub show_diff: bool,
    /// The repository status for diff mode, once it is read.
    pub repo: Option<Arc<RepoStatus>>,
    /// The base branch the user chose for this project (none: the default).
    pub diff_base: Option<String>,
    diff_marks: Arc<HashMap<String, FileMark>>,
    /// `]` at the last change asks first, then opens the next file: the
    /// direction of the first press.
    diff_edge: i8,
    /// `HEAD` and the base as commits, to see when they move.
    diff_fingerprint: String,
    diff_poll: Option<Instant>,
    /// The checked-out branch, for the branch button. Read every few
    /// seconds.
    pub head_name: Option<String>,
    head_poll: Option<Instant>,
    /// The authors of open pull requests, by branch (from `gh`).
    pr_authors: HashMap<String, crate::git::PrAuthor>,
    /// The last branch list of the switcher.
    switch_branches: Vec<Branch>,
    /// A branch switch runs.
    pub switching: bool,
    /// Say what diff mode compares when the status comes.
    announce_diff: bool,
    /// The window is closing: the work that runs before the app quits.
    pub closing: Option<Closing>,
    /// The app has quit (tests: the process does not end there).
    pub quit: bool,
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

/// Scrolls to a change, frames it, and puts the cursor on its first line.
fn go_to_change(t: &mut OpenFile, c: usize) {
    let Some(map) = t.rows.clone() else { return };
    let Some(&(first, _)) = map.changes.get(c) else { return };
    t.change = Some(c);
    t.scroll_to_row = Some((first, 1.0 / 3.0));
    t.cursor = Some(t.line_start(t.line_of_row(first)));
}

/// A small square button with a folder, drawn like the pull button. Its
/// name for screen readers and tests is "Go to file".
fn folder_button(ui: &mut egui::Ui, theme: &Theme) -> egui::Response {
    let side = ui.spacing().interact_size.y;
    let r = ui.add(egui::Button::new("").min_size(egui::vec2(side, side)));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Go to file"));
    let color = if r.hovered() { theme.fg_bright } else { theme.fg };
    let stroke = egui::Stroke::new(1.5, color);
    let c = r.rect.center();
    let (w, h) = (side * 0.58, side * 0.42);
    let (left, top) = (c.x - w / 2.0, c.y - h / 2.0 + side * 0.03);
    let (right, bottom) = (left + w, top + h);
    // The tab on top, then the body, as one outline.
    let tab_end = left + w * 0.42;
    let tab_top = top - side * 0.1;
    let points = vec![
        Pos2::new(left, bottom),
        Pos2::new(left, tab_top),
        Pos2::new(tab_end - side * 0.06, tab_top),
        Pos2::new(tab_end, top),
        Pos2::new(right, top),
        Pos2::new(right, bottom),
    ];
    ui.painter().add(egui::Shape::closed_line(points, stroke));
    r
}

/// A small square button with a circular arrow. No font of the app has a
/// refresh sign, so the arrow is drawn. Its name for screen readers and
/// tests is "Pull".
fn pull_button(ui: &mut egui::Ui, theme: &Theme) -> egui::Response {
    let side = ui.spacing().interact_size.y;
    let r = ui.add(egui::Button::new("").min_size(egui::vec2(side, side)));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Pull"));
    let color = if r.hovered() { theme.fg_bright } else { theme.fg };
    let stroke = egui::Stroke::new(1.5, color);
    let c = r.rect.center();
    let radius = side * 0.27;
    // From the top, most of the way around, clockwise.
    let (start, sweep) = (-std::f32::consts::FRAC_PI_2 + 0.35, std::f32::consts::TAU * 0.74);
    let at = |a: f32| c + radius * egui::vec2(a.cos(), a.sin());
    let points: Vec<Pos2> = (0..=24).map(|i| at(start + sweep * i as f32 / 24.0)).collect();
    let end = start + sweep;
    let tip = at(end);
    // The arrow head points along the circle.
    let along = egui::vec2(-end.sin(), end.cos());
    let out = egui::vec2(end.cos(), end.sin());
    let head = radius * 1.1;
    ui.painter().add(egui::Shape::line(points, stroke));
    ui.painter().add(egui::Shape::convex_polygon(
        vec![tip + along * head, tip + out * head * 0.65, tip - out * head * 0.65],
        color,
        egui::Stroke::NONE,
    ));
    r
}

/// A small square button that switches dark and light mode: a sun in
/// dark mode, a moon in light mode. Both are drawn, like the pull button.
/// Its name for screen readers and tests is "Light mode" or "Dark mode",
/// the mode a click turns on.
fn mode_button(ui: &mut egui::Ui, theme: &Theme) -> egui::Response {
    let side = ui.spacing().interact_size.y;
    let r = ui.add(egui::Button::new("").min_size(egui::vec2(side, side)));
    let name = if theme.dark { "Light mode" } else { "Dark mode" };
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    let color = if r.hovered() { theme.fg_bright } else { theme.fg };
    let c = r.rect.center();
    let painter = ui.painter();
    if theme.dark {
        // The sun: a disc and 8 rays.
        painter.circle_filled(c, side * 0.16, color);
        let stroke = egui::Stroke::new(1.5, color);
        for i in 0..8 {
            let a = std::f32::consts::TAU * i as f32 / 8.0;
            let d = egui::vec2(a.cos(), a.sin());
            painter.line_segment([c + d * side * 0.24, c + d * side * 0.34], stroke);
        }
    } else {
        // The moon: a disc with a disc of the button color over one side.
        let fill = if r.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            ui.visuals().widgets.inactive.weak_bg_fill
        };
        painter.circle_filled(c, side * 0.3, color);
        painter.circle_filled(c + egui::vec2(side * 0.15, -side * 0.1), side * 0.26, fill);
    }
    r
}

/// The saved mode: dark unless the user chose light.
fn saved_dark() -> bool {
    state_dir().and_then(|d| std::fs::read_to_string(d.join("mode")).ok()).map(|m| m.trim() != "light").unwrap_or(true)
}

fn save_dark(dark: bool) {
    if let Some(d) = state_dir() {
        let _ = std::fs::write(d.join("mode"), if dark { "dark" } else { "light" });
    }
}

/// After `]` opened a file: its first change (or its last one for `[`),
/// once its diff is there.
fn apply_pending_change(t: &mut OpenFile) {
    let Some(dir) = t.pending_change else { return };
    let Some(map) = t.rows.clone() else { return };
    t.pending_change = None;
    if !map.changes.is_empty() {
        go_to_change(t, if dir > 0 { 0 } else { map.changes.len() - 1 });
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Where Rubyscope keeps open tabs and recent projects. Tests use a temp
/// folder for each test thread, so they never touch the user's state or
/// each other's.
fn state_dir() -> Option<PathBuf> {
    let dir = if cfg!(test) {
        let thread: String =
            format!("{:?}", std::thread::current().id()).chars().filter(char::is_ascii_digit).collect();
        std::env::temp_dir().join(format!("rubyscope-test-state-{}-{thread}", std::process::id()))
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".local/state/rubyscope")
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn session_path(root: &Path) -> Option<PathBuf> {
    Some(state_dir()?.join(format!("session{}.json", state_name(root))))
}

/// A file name part for a project folder.
fn state_name(root: &Path) -> String {
    root.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// The base branch of diff mode for a project.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct DiffState {
    base: Option<String>,
}

fn diff_state_path(root: &Path) -> Option<PathBuf> {
    Some(state_dir()?.join(format!("diff{}.json", state_name(root))))
}

fn load_diff_base(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(diff_state_path(root)?).ok()?;
    serde_json::from_str::<DiffState>(&text).ok()?.base
}

fn save_diff_base(root: &Path, base: Option<&str>) {
    if let Some(p) = diff_state_path(root) {
        let state = DiffState { base: base.map(String::from) };
        let _ = std::fs::write(p, serde_json::to_string(&state).unwrap_or_default());
    }
}

/// `app/models` and `order.rb` for `app/models/order.rb`.
fn split_path(rel: &str) -> (&str, &str) {
    match rel.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", rel),
    }
}

/// The projects of the start screen, in the order the user gave them.
pub fn recent_projects() -> Vec<Project> {
    state_dir()
        .and_then(|d| std::fs::read_to_string(d.join("recent.json")).ok())
        .map(|t| projects::parse(&t))
        .unwrap_or_default()
}

pub fn save_projects(list: &[Project]) {
    if let Some(d) = state_dir() {
        let _ = std::fs::write(d.join("recent.json"), serde_json::to_string(list).unwrap_or_default());
    }
}

fn remember_project(root: &Path) {
    let mut list = recent_projects();
    if projects::add(&mut list, &root.display().to_string()) {
        save_projects(&list);
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
        let theme = Theme::load(saved_dark());
        theme.apply(ctx);
        let mut app = App {
            root: None,
            theme,
            keys: Keymap::load(),
            worker: Worker::new(ctx.clone()),
            index: None,
            index_status: String::new(),
            indexed: None,
            memory: Memory::new(),
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
            welcome_drag: None,
            renaming: None,
            project_icons: HashMap::new(),
            brand_logo: None,
            picking_folder: false,
            picked_to_new_window: false,
            toast: None,
            pulling: false,
            title: String::new(),
            started: Instant::now(),
            show_diff: false,
            repo: None,
            diff_base: None,
            diff_marks: Arc::new(HashMap::new()),
            diff_edge: 0,
            diff_fingerprint: String::new(),
            diff_poll: None,
            head_name: None,
            head_poll: None,
            pr_authors: HashMap::new(),
            switch_branches: Vec::new(),
            switching: false,
            announce_diff: false,
            closing: None,
            quit: false,
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
        self.diff_base = load_diff_base(&root);
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

    /// Closes the project and goes back to the project list. The open tabs
    /// are saved, so the project opens with them again.
    pub fn close_project(&mut self) {
        self.save_session();
        self.worker.restart();
        self.root = None;
        self.index = None;
        self.index_status.clear();
        self.indexed = None;
        self.tree = FileTree::default();
        self.all_files = Arc::new(Vec::new());
        self.tabs.clear();
        self.active = 0;
        self.history.clear();
        self.history_pos = 0;
        self.bottom = None;
        self.picker = None;
        self.file_items = None;
        self.symbol_items = None;
        self.choose = None;
        self.hover = None;
        self.context_menu = None;
        self.pending_changes.clear();
        self.last_change = None;
        self.picking_folder = false;
        self.show_diff = false;
        self.repo = None;
        self.diff_base = None;
        self.diff_marks = Arc::new(HashMap::new());
        self.diff_edge = 0;
        self.diff_fingerprint.clear();
        self.head_name = None;
        self.head_poll = None;
        self.pr_authors.clear();
        self.switch_branches.clear();
        self.switching = false;
        if self.sidebar_tab == SidebarTab::Changes {
            self.sidebar_tab = SidebarTab::Files;
        }
    }

    /// Opens another project in this window.
    fn switch_project(&mut self, root: PathBuf) {
        let root = root.canonicalize().unwrap_or(root);
        if self.root.as_ref() == Some(&root) {
            return;
        }
        if self.root.is_some() {
            self.close_project();
        }
        self.open_project(root);
    }

    /// The name the user gave the project, or its folder name.
    fn project_name(&self) -> String {
        let Some(root) = &self.root else { return String::new() };
        let path = root.display().to_string();
        recent_projects().into_iter().find(|p| p.path == path).unwrap_or_else(|| Project::new(&path)).title()
    }

    /// Opens a project with an index that is already built (tests).
    #[cfg(test)]
    pub fn open_prebuilt(&mut self, root: PathBuf, index: Arc<Index>) {
        self.tree = FileTree::new(&index.all_files);
        self.all_files = Arc::new(index.all_files.clone());
        self.diff_base = load_diff_base(&root);
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

    /// The window was asked to close (Super+W, the close button, Cmd+Q).
    /// The close is held back until the steps are done. A second close
    /// quits at once.
    fn close_requested(&mut self, ctx: &egui::Context) {
        if self.quit {
            return;
        }
        if self.closing.is_some() {
            self.quit(ctx);
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        let mut steps = vec![Step::SaveTabs, Step::StopWatcher];
        if self.worker.saving.load(std::sync::atomic::Ordering::SeqCst) {
            steps.push(Step::SaveIndex);
        }
        if self.pulling {
            steps.push(Step::Pull);
        }
        if self.switching {
            steps.push(Step::SwitchBranch);
        }
        self.closing = Some(Closing::new(steps));
    }

    /// Runs the closing steps that can run now, and quits when all are
    /// done (or after `GIVE_UP_AFTER`).
    fn run_closing(&mut self, ctx: &egui::Context) {
        let Some(closing) = &self.closing else { return };
        let given_up = closing.given_up();
        while let Some(step) = self.closing.as_ref().and_then(|c| c.current()) {
            let done = match step {
                Step::SaveTabs => {
                    self.save_session();
                    true
                }
                Step::StopWatcher => {
                    self.worker.stop_watching();
                    true
                }
                Step::SaveIndex => !self.worker.saving.load(std::sync::atomic::Ordering::SeqCst),
                Step::Pull => !self.pulling,
                Step::SwitchBranch => !self.switching,
            };
            if !done && !given_up {
                // The worker asks for a repaint when it is done; this is
                // for the dialog and the time limit.
                ctx.request_repaint_after(Duration::from_millis(50));
                return;
            }
            if let Some(c) = &mut self.closing {
                c.done += 1;
            }
        }
        self.quit(ctx);
    }

    /// Ends the app. The memory is not freed first: the system frees it
    /// all at once, and freeing the index alone takes a while.
    fn quit(&mut self, ctx: &egui::Context) {
        if let Some(c) = &self.closing {
            eprintln!("rubyscope: closed in {:.2} s", c.started.elapsed().as_secs_f64());
        }
        self.quit = true;
        if cfg!(test) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            std::process::exit(0);
        }
    }

    /// The message at the bottom, if one shows (tests).
    #[cfg(test)]
    pub fn toast_text(&self) -> Option<String> {
        self.toast.as_ref().map(|(t, _, _)| t.clone())
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast_for(text, 3.5);
    }

    fn toast_for(&mut self, text: impl Into<String>, secs: f64) {
        self.toast = Some((text.into(), self.started.elapsed().as_secs_f64(), secs));
    }

    /// The pull button: `git pull` in the background.
    fn pull(&mut self) {
        let Some(root) = self.root.clone() else { return };
        if self.pulling {
            return;
        }
        self.pulling = true;
        self.worker.pull(root);
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
        if self.show_diff && self.tabs[idx].diff.is_none() {
            self.request_diff(idx);
        }
        self.diff_edge = 0;
        if record {
            let line = target_line.unwrap_or(0);
            self.push_history(NavPoint { path: path.to_path_buf(), line });
        }
        self.save_session();
    }

    // ----- Diff mode -----

    /// Turns diff mode on or off.
    fn toggle_diff(&mut self) {
        if self.root.is_none() {
            return;
        }
        self.show_diff = !self.show_diff;
        self.diff_edge = 0;
        self.file_items = None;
        for t in &mut self.tabs {
            t.change = None;
            t.pending_change = None;
        }
        if self.show_diff {
            self.announce_diff = true;
            self.refresh_repo();
        } else {
            if self.sidebar_tab == SidebarTab::Changes {
                self.sidebar_tab = SidebarTab::Files;
            }
            self.toast("Diff mode off");
        }
        let on = self.show_diff;
        for t in &mut self.tabs {
            t.set_rows(on);
        }
    }

    /// Reads the repository status again (in the background). The diffs of
    /// the open files follow when it comes.
    fn refresh_repo(&mut self) {
        if let Some(root) = self.root.clone() {
            self.worker.repo_status(root, self.diff_base.clone());
        }
    }

    /// Asks for the diffs of a tab, when it is a file of the project.
    fn request_diff(&mut self, idx: usize) {
        let (Some(root), Some(repo)) = (self.root.clone(), self.repo.clone()) else { return };
        let Some(t) = self.tabs.get_mut(idx) else { return };
        if repo.error.is_some() || !t.path.starts_with(&root) {
            return;
        }
        t.diff_requested = true;
        self.worker.file_diff(
            root,
            t.path.clone(),
            t.rel.clone(),
            t.text.clone(),
            repo.merge_base.clone(),
            repo.head.clone(),
        );
    }

    /// `]` (dir 1) or `[` (dir -1): the next or previous change in the file.
    /// After the last one, a toast first, then the next changed file.
    fn jump_change(&mut self, dir: i8) {
        if !self.show_diff {
            let key = self.keys.shortcut(Action::ToggleDiff).map(|s| s.label()).unwrap_or_default();
            self.toast(format!("Turn on diff mode to jump between changes: {key}"));
            return;
        }
        let Some(t) = self.tabs.get_mut(self.active) else {
            self.jump_file(dir, "");
            return;
        };
        let name = t.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let Some(map) = t.rows.clone() else {
            if t.diff_requested {
                self.toast("The diff is not ready yet");
            } else {
                self.jump_file(dir, "No changes in this file.");
            }
            return;
        };
        let n = map.changes.len() as isize;
        if n == 0 {
            self.jump_file(dir, "No changes in this file.");
            return;
        }
        let next = match t.change {
            Some(c) => c as isize + dir as isize,
            None if dir > 0 => map.changes.iter().position(|c| c.0 >= t.top_row).map_or(n, |i| i as isize),
            None => map.changes.iter().rposition(|c| c.0 <= t.top_row).map_or(-1, |i| i as isize),
        };
        if next < 0 || next >= n {
            let why = if dir > 0 { format!("Last change in {name}.") } else { format!("First change in {name}.") };
            self.jump_file(dir, &why);
            return;
        }
        self.diff_edge = 0;
        go_to_change(t, next as usize);
    }

    /// Opens the next (or previous) changed file, at its first (or last)
    /// change. The first press only says so.
    fn jump_file(&mut self, dir: i8, why: &str) {
        let (Some(root), Some(repo)) = (self.root.clone(), self.repo.clone()) else { return };
        let files = repo.changed_paths();
        if files.is_empty() {
            self.toast(format!("Nothing changed against {}.", repo.base));
            return;
        }
        let n = files.len() as isize;
        let here = self.current().and_then(|t| files.iter().position(|f| *f == t.rel));
        let next = match here {
            Some(i) => files[(i as isize + dir as isize).rem_euclid(n) as usize].clone(),
            None if dir > 0 => files[0].clone(),
            None => files[files.len() - 1].clone(),
        };
        if self.diff_edge != dir {
            self.diff_edge = dir;
            let key = if dir > 0 { "]" } else { "[" };
            let (_, name) = split_path(&next);
            self.toast(format!("{why} Press {key} again for {name}").trim().to_string());
            return;
        }
        self.open_path(&root.join(&next), None, None, true);
        self.diff_edge = 0;
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.change = None;
            t.pending_change = Some(dir);
            apply_pending_change(t);
        }
        self.toast(format!("Opened {next}"));
    }

    /// The base branch picker. The branches load in the background.
    fn open_branch_picker(&mut self) {
        let Some(root) = self.root.clone() else { return };
        self.picker = Some(Picker::new(PickerKind::Branches, Arc::new(Vec::new())));
        self.worker.branches(root);
    }

    fn branch_items(&self, list: &[Branch]) -> Vec<PickItem> {
        let theme = &self.theme;
        let base = self.repo.as_ref().map(|r| r.base.clone()).or(self.diff_base.clone()).unwrap_or_default();
        let default = self.repo.as_ref().map(|r| r.default_base.clone()).unwrap_or_default();
        let now = now_secs();
        let mut list: Vec<&Branch> = list.iter().collect();
        // The default branch first in its group.
        list.sort_by_key(|b| (b.remote, b.name != default));
        list.into_iter()
            .map(|b| {
                let detail = if b.is_head && !b.remote {
                    "checked out (HEAD)".to_string()
                } else if b.time > 0 {
                    crate::git::relative_time(b.time, now)
                } else {
                    String::new()
                };
                let mut item = PickItem::new(&b.name, detail, Payload::Branch(b.name.clone()), 0);
                item.group = Some(if b.remote { "Remote" } else { "Local" });
                item.dim = b.is_head && !b.remote;
                if b.name == base {
                    item.tags.push(("✓ current base".into(), theme.green));
                } else if b.name == default {
                    item.tags.push(("default".into(), theme.fg_dim));
                }
                item
            })
            .collect()
    }

    /// The branch switcher. The branches load in the background, then the
    /// authors of their pull requests.
    fn open_switch_picker(&mut self) {
        let Some(root) = self.root.clone() else { return };
        let items = Arc::new(self.switch_items());
        self.picker = Some(Picker::new(PickerKind::SwitchBranch, items));
        self.worker.switch_branches(root);
    }

    fn switch_items(&self) -> Vec<PickItem> {
        let theme = &self.theme;
        let now = now_secs();
        let colors = [theme.blue, theme.green, theme.magenta, theme.cyan, theme.orange, theme.yellow, theme.red];
        self.switch_branches
            .iter()
            .map(|b| {
                let local = if b.remote { crate::git::local_name(&b.name) } else { b.name.as_str() };
                let pr = self.pr_authors.get(local);
                let mut detail = if b.is_head && !b.remote {
                    "checked out".to_string()
                } else if b.time > 0 {
                    crate::git::relative_time(b.time, now)
                } else {
                    String::new()
                };
                match pr {
                    Some(p) => detail.push_str(&format!(" · #{} {}", p.number, p.name)),
                    None if !b.author.is_empty() => detail.push_str(&format!(" · last commit by {}", b.author)),
                    None => {}
                }
                let mut item = PickItem::new(&b.name, detail, Payload::Switch(b.name.clone(), b.remote), 0);
                item.dim = b.is_head && !b.remote;
                if b.remote {
                    item.tags.push(("remote".into(), theme.fg_dim));
                }
                let (who, color) = match pr {
                    Some(p) => {
                        let hash = p.name.bytes().fold(0usize, |h, c| h.wrapping_mul(31).wrapping_add(c as usize));
                        (p.name.as_str(), colors[hash % colors.len()])
                    }
                    None => (b.author.as_str(), theme.fg_dim),
                };
                let initials = crate::git::initials(who);
                if !initials.is_empty() {
                    item.tags.push((initials, color));
                }
                item
            })
            .collect()
    }

    /// Checks out a branch in the background.
    fn switch_to(&mut self, name: String, remote: bool) {
        let Some(root) = self.root.clone() else { return };
        if !remote && self.head_name.as_deref() == Some(name.as_str()) {
            self.toast(format!("{name} is checked out"));
            return;
        }
        self.switching = true;
        self.worker.switch_branch(root, name, remote);
    }

    /// Diffs against another branch, and remembers it for the project.
    fn choose_base(&mut self, name: String) {
        if self.repo.as_ref().is_some_and(|r| r.head_name == name) {
            self.toast(format!("{name} is checked out. Pick another branch to diff against."));
            return;
        }
        if let Some(root) = &self.root {
            save_diff_base(root, Some(&name));
        }
        self.diff_base = Some(name.clone());
        self.diff_edge = 0;
        for t in &mut self.tabs {
            t.change = None;
        }
        if !self.show_diff {
            self.show_diff = true;
        }
        self.refresh_repo();
        self.toast(format!("Diff against {name}. Remembered for this project."));
    }

    /// The tags of a changed file in the pickers: `+13 −2 ● M`.
    fn change_tags(&self, rel: &str) -> Vec<(String, Color32)> {
        let Some(repo) = &self.repo else { return Vec::new() };
        let mark = self.diff_marks.get(rel).copied().unwrap_or_default();
        let counts = repo
            .branch_files
            .iter()
            .find(|f| f.rel == rel)
            .or_else(|| repo.uncommitted_files.iter().find(|f| f.rel == rel));
        let mut tags = Vec::new();
        if let Some(f) = counts {
            tags.push((format!("+{}", f.added), self.theme.green));
            tags.push((format!("−{}", f.removed), self.theme.red));
        }
        if mark.uncommitted {
            tags.push(("●".into(), self.theme.yellow));
        }
        if let Some(b) = mark.branch {
            tags.push((b.to_string(), if b == 'A' { self.theme.green } else { self.theme.blue }));
        }
        tags
    }

    fn changed_file_items(&self) -> Arc<Vec<PickItem>> {
        let files = self.repo.as_ref().map(|r| r.changed_paths()).unwrap_or_default();
        Arc::new(
            files
                .into_iter()
                .map(|rel| {
                    let mut item = PickItem::new(&rel, "", Payload::File(rel.clone()), 0);
                    item.tags = self.change_tags(&rel);
                    item
                })
                .collect(),
        )
    }

    fn changed_title(&self) -> String {
        format!("Go to changed file (vs {})", self.repo.as_ref().map(|r| r.base.as_str()).unwrap_or("…"))
    }

    /// Switches between dark and light mode, and keeps the choice for the
    /// next start.
    pub fn toggle_mode(&mut self, ctx: &egui::Context) {
        let dark = !self.theme.dark;
        self.theme = Theme::load(dark);
        self.theme.apply(ctx);
        // The logo of the start screen has a dark and a light version.
        self.brand_logo = None;
        save_dark(dark);
    }

    fn toggle_blame(&mut self) {
        self.show_blame = !self.show_blame;
        if self.show_blame
            && let (Some(root), Some(t)) = (self.root.clone(), self.tabs.get_mut(self.active))
            && !t.blame_requested
        {
            t.blame_requested = true;
            self.worker.blame(root, t.path.clone(), t.rel.clone());
        }
    }

    /// Go to file; in diff mode Tab shows the changed files.
    fn open_file_picker(&mut self, changed: bool) {
        let items = self.file_items();
        let mut picker = Picker::new(PickerKind::Files, items);
        if self.show_diff {
            picker = picker.with_changed(self.changed_file_items(), self.changed_title(), changed);
        }
        self.picker = Some(picker);
    }

    fn handle_repo_status(&mut self, st: Arc<RepoStatus>) {
        if !self.show_diff {
            return;
        }
        self.diff_marks = Arc::new(st.marks());
        self.file_items = None;
        self.repo = Some(st.clone());
        if self.announce_diff {
            self.announce_diff = false;
            let msg = match (&st.error, st.base_missing) {
                (Some(e), _) => format!("Diff mode needs git: this project is {e}"),
                (None, true) if st.base.is_empty() => {
                    "No main or master branch: choose a base with \"Diff against branch…\"".to_string()
                }
                (None, true) => format!("Branch {} not found: choose a base with \"Diff against branch…\"", st.base),
                (None, false) => format!("Diff mode on: {}", st.range_label()),
            };
            self.toast(msg);
        }
        // The open files: a diff for each, made with this merge-base and HEAD.
        for i in 0..self.tabs.len() {
            let t = &self.tabs[i];
            let stale = match &t.diff {
                None => !t.diff_requested,
                Some(d) => d.merge_base != st.merge_base || d.head != st.head,
            };
            if stale {
                self.request_diff(i);
            }
        }
        if let Some(p) = self.picker.take() {
            self.picker = Some(if p.kind == PickerKind::Files {
                let (items, title) = (self.changed_file_items(), self.changed_title());
                let mut fresh =
                    Picker::new(PickerKind::Files, self.file_items()).with_changed(items, title, p.show_changed);
                fresh.query = p.query;
                fresh
            } else {
                p
            });
        }
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

    /// A Ctrl+click. On the name in a class, module or constant definition
    /// it lists the usages, as there is nowhere else to go. Elsewhere it
    /// goes to the definition.
    fn click_at_cursor(&mut self, pos: Pos2) {
        let (Some(index), Some(file)) = (self.index.clone(), self.current_file_id()) else { return };
        let Some(cursor) = self.current().and_then(|t| t.cursor) else { return };
        match index.declaration_at(file, cursor) {
            Some(sym) => self.show_usages(&index, &sym, "Usages of"),
            None => self.definition_at_cursor(false, pos),
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
    fn pick_folder(&mut self, new_window: bool) {
        if self.picking_folder {
            return;
        }
        self.picking_folder = true;
        self.picked_to_new_window = new_window;
        let start = self
            .root
            .as_ref()
            .and_then(|r| r.parent().map(Path::to_path_buf))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("git")).filter(|p| p.is_dir()));
        self.worker.pick_folder(start);
    }

    /// Opens a folder in a new window, or in this one. The project list has
    /// no project, so a folder always opens there.
    fn open_folder(&mut self, folder: PathBuf, new_window: bool) {
        if new_window && self.root.is_some() {
            self.open_in_new_window(&folder);
        } else {
            self.switch_project(folder);
        }
    }

    fn open_in_new_window(&mut self, folder: &Path) {
        if self.root.as_deref() == Some(folder) {
            return;
        }
        let spawned = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg(folder).spawn());
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
            .map(|rel| {
                let mut item =
                    PickItem::new(rel, "", Payload::File(rel.clone()), if rel.ends_with(".rb") { 2 } else { 0 });
                if self.show_diff && self.diff_marks.contains_key(rel) {
                    item.tags = self.change_tags(rel);
                }
                item
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
                PickItem::new(
                    e.label,
                    format!("{} · {where_}", e.detail),
                    Payload::Loc(path, e.loc.name_span.start),
                    if e.project { 30 } else { 0 },
                )
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
                .map(|a| PickItem::new(a.title(), self.keys.labels(*a), Payload::Action(*a), 0))
                .collect(),
        )
    }

    pub fn run_action(&mut self, action: Action, ctx: &egui::Context) {
        let pointer = ctx.input(|i| i.pointer.latest_pos()).unwrap_or(Pos2::new(200.0, 200.0));
        match action {
            // From a project, a new window: one window per project.
            Action::OpenFolder => self.pick_folder(self.root.is_some()),
            Action::CloseProject => {
                if self.root.is_some() {
                    self.close_project();
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::GoToFile => self.open_file_picker(false),
            Action::GoToChangedFile => {
                if !self.show_diff {
                    self.toggle_diff();
                }
                self.open_file_picker(true);
            }
            Action::ToggleDiff => self.toggle_diff(),
            Action::DiffAgainstBranch => self.open_branch_picker(),
            Action::SwitchBranch => self.open_switch_picker(),
            Action::NextChange => self.jump_change(1),
            Action::PrevChange => self.jump_change(-1),
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
            Action::CloseTab => self.close_tabs(self.active, TabClose::This),
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
                } else if let Some(t) = self.tabs.get_mut(self.active).filter(|t| t.change.is_some()) {
                    t.change = None;
                } else if self.bottom.is_some() && self.show_bottom {
                    self.show_bottom = false;
                }
            }
            Action::ToggleBlame => self.toggle_blame(),
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
                    self.index_status.clear();
                    self.indexed = Some((s.files, s.project_files, took.as_secs_f64()));
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
                        self.open_folder(folder, self.picked_to_new_window);
                    }
                }
                Msg::RepoStatus(st) => self.handle_repo_status(st),
                Msg::FileDiff { path, text, diff } => {
                    let on = self.show_diff;
                    for t in self.tabs.iter_mut().filter(|t| t.path == path) {
                        // A diff of older text: a newer one is on its way.
                        if *t.text != *text {
                            continue;
                        }
                        t.diff = Some(diff.clone());
                        t.diff_requested = false;
                        t.set_rows(on);
                        apply_pending_change(t);
                    }
                    // Made with an older merge-base or HEAD: make it again.
                    if let Some(repo) = self.repo.clone()
                        && (diff.merge_base != repo.merge_base || diff.head != repo.head)
                        && let Some(i) = self.tabs.iter().position(|t| t.path == path && !t.diff_requested)
                    {
                        self.request_diff(i);
                    }
                }
                Msg::Branches(list) => {
                    let items = Arc::new(self.branch_items(&list));
                    if let Some(p) = self.picker.as_mut().filter(|p| p.kind == PickerKind::Branches) {
                        p.set_items(items);
                    }
                }
                Msg::Pulled(result) => {
                    self.pulling = false;
                    // Conflicts and errors stay longer: they need reading.
                    let secs = match result {
                        Pull::UpToDate | Pull::Pulled(_) => 3.5,
                        _ => 12.0,
                    };
                    self.toast_for(result.message(), secs);
                    // The file watcher reloads changed files; the diff
                    // follows the new HEAD.
                    if self.show_diff {
                        self.refresh_repo();
                    }
                }
                Msg::HeadName(name) => self.head_name = name,
                Msg::SwitchBranches(list) => {
                    self.switch_branches = list;
                    let items = Arc::new(self.switch_items());
                    if let Some(p) = self.picker.as_mut().filter(|p| p.kind == PickerKind::SwitchBranch) {
                        p.set_items(items);
                    }
                }
                Msg::PrAuthors(prs) => {
                    self.pr_authors = prs;
                    let items = Arc::new(self.switch_items());
                    if let Some(p) = self.picker.as_mut().filter(|p| p.kind == PickerKind::SwitchBranch) {
                        p.set_items(items);
                    }
                }
                Msg::Switched(result) => {
                    self.switching = false;
                    match result {
                        Ok(name) => {
                            self.toast(format!("Switched to {name}"));
                            self.head_name = Some(name);
                            // The files on disk changed: reload the open
                            // tabs and index again, as after an edit.
                            self.pending_changes.extend(self.tabs.iter().map(|t| t.path.clone()));
                            self.last_change = Some(Instant::now());
                        }
                        Err(e) => self.toast_for(format!("Could not switch: {e}"), 12.0),
                    }
                }
                Msg::Fingerprint(fp) => {
                    if fp != self.diff_fingerprint {
                        let first = self.diff_fingerprint.is_empty();
                        self.diff_fingerprint = fp;
                        if !first && self.show_diff {
                            self.refresh_repo();
                        }
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
            let mut reloaded = Vec::new();
            for (i, t) in self.tabs.iter_mut().enumerate() {
                if changed.contains(&t.path)
                    && let Ok(text) = std::fs::read_to_string(&t.path)
                    && *text != *t.text
                {
                    let mut fresh = OpenFile::new(t.path.clone(), t.rel.clone(), text);
                    fresh.cursor = t.cursor;
                    fresh.scroll_to_line = Some(t.top_line + 6);
                    self.worker.highlight(fresh.path.clone(), fresh.text.clone());
                    *t = fresh;
                    reloaded.push(i);
                }
            }
            // The new text needs its blame and its diff again.
            for i in reloaded {
                if self.show_blame
                    && let Some(root) = self.root.clone()
                {
                    let t = &mut self.tabs[i];
                    t.blame_requested = true;
                    self.worker.blame(root, t.path.clone(), t.rel.clone());
                }
                if self.show_diff {
                    self.request_diff(i);
                }
            }
            if self.show_diff {
                self.refresh_repo();
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
        let mut toggle_mode = false;
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme.bg)).show(ui, |ui| {
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                ui.add_space(6.0);
                let hint = if theme.dark { "Switch to light mode" } else { "Switch to dark mode" };
                toggle_mode = mode_button(ui, &theme).on_hover_text(hint).clicked();
            });
            ui.vertical_centered(|ui| {
                let logo = self.brand_logo.get_or_insert_with(|| {
                    let bytes: &[u8] = if theme.dark {
                        include_bytes!("../../../packaging/icon/rubyscope-dark.png")
                    } else {
                        include_bytes!("../../../packaging/icon/rubyscope-light.png")
                    };
                    let image = image::load_from_memory(bytes).expect("valid brand logo").to_rgba8();
                    let size = [image.width() as usize, image.height() as usize];
                    ui.ctx().load_texture(
                        "rubyscope-logo",
                        egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
                        egui::TextureOptions::LINEAR,
                    )
                });
                ui.image((logo.id(), egui::vec2(96.0, 96.0)));
                ui.add_space(8.0);
                ui.label(RichText::new("Rubyscope").size(32.0).color(theme.fg_bright));
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
                ui.add_space(32.0);
                if let Some(p) = self.project_list(ui) {
                    open = Some(p);
                }
                ui.add_space(24.0);
                if let Some(quit) = self.keys.shortcut(Action::CloseProject) {
                    ui.label(RichText::new(format!("{} to quit", quit.label())).color(theme.muted).small());
                }
            });
        });
        if toggle_mode {
            self.toggle_mode(ui.ctx());
        }
        if browse {
            self.pick_folder(false);
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

    /// The projects of the start screen: a number key, the icon, the name
    /// and the path of each. A click opens one, a drag moves it, and the
    /// three dots rename or remove it. Returns the project to open.
    fn project_list(&mut self, ui: &mut egui::Ui) -> Option<String> {
        const NUMBER_KEYS: [egui::Key; 9] = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ];
        let theme = self.theme.clone();
        let mut list = recent_projects();
        if list.is_empty() {
            return None;
        }
        let mut open = None;
        let mut changed = false;
        let mut remove = None;
        // A number opens its project, unless a text field has the keys.
        if ui.ctx().memory(|m| m.focused().is_none()) {
            for (i, key) in NUMBER_KEYS.iter().enumerate() {
                if ui.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, *key))
                    && let Some(p) = list.get(i)
                {
                    open = Some(p.path.clone());
                }
            }
        }
        let width = 560.0_f32.min(ui.available_width() - 32.0);
        let mut rows = Vec::new();
        for (i, p) in list.iter_mut().enumerate() {
            let title = p.title();
            if i > 0 {
                ui.add_space(6.0);
            }
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 48.0), egui::Sense::hover());
            rows.push(rect);
            let row = ui.interact(rect, egui::Id::unique(("project", &p.path)), egui::Sense::click_and_drag());
            row.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &title));
            let renaming = self.renaming.as_ref().is_some_and(|r| r.0 == i);
            if row.drag_started() {
                self.welcome_drag = Some(i);
            }
            let dragged = self.welcome_drag == Some(i);
            if row.clicked() && !renaming {
                open = Some(p.path.clone());
            }
            if dragged {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if row.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let painter = ui.painter_at(rect);
            let fill = if row.hovered() || dragged { theme.bg_light } else { theme.bg_dark };
            painter.rect_filled(rect, 6.0, fill);
            if dragged {
                painter.rect_stroke(rect, 6.0, egui::Stroke::new(1.0, theme.accent), egui::StrokeKind::Inside);
            }
            let y = rect.center().y;
            // The number key.
            if i < NUMBER_KEYS.len() {
                let key = egui::Rect::from_center_size(Pos2::new(rect.left() + 22.0, y), egui::vec2(20.0, 20.0));
                painter.rect_stroke(key, 4.0, egui::Stroke::new(1.0, theme.muted), egui::StrokeKind::Inside);
                painter.text(
                    key.center(),
                    egui::Align2::CENTER_CENTER,
                    (i + 1).to_string(),
                    egui::FontId::monospace(12.0),
                    theme.fg_dim,
                );
            }
            // The icon, or the first letter of the name.
            let icon = egui::Rect::from_center_size(Pos2::new(rect.left() + 58.0, y), egui::vec2(24.0, 24.0));
            let texture = self
                .project_icons
                .entry(p.path.clone())
                .or_insert_with(|| {
                    projects::find_icon(Path::new(&p.path)).map(|img| {
                        ui.ctx().load_texture(format!("project-icon-{}", p.path), img, egui::TextureOptions::LINEAR)
                    })
                })
                .clone();
            if let Some(t) = texture {
                let uv = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.image(t.id(), icon, uv, Color32::WHITE);
            } else {
                painter.rect_filled(icon, 5.0, theme.accent.gamma_multiply(0.25));
                let letter: String = title.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                painter.text(
                    icon.center(),
                    egui::Align2::CENTER_CENTER,
                    letter,
                    egui::FontId::proportional(14.0),
                    theme.accent,
                );
            }
            // The name, and the path under it.
            let text_left = rect.left() + 84.0;
            let text_clip = egui::Rect::from_min_max(
                Pos2::new(text_left, rect.top()),
                Pos2::new(rect.right() - 44.0, rect.bottom()),
            );
            if let Some((_, name, focused)) = self.renaming.as_mut().filter(|_| renaming) {
                let field = egui::Rect::from_min_max(
                    Pos2::new(text_left - 4.0, y - 21.0),
                    Pos2::new(text_clip.right(), y + 1.0),
                );
                let edit = ui.place(field, egui::TextEdit::singleline(name).hint_text(&title));
                if !*focused {
                    edit.request_focus();
                    *focused = true;
                } else if edit.lost_focus() {
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let name = name.trim();
                        p.name = (!name.is_empty()).then(|| name.to_string());
                        changed = true;
                    }
                    self.renaming = None;
                }
            } else {
                ui.painter_at(text_clip).text(
                    Pos2::new(text_left, y - 9.0),
                    egui::Align2::LEFT_CENTER,
                    &title,
                    egui::FontId::proportional(15.0),
                    theme.fg_bright,
                );
            }
            ui.painter_at(text_clip).text(
                Pos2::new(text_left, y + 11.0),
                egui::Align2::LEFT_CENTER,
                p.short_path(),
                egui::FontId::proportional(12.0),
                theme.fg_dim,
            );
            // The three dots: rename or remove.
            let dots_rect = egui::Rect::from_center_size(Pos2::new(rect.right() - 22.0, y), egui::vec2(26.0, 26.0));
            let dots = ui.place(dots_rect, egui::Button::new("").frame_when_inactive(false));
            dots.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("More for {title}")));
            let color = if dots.hovered() { theme.fg_bright } else { theme.fg_dim };
            for dx in [-5.0, 0.0, 5.0] {
                ui.painter().circle_filled(dots_rect.center() + egui::vec2(dx, 0.0), 1.6, color);
            }
            egui::Popup::menu(&dots).show(|ui| {
                ui.set_min_width(160.0);
                if ui.button("Rename").clicked() {
                    self.renaming = Some((i, p.name.clone().unwrap_or_default(), false));
                }
                if ui.button("Remove from the list").clicked() {
                    remove = Some(i);
                }
            });
        }
        // A drag shows where the project goes, and moves it on release.
        if let Some(from) = self.welcome_drag {
            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                let to = rows.iter().filter(|r| r.center().y < pos.y).count();
                if to != from && to != from + 1 {
                    let y = match rows.get(to) {
                        Some(r) => r.top() - 3.0,
                        None => rows[rows.len() - 1].bottom() + 3.0,
                    };
                    let (l, r) = (rows[0].left(), rows[0].right());
                    ui.painter().line_segment([Pos2::new(l, y), Pos2::new(r, y)], egui::Stroke::new(2.0, theme.accent));
                }
                if ui.input(|i| i.pointer.any_released()) {
                    projects::move_to(&mut list, from, to);
                    changed = true;
                    self.welcome_drag = None;
                }
            }
            if !ui.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
                self.welcome_drag = None;
            }
        }
        if let Some(i) = remove {
            list.remove(i);
            self.renaming = None;
            changed = true;
        }
        if changed {
            save_projects(&list);
        }
        ui.add_space(12.0);
        ui.label(
            RichText::new("Press 1 to 9 to open a project. Drag a project to change its number.")
                .color(theme.muted)
                .small(),
        );
        open
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        egui::Panel::top("toolbar").frame(egui::Frame::NONE.fill(theme.bg_dark).inner_margin(6.0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.root.is_some() {
                    let key = self.keys.shortcut(Action::GoToFile).map(|s| s.label()).unwrap_or_default();
                    if folder_button(ui, &theme).on_hover_text(format!("Go to file ({key})")).clicked() {
                        self.open_file_picker(false);
                    }
                }
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
                self.project_menu(ui);
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
                    let hint = if theme.dark { "Switch to light mode" } else { "Switch to dark mode" };
                    if mode_button(ui, &theme).on_hover_text(hint).clicked() {
                        self.toggle_mode(ui.ctx());
                    }
                    if ui.button("Commands").on_hover_text(format!("{}+K", keys::CMD)).clicked() {
                        self.picker = Some(Picker::new(PickerKind::Commands, self.command_items()));
                    }
                    if self.root.is_some() {
                        // Right to left: pull, the branch, Blame, Diff.
                        if self.pulling {
                            ui.spinner().on_hover_text("Pulling…");
                        } else if pull_button(ui, &theme)
                            .on_hover_text("Pull the latest commits of this branch (git pull)")
                            .clicked()
                        {
                            self.pull();
                        }
                        if self.switching {
                            ui.spinner().on_hover_text("Switching the branch…");
                        } else if let Some(name) = self.head_name.clone() {
                            let short: String = if name.chars().count() > 32 {
                                name.chars().take(31).chain(std::iter::once('…')).collect()
                            } else {
                                name.clone()
                            };
                            if ui.button(short).on_hover_text(format!("{name}: click to switch the branch")).clicked() {
                                self.open_switch_picker();
                            }
                        }
                        let key = self.keys.shortcut(Action::ToggleBlame).map(|s| s.label()).unwrap_or_default();
                        let hint = if self.show_blame { "Hide git blame" } else { "Show git blame" };
                        let button = if self.show_blame {
                            egui::Button::new(RichText::new("Blame").color(theme.bg_dark).strong()).fill(theme.accent)
                        } else {
                            egui::Button::new("Blame")
                        };
                        if ui.add(button).on_hover_text(format!("{hint} ({key})")).clicked() {
                            self.toggle_blame();
                        }
                        let key = self.keys.shortcut(Action::ToggleDiff).map(|s| s.label()).unwrap_or_default();
                        let hint = if self.show_diff { "Hide the diff" } else { "Show the diff" };
                        let button = if self.show_diff {
                            egui::Button::new(RichText::new("Diff").color(theme.bg_dark).strong()).fill(theme.accent)
                        } else {
                            egui::Button::new("Diff")
                        };
                        if ui.add(button).on_hover_text(format!("{hint} ({key})")).clicked() {
                            self.toggle_diff();
                        }
                    }
                    let busy = self.worker.building.load(std::sync::atomic::Ordering::SeqCst);
                    if busy {
                        ui.spinner();
                    }
                    self.status_numbers(ui);
                    if self.show_diff && self.diff_pill(ui) {
                        self.open_branch_picker();
                    }
                });
            });
        });
    }

    /// The index and memory numbers, each with its meaning on hover. The
    /// layout is right to left, so the last number comes first.
    fn status_numbers(&mut self, ui: &mut egui::Ui) {
        let dim = self.theme.fg_dim;
        let small = |text: String| RichText::new(text).color(dim).small();
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if !self.index_status.is_empty() {
                ui.label(small(self.index_status.clone()));
            } else if let Some((files, project, secs)) = self.indexed {
                ui.label(small(format!("{secs:.1} s"))).on_hover_text(format!(
                    "The last index took {secs:.1} seconds. It is built again when files change."
                ));
                ui.label(small("·".into()));
                ui.label(small(format!("{files} files"))).on_hover_text(format!(
                    "{files} Ruby files are indexed: {project} from this project, {} from gems and the Ruby \
                     standard library.",
                    files - project
                ));
            }
            if let Some(bytes) = self.memory.bytes() {
                let text = memory::label(bytes);
                ui.label(small("·".into()));
                ui.label(small(text.clone())).on_hover_text(format!(
                    "Rubyscope uses {text} of RAM now: the resident memory of this process, read every 2 seconds."
                ));
            }
        });
    }

    /// The diff bar in the top bar: `Diff: main…feature/order-refunds +77
    /// −3, 3 uncommitted`. A click chooses another base branch; the
    /// pointer on it shows what the colors mean.
    fn diff_pill(&self, ui: &mut egui::Ui) -> bool {
        let theme = &self.theme;
        let mut job = egui::text::LayoutJob::default();
        // The size of the other buttons, and their line height, so the top
        // bar does not grow when the pill shows.
        let font = egui::TextStyle::Button.resolve(ui.style());
        let mono = egui::FontId::monospace(font.size);
        let line_height = Some(ui.fonts_mut(|f| f.row_height(&font)));
        let mut add = |text: &str, color: Color32, font: &egui::FontId| {
            job.append(text, 0.0, egui::TextFormat { font_id: font.clone(), color, line_height, ..Default::default() });
        };
        add("Diff: ", theme.fg_dim, &font);
        match &self.repo {
            None => add("reading git…", theme.fg_dim, &font),
            Some(r) if r.error.is_some() => add(r.error.as_deref().unwrap_or_default(), theme.yellow, &font),
            Some(r) => {
                if r.base_missing {
                    let what =
                        if r.base.is_empty() { "no base branch".to_string() } else { format!("{} not found", r.base) };
                    add(&what, theme.yellow, &font);
                } else {
                    add(&r.base, theme.accent, &font);
                    add("…", theme.fg_dim, &font);
                    add(&r.head_name, theme.fg_bright, &font);
                    let (a, d) = r.branch_totals();
                    add(&format!("  +{a}"), theme.green, &mono);
                    add(&format!(" −{d}"), theme.red, &mono);
                }
                let u = r.uncommitted_files.len();
                if u > 0 {
                    add(&format!(", {u} uncommitted"), theme.yellow, &font);
                }
            }
        }
        let button = egui::Button::new(job).fill(theme.bg_light);
        let r = ui.add(button).on_hover_ui(|ui| self.diff_legend(ui));
        // The border is painted on top: a button stroke makes the button,
        // and so the top bar, a pixel taller.
        let radius = ui.style().visuals.widgets.inactive.corner_radius;
        ui.painter().rect_stroke(r.rect, radius, egui::Stroke::new(1.0, theme.muted), egui::StrokeKind::Inside);
        r.clicked()
    }

    /// What the colors of diff mode mean.
    fn diff_legend(&self, ui: &mut egui::Ui) {
        let theme = &self.theme;
        let base = self.repo.as_ref().map(|r| r.base.clone()).unwrap_or_else(|| "the base".into());
        ui.set_max_width(380.0);
        ui.label(RichText::new("What the colors mean").color(theme.fg_bright));
        let rows: [(Color32, &str, String); 4] = [
            (theme.green, "Added or changed on the branch", format!("committed, since the merge-base with {base}")),
            (theme.red, "Removed on the branch", "a ghost row: no line number, no links".into()),
            (theme.yellow, "Changed, not committed", "the file on disk vs HEAD".into()),
            (theme.yellow.gamma_multiply(0.6), "Removed, not committed", "a ghost row with a dashed bar".into()),
        ];
        for (color, title, small) in rows {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(4.0, 30.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 1.0, color);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).color(theme.fg));
                    ui.label(RichText::new(small).color(theme.fg_dim).small());
                });
            });
        }
        ui.label(
            RichText::new(
                "A changed line is a ghost row with the old text, then the new line. The code view is always \
                 the file on disk, so Ctrl+click, find and search work as before. Click to diff against \
                 another branch.",
            )
            .color(theme.fg_dim)
            .small(),
        );
    }

    /// The project name in the toolbar. Its menu goes back to the project
    /// list, or opens another project here or in a new window.
    fn project_menu(&mut self, ui: &mut egui::Ui) {
        enum Choice {
            List,
            Here(PathBuf),
            NewWindow(PathBuf),
            Pick { new_window: bool },
        }
        let theme = self.theme.clone();
        let current = self.root.as_ref().map(|r| r.display().to_string());
        let close_key = self.keys.shortcut(Action::CloseProject).map(|s| s.label()).unwrap_or_default();
        let mut choice = None;
        let label = RichText::new(self.project_name()).color(theme.fg_bright).strong();
        ui.menu_button(label, |ui| {
            ui.set_min_width(360.0);
            if ui.add(egui::Button::new("Back to the project list").shortcut_text(&close_key)).clicked() {
                choice = Some(Choice::List);
            }
            let recent: Vec<String> =
                recent_projects().into_iter().map(|p| p.path).filter(|p| Some(p) != current.as_ref()).collect();
            if !recent.is_empty() {
                ui.separator();
                ui.label(RichText::new("Recent projects").color(theme.fg_dim).small());
                for p in recent {
                    ui.horizontal(|ui| {
                        if ui.button("↗").on_hover_text("Open in a new window").clicked() {
                            choice = Some(Choice::NewWindow(PathBuf::from(&p)));
                        }
                        if ui.button(&p).on_hover_text("Open in this window").clicked() {
                            choice = Some(Choice::Here(PathBuf::from(&p)));
                        }
                    });
                }
            }
            ui.separator();
            if ui.button("Open a folder in this window…").clicked() {
                choice = Some(Choice::Pick { new_window: false });
            }
            if ui.button("Open a folder in a new window…").clicked() {
                choice = Some(Choice::Pick { new_window: true });
            }
        })
        .response
        .on_hover_text("Switch project");
        match choice {
            None => {}
            Some(Choice::List) => self.close_project(),
            Some(Choice::Here(p)) if p.is_dir() => self.switch_project(p),
            Some(Choice::NewWindow(p)) if p.is_dir() => self.open_in_new_window(&p),
            Some(Choice::Here(_) | Choice::NewWindow(_)) => self.toast("That folder does not exist"),
            Some(Choice::Pick { new_window }) => self.pick_folder(new_window),
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(260.0)
            .frame(egui::Frame::NONE.fill(theme.bg_dark).inner_margin(6.0))
            .show(ui, |ui| {
                if !self.show_diff && self.sidebar_tab == SidebarTab::Changes {
                    self.sidebar_tab = SidebarTab::Files;
                }
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Files, "Files");
                    ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Outline, "Outline");
                    if self.show_diff {
                        let n = self.repo.as_ref().map_or(0, |r| r.changed_paths().len());
                        let mut job = egui::text::LayoutJob::default();
                        job.append("Changes ", 0.0, egui::TextFormat { color: theme.fg, ..Default::default() });
                        job.append(
                            &format!(" {n} "),
                            0.0,
                            egui::TextFormat {
                                color: theme.bg_dark,
                                background: theme.accent,
                                font_id: egui::FontId::proportional(11.0),
                                ..Default::default()
                            },
                        );
                        ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Changes, job);
                    }
                });
                ui.separator();
                match self.sidebar_tab {
                    SidebarTab::Files => {
                        let current = self.current().map(|t| t.rel.clone());
                        let project = self.root.clone().unwrap_or_default();
                        let marks = self.show_diff.then(|| self.diff_marks.clone());
                        if let Some(rel) = self.tree.ui(ui, &theme, current.as_deref(), &project, marks.as_deref())
                            && let Some(root) = self.root.clone()
                        {
                            self.open_path(&root.join(rel), None, None, true);
                        }
                    }
                    SidebarTab::Changes => self.changes_list(ui),
                    SidebarTab::Outline => {
                        let (Some(index), Some(file)) = (self.index.clone(), self.current_file_id()) else {
                            ui.label(RichText::new("No outline").color(theme.fg_dim));
                            return;
                        };
                        let mut go = None;
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .wheel_scroll_multiplier(egui::Vec2::splat(tree::SIDEBAR_WHEEL_SPEED))
                            .show(ui, |ui| {
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

    /// The Changes tab of the side bar: the files the branch changed, and
    /// the files with changes that are not committed, with their counts.
    fn changes_list(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        let Some(repo) = self.repo.clone() else {
            ui.label(RichText::new("Reading git…").color(theme.fg_dim));
            return;
        };
        if let Some(e) = &repo.error {
            ui.label(RichText::new(format!("No changes: this project is {e}.")).color(theme.fg_dim));
            return;
        }
        let current = self.current().map(|t| t.rel.clone());
        let mut open = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("changes")
            .wheel_scroll_multiplier(egui::Vec2::splat(tree::SIDEBAR_WHEEL_SPEED))
            .show(ui, |ui| {
                let section = |ui: &mut egui::Ui, left: &str, right: String| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(left.to_uppercase()).color(theme.fg_dim).small());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(right).color(theme.fg_dim).small());
                        });
                    });
                };
                let row = |ui: &mut egui::Ui, f: &crate::diff::ChangedFile, uncommitted: bool| -> bool {
                    let (dir, name) = split_path(&f.rel);
                    let gone = f.status == 'D';
                    ui.horizontal(|ui| {
                        let (mark, color) = match (uncommitted, f.status) {
                            (true, 'D') => ("D", theme.red),
                            (true, _) => ("●", theme.yellow),
                            (false, 'A') => ("A", theme.green),
                            (false, 'D') => ("D", theme.red),
                            _ => ("M", theme.blue),
                        };
                        ui.label(RichText::new(mark).color(color).monospace().small());
                        let selected = current.as_deref() == Some(f.rel.as_str());
                        let text = RichText::new(name).color(if gone { theme.fg_dim } else { theme.fg });
                        let text = if selected { text.background_color(theme.selection) } else { text };
                        let r = ui.add(egui::Label::new(text).sense(egui::Sense::click())).on_hover_text(&f.rel);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(format!("−{}", f.removed)).color(theme.red).monospace().small());
                            ui.label(RichText::new(format!("+{}", f.added)).color(theme.green).monospace().small());
                            ui.add(egui::Label::new(RichText::new(dir).color(theme.fg_dim).small()).truncate());
                        });
                        r.clicked() && !gone
                    })
                    .inner
                };
                section(ui, "On the branch", format!("vs {}", repo.base));
                if repo.branch_files.is_empty() {
                    let why = if repo.base_missing {
                        "No base branch. Choose one with \"Diff against branch…\"."
                    } else {
                        "No commits on the branch since the merge-base."
                    };
                    ui.label(RichText::new(why).color(theme.fg_dim).small());
                }
                for f in &repo.branch_files {
                    if row(ui, f, false) {
                        open = Some(f.rel.clone());
                    }
                }
                ui.add_space(8.0);
                section(ui, "Not committed", "vs HEAD".into());
                if repo.uncommitted_files.is_empty() {
                    ui.label(RichText::new("The working tree is clean.").color(theme.fg_dim).small());
                }
                for f in &repo.uncommitted_files {
                    if row(ui, f, true) {
                        open = Some(f.rel.clone());
                    }
                }
            });
        if let (Some(rel), Some(root)) = (open, self.root.clone()) {
            self.open_path(&root.join(rel), None, None, true);
        }
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
                    // The name and its ✕ sit close together, as one tab.
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        let r = ui.selectable_label(active, text).on_hover_text(&t.rel);
                        if r.clicked() {
                            self.active = i;
                        }
                        if r.middle_clicked() {
                            close = Some((i, TabClose::This));
                        }
                        r.context_menu(|ui| {
                            for (what, label) in [
                                (TabClose::This, "Close"),
                                (TabClose::Left, "Close tabs to the left"),
                                (TabClose::Right, "Close tabs to the right"),
                                (TabClose::All, "Close all tabs"),
                            ] {
                                if ui.button(label).clicked() {
                                    close = Some((i, what));
                                }
                            }
                            ui.separator();
                            tree::copy_path_buttons(ui, &t.rel, &t.path);
                        });
                        if self.show_diff
                            && let Some(mark) = self.diff_marks.get(&t.rel)
                        {
                            ui.add_space(3.0);
                            if let Some(b) = mark.branch {
                                let color = if b == 'A' { theme.green } else { theme.blue };
                                ui.label(RichText::new(b.to_string()).color(color).monospace().small());
                            }
                            if mark.uncommitted {
                                ui.label(RichText::new("●").color(theme.yellow).small());
                            }
                            ui.add_space(3.0);
                        }
                        if active && ui.small_button("✕").clicked() {
                            close = Some((i, TabClose::This));
                        }
                    });
                }
            });
        });
        if let Some((i, what)) = close {
            self.close_tabs(i, what);
        }
    }

    /// Closes tab `i`, or the tabs to its left or right, or all tabs. The
    /// active tab stays active when it is still open; else tab `i` (or the
    /// tab next to it) becomes active.
    pub(crate) fn close_tabs(&mut self, i: usize, what: TabClose) {
        if i >= self.tabs.len() {
            return;
        }
        let keep = |j: usize| match what {
            TabClose::This => j != i,
            TabClose::Left => j >= i,
            TabClose::Right => j <= i,
            TabClose::All => false,
        };
        let active = self.active;
        let new_active =
            if keep(active) { (0..active).filter(|&j| keep(j)).count() } else { (0..i).filter(|&j| keep(j)).count() };
        let mut j = 0;
        self.tabs.retain(|_| {
            j += 1;
            keep(j - 1)
        });
        self.active = new_active.min(self.tabs.len().saturating_sub(1));
        self.save_session();
    }

    /// The bar above the code of a view file: links to the controller
    /// action (or mailer method) of the view. It does not scroll.
    fn view_action_bar(&mut self, ui: &mut egui::Ui) {
        let (Some(index), Some(file)) = (self.index.clone(), self.current_file_id()) else { return };
        let actions = index.view_actions(file);
        if actions.is_empty() {
            return;
        }
        let theme = self.theme.clone();
        // A class opened in many files gives many targets with one label.
        let mut labels: Vec<&str> = Vec::new();
        for t in &actions {
            if !labels.contains(&t.label.as_str()) {
                labels.push(&t.label);
            }
        }
        let class = labels[0].split('#').next().unwrap_or_default();
        let kind = if class.ends_with("Mailer") { "Mailer" } else { "Controller" };
        let mut go = None;
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 2)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(kind).color(theme.fg_dim));
                for label in &labels {
                    let Some(t) = actions.iter().find(|t| t.label == *label) else { continue };
                    let line = index.files[t.loc.file as usize].raw.line_of(t.loc.name_span.start) + 1;
                    let at = format!("{}:{line}", index.display_path(t.loc.file));
                    let r = ui.link(RichText::new(*label).color(theme.accent)).on_hover_text(at);
                    if r.clicked() {
                        go = Some((label.to_string(), r.rect.left_bottom()));
                    }
                }
            });
        });
        if let Some((label, pos)) = go {
            let targets = actions.iter().filter(|t| t.label == label).cloned().collect();
            self.go_to_targets(label, targets, pos);
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
                    Payload::Branch(name) => self.choose_base(name),
                    Payload::Switch(name, remote) => self.switch_to(name, remote),
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
        if let Some((text, at, secs)) = &self.toast {
            let age = self.started.elapsed().as_secs_f64() - at;
            if age > *secs {
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

    /// In diff mode, looks every few seconds if `HEAD` or the base branch
    /// moved (a commit, a checkout, a fetch), and then makes the diffs again.
    fn poll_git(&mut self, ctx: &egui::Context) {
        const EVERY: Duration = Duration::from_secs(4);
        let Some(root) = self.root.clone() else { return };
        // The branch button: the branch can change outside rubyscope.
        if self.head_poll.is_none_or(|t| t.elapsed() >= EVERY) {
            self.head_poll = Some(Instant::now());
            self.worker.head_name(root.clone());
            ctx.request_repaint_after(EVERY);
        }
        if !self.show_diff {
            return;
        }
        if self.diff_poll.is_none_or(|t| t.elapsed() >= EVERY) {
            self.diff_poll = Some(Instant::now());
            let base = self.repo.as_ref().map(|r| r.base.clone()).unwrap_or_default();
            self.worker.fingerprint(root, base);
        }
        ctx.request_repaint_after(EVERY);
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let project = self.root.as_ref().and_then(|r| r.file_name()).map(|n| n.to_string_lossy().to_string());
        let title = match (project, self.current()) {
            (Some(p), Some(t)) => format!("Rubyscope — {p} — {}", t.rel),
            (Some(p), None) => format!("Rubyscope — {p}"),
            _ => "Rubyscope".to_string(),
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
        // While closing, keys do nothing.
        if self.closing.is_none() {
            for action in self.keys.pressed(&ctx) {
                self.run_action(action, &ctx);
            }
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
        self.poll_git(&ctx);
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
            if self.tabs.is_empty() {
                // The tab bar closed the last tab.
                return;
            }
            self.view_action_bar(ui);
            self.find_bar(ui);
            let index = self.index.clone();
            let file_id = self.current_file_id();
            let show_blame = self.show_blame;
            let base = self.show_diff.then(|| self.repo.as_ref().map(|r| r.base.clone()).unwrap_or_default());
            let in_project = self.root.as_ref().is_some_and(|r| self.tabs[self.active].path.starts_with(r));
            let note = match &self.repo {
                None => Some("Reading git…"),
                Some(r) if r.error.is_some() => Some("Not a git repository"),
                Some(_) if !in_project => Some("Not a file of this project"),
                Some(_) => None,
            };
            let t = &mut self.tabs[self.active];
            let links = match (&index, file_id) {
                (Some(ix), Some(f)) => Some(Links { index: ix, file: f }),
                _ => None,
            };
            events = view::code_view(
                ui,
                t,
                &theme,
                links.as_ref().map(|l| l as &dyn Clickable),
                show_blame,
                now_secs(),
                base.as_deref().map(|base| view::DiffMode { base, note }),
            );
        });

        // What happened in the code view.
        if let Some((offset, pos)) = events.goto {
            self.hover = None;
            if let Some(t) = self.tabs.get_mut(self.active) {
                t.cursor = Some(offset);
            }
            self.click_at_cursor(pos);
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
        if events.ghost_click {
            self.toast("A removed line has no links: it is not in the file on disk.");
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
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) {
            self.close_requested(&ctx);
        }
        self.frame_ui(ui);
        self.run_closing(&ctx);
        if let Some(closing) = &self.closing {
            closing.ui(&ctx, &self.theme);
        }
    }
}
