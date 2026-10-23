//! The code view: an open file, drawn row by row.
//!
//! A row is a line of the file, or in diff mode a removed line (a "ghost
//! row") from the [`RowMap`]. The scroll area counts rows; everything else
//! (the cursor, the selection, find, go to line) counts file lines and
//! byte offsets, and turns them into rows only to draw and to scroll.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, TextFormat, Vec2, text::LayoutJob,
};
use rubyscope_index::model::Span;

use crate::diff::{DisplayRow, FileDiff, LineMark, RowMap, RunKind};
use crate::git::Blame;
use crate::highlight::LineTokens;
use crate::theme::{Theme, Token};

pub const FONT_SIZE: f32 = 14.0;
const TAB_WIDTH: usize = 2;
/// The blame column before the blame is loaded.
const BLAME_WIDTH: f32 = 200.0;
/// The blame text size, as a part of the code size.
const BLAME_FONT: f32 = 0.92;
/// The most characters of an author name in the blame column.
const BLAME_NAME_CHARS: usize = 16;
/// The mouse wheel scrolls this many times faster than egui's default.
const WHEEL_SPEED: f32 = 8.0;
/// egui's points per wheel line (`InputOptions::line_scroll_speed`). A
/// touchpad sends points, so this many points count as one wheel step.
const POINTS_PER_STEP: f32 = 40.0;
/// Lines an arrow key (or j / k) scrolls.
const KEY_LINES: f32 = 3.0;
/// Columns a left or right arrow scrolls.
const KEY_COLUMNS: f32 = 8.0;
/// The time between the two presses of `gg`, in seconds.
const GG_DELAY: f64 = 1.0;
/// The width of the change marks on the right edge in diff mode.
pub const CHANGE_MAP_WIDTH: f32 = 14.0;
/// How long the pointer rests on a removed line before its tooltip shows.
const GHOST_TIP_DELAY: f64 = 0.45;

#[derive(Debug, Default)]
pub struct FindState {
    pub open: bool,
    pub query: String,
    pub matches: Vec<(u32, u32)>,
    pub current: usize,
    pub focus: bool,
}

pub struct OpenFile {
    pub path: PathBuf,
    /// The path relative to the project, or the absolute path.
    pub rel: String,
    pub text: Arc<str>,
    /// Byte start and end (without the newline) of each line.
    pub lines: Vec<(u32, u32)>,
    pub max_line_chars: usize,
    pub highlights: Option<Arc<Vec<LineTokens>>>,
    pub blame: Option<Arc<Blame>>,
    pub blame_requested: bool,
    /// Diff mode: the diffs of this file, once they are made.
    pub diff: Option<Arc<FileDiff>>,
    /// A diff is being made for this file.
    pub diff_requested: bool,
    /// Diff mode: the rows with the removed lines. None shows one row per line.
    pub rows: Option<Arc<RowMap>>,
    /// The change that `]` and `[` went to (an index into `RowMap::changes`).
    pub change: Option<usize>,
    /// Go to the first (1) or last (-1) change when the diff is ready.
    pub pending_change: Option<i8>,
    /// Set to scroll so this 0-based line is near the top.
    pub scroll_to_line: Option<u32>,
    /// Set to scroll to a row, with this part of the view above it.
    pub scroll_to_row: Option<(u32, f32)>,
    /// A line to flash after a jump, with the time it started.
    pub flash: Option<(u32, f64)>,
    /// The first visible line, and the first visible row.
    pub top_line: u32,
    pub top_row: u32,
    /// The last place clicked: actions like "find usages" use it.
    pub cursor: Option<u32>,
    pub selection: Option<(u32, u32)>,
    drag_anchor: Option<u32>,
    pub find: FindState,
    /// Where the pointer rests, and since when (for the hover popup).
    pub hover: Option<(u32, f64)>,
    /// The ghost row under the pointer, and since when (for its tooltip).
    ghost_hover: Option<(u32, f64)>,
    /// The last layout: where the first visible row's text starts, the row
    /// height, the char width, and the first visible row. Tests use it to
    /// click on a line and column.
    pub layout: Option<(Pos2, f32, f32, usize)>,
    /// The scroll offset, the largest offset and the visible size, from the
    /// last frame. The scroll keys use them.
    pub scroll: Vec2,
    scroll_max: Vec2,
    pub view_size: Vec2,
    /// When the first `g` of `gg` was pressed.
    pending_g: Option<f64>,
    /// Digits typed before `g`: `500g` goes to line 500.
    count: Option<u32>,
    /// Wheel steps with Shift held that did not make a full page yet.
    shift_wheel: f32,
}

impl OpenFile {
    pub fn new(path: PathBuf, rel: String, text: String) -> OpenFile {
        let mut lines = Vec::new();
        let mut start = 0u32;
        let mut max_line_chars = 0;
        for l in text.split_inclusive('\n') {
            let content = l.trim_end_matches(['\n', '\r']);
            lines.push((start, start + content.len() as u32));
            max_line_chars = max_line_chars.max(content.chars().count() + content.matches('\t').count() * TAB_WIDTH);
            start += l.len() as u32;
        }
        if lines.is_empty() || text.ends_with('\n') {
            lines.push((start, start));
        }
        OpenFile {
            path,
            rel,
            text: text.into(),
            lines,
            max_line_chars,
            highlights: None,
            blame: None,
            blame_requested: false,
            diff: None,
            diff_requested: false,
            rows: None,
            change: None,
            pending_change: None,
            scroll_to_line: None,
            scroll_to_row: None,
            flash: None,
            top_line: 0,
            top_row: 0,
            cursor: None,
            selection: None,
            drag_anchor: None,
            find: FindState::default(),
            hover: None,
            ghost_hover: None,
            layout: None,
            scroll: Vec2::ZERO,
            scroll_max: Vec2::ZERO,
            view_size: Vec2::ZERO,
            pending_g: None,
            count: None,
            shift_wheel: 0.0,
        }
    }

