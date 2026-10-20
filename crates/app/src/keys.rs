//! Actions and their keys. The defaults are the short VS Code-style list;
//! `~/.config/rubyscope/keys.toml` can change any of them:
//!
//! ```toml
//! go_to_file = "cmd+p"
//! toggle_blame = "ctrl+b"
//! ```
//!
//! `cmd` is Cmd on macOS and Ctrl on Linux, so one file works on both.
//! `ctrl` is always the Ctrl key.

use eframe::egui::{self, Key, Modifiers};

/// On macOS the shortcuts use Cmd. Elsewhere they use Ctrl.
pub const MAC: bool = cfg!(target_os = "macos");

/// The name of the key that the shortcuts use: `Cmd` or `Ctrl`.
pub const CMD: &str = if MAC { "Cmd" } else { "Ctrl" };

/// True when the key that the shortcuts use is held: Cmd on macOS, Ctrl
/// elsewhere. Click-to-go uses it too.
pub fn cmd_held(m: Modifiers) -> bool {
    if MAC { m.mac_cmd } else { m.ctrl }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    OpenFolder,
    GoToFile,
    GoToSymbol,
    SearchProject,
    FindInFile,
    Commands,
    GoToLine,
    CloseTab,
    NextTab,
    PrevTab,
    ClosePopup,
    ToggleBlame,
    ToggleSidebar,
    Back,
    Forward,
    FindUsages,
    GoToRelated,
    TypeHierarchy,
    FileOutline,
    OpenInEditor,
    CopyPath,
    OpenOnGitHub,
    ToggleBottomPanel,
    Reindex,
}

impl Action {
    pub const ALL: &[Action] = &[
        Action::OpenFolder,
        Action::GoToFile,
        Action::GoToSymbol,
        Action::SearchProject,
        Action::FindInFile,
        Action::GoToLine,
        Action::Commands,
        Action::Back,
        Action::Forward,
        Action::FindUsages,
        Action::GoToRelated,
        Action::TypeHierarchy,
        Action::FileOutline,
        Action::ToggleBlame,
        Action::ToggleSidebar,
        Action::ToggleBottomPanel,
        Action::OpenInEditor,
        Action::OpenOnGitHub,
        Action::CopyPath,
        Action::NextTab,
        Action::PrevTab,
        Action::CloseTab,
        Action::Reindex,
        Action::ClosePopup,
    ];

