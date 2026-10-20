//! Window tests: a small Rails app on disk, driven with real key presses
//! and pointer events through egui_kittest, rendered with wgpu.
//!
//! Set `RUBYSCOPE_SHOTS=/some/dir` to save a screenshot of each step.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use rubyscope_index::{BuildOptions, Index};

use crate::app::{App, Bottom};

/// The key that the shortcuts use: Cmd on macOS, Ctrl elsewhere.
const CMD: Modifiers = if crate::keys::MAC { Modifiers::MAC_CMD } else { Modifiers::CTRL };

const FILES: &[(&str, &str)] = &[
    ("Gemfile", "source \"https://rubygems.org\"\n"),
    (
        "app/models/invoice.rb",
        "# An invoice that an entity sends.\nclass Invoice < ApplicationRecord\n  belongs_to :entity\n\n  # Marks the invoice as paid.\n  def pay!\n    update!(paid: true)\n  end\nend\n",
    ),
    ("app/models/entity.rb", "class Entity < ApplicationRecord\n  has_many :invoices\nend\n"),
    (
        "app/models/application_record.rb",
        "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
    ),
    (
        "app/services/billing.rb",
        "class Billing\n  def run\n    invoice = Invoice.find(1)\n    invoice.pay!\n    invoice.entity\n  end\nend\n",
    ),
];

/// Writes the app to a new temp folder, with one git commit.
fn fixture() -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("rubyscope-ui-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, code) in FILES {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, code).unwrap();
    }
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(["-c", "user.name=Test Author", "-c", "user.email=test@example.com"])
            .args(args)
            .output()
            .unwrap();
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Add billing"]);
    dir.canonicalize().unwrap()
}

fn harness(root: &Path) -> Harness<'static, App> {
    let index = Arc::new(Index::build(root, &BuildOptions { gems: false, stdlib: false }).unwrap());
    let root = root.to_path_buf();
    let mut h = Harness::builder().with_size([1300.0, 820.0]).wgpu().build_eframe(move |cc| {
        let mut app = App::new(&cc.egui_ctx, None, None);
        app.open_prebuilt(root.clone(), index.clone());
        app
    });
    h.run_ok();
    h
}

/// Runs frames until background jobs (coloring, blame) have reported.
fn settle(h: &mut Harness<'static, App>) {
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(40));
        let _ = h.run_ok();
    }
}

fn shot(h: &mut Harness<'static, App>, name: &str) {
    let Some(dir) = std::env::var_os("RUBYSCOPE_SHOTS") else { return };
    let img = h.render().expect("render");
    std::fs::create_dir_all(&dir).unwrap();
    img.save(PathBuf::from(dir).join(format!("{name}.png"))).unwrap();
}

fn type_text(h: &mut Harness<'static, App>, text: &str) {
    h.event(Event::Text(text.to_string()));
    let _ = h.run_ok();
}

fn press(h: &mut Harness<'static, App>, mods: Modifiers, key: Key) {
    h.key_press_modifiers(mods, key);
    let _ = h.run_ok();
}

/// The screen point of a 1-based line and column in the current tab.
fn point(h: &Harness<'static, App>, line: usize, col: usize) -> Pos2 {
    let t = h.state().current().expect("a tab");
    let (origin, row_h, char_w, first) = t.layout.expect("drawn");
    Pos2::new(origin.x + (col as f32 - 0.5) * char_w, origin.y + (line - 1 - first) as f32 * row_h + row_h * 0.35)
}

fn ctrl_click(h: &mut Harness<'static, App>, pos: Pos2) {
    let mods = CMD;
    h.event(Event::ModifiersChanged(mods));
    h.event(Event::PointerMoved(pos));
    let _ = h.run_ok();
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: mods });
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: mods });
    let _ = h.run_ok();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    let _ = h.run_ok();
}

fn open(h: &mut Harness<'static, App>, root: &Path, rel: &str) {
    h.state_mut().open_path(&root.join(rel), None, Some(0), true);
    settle(h);
}

fn current_rel(h: &Harness<'static, App>) -> String {
    h.state().current().map(|t| t.rel.clone()).unwrap_or_default()
}

fn cursor_line(h: &Harness<'static, App>) -> u32 {
    let t = h.state().current().unwrap();
    t.line_of(t.cursor.unwrap()) + 1
}

#[test]
fn go_to_file_with_ctrl_p() {
    let root = fixture();
    let mut h = harness(&root);
    shot(&mut h, "01-empty");
    press(&mut h, CMD, Key::P);
    type_text(&mut h, "billing");
    shot(&mut h, "02-ctrl-p");
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/services/billing.rb");
    shot(&mut h, "03-billing-open");
}