    /// The 0-based line of a byte offset.
    pub fn line_of(&self, offset: u32) -> u32 {
        match self.lines.binary_search_by(|(s, _)| s.cmp(&offset)) {
            Ok(i) => i as u32,
            Err(i) => i.saturating_sub(1) as u32,
        }
    }

    /// The byte offset of the start of a 0-based line.
    pub fn line_start(&self, line: u32) -> u32 {
        self.lines.get(line as usize).map(|l| l.0).unwrap_or(0)
    }

    /// The number of rows in the view: the lines, and the ghost rows in
    /// diff mode.
    pub fn row_count(&self) -> usize {
        self.rows.as_ref().map_or(self.lines.len(), |m| m.rows.len())
    }

    pub fn row(&self, row: usize) -> DisplayRow {
        match &self.rows {
            Some(m) => m.rows.get(row).copied().unwrap_or(DisplayRow::Line(row as u32)),
            None => DisplayRow::Line(row as u32),
        }
    }

    /// The row that shows a 0-based line.
    pub fn row_of_line(&self, line: u32) -> u32 {
        let line = line.min(self.lines.len().saturating_sub(1) as u32);
        match &self.rows {
            Some(m) => m.line_row.get(line as usize).copied().unwrap_or(line),
            None => line,
        }
    }

    /// The line a row shows. A ghost row gives the line after it.
    pub fn line_of_row(&self, row: u32) -> u32 {
        match &self.rows {
            Some(m) => m.line_of_row(row),
            None => row.min(self.lines.len().saturating_sub(1) as u32),
        }
    }

    /// The number of lines for git: without the empty part after the last
    /// newline.
    pub fn git_lines(&self) -> u32 {
        crate::diff::git_line_count(&self.text)
    }

    /// Shows the rows of the diff (in diff mode) or one row per line. The
    /// line at the top of the view stays at the top.
    pub fn set_rows(&mut self, on: bool) {
        let rows = match (&self.diff, on) {
            (Some(d), true) => Some(Arc::new(RowMap::build(d, self.git_lines(), self.lines.len() as u32))),
            _ => None,
        };
        let top = self.top_line;
        let was_drawn = self.layout.is_some();
        self.rows = rows;
        self.change = self.change.filter(|c| self.rows.as_ref().is_some_and(|m| *c < m.changes.len()));
        if was_drawn && self.scroll_to_line.is_none() && self.scroll_to_row.is_none() && self.scroll.y > 0.0 {
            self.scroll_to_row = Some((self.row_of_line(top), 0.0));
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let (a, b) = (a.min(b) as usize, a.max(b) as usize);
        (a < b).then(|| self.text[a..b.min(self.text.len())].to_string())
    }

    /// Finds the query in the file, case-insensitive.
    pub fn update_find(&mut self) {
        self.find.matches.clear();
        let q = self.find.query.to_lowercase();
        if q.is_empty() {
            return;
        }
        let lower = self.text.to_lowercase();
        // `to_lowercase` can change byte lengths; only use it when it did not.
        if lower.len() != self.text.len() {
            let mut from = 0;
            while let Some(p) = self.text[from..].find(&self.find.query) {
                let s = (from + p) as u32;
                self.find.matches.push((s, s + self.find.query.len() as u32));
                from += p + self.find.query.len().max(1);
            }
            return;
        }
        let mut from = 0;
        while let Some(p) = lower[from..].find(&q) {
            let s = (from + p) as u32;
            self.find.matches.push((s, s + q.len() as u32));
            from += p + q.len().max(1);
        }
        self.find.current = self.find.matches.iter().position(|m| m.0 >= self.line_start(self.top_line)).unwrap_or(0);
    }
}

/// What happened in the view this frame.
#[derive(Debug, Default)]
pub struct ViewEvents {
    /// Ctrl+click (Cmd+click on macOS): go to the definition at this offset.
    pub goto: Option<(u32, Pos2)>,
    /// Right click at this offset.
    pub context: Option<u32>,
    /// The pointer rested on this offset long enough for a hover popup.
    pub hover: Option<(u32, Pos2)>,
    /// A click on a blame block: the commit.
    pub blame_click: Option<String>,
    /// Ctrl+click on a removed line, which has no links.
    pub ghost_click: bool,
}

/// The code view needs to ask the index what is clickable.
pub trait Clickable {
    fn clickable_at(&self, offset: u32) -> Option<Span>;
}

/// One row as it is drawn: tabs become spaces, and each display char
/// knows its byte offset in the file.
struct Row {
    job: LayoutJob,
    /// File offset of each display char, and one more for the line end.
    byte_of_char: Vec<u32>,
}

/// Lays out a line of text that starts at byte `start` of the file, with
/// its syntax colors.
fn layout_row(text: &str, start: u32, tokens: Option<&LineTokens>, fmt: &dyn Fn(Token) -> TextFormat) -> Row {
    let mut job = LayoutJob::default();
    let mut byte_of_char = Vec::with_capacity(text.len() + 1);
    let push = |from: usize, to: usize, token: Token, job: &mut LayoutJob, map: &mut Vec<u32>| {
        let piece = &text[from..to];
        let mut display = String::with_capacity(piece.len());
        for (i, c) in piece.char_indices() {
            let abs = start + (from + i) as u32;
            if c == '\t' {
                for _ in 0..TAB_WIDTH {
                    display.push(' ');
                    map.push(abs);
                }
            } else {
                display.push(c);
                map.push(abs);
            }
        }
        job.append(&display, 0.0, fmt(token));
    };
    let mut pos = 0usize;
    if let Some(tokens) = tokens {
        for (s, e, t) in tokens {
            let (s, e) = ((*s as usize).min(text.len()), (*e as usize).min(text.len()));
            if s > pos && text.is_char_boundary(pos) && text.is_char_boundary(s) {
                push(pos, s, Token::Plain, &mut job, &mut byte_of_char);
            }
            if e > s && s >= pos && text.is_char_boundary(s) && text.is_char_boundary(e) {
                push(s, e, *t, &mut job, &mut byte_of_char);
                pos = e;
            }
        }
    }
    if pos < text.len() && text.is_char_boundary(pos) {
        push(pos, text.len(), Token::Plain, &mut job, &mut byte_of_char);
    }
    byte_of_char.push(start + text.len() as u32);
    Row { job, byte_of_char }
}

fn build_row(file: &OpenFile, line: usize, theme: &Theme, font: &FontId) -> Row {
    let (start, end) = file.lines[line];
    let text = &file.text[start as usize..end as usize];
    let fmt = |token: Token| TextFormat { font_id: font.clone(), color: theme.token(token), ..Default::default() };
    layout_row(text, start, file.highlights.as_ref().and_then(|h| h.get(line)), &fmt)
}

/// A removed line: its colors at half strength, struck through.
fn build_ghost_row(text: &str, tokens: &LineTokens, strike: Color32, theme: &Theme, font: &FontId) -> Row {
    let fmt = |token: Token| TextFormat {
        font_id: font.clone(),
        color: theme.token(token).gamma_multiply(0.5),
        strikethrough: Stroke::new(1.0, strike),
        ..Default::default()
    };
    layout_row(text, 0, Some(tokens), &fmt)
}

impl Row {
    /// The display char index of a file offset.
    fn char_of(&self, offset: u32) -> usize {
        self.byte_of_char.partition_point(|b| *b < offset)
    }
}

fn row_height(ui: &egui::Ui) -> f32 {
    let font = FontId::monospace(FONT_SIZE);
    (ui.fonts_mut(|f| f.row_height(&font)) * 1.4).round()
}

fn char_width(ui: &egui::Ui) -> f32 {
    let font = FontId::monospace(FONT_SIZE);
    ui.fonts_mut(|f| f.glyph_width(&font, 'M'))
}

/// The diff colors, from the theme: green on the branch, red for removed
/// lines, amber for what is not committed.
struct DiffColors {
    added: Color32,
    removed: Color32,
    amber: Color32,
}

impl DiffColors {
    fn new(theme: &Theme) -> DiffColors {
        DiffColors { added: theme.green, removed: theme.red, amber: theme.yellow }
    }

