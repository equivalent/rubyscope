//! rubyscope: a desktop app for reading Ruby code.
//!
//!     rubyscope [FOLDER] [FILE[:LINE]]

use std::path::PathBuf;

use eframe::egui;

mod app;
mod external;
mod git;
mod highlight;
mod keys;
mod picker;
mod theme;
mod tree;
#[cfg(test)]
mod ui_tests;
mod view;
mod worker;

/// `path/to/file.rb:12` → the file and the 0-based line.
fn parse_file_arg(arg: &str) -> (PathBuf, Option<u32>) {
    if let Some((p, l)) = arg.rsplit_once(':')
        && let Ok(n) = l.parse::<u32>()
    {
        return (PathBuf::from(p), Some(n.saturating_sub(1)));
    }
    (PathBuf::from(arg), None)
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut root: Option<PathBuf> = None;
    let mut open: Option<(PathBuf, Option<u32>)> = None;
    for a in &args {
        let (p, line) = parse_file_arg(a);
        let p = p.canonicalize().unwrap_or(p);
        if p.is_dir() && root.is_none() {
            root = Some(p);
        } else if p.is_file() {
            // A file without a folder: use its git repo as the project.
            if root.is_none() {
                root = p
                    .ancestors()
                    .find(|d| d.join(".git").exists() || d.join("Gemfile").is_file())
                    .map(|d| d.to_path_buf());
            }
            open = Some((p, line));
        }
    }
    if root.is_none() && args.is_empty() {
        let cwd = std::env::current_dir().ok();
        root = cwd.filter(|d| d.join("Gemfile").is_file() || d.join(".git").exists());
    }

    // The window class. A test run can set its own, so window rules for it
    // do not touch normal rubyscope windows.
    let app_id = std::env::var("RUBYSCOPE_APP_ID").unwrap_or_else(|_| "rubyscope".into());
    // Wayland takes the icon from the desktop entry of the app id; X11 and
    // macOS take this one.
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../packaging/icon/rubyscope-256.png"))
        .expect("the app icon is a valid PNG");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("rubyscope")
            .with_app_id(app_id)
            .with_icon(icon)
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([600.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native("rubyscope", options, Box::new(move |cc| Ok(Box::new(app::App::new(&cc.egui_ctx, root, open)))))
}
