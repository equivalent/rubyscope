//! Colors from the current Omarchy theme
//! (`~/.local/state/omarchy/current/theme/colors.toml`), with Tokyo Night
//! as the default when there is none.

use std::path::PathBuf;

use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Token {
    Plain,
    Comment,
    String,
    Number,
    Symbol,
    Keyword,
    Constant,
    Method,
    Ivar,
    Param,
    Tag,
    Attribute,
    Punctuation,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub dark: bool,
    pub bg: Color32,
    pub bg_dark: Color32,
    pub bg_light: Color32,
    pub fg: Color32,
    pub fg_dim: Color32,
    pub fg_bright: Color32,
    pub accent: Color32,
    pub selection: Color32,
    pub muted: Color32,
    pub red: Color32,
    pub yellow: Color32,
    pub orange: Color32,
    pub green: Color32,
    pub cyan: Color32,
    pub blue: Color32,
    pub magenta: Color32,
}

fn hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_matches('"').trim_start_matches('#');
    if s.len() < 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some(Color32::from_rgb(p(0)?, p(2)?, p(4)?))
}

impl Default for Theme {
    fn default() -> Self {
        // Tokyo Night, the Omarchy default.
        let c = |s| hex(s).expect("valid color");
        Theme {
            dark: true,
            bg: c("#1a1b26"),
            bg_dark: c("#13141c"),
            bg_light: c("#24283b"),
            fg: c("#a9b1d6"),
            fg_dim: c("#565f89"),
            fg_bright: c("#c0caf5"),
            accent: c("#7aa2f7"),
            selection: c("#292e42"),
            muted: c("#414868"),
            red: c("#f7768e"),
            yellow: c("#e0af68"),
            orange: c("#eb927b"),
            green: c("#9ece6a"),
            cyan: c("#449dab"),
            blue: c("#7aa2f7"),
            magenta: c("#bb9af7"),
        }
    }
}

pub fn omarchy_colors_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home).join(".local/state/omarchy/current/theme/colors.toml");
    p.is_file().then_some(p)
}

impl Theme {
    /// The Omarchy theme, or the default.
    pub fn load() -> Theme {
        omarchy_colors_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| Theme::from_toml(&t))
            .unwrap_or_default()
    }

    /// Reads an Omarchy `colors.toml`. Missing keys keep the default.
    pub fn from_toml(text: &str) -> Theme {
        let table: toml::Table = text.parse().unwrap_or_default();
        let get = |k: &str| table.get(k).and_then(|v| v.as_str()).and_then(hex);
        let mut t = Theme { dark: table.get("mode").and_then(|v| v.as_str()) != Some("light"), ..Theme::default() };
        let set = |slot: &mut Color32, keys: &[&str]| {
            if let Some(c) = keys.iter().find_map(|k| get(k)) {
                *slot = c;
            }
        };
        set(&mut t.bg, &["background"]);
        set(&mut t.bg_dark, &["dark_background", "darker_background"]);
        set(&mut t.bg_light, &["lighter_background", "selection"]);
        set(&mut t.fg, &["foreground"]);
        set(&mut t.fg_dim, &["dark_foreground", "muted"]);
        set(&mut t.fg_bright, &["bright_foreground", "light_foreground", "foreground"]);
        set(&mut t.accent, &["accent", "blue"]);
        set(&mut t.selection, &["selection", "lighter_background"]);
        set(&mut t.muted, &["muted", "dark_foreground"]);
        set(&mut t.red, &["red"]);
        set(&mut t.yellow, &["yellow"]);
        set(&mut t.orange, &["orange", "bright_yellow", "yellow"]);
        set(&mut t.green, &["green"]);
        set(&mut t.cyan, &["cyan", "bright_cyan"]);
        set(&mut t.blue, &["blue", "accent"]);
        set(&mut t.magenta, &["bright_magenta", "magenta"]);
        t
    }

    pub fn token(&self, token: Token) -> Color32 {
        match token {
            Token::Plain => self.fg,
            Token::Comment => self.fg_dim,
            Token::String => self.green,
            Token::Number => self.orange,
            Token::Symbol => self.cyan,
            Token::Keyword => self.magenta,
            Token::Constant => self.yellow,
            Token::Method => self.blue,
            Token::Ivar => self.red,
            Token::Param => self.orange,
            Token::Tag => self.red,
            Token::Attribute => self.yellow,
            Token::Punctuation => self.fg_dim,
        }
    }

    /// The color for a blame block: newer changes are stronger.
    pub fn blame_age(&self, age_days: f32) -> Color32 {
        let t = (1.0 - (age_days / 365.0).min(1.0)).powf(1.5);
        let base = self.bg;
        let hot = self.accent;
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t * 0.16) as u8;
        Color32::from_rgb(mix(base.r(), hot.r()), mix(base.g(), hot.g()), mix(base.b(), hot.b()))
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = if self.dark { Visuals::dark() } else { Visuals::light() };
        v.panel_fill = self.bg_dark;
        v.window_fill = self.bg_light;
        v.extreme_bg_color = self.bg;
        v.faint_bg_color = self.bg_light;
        v.code_bg_color = self.bg;
        v.override_text_color = Some(self.fg);
        v.hyperlink_color = self.accent;
        v.selection.bg_fill = self.selection;
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.window_stroke = Stroke::new(1.0, self.muted);
        v.window_corner_radius = CornerRadius::same(6);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.muted);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.fg);
        v.widgets.inactive.weak_bg_fill = self.bg_light;
        v.widgets.inactive.bg_fill = self.bg_light;
        v.widgets.hovered.weak_bg_fill = self.selection;
        v.widgets.hovered.bg_fill = self.selection;
        v.widgets.active.weak_bg_fill = self.muted;
        ctx.set_visuals(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_omarchy_colors() {
        let t = Theme::from_toml("mode = \"light\"\nbackground = \"#ffffff\"\naccent = \"#ff0000\"\n");
        assert!(!t.dark);
        assert_eq!(t.bg, Color32::from_rgb(255, 255, 255));
        assert_eq!(t.accent, Color32::from_rgb(255, 0, 0));
        // Keys that are missing keep the default.
        assert_eq!(t.green, Theme::default().green);
    }
}