    /// The row background and the gutter bar of a row.
    fn of_row(&self, map: &RowMap, row: DisplayRow) -> Option<(Color32, Color32, bool)> {
        match row {
            DisplayRow::Line(l) => match map.marks.get(l as usize).copied().unwrap_or_default() {
                LineMark::None => None,
                LineMark::Branch => Some((self.added.gamma_multiply(0.09), self.added, false)),
                LineMark::Uncommitted => Some((self.amber.gamma_multiply(0.12), self.amber, false)),
            },
            DisplayRow::Ghost(g) if map.ghosts[g as usize].uncommitted => {
                Some((self.amber.gamma_multiply(0.07), self.amber, true))
            }
            DisplayRow::Ghost(_) => Some((self.removed.gamma_multiply(0.095), self.removed, false)),
        }
    }

    fn of_run(&self, kind: RunKind) -> Color32 {
        match kind {
            RunKind::Added => self.added,
            RunKind::Removed => self.removed,
            RunKind::Uncommitted => self.amber,
        }
    }
}

/// A vertical bar of dashes: the gutter bar of a line removed in the
/// working tree.
fn dashed_bar(painter: &egui::Painter, rect: Rect, color: Color32) {
    let mut y = rect.top();
    while y < rect.bottom() {
        let r = Rect::from_min_max(Pos2::new(rect.left(), y), Pos2::new(rect.right(), (y + 4.0).min(rect.bottom())));
        painter.rect_filled(r, 0.0, color);
        y += 7.0;
    }
}

/// Thin diagonal stripes: the blame cell of a ghost row, which has no
/// commit.
fn hatch(painter: &egui::Painter, rect: Rect, color: Color32) {
    let p = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let mut x = rect.left() - rect.height();
    while x < rect.right() {
        p.line_segment(
            [Pos2::new(x, rect.bottom()), Pos2::new(x + rect.height(), rect.top())],
            Stroke::new(3.0, color),
        );
        x += 8.0;
    }
}

/// A part of the chip in the corner of the code view.
enum Chip {
    Text(String, Color32),
    Key(&'static str),
    Sep,
}

/// Draws a chip with its bottom right corner at `corner`.
fn draw_chip(painter: &egui::Painter, theme: &Theme, corner: Pos2, parts: &[Chip]) {
    let font = FontId::proportional(13.0);
    let key_font = FontId::monospace(12.0);
    let gap = 7.0;
    let items: Vec<(Option<Arc<egui::Galley>>, f32)> = parts
        .iter()
        .map(|p| match p {
            Chip::Text(t, c) => {
                let g = painter.layout_no_wrap(t.clone(), font.clone(), *c);
                let w = g.size().x;
                (Some(g), w)
            }
            Chip::Key(t) => {
                let g = painter.layout_no_wrap(t.to_string(), key_font.clone(), theme.fg_bright);
                let w = g.size().x + 10.0;
                (Some(g), w)
            }
            Chip::Sep => (None, 1.0),
        })
        .collect();
    let width: f32 = items.iter().map(|(_, w)| w).sum::<f32>() + gap * (items.len().saturating_sub(1)) as f32 + 22.0;
    let rect = Rect::from_min_max(corner - Vec2::new(width, 28.0), corner);
    painter.rect_filled(rect.translate(Vec2::new(0.0, 3.0)).expand(2.0), 8.0, Color32::from_black_alpha(60));
    painter.rect(rect, 6.0, theme.bg_light, Stroke::new(1.0, theme.muted), StrokeKind::Inside);
    let mut x = rect.left() + 11.0;
    let cy = rect.center().y;
    for (part, (galley, w)) in parts.iter().zip(items) {
        match (part, galley) {
            (Chip::Text(_, c), Some(g)) => {
                painter.galley(Pos2::new(x, cy - g.size().y / 2.0), g, *c);
            }
            (Chip::Key(_), Some(g)) => {
                let r = Rect::from_min_size(Pos2::new(x, cy - 9.0), Vec2::new(w, 18.0));
                painter.rect(r, 4.0, theme.bg_light, Stroke::new(1.0, theme.muted), StrokeKind::Inside);
                painter.galley(Pos2::new(x + 5.0, cy - g.size().y / 2.0), g, theme.fg_bright);
            }
            _ => {
                painter.line_segment([Pos2::new(x, cy - 7.0), Pos2::new(x, cy + 7.0)], Stroke::new(1.0, theme.muted));
            }
        }
        x += w + gap;
    }
}

/// Diff mode, for the code view.
pub struct DiffMode<'a> {
    /// The base branch.
    pub base: &'a str,
    /// Why this file has no diff (not a git repository, a gem file), for
    /// the chip in the corner. None while the diff is being made.
    pub note: Option<&'a str>,
}

/// Draws the file and handles the pointer. `show_blame` shows the blame
/// column. In diff mode the view also shows the change marks on the right
/// edge and the chip with the number of changes.
pub fn code_view(
    ui: &mut egui::Ui,
    file: &mut OpenFile,
    theme: &Theme,
    links: Option<&dyn Clickable>,
    show_blame: bool,
    now_secs: i64,
    diff: Option<DiffMode>,
) -> ViewEvents {
    let diff_base = diff.as_ref().map(|d| d.base);
    let mut events = ViewEvents::default();
    let font = FontId::monospace(FONT_SIZE);
    let row_h = row_height(ui);
    let char_w = char_width(ui);
    let digits = (file.lines.len().max(1) as f32).log10().floor() as usize + 1;
    let blame_w = match (show_blame, &file.blame) {
        (false, _) => 0.0,
        (true, None) => BLAME_WIDTH,
        (true, Some(blame)) => {
            let name = blame.commits.iter().map(|c| c.last_name().chars().count()).max().unwrap_or(0);
            // `16/07/2025 Novak`, and space on both sides.
            (11 + name.min(BLAME_NAME_CHARS)) as f32 * char_w * BLAME_FONT + 22.0
        }
    };
    let gutter_w = digits as f32 * char_w + 24.0;
    let code_x = blame_w + gutter_w;
    let map = file.rows.clone();
    let max_chars = file.max_line_chars.max(map.as_ref().map_or(0, |m| m.max_ghost_chars));
    let width = code_x + (max_chars as f32 + 4.0) * char_w;
    let time = ui.input(|i| i.time);
    let ctrl = ui.input(|i| crate::keys::cmd_held(i.modifiers));
    let colors = DiffColors::new(theme);

    let mut area = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .id_salt(("code", &file.rel))
        .wheel_scroll_multiplier(Vec2::splat(WHEEL_SPEED));
    let mut to = scroll_keys(ui, file, row_h, char_w);
    if let Some(y) = shift_wheel(ui, file, row_h, to.unwrap_or(file.scroll).y) {
        to = Some(Vec2::new(to.unwrap_or(file.scroll).x, y));
    }
    if let Some(to) = to {
        area = area.scroll_offset(to);
    }
    if let Some(line) = file.scroll_to_line.take() {
        area = area.vertical_scroll_offset(file.row_of_line(line).saturating_sub(6) as f32 * row_h);
    }
    if let Some((row, above)) = file.scroll_to_row.take() {
        area = area.vertical_scroll_offset((row as f32 * row_h - file.view_size.y * above).max(0.0));
    }
    let total = file.row_count();
    let full = ui.available_rect_before_wrap();
    let map_w = if diff_base.is_some() { CHANGE_MAP_WIDTH } else { 0.0 };
    let code_rect = Rect::from_min_max(full.min, Pos2::new(full.right() - map_w, full.bottom()));
    ui.painter().rect_filled(full, 0.0, theme.bg);
    let mut code_ui = ui.new_child(egui::UiBuilder::new().max_rect(code_rect));
    // `show_rows` counts each row as its height plus the item spacing. The
    // rows are drawn with no space between them, so the spacing must be 0
    // for a scroll offset of `row * row_h` to show that row at the top.
    code_ui.spacing_mut().item_spacing.y = 0.0;
    let output = area.show_rows(&mut code_ui, row_h, total, |ui, range| {
        file.top_row = range.start as u32;
        file.top_line = file.line_of_row(range.start as u32);
        let kinds: Vec<DisplayRow> = range.clone().map(|r| file.row(r)).collect();
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(width.max(ui.available_width()), row_h * range.len() as f32),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(ui.clip_rect());
        let rows: Vec<Row> = kinds
            .iter()
            .map(|k| match (*k, &map) {
                (DisplayRow::Ghost(g), Some(m)) => {
                    let ghost = &m.ghosts[g as usize];
                    let strike = if ghost.uncommitted { colors.amber } else { colors.removed };
                    build_ghost_row(&ghost.text, &ghost.tokens, strike.gamma_multiply(0.6), theme, &font)
                }
                (DisplayRow::Line(l), _) => build_row(file, l as usize, theme, &font),
                (DisplayRow::Ghost(_), None) => layout_row("", 0, None, &|_| TextFormat::default()),
            })
            .collect();
        let galleys: Vec<_> = rows.iter().map(|r| ui.fonts_mut(|f| f.layout_job(r.job.clone()))).collect();
        let row_top = |i: usize| rect.top() + i as f32 * row_h;
        let text_pos = |i: usize| Pos2::new(rect.left() + code_x, row_top(i) + (row_h - FONT_SIZE * 1.2) / 2.0);
        file.layout = Some((text_pos(0), row_h, char_w, range.start));
        let line_at_row = |i: usize| match kinds.get(i) {
            Some(DisplayRow::Line(l)) => Some(*l as usize),
            _ => None,
        };
        let row_at = |pos: Pos2| -> Option<usize> {
            let i = ((pos.y - rect.top()) / row_h).floor();
            (i >= 0.0 && (i as usize) < kinds.len()).then_some(i as usize)
        };
        // The line under the pointer (in any column): for the blame column.
        let line_at = |pos: Pos2| row_at(pos).and_then(line_at_row);
        // A ghost row under the pointer, in the code part of the row.
        let ghost_at = |pos: Pos2| {
            row_at(pos).filter(|i| pos.x >= rect.left() + blame_w && matches!(kinds[*i], DisplayRow::Ghost(_)))
        };

        // The pointer as a file offset. Ghost rows have none.
        let offset_at = |pos: Pos2| -> Option<u32> {
            if pos.x < rect.left() + code_x {
                return None;
            }
            let i = row_at(pos)?;
            line_at_row(i)?;
            let local = pos - text_pos(i);
            let c = galleys[i].cursor_from_pos(Vec2::new(local.x, galleys[i].size().y / 2.0)).index.0;
            rows[i].byte_of_char.get(c).copied()
        };
        // While selecting, a ghost row counts as the start of the next line.
        let next_line_start: Vec<u32> =
            (0..kinds.len()).map(|i| file.line_start(file.line_of_row((range.start + i) as u32))).collect();
        let drag_offset = |pos: Pos2| -> Option<u32> {
            offset_at(pos).or_else(|| {
                let i = row_at(pos)?;
                (pos.x >= rect.left() + code_x).then(|| next_line_start[i])
            })
        };

        // Backgrounds: the diff colors, the flash line, find matches, the
        // selection.
        for (i, kind) in kinds.iter().enumerate() {
            let y = row_top(i);
            if let Some(m) = &map
                && let Some((bg, bar, dashed)) = colors.of_row(m, *kind)
            {
                let left = rect.left() + blame_w;
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(left, y),
                        Pos2::new(rect.right().max(ui.clip_rect().right()), y + row_h),
                    ),
                    0.0,
                    bg,
                );
                let bx = left + gutter_w - 6.0;
                let bar_rect = Rect::from_min_max(Pos2::new(bx, y), Pos2::new(bx + 3.0, y + row_h));
                if dashed {
                    dashed_bar(&painter, bar_rect, bar);
                } else {
                    painter.rect_filled(bar_rect, 1.0, bar);
                }
            }
            let Some(line) = line_at_row(i) else { continue };
            if let Some((fl, started)) = file.flash
                && fl as usize == line
            {
                let a = (1.0 - ((time - started) / 1.2) as f32).clamp(0.0, 1.0);
                if a > 0.0 {
                    let c = theme.accent.gamma_multiply(0.35 * a);
                    painter.rect_filled(
                        Rect::from_min_size(Pos2::new(rect.left() + code_x, y), Vec2::new(width, row_h)),
                        0.0,
                        c,
                    );
                    ui.ctx().request_repaint();
                }
            }
            let (ls, le) = file.lines[line];
            let mark = |a: u32, b: u32, color: Color32| {
                let (a, b) = (a.max(ls), b.min(le));
                if a >= b {
                    return;
                }
                let ca = rows[i].char_of(a);
                let cb = rows[i].char_of(b);
                let xa = galleys[i].pos_from_cursor(egui::text::CCursor::new(ca)).left();
                let xb = galleys[i].pos_from_cursor(egui::text::CCursor::new(cb)).left().max(xa + 2.0);
                let p = text_pos(i);
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(p.x + xa, y), Pos2::new(p.x + xb, y + row_h)),
                    2.0,
                    color,
                );
            };
            for (n, (a, b)) in file.find.matches.iter().enumerate() {
                if *b < ls || *a > le {
                    continue;
                }
                let color = if n == file.find.current {
                    theme.yellow.gamma_multiply(0.55)
                } else {
                    theme.yellow.gamma_multiply(0.25)
                };
                mark(*a, *b, color);
            }
            if let Some((a, b)) = file.selection {
                let (a, b) = (a.min(b), a.max(b));
                if a < b && a <= le && b >= ls {
                    mark(a, b, theme.selection.gamma_multiply(1.6));
                }
            }
        }

        // Blame column, as in RubyMine: the date and the author's last name
        // on each line. The lines of the commit under the pointer, or else
        // of the cursor line, have brighter text. Lines that are not
        // committed say so in amber; ghost rows have no commit.
        if let (true, Some(blame)) = (show_blame, file.blame.clone()) {
            let active = response
                .hover_pos()
                .filter(|p| p.x < rect.left() + blame_w)
                .and_then(line_at)
                .or(file.cursor.map(|c| file.line_of(c) as usize))
                .and_then(|l| blame.lines.get(l).copied());
            let blame_font = FontId::monospace(FONT_SIZE * BLAME_FONT);
            for (i, kind) in kinds.iter().enumerate() {
                let r = Rect::from_min_size(Pos2::new(rect.left(), row_top(i)), Vec2::new(blame_w - 6.0, row_h));
                let line = match kind {
                    DisplayRow::Ghost(_) => {
                        hatch(&painter, r, theme.muted.gamma_multiply(0.3));
                        continue;
                    }
                    DisplayRow::Line(l) => *l as usize,
                };
                let Some(c) = blame.lines.get(line).copied() else { continue };
                let commit = &blame.commits[c as usize];
                let (bg, text, color) = if commit.is_uncommitted() {
                    (theme.yellow.gamma_multiply(0.16), "not committed".to_string(), theme.yellow)
                } else {
                    let name: String = commit.last_name().chars().take(BLAME_NAME_CHARS).collect();
                    let color = if active == Some(c) { theme.fg_bright } else { theme.fg.gamma_multiply(0.7) };
                    (theme.blame_color(commit.rank), format!("{:<10} {name}", commit.short_date(now_secs)), color)
                };
                painter.rect_filled(r, 0.0, bg);
                painter.text(
                    Pos2::new(r.left() + 8.0, r.center().y),
                    Align2::LEFT_CENTER,
                    text,
                    blame_font.clone(),
                    color,
                );
            }
        } else if show_blame {
            painter.text(
                Pos2::new(rect.left() + 8.0, row_top(0) + row_h / 2.0),
                Align2::LEFT_CENTER,
                "loading blame…",
                FontId::proportional(12.0),
                theme.fg_dim,
            );
        }

        // Line numbers. A ghost row has a dim minus.
        let cursor_line = file.cursor.map(|c| file.line_of(c) as usize);
        for (i, kind) in kinds.iter().enumerate() {
            let (text, color) = match (kind, &map) {
                (DisplayRow::Line(l), _) => {
                    let line = *l as usize;
                    ((line + 1).to_string(), if Some(line) == cursor_line { theme.fg } else { theme.muted })
                }
                (DisplayRow::Ghost(g), Some(m)) => {
                    let c = if m.ghosts[*g as usize].uncommitted { colors.amber } else { colors.removed };
                    ("−".to_string(), c.gamma_multiply(0.8))
                }
                (DisplayRow::Ghost(_), None) => continue,
            };
            painter.text(
                Pos2::new(rect.left() + blame_w + gutter_w - 12.0, row_top(i) + row_h / 2.0),
                Align2::RIGHT_CENTER,
                text,
                font.clone(),
                color,
            );
        }

        // Text.
        for (i, g) in galleys.iter().enumerate() {
            painter.galley(text_pos(i), g.clone(), theme.fg);
        }

        // The change that `]` and `[` went to has a frame.
        if let (Some(m), Some(c)) = (&map, file.change)
            && let Some(&(a, b)) = m.changes.get(c)
        {
            let (a, b) = (a as usize, b as usize);
            if a < range.end && b >= range.start {
                let top = row_top(a.max(range.start) - range.start);
                let bottom = row_top(b.min(range.end - 1) - range.start) + row_h;
                let clip = ui.clip_rect();
                let frame = Rect::from_min_max(
                    Pos2::new(rect.left() + blame_w + 1.0, top),
                    Pos2::new(clip.right() - 2.0, bottom),
                );
                painter.rect_stroke(frame, 3.0, Stroke::new(1.0, theme.accent), StrokeKind::Inside);
                if a >= range.start {
                    let label = format!("change {} of {}", c + 1, m.changes.len());
                    let g = painter.layout_no_wrap(label, FontId::proportional(11.5), theme.bg_dark);
                    let lr = Rect::from_min_size(
                        Pos2::new(frame.right() - g.size().x - 18.0, (top - 9.0).max(clip.top())),
                        g.size() + Vec2::new(12.0, 2.0),
                    );
                    painter.rect_filled(lr, 3.0, theme.accent);
                    painter.galley(lr.min + Vec2::new(6.0, 1.0), g, theme.bg_dark);
                }
            }
        }

        // Pointer.
        let pointer = response.hover_pos();
        let hovered = pointer.and_then(offset_at);
        if let (Some(off), Some(links)) = (hovered, links) {
            if let Some(span) = links.clickable_at(off) {
                if ctrl {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    let row = file.row_of_line(file.line_of(span.start)) as usize;
                    if range.contains(&row) {
                        let i = row - range.start;
                        let ca = rows[i].char_of(span.start);
                        let cb = rows[i].char_of(span.end);
                        let xa = galleys[i].pos_from_cursor(egui::text::CCursor::new(ca)).left();
                        let xb = galleys[i].pos_from_cursor(egui::text::CCursor::new(cb)).left();
                        let p = text_pos(i);
                        let y = p.y + FONT_SIZE * 1.15;
                        painter.line_segment(
                            [Pos2::new(p.x + xa, y), Pos2::new(p.x + xb, y)],
                            Stroke::new(1.5, theme.accent),
                        );
                    }
                    file.hover = None;
                } else {
                    match file.hover {
                        Some((s, since)) if s == span.start => {
                            if time - since > 0.45 {
                                events.hover = Some((off, pointer.unwrap_or_default()));
                            } else {
                                ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
                            }
                        }
                        _ => {
                            file.hover = Some((span.start, time));
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                        }
                    }
                }
            } else {
                file.hover = None;
            }
        } else {
            file.hover = None;
        }

        // A removed line: a tooltip says why it has no links.
        match (pointer.and_then(ghost_at), &map) {
            (Some(i), Some(m)) => {
                let row = (range.start + i) as u32;
                let since = match file.ghost_hover {
                    Some((r, since)) if r == row => since,
                    _ => {
                        file.ghost_hover = Some((row, time));
                        time
                    }
                };
                if ctrl {
                    ui.ctx().set_cursor_icon(CursorIcon::NotAllowed);
                }
                if ctrl || time - since > GHOST_TIP_DELAY {
                    let uncommitted = matches!(kinds[i], DisplayRow::Ghost(g) if m.ghosts[g as usize].uncommitted);
                    let (color, body) = if uncommitted {
                        (
                            colors.amber,
                            "Removed in the working tree (not committed). It is not in the file on disk, \
                             so Ctrl+click has nothing to go to here."
                                .to_string(),
                        )
                    } else {
                        (
                            colors.removed,
                            format!(
                                "Removed on this branch since the merge-base with {}. It is not in the file \
                                 on disk, so Ctrl+click has nothing to go to here.",
                                diff_base.unwrap_or("the base branch")
                            ),
                        )
                    };
                    let pos = pointer.unwrap_or_default();
                    egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::unique("ghost-tip"), pos)
                        .show(|ui| {
                            ui.set_max_width(360.0);
                            ui.label(egui::RichText::new("Removed line, not in the index").color(color));
                            ui.label(egui::RichText::new(body).color(theme.fg_dim).size(12.5));
                        });
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
                }
            }
            _ => file.ghost_hover = None,
        }

        // The drag starts only after the pointer moves a few pixels, so the
        // selection starts where the button went down, not where it is now.
        if response.drag_started_by(egui::PointerButton::Primary)
            && !ctrl
            && let Some(off) = ui.input(|i| i.pointer.press_origin()).and_then(drag_offset)
        {
            file.drag_anchor = Some(off);
            file.selection = Some((off, off));
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let (Some(anchor), Some(off)) = (file.drag_anchor, response.interact_pointer_pos().and_then(drag_offset))
        {
            file.selection = Some((anchor, off));
        }
        if response.drag_stopped() {
            file.drag_anchor = None;
        }
        if response.clicked() {
            let pos = response.interact_pointer_pos().unwrap_or_default();
            if let Some(off) = offset_at(pos) {
                file.cursor = Some(off);
                if ctrl {
                    events.goto = Some((off, pos));
                } else {
                    file.selection = None;
                }
            } else if ghost_at(pos).is_some() {
                if ctrl {
                    events.ghost_click = true;
                } else {
                    file.selection = None;
                }
            } else if show_blame
                && pos.x < rect.left() + blame_w
                && let Some(blame) = &file.blame
                && let Some(line) = line_at(pos)
                && let Some(commit) = blame.lines.get(line).map(|c| &blame.commits[*c as usize])
                && !commit.is_uncommitted()
            {
                events.blame_click = Some(commit.sha.clone());
            }
        }
        if response.double_clicked()
            && let Some(off) = response.interact_pointer_pos().and_then(offset_at)
        {
            let (a, b) = word_at(&file.text, off as usize);
            file.selection = Some((a as u32, b as u32));
        }
        if response.secondary_clicked()
            && let Some(off) = response.interact_pointer_pos().and_then(offset_at)
        {
            file.cursor = Some(off);
            events.context = Some(off);
        }
        // Blame tooltip.
        if show_blame
            && let (Some(pos), Some(blame)) = (pointer, &file.blame)
            && pos.x < rect.left() + blame_w
            && let Some(line) = line_at(pos)
            && let Some(commit) = blame.lines.get(line).map(|c| &blame.commits[*c as usize])
        {
            let text = if commit.is_uncommitted() {
                "Not committed yet".to_string()
            } else {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                format!(
                    "{}\n{} · {}\n{}\n\nClick to open on GitHub",
                    commit.summary,
                    commit.author,
                    crate::git::relative_time(commit.time, now_secs),
                    &commit.sha[..commit.sha.len().min(12)]
                )
            };
            egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::unique("blame-tip"), pos)
                .show(|ui| ui.label(text));
        }
    });
    file.scroll = output.state.clamped_offset();
    file.view_size = output.inner_rect.size();
    file.scroll_max = (output.content_size - file.view_size).max(Vec2::ZERO);
    ui.advance_cursor_after_rect(full);

    if let Some(base) = diff_base {
        change_map(ui, file, theme, &colors, full, output.inner_rect, output.content_size.y, row_h);
        let parts: Vec<Chip> = match &file.rows {
            None => {
                let note = diff.as_ref().and_then(|d| d.note).unwrap_or("Making the diff…");
                vec![Chip::Text(note.into(), theme.fg_dim)]
            }
            Some(m) if m.changes.is_empty() => {
                vec![
                    Chip::Text("No changes in this file vs".into(), theme.fg),
                    Chip::Text(base.into(), theme.fg_bright),
                ]
            }
            Some(m) => {
                let n = m.changes.len();
                let mut parts = match file.change {
                    Some(c) => vec![
                        Chip::Text("change".into(), theme.fg),
                        Chip::Text(format!("{}/{n}", c + 1), theme.fg_bright),
                    ],
                    None => vec![
                        Chip::Text(n.to_string(), theme.fg_bright),
                        Chip::Text(if n == 1 { "change" } else { "changes" }.into(), theme.fg),
                    ],
                };
                parts.extend([
                    Chip::Sep,
                    Chip::Key("]"),
                    Chip::Text("next".into(), theme.fg),
                    Chip::Key("["),
                    Chip::Text("previous".into(), theme.fg),
                ]);
                parts
            }
        };
        let corner = Pos2::new(output.inner_rect.right() - 14.0, output.inner_rect.bottom() - 12.0);
        draw_chip(ui.painter(), theme, corner, &parts);
    }
    events
}

