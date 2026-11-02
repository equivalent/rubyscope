//! Small language icons for the tabs. Each is painted by hand, so the app
//! needs no image files: a red gem for Ruby, a colored badge with letters
//! for most other languages, and a plain page for anything else.

use std::path::Path;

use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};

/// The kind of file a tab shows, as far as its icon cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Ruby,
    Erb,
    JavaScript,
    TypeScript,
    Json,
    Html,
    Css,
    Yaml,
    Markdown,
    Rust,
    Python,
    Shell,
    Sql,
    Text,
}

impl Lang {
    pub fn of(path: &Path) -> Lang {
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        match name.as_str() {
            "gemfile" | "rakefile" | "guardfile" | "capfile" | "brewfile" | "gemfile.lock" | ".irbrc" | ".pryrc" => {
                return Lang::Ruby;
            }
            _ => {}
        }
        if name.ends_with(".erb") {
            return Lang::Erb;
        }
        let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        match ext {
            "rb" | "rake" | "gemspec" | "ru" | "rbs" | "jbuilder" | "builder" | "thor" => Lang::Ruby,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "ts" | "mts" | "cts" | "tsx" => Lang::TypeScript,
            "json" => Lang::Json,
            "html" | "htm" | "haml" | "slim" => Lang::Html,
            "css" | "scss" | "sass" | "less" => Lang::Css,
            "yml" | "yaml" => Lang::Yaml,
            "md" | "markdown" => Lang::Markdown,
            "rs" => Lang::Rust,
            "py" => Lang::Python,
            "sh" | "bash" | "zsh" | "fish" => Lang::Shell,
            "sql" => Lang::Sql,
            _ => Lang::Text,
        }
    }
}

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Paints the icon for `lang` into the square `rect`. `dim` colors the
/// plain page icon, which follows the theme.
pub fn paint(painter: &egui::Painter, rect: Rect, lang: Lang, dim: Color32) {
    match lang {
        Lang::Ruby => gem(painter, rect, rgb(0xcc342d)),
        Lang::Erb => {
            gem(painter, rect, rgb(0xcc342d));
            // A small orange tag in the corner marks a template.
            let tag = Rect::from_min_size(rect.right_bottom() - Vec2::splat(6.0), Vec2::splat(6.0));
            painter.rect_filled(tag, CornerRadius::same(1), rgb(0xe34f26));
        }
        Lang::JavaScript => badge(painter, rect, rgb(0xf7df1e), Color32::BLACK, "JS"),
        Lang::TypeScript => badge(painter, rect, rgb(0x3178c6), Color32::WHITE, "TS"),
        Lang::Json => badge(painter, rect, rgb(0xcbcb41), Color32::BLACK, "{}"),
        Lang::Html => badge(painter, rect, rgb(0xe34f26), Color32::WHITE, "<>"),
        Lang::Css => badge(painter, rect, rgb(0x1572b6), Color32::WHITE, "#"),
        Lang::Yaml => badge(painter, rect, rgb(0xcb171e), Color32::WHITE, "Y"),
        Lang::Markdown => badge(painter, rect, rgb(0x519aba), Color32::WHITE, "M"),
        Lang::Rust => badge(painter, rect, rgb(0xdea584), Color32::BLACK, "R"),
        Lang::Python => badge(painter, rect, rgb(0x3776ab), rgb(0xffd43b), "Py"),
        Lang::Shell => badge(painter, rect, rgb(0x4eaa25), Color32::WHITE, "$"),
        Lang::Sql => badge(painter, rect, rgb(0xe38c00), Color32::WHITE, "DB"),
        Lang::Text => page(painter, rect, dim),
    }
}

/// A cut gem, seen from the side: a flat top, angled shoulders and a point.
fn gem(painter: &egui::Painter, rect: Rect, color: Color32) {
    let r = rect.shrink(1.0);
    let (l, t, w, h) = (r.left(), r.top(), r.width(), r.height());
    let p = |x: f32, y: f32| Pos2::new(l + x * w, t + y * h);
    let outline = vec![p(0.25, 0.1), p(0.75, 0.1), p(1.0, 0.4), p(0.5, 0.95), p(0.0, 0.4)];
    painter.add(Shape::convex_polygon(outline, color, Stroke::NONE));
    // Light facet lines make it read as a gem, not a blob.
    let facet = Stroke::new(1.0, Color32::from_white_alpha(110));
    painter.line_segment([p(0.0, 0.4), p(1.0, 0.4)], facet);
    painter.line_segment([p(0.3, 0.4), p(0.5, 0.95)], facet);
    painter.line_segment([p(0.7, 0.4), p(0.5, 0.95)], facet);
}

fn badge(painter: &egui::Painter, rect: Rect, bg: Color32, fg: Color32, text: &str) {
    let r = rect.shrink(0.5);
    painter.rect_filled(r, CornerRadius::same(2), bg);
    let size = if text.chars().count() > 1 { 7.5 } else { 9.5 };
    painter.text(r.center() + Vec2::new(0.0, 0.5), egui::Align2::CENTER_CENTER, text, FontId::monospace(size), fg);
}

/// A sheet of paper with a folded corner and two lines of text.
fn page(painter: &egui::Painter, rect: Rect, color: Color32) {
    let r = Rect::from_center_size(rect.center(), Vec2::new(rect.width() * 0.7, rect.height() * 0.9));
    let fold = r.width() * 0.35;
    let stroke = Stroke::new(1.0, color);
    let pts = vec![
        r.left_top(),
        Pos2::new(r.right() - fold, r.top()),
        Pos2::new(r.right(), r.top() + fold),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.add(Shape::line(pts, stroke));
    painter.line_segment([Pos2::new(r.right() - fold, r.top()), Pos2::new(r.right() - fold, r.top() + fold)], stroke);
    painter.line_segment([Pos2::new(r.right() - fold, r.top() + fold), Pos2::new(r.right(), r.top() + fold)], stroke);
    for y in [0.55, 0.75] {
        let y = r.top() + r.height() * y;
        painter.line_segment([Pos2::new(r.left() + 2.0, y), Pos2::new(r.right() - 2.0, y)], stroke);
    }
}

/// The ✕ that takes the icon's place while the pointer is over the tab.
pub fn paint_close(painter: &egui::Painter, rect: Rect, color: Color32, bg: Option<Color32>) {
    if let Some(bg) = bg {
        painter.rect(rect, CornerRadius::same(3), bg, Stroke::NONE, StrokeKind::Inside);
    }
    let r = rect.shrink(4.0);
    let stroke = Stroke::new(1.4, color);
    painter.line_segment([r.left_top(), r.right_bottom()], stroke);
    painter.line_segment([r.right_top(), r.left_bottom()], stroke);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_language_from_the_name() {
        let of = |p: &str| Lang::of(Path::new(p));
        assert_eq!(of("app/models/order.rb"), Lang::Ruby);
        assert_eq!(of("Gemfile"), Lang::Ruby);
        assert_eq!(of("lib/tasks/orders.rake"), Lang::Ruby);
        assert_eq!(of("app/views/orders/show.html.erb"), Lang::Erb);
        assert_eq!(of("app/javascript/controllers/cart_controller.js"), Lang::JavaScript);
        assert_eq!(of("web/app.tsx"), Lang::TypeScript);
        assert_eq!(of("config/database.yml"), Lang::Yaml);
        assert_eq!(of("README.md"), Lang::Markdown);
        assert_eq!(of("LICENSE"), Lang::Text);
        assert_eq!(of("notes.txt"), Lang::Text);
    }
}
