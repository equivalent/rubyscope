//! The code view: an open file, drawn row by row.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, TextFormat, Vec2, text::LayoutJob,
};
use rubyscope_index::model::Span;

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
const WHEEL_SPEED: f32 = 3.0;
/// Lines an arrow key (or j / k) scrolls.
const KEY_LINES: f32 = 3.0;
/// Columns a left or right arrow scrolls.
const KEY_COLUMNS: f32 = 8.0;
/// The time between the two presses of `gg`, in seconds.
const GG_DELAY: f64 = 1.0;

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
    /// Set to scroll so this 0-based line is near the top.
    pub scroll_to_line: Option<u32>,
    /// A line to flash after a jump, with the time it started.
    pub flash: Option<(u32, f64)>,
    pub top_line: u32,
    /// The last place clicked: actions like "find usages" use it.
    pub cursor: Option<u32>,
    pub selection: Option<(u32, u32)>,
    drag_anchor: Option<u32>,
    pub find: FindState,
    /// Where the pointer rests, and since when (for the hover popup).
    pub hover: Option<(u32, f64)>,
    /// The last layout: where the first visible row's text starts, the row
    /// height, the char width, and the first visible line. Tests use it to
    /// click on a line and column.
    pub layout: Option<(Pos2, f32, f32, usize)>,
    /// The scroll offset, the largest offset and the visible size, from the
    /// last frame. The scroll keys use them.
    pub scroll: Vec2,
    scroll_max: Vec2,
    view_size: Vec2,
    /// When the first `g` of `gg` was pressed.
    pending_g: Option<f64>,
    /// Digits typed before `g`: `500g` goes to line 500.
    count: Option<u32>,
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
            scroll_to_line: None,
            flash: None,
            top_line: 0,
            cursor: None,
            selection: None,
            drag_anchor: None,
            find: FindState::default(),
            hover: None,
            layout: None,
            scroll: Vec2::ZERO,
            scroll_max: Vec2::ZERO,
            view_size: Vec2::ZERO,
            pending_g: None,
            count: None,
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
    /// Ctrl+click: go to the definition at this offset.
    pub goto: Option<(u32, Pos2)>,
    /// Right click at this offset.
    pub context: Option<u32>,
    /// The pointer rested on this offset long enough for a hover popup.
    pub hover: Option<(u32, Pos2)>,
    /// A click on a blame block: the commit.
    pub blame_click: Option<String>,
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