/// The marks on the right edge in diff mode: where each change is in the
/// file. A click on a mark goes to that change; a click elsewhere scrolls
/// there.
#[allow(clippy::too_many_arguments)]
fn change_map(
    ui: &mut egui::Ui,
    file: &mut OpenFile,
    theme: &Theme,
    colors: &DiffColors,
    full: Rect,
    view: Rect,
    content_h: f32,
    row_h: f32,
) {
    let strip = Rect::from_min_max(Pos2::new(full.right() - CHANGE_MAP_WIDTH, full.top()), full.max);
    let painter = ui.painter_at(strip);
    painter.rect_filled(strip, 0.0, theme.bg);
    painter.line_segment([strip.left_top(), strip.left_bottom()], Stroke::new(1.0, theme.muted.gamma_multiply(0.35)));
    let h = strip.height();
    let total = (file.row_count() as f32).max(view.height() / row_h).max(1.0);
    if content_h > view.height() + 2.0 {
        let top = strip.top() + file.scroll.y / content_h * h;
        let thumb = Rect::from_min_size(
            Pos2::new(strip.left() + 2.0, top),
            Vec2::new(CHANGE_MAP_WIDTH - 4.0, (view.height() / content_h * h).max(6.0)),
        );
        painter.rect_filled(thumb, 3.0, theme.muted.gamma_multiply(0.6));
    }
    let Some(map) = file.rows.clone() else { return };
    let tick = |a: u32, b: u32| {
        let top = strip.top() + a as f32 / total * h;
        let height = ((b - a + 1) as f32 / total * h).max(3.0);
        Rect::from_min_size(Pos2::new(strip.left() + 3.0, top), Vec2::new(CHANGE_MAP_WIDTH - 6.0, height))
    };
    for &(a, b, kind, change) in &map.runs {
        let mut r = tick(a, b);
        if file.change == Some(change) {
            r = r.expand2(Vec2::new(2.0, 0.0));
            painter.rect_stroke(r, 1.0, Stroke::new(1.0, theme.fg_bright), StrokeKind::Outside);
        }
        painter.rect_filled(r, 1.0, colors.of_run(kind));
    }
    let resp = ui.interact(strip, egui::Id::unique(("change-map", file.rel.as_str())), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let hit = map.changes.iter().position(|&(a, b)| {
            let r = tick(a, b).expand2(Vec2::new(0.0, 3.0));
            pos.y >= r.top() && pos.y <= r.bottom()
        });
        match hit {
            Some(c) => {
                file.change = Some(c);
                file.scroll_to_row = Some((map.changes[c].0, 1.0 / 3.0));
            }
            None => {
                let row = ((pos.y - strip.top()) / h * total).max(0.0) as u32;
                file.scroll_to_row = Some((row, 0.5));
            }
        }
    }
}

