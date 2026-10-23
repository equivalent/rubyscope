//! The file tree in the side bar.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use eframe::egui::{self, RichText};

use crate::diff::FileMark;
use crate::theme::Theme;

/// The mouse wheel scrolls the side bar this many times faster than
/// egui's default.
pub const SIDEBAR_WHEEL_SPEED: f32 = 2.5;

#[derive(Debug, Default)]
pub struct Dir {
    pub dirs: BTreeMap<String, Dir>,
    pub files: Vec<String>,
}

#[derive(Debug, Default)]
pub struct FileTree {
    pub root: Dir,
    /// Open folders, by relative path.
    pub open: HashSet<String>,
    /// Set to scroll the tree to a file once.
    pub reveal: Option<String>,
}

impl FileTree {
    pub fn new(files: &[String]) -> FileTree {
        let mut root = Dir::default();
        for f in files {
            let mut dir = &mut root;
            let parts: Vec<&str> = f.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                dir = dir.dirs.entry(part.to_string()).or_default();
            }
            dir.files.push(parts[parts.len() - 1].to_string());
        }
        sort(&mut root);
        FileTree { root, open: HashSet::new(), reveal: None }
    }

    /// Opens the folders of a file and scrolls to it.
    pub fn reveal(&mut self, rel: &str) {
        let parts: Vec<&str> = rel.split('/').collect();
        for i in 1..parts.len() {
            self.open.insert(parts[..i].join("/"));
        }
        self.reveal = Some(rel.to_string());
    }

    /// Draws the tree. Returns the file that was clicked. `project` is the
    /// project folder, for "Copy full path" in the right-click menu. In diff
    /// mode, `marks` has the changed files: A or M for the branch, an amber
    /// dot for changes that are not committed.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        current: Option<&str>,
        project: &Path,
        marks: Option<&HashMap<String, FileMark>>,
    ) -> Option<String> {
        let mut clicked = None;
        let FileTree { root, open, reveal } = self;
        // Folders with a changed file in them, to mark them when they are closed.
        let changed_dirs: HashSet<String> = marks
            .map(|m| {
                m.keys()
                    .flat_map(|rel| {
                        let parts: Vec<&str> = rel.split('/').collect();
                        (1..parts.len()).map(move |i| parts[..i].join("/")).collect::<Vec<_>>()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ctx = Ctx { theme, current, project, marks, changed_dirs: &changed_dirs };
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt("tree")
            .wheel_scroll_multiplier(egui::Vec2::splat(SIDEBAR_WHEEL_SPEED))
            .show(ui, |ui| {
                dir_ui(ui, &ctx, root, "", 0, open, reveal, &mut clicked);
            });
        clicked
    }
}

fn sort(dir: &mut Dir) {
    dir.files.sort();
    for d in dir.dirs.values_mut() {
        sort(d);
    }
}

/// What every row of the tree needs to know.
struct Ctx<'a> {
    theme: &'a Theme,
    current: Option<&'a str>,
    project: &'a Path,
    marks: Option<&'a HashMap<String, FileMark>>,
    changed_dirs: &'a HashSet<String>,
}

/// The marks of a changed file, right-aligned: an amber dot, then A or M.
pub fn change_marks(ui: &mut egui::Ui, theme: &Theme, mark: FileMark) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        if let Some(b) = mark.branch {
            let (color, what) = match b {
                'A' => (theme.green, "Added on this branch"),
                'D' => (theme.red, "Deleted on this branch"),
                _ => (theme.blue, "Modified on this branch"),
            };
            ui.label(RichText::new(b.to_string()).color(color).monospace().small()).on_hover_text(what);
        }
        if mark.uncommitted {
            ui.label(RichText::new("●").color(theme.yellow).small()).on_hover_text("Not committed");
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn dir_ui(
    ui: &mut egui::Ui,
    ctx: &Ctx,
    dir: &Dir,
    prefix: &str,
    depth: usize,
    open: &mut HashSet<String>,
    reveal: &mut Option<String>,
    clicked: &mut Option<String>,
) {
    let Ctx { theme, current, project, marks, .. } = *ctx;
    let indent = depth as f32 * 14.0;
    for (name, sub) in &dir.dirs {
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let is_open = open.contains(&path);
        let icon = if is_open { "▾" } else { "▸" };
        let row = ui
            .horizontal(|ui| {
                ui.add_space(indent);
                let r = ui.add(
                    egui::Label::new(RichText::new(format!("{icon} {name}")).color(theme.fg))
                        .sense(egui::Sense::click()),
                );
                if !is_open && ctx.changed_dirs.contains(&path) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("•").color(theme.muted)).on_hover_text("Has changed files");
                    });
                }
                r
            })
            .inner;
        if row.clicked() {
            if is_open {
                open.remove(&path);
            } else {
                open.insert(path.clone());
            }
        }
        if is_open {
            dir_ui(ui, ctx, sub, &path, depth + 1, open, reveal, clicked);
        }
    }
    for name in &dir.files {
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let selected = current == Some(path.as_str());
        let mark = marks.and_then(|m| m.get(&path)).copied();
        let color = match (selected, mark.and_then(|m| m.branch)) {
            (true, _) => theme.fg_bright,
            (false, Some('A')) => theme.green,
            (false, Some('M')) => theme.blue,
            _ if name.ends_with(".rb") => theme.fg,
            _ => theme.fg_dim,
        };
        let resp = ui
            .horizontal(|ui| {
                ui.add_space(indent + 14.0);
                let text = RichText::new(name).color(color);
                let text = if selected { text.background_color(theme.selection) } else { text };
                let r = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
                if let Some(mark) = mark {
                    change_marks(ui, theme, mark);
                }
                r
            })
            .inner;
        if reveal.as_deref() == Some(path.as_str()) {
            resp.scroll_to_me(Some(egui::Align::Center));
            *reveal = None;
        }
        resp.context_menu(|ui| copy_path_buttons(ui, &path, &project.join(&path)));
        if resp.clicked() {
            *clicked = Some(path);
        }
    }
}

/// Buttons for the right-click menu of a file name: copy its path in the
/// project, or its full path on disk.
pub fn copy_path_buttons(ui: &mut egui::Ui, rel: &str, full: &Path) {
    if ui.button("Copy path").clicked() {
        ui.ctx().copy_text(rel.to_string());
    }
    if ui.button("Copy full path").clicked() {
        ui.ctx().copy_text(full.display().to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_reveals() {
        let mut t = FileTree::new(&["app/models/user.rb".into(), "app/models/invoice.rb".into(), "Gemfile".into()]);
        assert_eq!(t.root.files, vec!["Gemfile"]);
        assert_eq!(t.root.dirs["app"].dirs["models"].files, vec!["invoice.rb", "user.rb"]);
        t.reveal("app/models/user.rb");
        assert!(t.open.contains("app") && t.open.contains("app/models"));
    }
}