fn build_row(file: &OpenFile, line: usize, theme: &Theme, font: &FontId) -> Row {
    let (start, end) = file.lines[line];
    let text = &file.text[start as usize..end as usize];
    let mut job = LayoutJob::default();
    let mut byte_of_char = Vec::with_capacity(text.len() + 1);
    let fmt = |token: Token| TextFormat { font_id: font.clone(), color: theme.token(token), ..Default::default() };
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
    let tokens = file.highlights.as_ref().and_then(|h| h.get(line));
    let mut pos = 0usize;
    if let Some(tokens) = tokens {
        for (s, e, t) in tokens {
            let (s, e) = ((*s as usize).min(text.len()), (*e as usize).min(text.len()));
            if s > pos {
                push(pos, s, Token::Plain, &mut job, &mut byte_of_char);
            }
            if e > s && text.is_char_boundary(s) && text.is_char_boundary(e) {
                push(s, e, *t, &mut job, &mut byte_of_char);
                pos = e;
            }
        }
    }
    if pos < text.len() {
        push(pos, text.len(), Token::Plain, &mut job, &mut byte_of_char);
    }
    byte_of_char.push(end);
    Row { job, byte_of_char }
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

/// Draws the file and handles the pointer. `blame` shows the blame column.
pub fn code_view(
    ui: &mut egui::Ui,
    file: &mut OpenFile,
    theme: &Theme,
    links: Option<&dyn Clickable>,
    show_blame: bool,
    now_secs: i64,
) -> ViewEvents {
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
            // `16/07/2025 Henriksson`, and space on both sides.
            (11 + name.min(BLAME_NAME_CHARS)) as f32 * char_w * BLAME_FONT + 22.0
        }
    };
    let gutter_w = digits as f32 * char_w + 24.0;
    let code_x = blame_w + gutter_w;
    let width = code_x + (file.max_line_chars as f32 + 4.0) * char_w;
    let time = ui.input(|i| i.time);
    let ctrl = ui.input(|i| i.modifiers.ctrl);

    let mut area = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .id_salt(("code", &file.rel))
        .wheel_scroll_multiplier(Vec2::splat(WHEEL_SPEED));
    if let Some(to) = scroll_keys(ui, file, row_h, char_w) {
        area = area.scroll_offset(to);
    }
    if let Some(line) = file.scroll_to_line.take() {
        area = area.vertical_scroll_offset((line.saturating_sub(6)) as f32 * row_h);
    }
    let total = file.lines.len();
    ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, theme.bg);
    let output = area.show_rows(ui, row_h, total, |ui, range| {
        file.top_line = range.start as u32;
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(width.max(ui.available_width()), row_h * range.len() as f32),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(ui.clip_rect());
        let rows: Vec<Row> = range.clone().map(|l| build_row(file, l, theme, &font)).collect();
        let galleys: Vec<_> = rows.iter().map(|r| ui.fonts_mut(|f| f.layout_job(r.job.clone()))).collect();
        let row_top = |i: usize| rect.top() + i as f32 * row_h;
        let text_pos = |i: usize| Pos2::new(rect.left() + code_x, row_top(i) + (row_h - FONT_SIZE * 1.2) / 2.0);
        file.layout = Some((text_pos(0), row_h, char_w, range.start));

        // The pointer as a file offset.
        let offset_at = |pos: Pos2| -> Option<u32> {
            if pos.x < rect.left() + code_x {
                return None;
            }
            let i = ((pos.y - rect.top()) / row_h).floor();
            if i < 0.0 || i as usize >= rows.len() {
                return None;
            }
            let i = i as usize;
            let local = pos - text_pos(i);
            let c = galleys[i].cursor_from_pos(Vec2::new(local.x, galleys[i].size().y / 2.0)).index.0;
            rows[i].byte_of_char.get(c).copied()
        };

        // Backgrounds: the flash line, find matches, the selection.
        for (i, line) in range.clone().enumerate() {
            let y = row_top(i);
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
        // of the cursor line, have brighter text.
        if let (true, Some(blame)) = (show_blame, file.blame.clone()) {
            let line_at = |pos: Pos2| range.start + ((pos.y - rect.top()) / row_h) as usize;
            let active = response
                .hover_pos()
                .filter(|p| p.x < rect.left() + blame_w)
                .map(line_at)
                .or(file.cursor.map(|c| file.line_of(c) as usize))
                .and_then(|l| blame.lines.get(l).copied());
            let blame_font = FontId::monospace(FONT_SIZE * BLAME_FONT);
            for (i, line) in range.clone().enumerate() {
                let Some(c) = blame.lines.get(line).copied() else { break };
                let commit = &blame.commits[c as usize];
                // Lines that are not committed yet stay empty.
                if commit.is_uncommitted() {
                    continue;
                }
                let r = Rect::from_min_size(Pos2::new(rect.left(), row_top(i)), Vec2::new(blame_w - 6.0, row_h));
                painter.rect_filled(r, 0.0, theme.blame_color(commit.rank));
                let name: String = commit.last_name().chars().take(BLAME_NAME_CHARS).collect();
                let color = if active == Some(c) { theme.fg_bright } else { theme.fg.gamma_multiply(0.7) };
                painter.text(
                    Pos2::new(r.left() + 8.0, r.center().y),
                    Align2::LEFT_CENTER,
                    format!("{:<10} {name}", commit.short_date(now_secs)),
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

        // Line numbers.
        let cursor_line = file.cursor.map(|c| file.line_of(c) as usize);
        for (i, line) in range.clone().enumerate() {
            let color = if Some(line) == cursor_line { theme.fg } else { theme.muted };
            painter.text(
                Pos2::new(rect.left() + blame_w + gutter_w - 12.0, row_top(i) + row_h / 2.0),
                Align2::RIGHT_CENTER,
                (line + 1).to_string(),
                font.clone(),
                color,
            );
        }

        // Text.
        for (i, g) in galleys.iter().enumerate() {
            painter.galley(text_pos(i), g.clone(), theme.fg);
        }

        // Pointer.
        let pointer = response.hover_pos();
        let hovered = pointer.and_then(offset_at);
        if let (Some(off), Some(links)) = (hovered, links) {
            if let Some(span) = links.clickable_at(off) {
                if ctrl {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    let line = file.line_of(span.start) as usize;
                    if range.contains(&line) {
                        let i = line - range.start;
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

        // The drag starts only after the pointer moves a few pixels, so the
        // selection starts where the button went down, not where it is now.
        if response.drag_started_by(egui::PointerButton::Primary)
            && !ctrl
            && let Some(off) = ui.input(|i| i.pointer.press_origin()).and_then(offset_at)
        {
            file.drag_anchor = Some(off);
            file.selection = Some((off, off));
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let (Some(anchor), Some(off)) = (file.drag_anchor, response.interact_pointer_pos().and_then(offset_at))
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
            } else if show_blame
                && pos.x < rect.left() + blame_w
                && let Some(blame) = &file.blame
            {
                let line = range.start + ((pos.y - rect.top()) / row_h) as usize;
                if let Some(commit) = blame.lines.get(line).map(|c| &blame.commits[*c as usize])
                    && !commit.is_uncommitted()
                {
                    events.blame_click = Some(commit.sha.clone());
                }
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
        {
            let line = range.start + ((pos.y - rect.top()) / row_h) as usize;
            if let Some(commit) = blame.lines.get(line).map(|c| &blame.commits[*c as usize])
                && !commit.is_uncommitted()
            {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                let text = format!(
                    "{}\n{} · {}\n{}\n\nClick to open on GitHub",
                    commit.summary,
                    commit.author,
                    crate::git::relative_time(commit.time, now_secs),
                    &commit.sha[..commit.sha.len().min(12)]
                );
                egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::unique("blame-tip"), pos)
                    .show(|ui| ui.label(text));
            }
        }
    });
    file.scroll = output.state.clamped_offset();
    file.view_size = output.inner_rect.size();
    file.scroll_max = (output.content_size - file.view_size).max(Vec2::ZERO);
    events
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
        let plain = !modifiers.ctrl && !modifiers.alt;
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