/// Scrolls with keys when no text field has focus: the arrows, j and k,
/// Page Up and Page Down, Space, Home and End, Ctrl+D and Ctrl+U, and `gg`
/// and `G` as in Vim. A number and then `g` or `G` (`500g`) goes to that
/// line. `0` scrolls to the left edge. Returns the new scroll offset.
fn scroll_keys(ui: &egui::Ui, file: &mut OpenFile, row_h: f32, char_w: f32) -> Option<Vec2> {
    if ui.ctx().memory(|m| m.focused().is_some()) {
        return None;
    }
    let now = ui.input(|i| i.time);
    let page = (file.view_size.y - 2.0 * row_h).max(row_h);
    let events = ui.input(|i| i.events.clone());
    let mut to: Option<Vec2> = None;
    for e in events {
        let egui::Event::Key { key, pressed: true, modifiers, .. } = e else { continue };
        let at = to.unwrap_or(file.scroll);
        let (mut x, mut y) = (at.x, at.y);
        let plain = !modifiers.ctrl && !modifiers.alt && !modifiers.mac_cmd;
        if key != egui::Key::G || !plain || modifiers.shift {
            file.pending_g = None;
        }
        if let Some(digit) = digit(key).filter(|_| plain && !modifiers.shift) {
            match file.count {
                None if digit == 0 => to = Some(Vec2::new(0.0, y)),
                n => file.count = Some(n.unwrap_or(0).saturating_mul(10).saturating_add(digit)),
            }
            continue;
        }
        if let Some(line) = file.count.take()
            && key == egui::Key::G
            && plain
        {
            let line = line.clamp(1, file.lines.len() as u32) - 1;
            file.scroll_to_line = Some(line);
            file.cursor = Some(file.line_start(line));
            file.flash = Some((line, now));
            continue;
        }
        match key {
            egui::Key::ArrowDown | egui::Key::J if plain && !modifiers.shift => y += KEY_LINES * row_h,
            egui::Key::ArrowUp | egui::Key::K if plain && !modifiers.shift => y -= KEY_LINES * row_h,
            egui::Key::ArrowRight if plain => x += KEY_COLUMNS * char_w,
            egui::Key::ArrowLeft if plain => x -= KEY_COLUMNS * char_w,
            egui::Key::PageDown if plain => y += page,
            egui::Key::PageUp if plain => y -= page,
            egui::Key::Space if plain => y += if modifiers.shift { -page } else { page },
            egui::Key::D if modifiers.ctrl && !modifiers.shift => y += page / 2.0,
            egui::Key::U if modifiers.ctrl && !modifiers.shift => y -= page / 2.0,
            egui::Key::Home if !modifiers.alt => (x, y) = (0.0, 0.0),
            egui::Key::End if !modifiers.alt => y = f32::MAX,
            egui::Key::G if plain && modifiers.shift => y = f32::MAX,
            egui::Key::G if plain => {
                if file.pending_g.take().is_some_and(|t| now - t < GG_DELAY) {
                    (x, y) = (0.0, 0.0);
                } else {
                    file.pending_g = Some(now);
                    continue;
                }
            }
            _ => continue,
        }
        to = Some(Vec2::new(x.clamp(0.0, file.scroll_max.x), y.clamp(0.0, file.scroll_max.y)));
    }
    to
}