    /// The name in `keys.toml`.
    pub fn id(self) -> &'static str {
        match self {
            Action::GoToFile => "go_to_file",
            Action::GoToSymbol => "go_to_symbol",
            Action::SearchProject => "search_project",
            Action::FindInFile => "find_in_file",
            Action::Commands => "commands",
            Action::GoToLine => "go_to_line",
            Action::CloseTab => "close_tab",
            Action::NextTab => "next_tab",
            Action::PrevTab => "prev_tab",
            Action::ClosePopup => "close_popup",
            Action::ToggleBlame => "toggle_blame",
            Action::ToggleSidebar => "toggle_sidebar",
            Action::Back => "back",
            Action::Forward => "forward",
            Action::FindUsages => "find_usages",
            Action::GoToRelated => "go_to_view",
            Action::OpenFolder => "open_folder",
            Action::TypeHierarchy => "type_hierarchy",
            Action::FileOutline => "file_outline",
            Action::OpenInEditor => "open_in_editor",
            Action::CopyPath => "copy_path",
            Action::OpenOnGitHub => "open_on_github",
            Action::ToggleBottomPanel => "toggle_bottom_panel",
            Action::Reindex => "reindex",
        }
    }

    /// The name in the command list.
    pub fn title(self) -> &'static str {
        match self {
            Action::GoToFile => "Go to file",
            Action::GoToSymbol => "Go to symbol (class, module, method, constant)",
            Action::SearchProject => "Search in all files",
            Action::FindInFile => "Find in this file",
            Action::Commands => "Show all commands",
            Action::GoToLine => "Go to line",
            Action::CloseTab => "Close tab",
            Action::NextTab => "Next tab",
            Action::PrevTab => "Previous tab",
            Action::ClosePopup => "Close popup",
            Action::ToggleBlame => "Show or hide git blame",
            Action::ToggleSidebar => "Show or hide the file tree",
            Action::Back => "Back",
            Action::Forward => "Forward",
            Action::FindUsages => "Find usages of the symbol at the cursor",
            Action::GoToRelated => "Go to the view of the action, or the action of the view",
            Action::OpenFolder => "Open a project folder",
            Action::TypeHierarchy => "Type hierarchy of the class at the cursor",
            Action::FileOutline => "Outline of this file",
            Action::OpenInEditor => "Open in editor (nvim) at this line",
            Action::CopyPath => "Copy path:line",
            Action::OpenOnGitHub => "Open this line on GitHub",
            Action::ToggleBottomPanel => "Show or hide the results panel",
            Action::Reindex => "Index the project again, parsing every file",
        }
    }

    fn default_key(self) -> Option<&'static str> {
        Some(match self {
            Action::OpenFolder => "cmd+o",
            Action::GoToFile => "cmd+p",
            Action::GoToSymbol => "cmd+t",
            Action::SearchProject => "cmd+shift+f",
            Action::FindInFile => "cmd+f",
            Action::Commands => "cmd+k",
            Action::GoToLine => "cmd+g",
            Action::CloseTab => "cmd+w",
            // Cmd+Tab switches apps on macOS, so the tab keys stay on Ctrl.
            Action::NextTab => "ctrl+tab",
            Action::PrevTab => "ctrl+shift+tab",
            Action::ClosePopup => "escape",
            Action::ToggleBlame => "cmd+b",
            Action::ToggleSidebar => "cmd+shift+b",
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl Shortcut {
    /// Parses `cmd+shift+f`, `escape`, `ctrl+tab`.
    pub fn parse(text: &str) -> Option<Shortcut> {
        Shortcut::parse_on(text, MAC)
    }

    fn parse_on(text: &str, mac: bool) -> Option<Shortcut> {
        let mut modifiers = Modifiers::NONE;
        let mut key = None;
        for part in text.split('+').map(|p| p.trim().to_lowercase()) {
            match part.as_str() {
                "ctrl" | "control" => modifiers.ctrl = true,
                "cmd" | "command" if mac => modifiers.mac_cmd = true,
                "cmd" | "command" => modifiers.ctrl = true,
                "shift" => modifiers.shift = true,
                "alt" => modifiers.alt = true,
                "esc" | "escape" => key = Some(Key::Escape),
                "tab" => key = Some(Key::Tab),
                "enter" | "return" => key = Some(Key::Enter),
                "space" => key = Some(Key::Space),
                other => {
                    let name = if other.len() == 1 { other.to_uppercase() } else { capitalize(other) };
                    key = Key::from_name(&name);
                }
            }
        }
        modifiers.command = if mac { modifiers.mac_cmd } else { modifiers.ctrl };
        Some(Shortcut { modifiers, key: key? })
    }

    /// `Ctrl+Shift+F`, or `Cmd+Shift+F` on macOS.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.modifiers.mac_cmd {
            parts.push("Cmd".to_string());
        }
        if self.modifiers.shift {
            parts.push("Shift".to_string());
        }
        if self.modifiers.alt {
            parts.push("Alt".to_string());
        }
        parts.push(match self.key {
            Key::Escape => "Esc".to_string(),
            k => k.name().to_string(),
        });
        parts.join("+")
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[derive(Debug, Clone)]
pub struct Keymap {
    pub bindings: Vec<(Action, Shortcut)>,
}

impl Keymap {
    pub fn load() -> Keymap {
        let path = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config/rubyscope/keys.toml"));
        let custom: toml::Table =
            path.and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| t.parse().ok()).unwrap_or_default();
        Keymap::from_table(&custom)
    }

    pub fn from_table(custom: &toml::Table) -> Keymap {
        let mut bindings = Vec::new();
        for action in Action::ALL {
            let text = custom.get(action.id()).and_then(|v| v.as_str()).or(action.default_key());
            if let Some(s) = text.and_then(Shortcut::parse) {
                bindings.push((*action, s));
            }
        }
        // Check bindings with more modifiers first, so Ctrl+Shift+F is not
        // taken by Ctrl+F.
        bindings.sort_by_key(|(_, s)| std::cmp::Reverse(s.modifiers.shift as u8 + s.modifiers.alt as u8));
        Keymap { bindings }
    }

    pub fn shortcut(&self, action: Action) -> Option<Shortcut> {
        self.bindings.iter().find(|(a, _)| *a == action).map(|(_, s)| *s)
    }

    /// Takes the pressed shortcuts out of the input and returns their actions.
    pub fn pressed(&self, ctx: &egui::Context) -> Vec<Action> {
        let mut out = Vec::new();
        ctx.input_mut(|input| {
            for (action, s) in &self.bindings {
                let mut hit = false;
                input.events.retain(|e| {
                    let egui::Event::Key { key, pressed: true, modifiers, .. } = e else { return true };
                    let same = *key == s.key
                        && modifiers.ctrl == s.modifiers.ctrl
                        && modifiers.mac_cmd == s.modifiers.mac_cmd
                        && modifiers.shift == s.modifiers.shift
                        && modifiers.alt == s.modifiers.alt;
                    if same {
                        hit = true;
                    }
                    !same
                });
                if hit {
                    out.push(*action);
                }
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shortcuts() {
        let s = Shortcut::parse_on("cmd+shift+f", false).unwrap();
        assert!(s.modifiers.ctrl && s.modifiers.shift);
        assert_eq!(s.key, Key::F);
        assert_eq!(s.label(), "Ctrl+Shift+F");
        assert_eq!(Shortcut::parse("escape").unwrap().key, Key::Escape);
        assert_eq!(Shortcut::parse("ctrl+tab").unwrap().key, Key::Tab);
    }

    #[test]
    fn cmd_is_the_command_key_on_macos() {
        let s = Shortcut::parse_on("cmd+p", true).unwrap();
        assert!(s.modifiers.mac_cmd && !s.modifiers.ctrl);
        assert_eq!(s.label(), "Cmd+P");
        let tab = Shortcut::parse_on("ctrl+tab", true).unwrap();
        assert!(tab.modifiers.ctrl && !tab.modifiers.mac_cmd);
    }

    #[test]
    fn custom_keys_replace_defaults() {
        let table: toml::Table = "toggle_blame = \"ctrl+alt+b\"".parse().unwrap();
        let k = Keymap::from_table(&table);
        assert!(k.shortcut(Action::ToggleBlame).unwrap().modifiers.alt);
        assert_eq!(k.shortcut(Action::GoToFile).unwrap().label(), format!("{CMD}+P"));
    }
}
