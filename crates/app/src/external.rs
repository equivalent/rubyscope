//! Opening things outside rubyscope: the editor, the browser.

use std::path::Path;
use std::process::{Command, Stdio};

fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(cmd).is_file()))
}

fn spawn(cmd: &mut Command) {
    let _ = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// Opens a file at a 1-based line in the editor: Omarchy's default editor
/// when it is there, else nvim in a terminal.
pub fn open_in_editor(path: &Path, line: u32) {
    let line_arg = format!("+{line}");
    if on_path("omarchy-launch-editor") {
        spawn(Command::new("omarchy-launch-editor").arg(&line_arg).arg(path));
    } else if on_path("xdg-terminal-exec") {
        spawn(Command::new("xdg-terminal-exec").arg("nvim").arg(&line_arg).arg(path));
    } else {
        spawn(Command::new("nvim").arg(&line_arg).arg(path));
    }
}

pub fn open_url(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    spawn(Command::new(opener).arg(url));
}

/// The commit `HEAD` points to, for GitHub links to a line.
pub fn head_sha(root: &Path) -> Option<String> {
    let out = Command::new("git").current_dir(root).args(["rev-parse", "HEAD"]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
