//! The file tree in the side bar.

use std::collections::{BTreeMap, HashSet};

use eframe::egui::{self, RichText};

use crate::theme::Theme;

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

    /// Draws the tree. Returns the file that was clicked.
    pub fn ui(&mut self, ui: &mut egui::Ui, theme: &Theme, current: Option<&str>) -> Option<String> {
        let mut clicked = None;
        let FileTree { root, open, reveal } = self;
        egui::ScrollArea::both().auto_shrink([false, false]).id_salt("tree").show(ui, |ui| {
            dir_ui(ui, theme, root, "", 0, open, reveal, current, &mut clicked);
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

#[allow(clippy::too_many_arguments)]
fn dir_ui(
    ui: &mut egui::Ui,
    theme: &Theme,
    dir: &Dir,
    prefix: &str,
    depth: usize,
    open: &mut HashSet<String>,
    reveal: &mut Option<String>,
    current: Option<&str>,
    clicked: &mut Option<String>,
) {
    let indent = depth as f32 * 14.0;
    for (name, sub) in &dir.dirs {
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let is_open = open.contains(&path);
        let icon = if is_open { "▾" } else { "▸" };
        let row = ui
            .horizontal(|ui| {
                ui.add_space(indent);
                ui.add(
                    egui::Label::new(RichText::new(format!("{icon} {name}")).color(theme.fg))
                        .sense(egui::Sense::click()),
                )
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
            dir_ui(ui, theme, sub, &path, depth + 1, open, reveal, current, clicked);
        }
    }
    for name in &dir.files {
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let selected = current == Some(path.as_str());
        let color = if selected {
            theme.fg_bright
        } else if name.ends_with(".rb") {
            theme.fg
        } else {
            theme.fg_dim
        };
        let resp = ui
            .horizontal(|ui| {
                ui.add_space(indent + 14.0);
                let text = RichText::new(name).color(color);
                let text = if selected { text.background_color(theme.selection) } else { text };
                ui.add(egui::Label::new(text).sense(egui::Sense::click()))
            })
            .inner;
        if reveal.as_deref() == Some(path.as_str()) {
            resp.scroll_to_me(Some(egui::Align::Center));
            *reveal = None;
        }
        if resp.clicked() {
            *clicked = Some(path);
        }
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