#[test]
fn ctrl_click_goes_to_definitions_and_back_returns() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/services/billing.rb");
    // `Invoice` on line 3, column 15.
    let p = point(&h, 3, 16);
    ctrl_click(&mut h, p);
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/models/invoice.rb");
    assert_eq!(cursor_line(&h), 2);
    shot(&mut h, "04-after-ctrl-click-class");
    // Back to billing, then `pay!` on line 4: a method on a typed local.
    press(&mut h, Modifiers::NONE, Key::Escape);
    let ctx = h.ctx.clone();
    h.state_mut().run_action(crate::keys::Action::Back, &ctx);
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/services/billing.rb");
    let p = point(&h, 4, 14);
    ctrl_click(&mut h, p);
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/models/invoice.rb");
    assert_eq!(cursor_line(&h), 6, "pay! is on line 6");
}

#[test]
fn hover_shows_the_docs() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/services/billing.rb");
    let p = point(&h, 4, 14);
    h.event(Event::PointerMoved(p));
    for _ in 0..15 {
        std::thread::sleep(Duration::from_millis(60));
        h.event(Event::PointerMoved(p));
        let _ = h.run_ok();
    }
    shot(&mut h, "05-hover");
    h.get_by_label_contains("Marks the invoice as paid.");
}

#[test]
fn find_usages_fills_the_results_panel() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/invoice.rb");
    let t = h.state_mut().tabs.iter_mut().find(|t| t.rel.ends_with("invoice.rb")).unwrap();
    // Cursor on `pay!` (line 6).
    t.cursor = Some(t.line_start(5) + 6);
    let ctx = h.ctx.clone();
    h.state_mut().run_action(crate::keys::Action::FindUsages, &ctx);
    settle(&mut h);
    match &h.state().bottom {
        Some(Bottom::Results { rows, title }) => {
            assert_eq!(rows.len(), 1, "{title}");
            assert_eq!(rows[0].rel, "app/services/billing.rb");
        }
        _ => panic!("no results panel"),
    }
    shot(&mut h, "06-usages");
}

#[test]
fn blame_panel_and_command_list() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/invoice.rb");
    press(&mut h, CMD, Key::B);
    settle(&mut h);
    let t = h.state().current().unwrap();
    let blame = t.blame.as_ref().expect("blame loaded");
    assert_eq!(blame.commits[0].author, "Test Author");
    shot(&mut h, "07-blame");
    press(&mut h, CMD, Key::K);
    type_text(&mut h, "usages");
    shot(&mut h, "08-commands");
    h.get_by_label_contains("Find usages");
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(h.state().picker.is_none());
    // Ctrl+Shift+B hides the side bar.
    press(&mut h, CMD | Modifiers::SHIFT, Key::B);
    assert!(!h.state().show_sidebar);
}

#[test]
fn go_to_symbol_and_line() {
    let root = fixture();
    let mut h = harness(&root);
    press(&mut h, CMD, Key::T);
    type_text(&mut h, "Invoice#pay");
    shot(&mut h, "09-ctrl-t");
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/models/invoice.rb");
    assert_eq!(cursor_line(&h), 6);
    press(&mut h, CMD, Key::G);
    type_text(&mut h, "3");
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    assert_eq!(cursor_line(&h), 3);
    // Ctrl+W closes the tab.
    press(&mut h, CMD, Key::W);
    assert!(h.state().tabs.is_empty());
}

#[test]
fn find_in_file() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/services/billing.rb");
    press(&mut h, CMD, Key::F);
    type_text(&mut h, "invoice");
    shot(&mut h, "10-find");
    let t = h.state().current().unwrap();
    assert_eq!(t.find.matches.len(), 4, "{:?}", t.find.matches);
}

/// A real project: run with `RUBYSCOPE_PROJECT=~/code/shop`. It opens a
/// big file, goes to `ApplicationRecord` with Ctrl+T, shows blame and finds
/// usages.
#[test]
fn real_project_smoke() {
    let Some(root) = std::env::var_os("RUBYSCOPE_PROJECT").map(PathBuf::from) else { return };
    let started = std::time::Instant::now();
    let index = Arc::new(Index::build(&root, &BuildOptions::default()).unwrap());
    eprintln!("index: {:?}", started.elapsed());
    let r = root.clone();
    let mut h = Harness::builder().with_size([1500.0, 950.0]).wgpu().build_eframe(move |cc| {
        let mut app = App::new(&cc.egui_ctx, None, None);
        app.open_prebuilt(r.clone(), index.clone());
        app
    });
    h.run_ok();
    // A big file.
    let t = std::time::Instant::now();
    open(&mut h, &root, "db/schema.rb");
    eprintln!("open db/schema.rb: {:?}", t.elapsed());
    shot(&mut h, "21-project-schema");
    // Ctrl+T over every symbol of the project and its gems.
    let t = std::time::Instant::now();
    press(&mut h, CMD, Key::T);
    type_text(&mut h, "ApplicationRecord");
    eprintln!("ctrl+t: {:?}", t.elapsed());
    shot(&mut h, "22-project-ctrl-t");
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/models/application_record.rb");
    press(&mut h, CMD, Key::B);
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(100));
        let _ = h.run_ok();
        if h.state().current().is_some_and(|t| t.blame.is_some()) {
            break;
        }
    }
    shot(&mut h, "23-project-blame");
    let ctx = h.ctx.clone();
    h.state_mut().run_action(crate::keys::Action::FindUsages, &ctx);
    settle(&mut h);
    shot(&mut h, "24-project-usages");
}

