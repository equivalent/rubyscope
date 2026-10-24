//! Opening things outside Rubyscope: the editor, the browser.

use std::path::Path;
use std::process::{Command, Stdio};

fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(cmd).is_file()))
}

fn spawn(cmd: &mut Command) {
    let _ = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// Opens a file at a 1-based line in the editor: Omarchy's default editor
/// when it is there, else nvim in a terminal. On macOS, nvim in a new
/// Terminal window.
pub fn open_in_editor(path: &Path, line: u32) {
    let line_arg = format!("+{line}");
    if cfg!(target_os = "macos") {
        let script = format!(
            "tell application \"Terminal\"\n  activate\n  do script \"{}\"\nend tell",
            applescript_quote(&format!("nvim {line_arg} {}", shell_quote(&path.to_string_lossy())))
        );
        spawn(Command::new("osascript").arg("-e").arg(script));
    } else if on_path("omarchy-launch-editor") {
        spawn(Command::new("omarchy-launch-editor").arg(&line_arg).arg(path));
    } else if on_path("xdg-terminal-exec") {
        spawn(Command::new("xdg-terminal-exec").arg("nvim").arg(&line_arg).arg(path));
    } else {
        spawn(Command::new("nvim").arg(&line_arg).arg(path));
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
/// `it's` → `'it'\''s'`, for a shell command line.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
/// Escapes text for an AppleScript string.
fn applescript_quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// An app started from Finder or the Dock on macOS gets only the system
/// PATH, without Homebrew, rbenv or mise. Takes the PATH from the user's
/// login shell, so `ruby` is the project's Ruby and `git` is the same as in
/// the terminal.
pub fn use_login_shell_path() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let Ok(out) =
        Command::new(shell).args(["-lic", "printf '\\n@@PATH@@%s@@\\n' \"$PATH\""]).stdin(Stdio::null()).output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    if let Some(path) = text.split("@@PATH@@").nth(1).and_then(|r| r.split("@@").next())
        && !path.is_empty()
    {
        // SAFETY: this runs first in `main`, before any other thread starts.
        unsafe { std::env::set_var("PATH", path) };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_for_terminal() {
        assert_eq!(shell_quote("/a/it's.rb"), "'/a/it'\\''s.rb'");
        assert_eq!(applescript_quote(r#"nvim "a\b""#), r#"nvim \"a\\b\""#);
    }
}
