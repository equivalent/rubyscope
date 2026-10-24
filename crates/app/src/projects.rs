//! The project list of the start screen: its order, the names the user
//! gives, and the icons found in the projects.

use std::path::{Path, PathBuf};

use eframe::egui;

/// A project in the list. `name` is the name the user chose; without it
/// the list shows the folder name.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Project {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Project {
    pub fn new(path: &str) -> Project {
        Project { path: path.to_string(), name: None }
    }

    /// The chosen name, or the folder name.
    pub fn title(&self) -> String {
        if let Some(n) = &self.name {
            return n.clone();
        }
        Path::new(&self.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| self.path.clone())
    }

    /// The path with the home folder as `~`.
    pub fn short_path(&self) -> String {
        match std::env::var("HOME") {
            Ok(home) if !home.is_empty() && self.path.starts_with(&home) => format!("~{}", &self.path[home.len()..]),
            _ => self.path.clone(),
        }
    }
}

/// An entry of the saved list. Older versions saved the paths only.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Entry {
    Path(String),
    Project(Project),
}

pub fn parse(text: &str) -> Vec<Project> {
    let entries: Vec<Entry> = serde_json::from_str(text).unwrap_or_default();
    entries
        .into_iter()
        .map(|e| match e {
            Entry::Path(p) => Project::new(&p),
            Entry::Project(p) => p,
        })
        .collect()
}

/// Adds a project at the end of the list. A project that is in the list
/// keeps its place, so its number does not change.
pub fn add(list: &mut Vec<Project>, path: &str) -> bool {
    if list.iter().any(|p| p.path == path) {
        return false;
    }
    list.push(Project::new(path));
    true
}

/// Moves the project at `from` so it sits before the project that was at
/// `to`. `to` may be the length of the list: the end.
pub fn move_to(list: &mut Vec<Project>, from: usize, to: usize) {
    if from >= list.len() {
        return;
    }
    let p = list.remove(from);
    let to = if to > from { to - 1 } else { to };
    list.insert(to.min(list.len()), p);
}

/// The icon files a project may have, best first.
const ICONS: [&str; 9] = [
    "public/favicon-32x32.png",
    "public/favicon.png",
    "public/icon.png",
    "public/apple-touch-icon.png",
    "public/favicon.ico",
    "app/assets/images/favicon.png",
    "app/assets/images/favicon.ico",
    "favicon.png",
    "favicon.ico",
];

/// The icon of a project, made small, as RGBA pixels.
pub fn find_icon(root: &Path) -> Option<egui::ColorImage> {
    ICONS.iter().map(|f| root.join(f)).filter(|p| p.is_file()).find_map(|p| load_icon(&p))
}

fn load_icon(path: &PathBuf) -> Option<egui::ColorImage> {
    let img = image::open(path).ok()?;
    let img = img.resize(48, 48, image::imageops::FilterType::Triangle).to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[Project]) -> Vec<&str> {
        list.iter().map(|p| p.path.as_str()).collect()
    }

    #[test]
    fn reads_the_old_list_of_paths_and_the_new_one() {
        let list = parse(r#"["/src/shop", {"path": "/src/blog", "name": "Blog"}]"#);
        assert_eq!(
            list,
            vec![Project::new("/src/shop"), Project { path: "/src/blog".into(), name: Some("Blog".into()) }]
        );
    }

    #[test]
    fn the_title_is_the_chosen_name_or_the_folder() {
        assert_eq!(Project::new("/src/shop").title(), "shop");
        assert_eq!(Project { path: "/src/shop".into(), name: Some("Shop app".into()) }.title(), "Shop app");
    }

    #[test]
    fn a_new_project_goes_last_and_a_known_one_stays() {
        let mut list = vec![Project::new("/a"), Project::new("/b")];
        assert!(!add(&mut list, "/b"));
        assert!(add(&mut list, "/c"));
        assert_eq!(paths(&list), ["/a", "/b", "/c"]);
    }

    #[test]
    fn moves_a_project_up_and_down() {
        let mut list = vec![Project::new("/a"), Project::new("/b"), Project::new("/c")];
        move_to(&mut list, 2, 0);
        assert_eq!(paths(&list), ["/c", "/a", "/b"]);
        move_to(&mut list, 0, 3);
        assert_eq!(paths(&list), ["/a", "/b", "/c"]);
        move_to(&mut list, 0, 1);
        assert_eq!(paths(&list), ["/a", "/b", "/c"]);
    }
}
