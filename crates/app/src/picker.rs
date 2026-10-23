//! The fuzzy pickers: go to file (Ctrl+P), go to symbol (Ctrl+T), the
//! command list (Ctrl+K), go to line (Ctrl+G), the project search box
//! (Ctrl+Shift+F) and the base branch of diff mode.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Color32, Key, RichText};
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
    Branches,
    SwitchBranch,
}

#[derive(Debug, Clone)]
pub enum Payload {
    File(String),
    /// A file and a byte offset in it.
    Loc(PathBuf, u32),
    Action(Action),
    /// A base branch for diff mode.
    Branch(String),
    /// A branch to check out, and whether it is a remote branch.
    Switch(String, bool),
}

#[derive(Debug, Clone)]
pub struct PickItem {
    pub label: String,
    pub detail: String,
    pub payload: Payload,
    /// Added to the match score (project files rank above gems).
    pub bonus: u32,
    /// Short colored words on the right, before the detail: `+13 −2 M`.
    pub tags: Vec<(String, Color32)>,
    /// A heading above the item when the list is not filtered: `Local`.
    pub group: Option<&'static str>,
    /// Drawn dim: the item is there to see, not to choose.
    pub dim: bool,
}

impl PickItem {
    pub fn new(label: impl Into<String>, detail: impl Into<String>, payload: Payload, bonus: u32) -> PickItem {
        PickItem {
            label: label.into(),
            detail: detail.into(),
            payload,
            bonus,
            tags: Vec::new(),
            group: None,
            dim: false,
        }
    }
}

pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    pub selected: usize,
    items: Arc<Vec<PickItem>>,
    results: Vec<usize>,
    last_query: Option<String>,
    focus: bool,
    /// Go to file in diff mode: the changed files, which Tab switches to.
    changed: Option<Arc<Vec<PickItem>>>,
    pub show_changed: bool,
    /// The title of the changed files list: `Go to changed file (vs main)`.
    changed_title: String,
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
        Picker {
            kind,
            query: String::new(),
            selected: 0,
            items,
            results: Vec::new(),
            last_query: None,
            focus: true,
            changed: None,
            show_changed: false,
            changed_title: String::new(),
        }
    }

    /// Go to file with a second list, the changed files, that Tab shows.
    pub fn with_changed(mut self, changed: Arc<Vec<PickItem>>, title: String, show: bool) -> Picker {
        self.changed = Some(changed);
        self.changed_title = title;
        self.show_changed = show;
        self
    }

    /// New items, for a list that loads in the background.
    pub fn set_items(&mut self, items: Arc<Vec<PickItem>>) {
        self.items = items;
        self.last_query = None;
    }

    /// The items of the list that shows now.
    fn current(&self) -> &Arc<Vec<PickItem>> {
        match (&self.changed, self.show_changed) {
            (Some(c), true) => c,
            _ => &self.items,
        }
    }

    /// The label of the first result (tests).
    #[cfg(test)]
    pub fn first_label(&self) -> Option<String> {
        let i = *self.results.first()?;
        self.current().get(i).map(|item| item.label.clone())
    }

    pub fn title(&self) -> &str {
        match self.kind {
            PickerKind::Files if self.show_changed && self.changed.is_some() => &self.changed_title,
            PickerKind::Files => "Go to file",
            PickerKind::Symbols => "Go to symbol",
            PickerKind::Commands => "Commands",
            PickerKind::Line => "Go to line",
            PickerKind::Search => "Search in all files",
            PickerKind::Branches => "Diff against branch",
            PickerKind::SwitchBranch => "Switch branch",
        }
    }

    /// Switches between all files and the changed files.
    fn toggle_changed(&mut self) {
        if self.changed.is_some() {
            self.show_changed = !self.show_changed;
            self.last_query = None;
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
        let items = self.current().clone();
        if self.query.trim().is_empty() {
            self.results = (0..items.len().min(MAX_RESULTS)).collect();
            return;
        }
        let config = if self.kind == PickerKind::Files { Config::DEFAULT.match_paths() } else { Config::DEFAULT };
        let pattern = Pattern::parse(&self.query, CaseMatching::Smart, Normalization::Smart);
        let mut scored: Vec<(u32, usize)> = items
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
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(items[a.1].label.len().cmp(&items[b.1].label.len())));
        scored.truncate(MAX_RESULTS);
        self.results = scored.into_iter().map(|(_, i)| i).collect();
    }

    #[cfg(test)]
    pub fn result_labels(&mut self) -> Vec<String> {
        self.refresh();
        let items = self.current().clone();
        self.results.iter().map(|i| items[*i].label.clone()).collect()
    }

    /// Draws the picker. Returns the choice, or `Err(())` when it closes.
    pub fn ui(&mut self, ctx: &egui::Context, theme: &Theme) -> Option<Result<Picked, ()>> {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Tab)) {
            self.toggle_changed();
        }
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
                    let mut toggle = false;
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(self.title()).color(theme.fg_dim).small());
                        if let Some(changed) = &self.changed {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                let n = changed.len();
                                let seg = |ui: &mut egui::Ui, on: bool, text: String| {
                                    let t = RichText::new(text).small().color(if on {
                                        theme.fg_bright
                                    } else {
                                        theme.fg_dim
                                    });
                                    ui.selectable_label(on, t).clicked()
                                };
                                toggle |=
                                    seg(ui, self.show_changed, format!("Changed files {n}")) && !self.show_changed;
                                toggle |= seg(ui, !self.show_changed, "All files".into()) && self.show_changed;
                            });
                        }
                    });
                    if toggle {
                        self.toggle_changed();
                        self.focus = true;
                    }
                    let hint = match self.kind {
                        PickerKind::Files => "file name…",
                        PickerKind::Symbols => "class, module, method or constant…",
                        PickerKind::Commands => "command…",
                        PickerKind::Line => "line number",
                        PickerKind::Search => "text to find, then Enter",
                        PickerKind::Branches | PickerKind::SwitchBranch => "branch…",
                    };
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text(hint)
                            .desired_width(f32::INFINITY)
                            .lock_focus(true),
                    );
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
                                Some(i) => Ok(Picked::Item(self.current()[*i].clone())),
                                None => Err(()),
                            },
                        });
                    }
                    let items = self.current().clone();
                    if self.results.is_empty() {
                        if self.kind == PickerKind::Files && self.show_changed {
                            ui.separator();
                            ui.label(RichText::new("No changed file matches.").color(theme.fg_dim));
                        }
                        if matches!(self.kind, PickerKind::Branches | PickerKind::SwitchBranch) && items.is_empty() {
                            ui.separator();
                            ui.label(RichText::new("Reading the branches…").color(theme.fg_dim));
                        }
                    } else {
                        ui.separator();
                        let headings = self.query.trim().is_empty();
                        let mut group = None;
                        egui::ScrollArea::vertical().max_height(screen.height() * 0.55).show(ui, |ui| {
                            for (row, i) in self.results.iter().enumerate() {
                                let item = &items[*i];
                                if headings && item.group.is_some() && item.group != group {
                                    group = item.group;
                                    ui.label(
                                        RichText::new(item.group.unwrap_or_default().to_uppercase())
                                            .color(theme.fg_dim)
                                            .small(),
                                    );
                                }
                                let selected = row == self.selected;
                                let resp = ui
                                    .horizontal(|ui| {
                                        let color = match (selected, item.dim) {
                                            (_, true) => theme.fg_dim,
                                            (true, _) => theme.fg_bright,
                                            _ => theme.fg,
                                        };
                                        let label = RichText::new(&item.label).color(color);
                                        let label =
                                            if selected { label.background_color(theme.selection) } else { label };
                                        let r = ui.add(egui::Label::new(label).sense(egui::Sense::click()).truncate());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(RichText::new(&item.detail).color(theme.fg_dim).small());
                                            for (tag, color) in item.tags.iter().rev() {
                                                let text = RichText::new(tag).color(*color).small();
                                                // Counts and marks line up in monospace; words do not need it.
                                                let text = if tag.contains(' ') { text } else { text.monospace() };
                                                ui.label(text);
                                            }
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
                    }
                    let foot = match self.kind {
                        PickerKind::Files if self.changed.is_some() => {
                            Some("Tab: all files / changed files · Enter: open · Esc: close".to_string())
                        }
                        PickerKind::Branches => Some(
                            "The diff starts at the merge-base of the branch and HEAD, like a pull request. \
                             The choice is remembered for this project."
                                .to_string(),
                        ),
                        PickerKind::SwitchBranch => Some(
                            "The newest commit first. The initials are the author of the open pull request; \
                             dim initials are the author of the last commit, when there is no pull request. \
                             Changes that are not committed stay; git refuses when they would be lost."
                                .to_string(),
                        ),
                        _ => None,
                    };
                    if let Some(foot) = foot {
                        ui.separator();
                        ui.label(RichText::new(foot).color(theme.fg_dim).small());
                    }
                });
            });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(labels: &[&str]) -> Arc<Vec<PickItem>> {
        Arc::new(labels.iter().map(|l| PickItem::new(*l, "", Payload::File(l.to_string()), 0)).collect())
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

    #[test]
    fn tab_list_of_changed_files() {
        let mut p = Picker::new(PickerKind::Files, items(&["app/models/order.rb", "app/models/refund.rb"]))
            .with_changed(items(&["app/models/refund.rb"]), "Go to changed file (vs main)".into(), false);
        assert_eq!(p.result_labels().len(), 2);
        p.toggle_changed();
        assert_eq!(p.result_labels(), vec!["app/models/refund.rb"]);
        assert_eq!(p.title(), "Go to changed file (vs main)");
    }
}
