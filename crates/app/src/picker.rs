//! The fuzzy pickers: go to file (Ctrl+P), go to symbol (Ctrl+T), the
//! command list (Ctrl+K), go to line (Ctrl+G) and the project search box
//! (Ctrl+Shift+F).

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Key, RichText};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use rayon::prelude::*;

use crate::keys::Action;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Files,
    Symbols,
    Commands,
    Line,
    Search,
}

#[derive(Debug, Clone)]
pub enum Payload {
    File(String),
    /// A file and a byte offset in it.
    Loc(PathBuf, u32),
    Action(Action),
}

#[derive(Debug, Clone)]
pub struct PickItem {
    pub label: String,
    pub detail: String,
    pub payload: Payload,
    /// Added to the match score (project files rank above gems).
    pub bonus: u32,
}

pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    pub selected: usize,
    items: Arc<Vec<PickItem>>,
    results: Vec<usize>,
    last_query: Option<String>,
    focus: bool,
}

/// What the user chose.
pub enum Picked {
    Item(PickItem),
    Line(u32),
    Search(String),
}

const MAX_RESULTS: usize = 200;

impl Picker {
    pub fn new(kind: PickerKind, items: Arc<Vec<PickItem>>) -> Picker {
        Picker { kind, query: String::new(), selected: 0, items, results: Vec::new(), last_query: None, focus: true }
    }

    pub fn title(&self) -> &'static str {
        match self.kind {
            PickerKind::Files => "Go to file",
            PickerKind::Symbols => "Go to symbol",
            PickerKind::Commands => "Commands",
            PickerKind::Line => "Go to line",
            PickerKind::Search => "Search in all files",
        }
    }

    fn refresh(&mut self) {
        if self.last_query.as_deref() == Some(self.query.as_str()) {
            return;
        }
        self.last_query = Some(self.query.clone());
        self.selected = 0;
        if matches!(self.kind, PickerKind::Line | PickerKind::Search) {
            self.results.clear();
            return;
        }
        if self.query.trim().is_empty() {
            self.results = (0..self.items.len().min(MAX_RESULTS)).collect();
            return;
        }
        let config = if self.kind == PickerKind::Files { Config::DEFAULT.match_paths() } else { Config::DEFAULT };
        let pattern = Pattern::parse(&self.query, CaseMatching::Smart, Normalization::Smart);
        let mut scored: Vec<(u32, usize)> = self
            .items
            .par_iter()
            .enumerate()
            .map_init(
                || (Matcher::new(config.clone()), Vec::new()),
                |(matcher, buf), (i, item)| {
                    let score = pattern.score(Utf32Str::new(&item.label, buf), matcher)?;
                    Some((score + item.bonus, i))
                },
            )
            .flatten()
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(self.items[a.1].label.len().cmp(&self.items[b.1].label.len())));
        scored.truncate(MAX_RESULTS);
        self.results = scored.into_iter().map(|(_, i)| i).collect();
    }

    #[cfg(test)]
    pub fn result_labels(&mut self) -> Vec<String> {
        self.refresh();
        self.results.iter().map(|i| self.items[*i].label.clone()).collect()
    }

    /// Draws the picker. Returns the choice, or `Err(())` when it closes.
    pub fn ui(&mut self, ctx: &egui::Context, theme: &Theme) -> Option<Result<Picked, ()>> {
        self.refresh();
        let mut out = None;
        let (up, down, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if esc {
            return Some(Err(()));
        }
        if down && self.selected + 1 < self.results.len() {
            self.selected += 1;
        }
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        let screen = ctx.content_rect();
        let width = (screen.width() * 0.6).clamp(420.0, 820.0);
        egui::Area::new(egui::Id::unique("picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(screen.center().x - width / 2.0, screen.top() + 60.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(theme.bg_light).show(ui, |ui| {
                    ui.set_width(width);
                    ui.label(RichText::new(self.title()).color(theme.fg_dim).small());
                    let hint = match self.kind {
                        PickerKind::Files => "file name…",
                        PickerKind::Symbols => "class, module, method or constant…",
                        PickerKind::Commands => "command…",
                        PickerKind::Line => "line number",
                        PickerKind::Search => "text to find, then Enter",
                    };
                    let edit = ui
                        .add(egui::TextEdit::singleline(&mut self.query).hint_text(hint).desired_width(f32::INFINITY));
                    if self.focus {
                        edit.request_focus();
                        self.focus = false;
                    }
                    if enter {
                        out = Some(match self.kind {
                            PickerKind::Line => match self.query.trim().parse::<u32>() {
                                Ok(n) => Ok(Picked::Line(n.max(1) - 1)),
                                Err(_) => Err(()),
                            },
                            PickerKind::Search if !self.query.is_empty() => Ok(Picked::Search(self.query.clone())),
                            PickerKind::Search => Err(()),
                            _ => match self.results.get(self.selected) {
                                Some(i) => Ok(Picked::Item(self.items[*i].clone())),
                                None => Err(()),
                            },
                        });
                    }
                    if self.results.is_empty() {
                        return;
                    }
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(screen.height() * 0.55).show(ui, |ui| {
                        for (row, i) in self.results.iter().enumerate() {
                            let item = &self.items[*i];
                            let selected = row == self.selected;
                            let resp = ui
                                .horizontal(|ui| {
                                    let label = RichText::new(&item.label).color(if selected {
                                        theme.fg_bright
                                    } else {
                                        theme.fg
                                    });
                                    let label = if selected { label.background_color(theme.selection) } else { label };
                                    let r = ui.add(egui::Label::new(label).sense(egui::Sense::click()).truncate());
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(RichText::new(&item.detail).color(theme.fg_dim).small());
                                    });
                                    r
                                })
                                .inner;
                            if selected && (up || down) {
                                resp.scroll_to_me(None);
                            }
                            if resp.clicked() {
                                out = Some(Ok(Picked::Item(item.clone())));
                            }
                        }
                    });
                });
            });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(labels: &[&str]) -> Arc<Vec<PickItem>> {
        Arc::new(
            labels
                .iter()
                .map(|l| PickItem {
                    label: l.to_string(),
                    detail: String::new(),
                    payload: Payload::File(l.to_string()),
                    bonus: 0,
                })
                .collect(),
        )
    }

    #[test]
    fn fuzzy_file_match() {
        let mut p = Picker::new(
            PickerKind::Files,
            items(&["app/models/invoice.rb", "app/models/user.rb", "spec/models/invoice_spec.rb"]),
        );
        p.query = "inv mod".into();
        let r = p.result_labels();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], "app/models/invoice.rb");
    }
}