#[test]
fn right_click_menu_and_choose_list() {
    let root = fixture();
    // Two classes with a method of the same name: a call on an unknown
    // receiver gives a list to choose from.
    std::fs::write(root.join("app/models/user.rb"), "class User\n  def pay!; end\nend\n").unwrap();
    std::fs::write(root.join("app/services/any.rb"), "class Any\n  def run(x)\n    x.pay!\n  end\nend\n").unwrap();
    let mut h = harness(&root);
    open(&mut h, &root, "app/services/any.rb");
    let p = point(&h, 3, 8);
    ctrl_click(&mut h, p);
    settle(&mut h);
    shot(&mut h, "11-choose");
    h.get_by_label_contains("Choose a definition of pay!");
    h.get_by_label("Invoice#pay!").click();
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/models/invoice.rb");

    // Right-click on `entity` in billing.rb, then "Find usages".
    open(&mut h, &root, "app/models/invoice.rb");
    let p = point(&h, 6, 8);
    h.event(Event::PointerMoved(p));
    h.event(Event::PointerButton {
        pos: p,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.event(Event::PointerButton {
        pos: p,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    let _ = h.run_ok();
    shot(&mut h, "12-context-menu");
    h.get_by_label("Find usages").click();
    settle(&mut h);
    match &h.state().bottom {
        Some(Bottom::Results { rows, .. }) => assert!(rows.iter().any(|r| r.rel == "app/services/billing.rb")),
        _ => panic!("no usages"),
    }
}

#[test]
fn changed_files_are_indexed_again() {
    let root = fixture();
    let mut h = harness(&root);
    // Use the real background worker and file watcher.
    h.state_mut().open_project(root.clone());
    // Wait up to 30 s: when all tests run at once, the machine is busy and indexing is slow.
    for _ in 0..600 {
        std::thread::sleep(Duration::from_millis(50));
        let _ = h.run_ok();
        if h.state().index.as_ref().is_some_and(|i| i.consts.contains_key("Billing")) {
            break;
        }
    }
    open(&mut h, &root, "app/services/billing.rb");
    std::fs::write(root.join("app/services/refund.rb"), "class Refund\nend\n").unwrap();
    let mut found = false;
    for _ in 0..600 {
        std::thread::sleep(Duration::from_millis(50));
        let _ = h.run_ok();
        if h.state().index.as_ref().is_some_and(|i| i.consts.contains_key("Refund")) {
            found = true;
            break;
        }
    }
    assert!(found, "the new class was not indexed");
}

#[test]
fn call_hierarchy_goes_up_one_level() {
    let root = fixture();
    std::fs::write(
        root.join("app/services/nightly.rb"),
        "class Nightly\n  def perform\n    Billing.new.run\n  end\nend\n",
    )
    .unwrap();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/invoice.rb");
    let t = h.state_mut().tabs.iter_mut().find(|t| t.rel.ends_with("invoice.rb")).unwrap();
    t.cursor = Some(t.line_start(5) + 6);
    let ctx = h.ctx.clone();
    h.state_mut().run_action(crate::keys::Action::FindUsages, &ctx);
    settle(&mut h);
    // `pay!` is called in Billing#run; its callers button shows Nightly#perform.
    h.get_by_label("callers").click();
    settle(&mut h);
    shot(&mut h, "13-callers");
    match &h.state().bottom {
        Some(Bottom::Results { title, rows }) => {
            assert!(title.starts_with("Callers of Billing#run"), "{title}");
            assert_eq!(rows[0].rel, "app/services/nightly.rb");
        }
        _ => panic!("no callers"),
    }
}

fn right_click(h: &mut Harness<'static, App>, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    let _ = h.run_ok();
}

#[test]
fn keys_scroll_the_code() {
    let root = fixture();
    let long: String = (1..=400).map(|n| format!("# line {n}\n")).collect();
    std::fs::write(root.join("app/models/long.rb"), long).unwrap();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/long.rb");
    let y = |h: &Harness<'static, App>| h.state().current().unwrap().scroll.y;
    assert_eq!(y(&h), 0.0);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    let _ = h.run_ok();
    let one = y(&h);
    assert!(one > 0.0, "the down arrow did not scroll");
    press(&mut h, Modifiers::NONE, Key::J);
    let _ = h.run_ok();
    assert!(y(&h) > one);
    // Shift+G goes to the end, gg to the top, as in Vim.
    press(&mut h, Modifiers::SHIFT, Key::G);
    let _ = h.run_ok();
    let end = y(&h);
    assert!(end > 5000.0, "{end}");
    press(&mut h, Modifiers::NONE, Key::G);
    assert_eq!(y(&h), end, "one g must not scroll");
    // The harness moves the clock on by more than a second per run, so
    // both presses of `gg` go in one frame.
    h.key_press(Key::G);
    h.key_press(Key::G);
    let _ = h.run_ok();
    assert_eq!(y(&h), 0.0);
    // `120g` goes to line 120: it is on screen, and the cursor is on it.
    for key in [Key::Num1, Key::Num2, Key::Num0, Key::G] {
        h.key_press(key);
    }
    let _ = h.run_ok();
    let _ = h.run_ok();
    assert_eq!(cursor_line(&h), 120);
    let first = h.state().current().unwrap().top_line;
    assert!(first > 80 && first < 120, "{first}");
    press(&mut h, Modifiers::NONE, Key::Home);
    press(&mut h, Modifiers::NONE, Key::PageDown);
    let _ = h.run_ok();
    assert!(y(&h) > 300.0);
    press(&mut h, Modifiers::NONE, Key::Home);
    let _ = h.run_ok();
    assert_eq!(y(&h), 0.0);
    // Keys typed in a text field do not scroll.
    press(&mut h, CMD, Key::F);
    type_text(&mut h, "j");
    press(&mut h, Modifiers::NONE, Key::J);
    assert_eq!(y(&h), 0.0);
}

#[test]
fn controller_action_goes_to_its_view_and_back() {
    let root = fixture();
    std::fs::create_dir_all(root.join("app/controllers")).unwrap();
    std::fs::create_dir_all(root.join("app/views/invoices")).unwrap();
    std::fs::write(
        root.join("app/controllers/invoices_controller.rb"),
        "class InvoicesController < ApplicationController\n  def edit\n    @invoice = Invoice.find(1)\n  end\nend\n",
    )
    .unwrap();
    std::fs::write(root.join("app/views/invoices/edit.html.erb"), "<h1>Edit</h1>\n").unwrap();
    let mut h = harness(&root);
    open(&mut h, &root, "app/controllers/invoices_controller.rb");
    // Ctrl+click on the action name opens the template.
    let p = point(&h, 2, 8);
    ctrl_click(&mut h, p);
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/views/invoices/edit.html.erb");
    // In the template, the right-click menu goes back to the action.
    let p = point(&h, 1, 3);
    right_click(&mut h, p);
    shot(&mut h, "14-go-to-controller");
    h.get_by_label("Go to controller action").click();
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/controllers/invoices_controller.rb");
    assert_eq!(cursor_line(&h), 2);
    // Inside the action body, the menu has "Go to view".
    let p = point(&h, 3, 10);
    right_click(&mut h, p);
    h.get_by_label("Go to view").click();
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/views/invoices/edit.html.erb");
}

#[test]
fn start_screen_opens_a_folder_from_the_dialog() {
    let root = fixture();
    let mut h =
        Harness::builder().with_size([1000.0, 700.0]).wgpu().build_eframe(|cc| App::new(&cc.egui_ctx, None, None));
    let _ = h.run_ok();
    h.get_by_label("Browse…");
    shot(&mut h, "15-start-screen");
    // The dialog runs on its own thread and sends the folder back.
    let _ = h.state().worker.tx.send(crate::worker::Msg::PickedFolder(Some(root.clone())));
    let _ = h.run_ok();
    assert_eq!(h.state().root.as_deref(), Some(root.as_path()));
}

#[test]
fn ctrl_c_copies_the_selected_text() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/services/billing.rb");
    // Left of the middle of a letter, so the text position is before it.
    let left = Vec2::new(2.0, 0.0);
    let (from, to) = (point(&h, 1, 1) - left, point(&h, 1, 6) - left);
    h.event(Event::PointerMoved(from));
    h.event(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    let _ = h.run_ok();
    h.event(Event::PointerMoved(to));
    let _ = h.run_ok();
    h.event(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    let _ = h.run_ok();
    assert_eq!(h.state().current().unwrap().selected_text().as_deref(), Some("class"));

    // egui-winit turns Ctrl+C into this event.
    h.event(Event::Copy);
    h.step();
    let copied =
        h.output().platform_output.commands.iter().any(|c| *c == eframe::egui::OutputCommand::CopyText("class".into()));
    assert!(copied, "{:?}", h.output().platform_output.commands);
}