/// Shift and the mouse wheel scroll one page per wheel step, as Page Down
/// and Page Up do. egui would scroll sideways, so this takes the wheel
/// away from egui while Shift is held over the code. `y` is the scroll
/// offset so far. Returns the new one.
fn shift_wheel(ui: &egui::Ui, file: &mut OpenFile, row_h: f32, y: f32) -> Option<f32> {
    let shift = ui.input(|i| i.modifiers.shift && !i.modifiers.ctrl && !i.modifiers.command);
    if !shift || !ui.rect_contains_pointer(ui.available_rect_before_wrap()) {
        file.shift_wheel = 0.0;
        return None;
    }
    let steps: f32 = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::MouseWheel { unit, delta, .. } => {
                    // With Shift, macOS sends the wheel as a sideways scroll.
                    let d = if delta.y != 0.0 { delta.y } else { delta.x };
                    Some(match unit {
                        egui::MouseWheelUnit::Point => d / POINTS_PER_STEP,
                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => d,
                    })
                }
                _ => None,
            })
            .sum()
    });
    ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    file.shift_wheel += steps;
    let pages = file.shift_wheel.trunc();
    if pages == 0.0 {
        return None;
    }
    file.shift_wheel -= pages;
    let page = (file.view_size.y - 2.0 * row_h).max(row_h);
    // The wheel turned down gives a negative delta.
    Some((y - pages * page).clamp(0.0, file.scroll_max.y))
}

fn digit(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    let keys = [Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9];
    keys.iter().position(|k| *k == key).map(|d| d as u32)
}

/// The word around a byte offset (letters, digits, `_`, `?`, `!`, `@`).
pub fn word_at(text: &str, offset: usize) -> (usize, usize) {
    let is_word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '?' | '!' | '@' | '$');
    let offset = offset.min(text.len());
    let start = text[..offset].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(offset, |(i, _)| i);
    let end = text[offset..].char_indices().find(|(_, c)| !is_word(*c)).map_or(text.len(), |(i, _)| offset + i);
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_words() {
        let f = OpenFile::new("a.rb".into(), "a.rb".into(), "class Foo\n  def bar?\nend\n".into());
        assert_eq!(f.lines.len(), 4);
        assert_eq!(f.line_of(12), 1);
        assert_eq!(word_at(&f.text, 17), (16, 20));
    }

    #[test]
    fn find_in_file() {
        let mut f = OpenFile::new("a.rb".into(), "a.rb".into(), "Foo foo FOO\n".into());
        f.find.query = "foo".into();
        f.update_find();
        assert_eq!(f.find.matches.len(), 3);
    }
}
