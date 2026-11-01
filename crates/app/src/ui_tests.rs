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
    // In diff mode, ghost rows sit between the lines.
    let row = t.row_of_line(line as u32 - 1) as usize;
    Pos2::new(origin.x + (col as f32 - 0.5) * char_w, origin.y + (row - first) as f32 * row_h + row_h * 0.35)
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
fn ctrl_click_on_a_class_definition_lists_its_usages() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/invoice.rb");
    // `Invoice` in `class Invoice < ApplicationRecord`, line 2.
    let p = point(&h, 2, 8);
    ctrl_click(&mut h, p);
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/models/invoice.rb", "the click stays in the file");
    match &h.state().bottom {
        Some(Bottom::Results { rows, title, .. }) => {
            assert_eq!(title, "Usages of Invoice (1)");
            assert_eq!(rows[0].rel, "app/services/billing.rb");
            assert_eq!(rows[0].line, 2);
        }
        _ => panic!("no results panel"),
    }
    shot(&mut h, "04b-ctrl-click-class-definition");
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
        Some(Bottom::Results { rows, title, .. }) => {
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
        Some(Bottom::Results { title, rows, .. }) => {
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
fn view_has_a_link_to_its_controller_action() {
    let root = fixture();
    for dir in ["app/controllers", "app/mailers", "app/views/invoices", "app/views/invoice_mailer"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(
        root.join("app/controllers/invoices_controller.rb"),
        "class InvoicesController < ApplicationController\n  def index; end\n\n  def edit\n    @invoice = Invoice.find(1)\n  end\nend\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app/mailers/invoice_mailer.rb"),
        "class InvoiceMailer < ApplicationMailer\n  def sent\n    mail\n  end\nend\n",
    )
    .unwrap();
    std::fs::write(root.join("app/views/invoices/edit.html.erb"), "<h1>Edit</h1>\n").unwrap();
    std::fs::write(root.join("app/views/invoices/_row.html.erb"), "<tr></tr>\n").unwrap();
    std::fs::write(root.join("app/views/invoice_mailer/sent.html.erb"), "<p>Sent</p>\n").unwrap();
    let mut h = harness(&root);
    // A partial has no action, so it has no bar.
    open(&mut h, &root, "app/views/invoices/_row.html.erb");
    assert!(h.query_by_label("Controller").is_none());
    // A template has a link to its action, above the code.
    open(&mut h, &root, "app/views/invoices/edit.html.erb");
    shot(&mut h, "15-view-action-bar");
    assert!(h.query_by_label("Controller").is_some());
    h.get_by_label("InvoicesController#edit").click();
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/controllers/invoices_controller.rb");
    assert_eq!(cursor_line(&h), 4);
    // A mailer template links to its mailer method.
    open(&mut h, &root, "app/views/invoice_mailer/sent.html.erb");
    assert!(h.query_by_label("Mailer").is_some());
    h.get_by_label("InvoiceMailer#sent").click();
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/mailers/invoice_mailer.rb");
    assert_eq!(cursor_line(&h), 2);
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

fn asked_to_close(h: &Harness<'static, App>) -> bool {
    h.output().viewport_output.values().any(|v| v.commands.contains(&eframe::egui::ViewportCommand::Close))
}

#[test]
fn project_name_goes_back_to_the_project_list() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/services/billing.rb");
    let name = root.file_name().unwrap().to_string_lossy().to_string();
    h.get_by_label(&name).click();
    let _ = h.run_ok();
    shot(&mut h, "17-project-menu");
    h.get_by_label("Back to the project list Ctrl+Q").click();
    let _ = h.run_ok();
    assert!(h.state().root.is_none());
    assert!(h.state().tabs.is_empty());
    h.get_by_label("Browse…");
}

#[test]
fn ctrl_q_closes_the_project_then_quits() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/services/billing.rb");
    h.key_press_modifiers(Modifiers::CTRL, Key::Q);
    h.step();
    assert!(h.state().root.is_none());
    assert!(!asked_to_close(&h));
    let _ = h.run_ok();
    h.get_by_label("Browse…");

    // The harness keeps the output of the last frame only, and it runs one
    // frame per event, so the key press is the last event.
    let mods = Modifiers::CTRL;
    h.event(Event::ModifiersChanged(mods));
    h.event(Event::Key { key: Key::Q, physical_key: None, pressed: true, repeat: false, modifiers: mods });
    h.step();
    assert!(asked_to_close(&h));
}

/// The text that the last frame copied.
fn copied(h: &Harness<'static, App>) -> Vec<String> {
    h.output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            eframe::egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// Right-clicks the name of a tab (`tab`) or of a file in the side bar.
fn right_click_name(h: &mut Harness<'static, App>, name: &str, tab: bool) {
    let nodes: Vec<_> = h.get_all_by_label(name).collect();
    let node = if tab {
        nodes.iter().max_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x))
    } else {
        nodes.iter().min_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x))
    };
    node.expect("the name").click_secondary();
    let _ = h.run_ok();
}

#[test]
fn copy_paths_from_the_tree_and_the_tabs() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/invoice.rb");
    let full = root.join("app/models/invoice.rb").display().to_string();

    right_click_name(&mut h, "invoice.rb", false);
    shot(&mut h, "30-tree-menu");
    h.get_by_label("Copy path").click();
    h.step();
    assert_eq!(copied(&h), vec!["app/models/invoice.rb".to_string()]);

    right_click_name(&mut h, "invoice.rb", true);
    shot(&mut h, "31-tab-menu");
    h.get_by_label("Copy full path").click();
    h.step();
    assert_eq!(copied(&h), vec![full]);
}

#[test]
fn close_tabs_from_the_tab_menu() {
    let root = fixture();
    let mut h = harness(&root);
    let names = ["entity.rb", "invoice.rb", "billing.rb", "application_record.rb"];
    for rel in
        ["app/models/entity.rb", "app/models/invoice.rb", "app/services/billing.rb", "app/models/application_record.rb"]
    {
        open(&mut h, &root, rel);
    }
    let tabs = |h: &Harness<'static, App>| -> Vec<String> {
        h.state().tabs.iter().map(|t| t.path.file_name().unwrap().to_string_lossy().to_string()).collect()
    };
    assert_eq!(tabs(&h), names);

    // Close the tabs to the right of invoice.rb: the active tab
    // (application_record.rb) closes, so invoice.rb becomes active.
    right_click_name(&mut h, "invoice.rb", true);
    h.get_by_label("Close tabs to the right").click();
    let _ = h.run_ok();
    assert_eq!(tabs(&h), ["entity.rb", "invoice.rb"]);
    assert_eq!(current_rel(&h), "app/models/invoice.rb");

    // Close the tabs to the left of invoice.rb.
    right_click_name(&mut h, "invoice.rb", true);
    h.get_by_label("Close tabs to the left").click();
    let _ = h.run_ok();
    assert_eq!(tabs(&h), ["invoice.rb"]);

    open(&mut h, &root, "app/models/entity.rb");
    right_click_name(&mut h, "invoice.rb", true);
    h.get_by_label("Close").click();
    let _ = h.run_ok();
    assert_eq!(tabs(&h), ["entity.rb"]);
    assert_eq!(current_rel(&h), "app/models/entity.rb");

    open(&mut h, &root, "app/models/invoice.rb");
    right_click_name(&mut h, "entity.rb", true);
    h.get_by_label("Close all tabs").click();
    let _ = h.run_ok();
    assert!(h.state().tabs.is_empty());
}

#[test]
fn tab_close_button_sits_next_to_the_name() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/invoice.rb");
    let name = h.get_all_by_label("invoice.rb").map(|n| n.rect()).max_by(|a, b| a.min.x.total_cmp(&b.min.x)).unwrap();
    let close = h.get_by_label("✕").rect();
    assert!(close.min.x - name.max.x <= 2.0, "gap {}", close.min.x - name.max.x);
}

/// Turns the mouse wheel `steps` steps over the code (down when negative).
fn wheel(h: &mut Harness<'static, App>, steps: f32, modifiers: Modifiers) {
    let pos = Pos2::new(700.0, 400.0);
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::PointerMoved(pos));
    let _ = h.run_ok();
    h.event(Event::MouseWheel {
        unit: eframe::egui::MouseWheelUnit::Line,
        delta: Vec2::new(0.0, steps),
        phase: eframe::egui::TouchPhase::Move,
        source: eframe::egui::MouseWheelSource::Wheel,
        modifiers,
    });
    let _ = h.run_ok();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    let _ = h.run_ok();
}

#[test]
fn wheel_scrolls_fast_and_shift_wheel_scrolls_a_page() {
    let root = fixture();
    let long: String = (1..=400).map(|n| format!("# line {n}\n")).collect();
    std::fs::write(root.join("app/models/long.rb"), long).unwrap();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/long.rb");
    let t = |h: &Harness<'static, App>| {
        let t = h.state().current().unwrap();
        (t.scroll, t.layout.unwrap().1)
    };

    // One wheel step scrolls more than ten lines.
    wheel(&mut h, -1.0, Modifiers::NONE);
    settle(&mut h);
    let (scroll, row_h) = t(&h);
    assert!(scroll.y > 10.0 * row_h, "{} for rows of {row_h}", scroll.y);

    // With Shift, one step scrolls a page down, not sideways.
    press(&mut h, Modifiers::NONE, Key::Home);
    let _ = h.run_ok();
    press(&mut h, Modifiers::NONE, Key::PageDown);
    let _ = h.run_ok();
    let page = t(&h).0.y;
    press(&mut h, Modifiers::NONE, Key::Home);
    let _ = h.run_ok();
    wheel(&mut h, -1.0, Modifiers::SHIFT);
    settle(&mut h);
    assert_eq!(t(&h).0, Vec2::new(0.0, page));
    wheel(&mut h, -2.0, Modifiers::SHIFT);
    settle(&mut h);
    assert_eq!(t(&h).0.y, 3.0 * page);
    wheel(&mut h, 1.0, Modifiers::SHIFT);
    settle(&mut h);
    assert_eq!(t(&h).0.y, 2.0 * page);
}

#[test]
fn wheel_scrolls_the_side_bar_faster_than_egui_does() {
    let root = fixture();
    for n in 0..100 {
        std::fs::write(root.join(format!("note_{n:03}.rb")), "").unwrap();
    }
    let mut h = harness(&root);
    let y = |h: &Harness<'static, App>| h.get_by_label("note_050.rb").rect().min.y;
    let before = y(&h);
    let pos = Pos2::new(100.0, 400.0);
    h.event(Event::PointerMoved(pos));
    let _ = h.run_ok();
    h.event(Event::MouseWheel {
        unit: eframe::egui::MouseWheelUnit::Line,
        delta: Vec2::new(0.0, -1.0),
        phase: eframe::egui::TouchPhase::Move,
        source: eframe::egui::MouseWheelSource::Wheel,
        modifiers: Modifiers::NONE,
    });
    settle(&mut h);
    // egui scrolls 40 points for one wheel step; the side bar scrolls 2.5 times that.
    let moved = before - y(&h);
    assert!((moved - 100.0).abs() < 1.0, "moved {moved}");
}

// ----- Diff mode -----

const ORDER_RELEASE: &str = "\
# An order that a customer places in the shop.
class Order < ApplicationRecord
  belongs_to :customer

  enum :status, { cart: 0, paid: 1, shipped: 2 }

  # The sum of all line items, in cents.
  def total_cents
    line_items.sum(&:price_cents)
  end
end
";

/// main: one more line than the release branch (line 6).
const ORDER_MAIN: &str = "\
# An order that a customer places in the shop.
class Order < ApplicationRecord
  belongs_to :customer

  enum :status, { cart: 0, paid: 1, shipped: 2 }
  validates :email, presence: true

  # The sum of all line items, in cents.
  def total_cents
    line_items.sum(&:price_cents)
  end
end
";

/// The branch: line 4 added, the enum (line 6) changed, a method added.
const ORDER_BRANCH: &str = "\
# An order that a customer places in the shop.
class Order < ApplicationRecord
  belongs_to :customer
  has_many :refunds

  enum :status, { cart: 0, paid: 1, shipped: 2, refunded: 3 }
  validates :email, presence: true

  # The sum of all line items, in cents.
  def total_cents
    line_items.sum(&:price_cents)
  end

  # The money that can still go back to the customer, in cents.
  def refundable_cents
    total_cents - refunds.sum(:amount_cents)
  end
end
";

/// On disk, not committed: line 16 changed.
const ORDER_DISK: &str = "\
# An order that a customer places in the shop.
class Order < ApplicationRecord
  belongs_to :customer
  has_many :refunds

  enum :status, { cart: 0, paid: 1, shipped: 2, refunded: 3 }
  validates :email, presence: true

  # The sum of all line items, in cents.
  def total_cents
    line_items.sum(&:price_cents)
  end

  # The money that can still go back to the customer, in cents.
  def refundable_cents
    total_cents - refunds.succeeded.sum(:amount_cents)
  end
end
";

/// A shop in git: `release/2.4` and `main`, the checked-out branch
/// `feature/order-refunds` with one commit, and changes that are not
/// committed (one of them a new file).
fn diff_fixture() -> PathBuf {
    let root = fixture_named("diff");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&root)
            .args(["-c", "user.name=Test Author", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    git(&["init", "-q"]);
    git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
    write("Gemfile", "source \"https://rubygems.org\"\n");
    write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n");
    write("app/models/order.rb", ORDER_RELEASE);
    write("app/services/shipping.rb", "class Shipping\n  def run(order)\n    order.total_cents\n  end\nend\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Orders"]);
    git(&["branch", "release/2.4"]);
    write("app/models/order.rb", ORDER_MAIN);
    git(&["commit", "-q", "-am", "Validate emails"]);
    git(&["checkout", "-q", "-b", "feature/order-refunds"]);
    write("app/models/order.rb", ORDER_BRANCH);
    write("app/models/refund.rb", "class Refund < ApplicationRecord\n  belongs_to :order\nend\n");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Add refunds"]);
    write("app/models/order.rb", ORDER_DISK);
    write("app/models/refund.rb", "class Refund < ApplicationRecord\n  WINDOW = 30.days\n  belongs_to :order\nend\n");
    write(
        "app/services/refund_order.rb",
        "class RefundOrder\n  def call(order)\n    order.refundable_cents\n  end\nend\n",
    );
    root
}

/// An empty temp folder (no git).
fn fixture_named(what: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("rubyscope-ui-{what}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// Runs frames until the diff of the current tab is there.
fn wait_for_diff(h: &mut Harness<'static, App>) {
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(25));
        let _ = h.run_ok();
        let ready = h.state().current().is_some_and(|t| t.rows.is_some() && !t.diff_requested);
        if ready && h.state().repo.is_some() {
            break;
        }
    }
    settle(h);
}

/// The rows of the current tab as text: `-old` for a ghost row (`u-old`
/// when not committed), and the line number with `+` (branch) or `~`
/// (not committed) for a changed line.
fn row_text(h: &Harness<'static, App>) -> Vec<String> {
    use crate::diff::{DisplayRow, LineMark};
    let t = h.state().current().unwrap();
    let map = t.rows.as_ref().expect("diff rows");
    map.rows
        .iter()
        .map(|r| match r {
            DisplayRow::Line(l) => {
                let m = match map.marks[*l as usize] {
                    LineMark::None => "",
                    LineMark::Branch => "+",
                    LineMark::Uncommitted => "~",
                };
                format!("{m}{}", l + 1)
            }
            DisplayRow::Ghost(g) => {
                let g = &map.ghosts[*g as usize];
                format!("{}{}", if g.uncommitted { "u-" } else { "-" }, g.text.trim())
            }
        })
        .collect()
}

fn toast_text(h: &Harness<'static, App>) -> String {
    h.state().toast_text().unwrap_or_default()
}

#[test]
fn diff_mode_opens_the_folders_of_the_changed_files() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "Gemfile");
    assert!(!h.state().open_folders().contains("app/models"));
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    wait_for_diff(&mut h);
    let open = h.state().open_folders();
    for dir in ["app", "app/models", "app/services"] {
        assert!(open.contains(dir), "{dir} not open: {open:?}");
    }
}

#[test]
fn diff_mode_shows_the_branch_and_what_is_not_committed() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    wait_for_diff(&mut h);
    assert!(h.state().show_diff);
    let repo = h.state().repo.clone().expect("repo status");
    assert_eq!(repo.range_label(), "main…feature/order-refunds");
    assert_eq!(repo.uncommitted_files.len(), 3, "{:?}", repo.uncommitted_files);
    let rows = row_text(&h);
    assert_eq!(
        rows,
        [
            "1",
            "2",
            "3",
            "+4",
            "5",
            "-enum :status, { cart: 0, paid: 1, shipped: 2 }",
            "+6",
            "7",
            "8",
            "9",
            "10",
            "11",
            // git takes `end` on line 12 as the old `end`, and the method
            // as added after it.
            "12",
            "+13",
            "+14",
            "+15",
            "u-total_cents - refunds.sum(:amount_cents)",
            "~16",
            "+17",
            "18",
            "19",
        ]
        .map(String::from),
        "{rows:#?}"
    );
    shot(&mut h, "40-diff-mode");
    h.get_by_label_contains("Diff: main…feature/order-refunds");
    h.get_by_label_contains("3 uncommitted");
    // The chip in the corner counts the changes.
    assert_eq!(h.state().current().unwrap().rows.as_ref().unwrap().changes.len(), 3);

    // Ctrl+Shift+D again: one row per line.
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    assert!(!h.state().show_diff);
    let t = h.state().current().unwrap();
    assert!(t.rows.is_none());
    assert_eq!(t.row_count(), t.lines.len());
}

#[test]
fn ctrl_click_works_below_ghost_rows() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    wait_for_diff(&mut h);
    // `total_cents` on line 16, below two ghost rows: goes to line 10.
    let p = point(&h, 16, 6);
    ctrl_click(&mut h, p);
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/models/order.rb");
    assert_eq!(cursor_line(&h), 10);
    // A ghost row has no links.
    let t = h.state().current().unwrap();
    let ghost_row = t.row_of_line(5) - 1;
    let (origin, row_h, char_w, first) = t.layout.unwrap();
    let ghost =
        Pos2::new(origin.x + 5.5 * char_w, origin.y + (ghost_row as usize - first) as f32 * row_h + row_h * 0.35);
    ctrl_click(&mut h, ghost);
    assert_eq!(cursor_line(&h), 10, "the click on a removed line did not move the cursor");
    assert!(toast_text(&h).contains("A removed line has no links"), "{}", toast_text(&h));
    // Find and go to line still count file lines.
    press(&mut h, Modifiers::NONE, Key::Escape);
    press(&mut h, CMD, Key::G);
    type_text(&mut h, "17");
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    assert_eq!(cursor_line(&h), 17);
    press(&mut h, CMD, Key::F);
    type_text(&mut h, "refunds.succeeded");
    let t = h.state().current().unwrap();
    assert_eq!(t.find.matches.len(), 1);
    assert_eq!(t.line_of(t.find.matches[0].0) + 1, 16);
}

#[test]
fn brackets_jump_between_changes_and_files() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    wait_for_diff(&mut h);
    let change = |h: &Harness<'static, App>| h.state().current().unwrap().change;
    press(&mut h, Modifiers::NONE, Key::CloseBracket);
    assert_eq!(change(&h), Some(0));
    assert_eq!(cursor_line(&h), 4);
    press(&mut h, Modifiers::NONE, Key::CloseBracket);
    assert_eq!(change(&h), Some(1));
    assert_eq!(cursor_line(&h), 6, "the change starts with a ghost row; the cursor goes to the next line");
    // Alt+Down does the same as ].
    press(&mut h, Modifiers::ALT, Key::ArrowDown);
    assert_eq!(change(&h), Some(2));
    settle(&mut h);
    shot(&mut h, "41-diff-jump");
    // [ goes back.
    press(&mut h, Modifiers::NONE, Key::OpenBracket);
    assert_eq!(change(&h), Some(1));
    press(&mut h, Modifiers::NONE, Key::CloseBracket);
    // After the last change: first a toast, then the next changed file.
    press(&mut h, Modifiers::NONE, Key::CloseBracket);
    assert_eq!(current_rel(&h), "app/models/order.rb");
    assert!(toast_text(&h).contains("Last change in order.rb. Press ] again for refund.rb"), "{}", toast_text(&h));
    press(&mut h, Modifiers::NONE, Key::CloseBracket);
    assert_eq!(current_rel(&h), "app/models/refund.rb");
    wait_for_diff(&mut h);
    assert_eq!(change(&h), Some(0), "the new file opens at its first change");
    // New on the branch: all green, but for the line that is not committed.
    assert_eq!(row_text(&h), ["+1", "~2", "+3", "+4", "5"]);
    // Esc takes the frame away.
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert_eq!(change(&h), None);
    // In a text field, ] is text.
    press(&mut h, CMD, Key::F);
    h.key_press(Key::CloseBracket);
    let _ = h.run_ok();
    assert_eq!(current_rel(&h), "app/models/refund.rb");
    assert_eq!(change(&h), None);
}

#[test]
fn diff_against_another_branch() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    wait_for_diff(&mut h);
    let line7 = |h: &Harness<'static, App>| {
        let t = h.state().current().unwrap();
        t.rows.as_ref().unwrap().marks[6]
    };
    assert_eq!(line7(&h), crate::diff::LineMark::None, "main has line 7 already");
    press(&mut h, CMD, Key::K);
    type_text(&mut h, "diff against");
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    assert_eq!(h.state().picker.as_ref().map(|p| p.kind), Some(crate::picker::PickerKind::Branches));
    shot(&mut h, "42-diff-branches");
    h.get_by_label_contains("✓ current base");
    h.get_by_label("release/2.4");
    type_text(&mut h, "release");
    press(&mut h, Modifiers::NONE, Key::Enter);
    wait_for_diff(&mut h);
    let repo = h.state().repo.clone().unwrap();
    assert_eq!(repo.base, "release/2.4");
    assert_eq!(line7(&h), crate::diff::LineMark::Branch, "the release branch does not have line 7");
    // Remembered for the project.
    let mut again = harness(&root);
    assert_eq!(again.state().diff_base.as_deref(), Some("release/2.4"));
    let _ = again.run_ok();
}

#[test]
fn changed_files_in_ctrl_p_the_tree_and_the_side_bar() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    wait_for_diff(&mut h);
    press(&mut h, CMD, Key::P);
    press(&mut h, Modifiers::NONE, Key::Tab);
    shot(&mut h, "43-diff-ctrl-p");
    let labels = h.state_mut().picker.as_mut().unwrap().result_labels();
    assert_eq!(labels, ["app/models/order.rb", "app/models/refund.rb", "app/services/refund_order.rb"]);
    h.get_by_label_contains("Go to changed file (vs main)");
    // Tab again: all files.
    press(&mut h, Modifiers::NONE, Key::Tab);
    assert!(h.state_mut().picker.as_mut().unwrap().result_labels().len() > 3);
    press(&mut h, Modifiers::NONE, Key::Escape);
    // The Changes tab of the side bar.
    h.get_by_label_contains("Changes").click();
    let _ = h.run_ok();
    shot(&mut h, "44-diff-changes-tab");
    h.get_by_label("ON THE BRANCH");
    h.get_by_label("NOT COMMITTED");
    h.get_all_by_label("refund_order.rb").next().unwrap().click();
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/services/refund_order.rb");
    wait_for_diff(&mut h);
    // A new file that git does not know yet: every line is not committed.
    assert_eq!(row_text(&h), ["~1", "~2", "~3", "~4", "~5", "6"]);
}

#[test]
fn blame_and_diff_together() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    press(&mut h, CMD, Key::B);
    wait_for_diff(&mut h);
    let t = h.state().current().unwrap();
    let blame = t.blame.as_ref().expect("blame");
    // Line 16 is not committed; blame counts file lines, not rows.
    assert!(blame.commits[blame.lines[15] as usize].is_uncommitted());
    assert!(!blame.commits[blame.lines[16] as usize].is_uncommitted());
    shot(&mut h, "45-diff-blame");
}

#[test]
fn the_diff_button_turns_diff_mode_on_and_off() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    h.get_by_label("Diff").click();
    wait_for_diff(&mut h);
    assert!(h.state().show_diff);
    assert!(h.state().current().unwrap().rows.is_some());
    shot(&mut h, "49-diff-button-on");
    // The button and the key work together.
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    assert!(!h.state().show_diff);
    h.get_by_label("Diff").click();
    wait_for_diff(&mut h);
    assert!(h.state().show_diff);
    h.get_by_label("Diff").click();
    h.run();
    assert!(!h.state().show_diff);
    assert!(h.state().current().unwrap().rows.is_none());
    shot(&mut h, "49-diff-button-off");
}

#[test]
fn diff_mode_does_not_move_the_window_content() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    let before = h.get_by_label("Files").rect();
    h.get_by_label("Diff").click();
    wait_for_diff(&mut h);
    h.get_by_label_contains("Diff: main");
    assert_eq!(h.get_by_label("Files").rect(), before);
}

/// A shop cloned from a remote, and a second clone that pushes a new
/// commit to it. Returns the first clone, which the window opens.
fn pull_fixture() -> PathBuf {
    let dir = fixture_named("pull");
    let git = |cwd: &Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(["-c", "user.name=Test Author", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&dir, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
    git(&dir, &["clone", "-q", "remote.git", "shop"]);
    let shop = dir.join("shop");
    std::fs::create_dir_all(shop.join("app/models")).unwrap();
    std::fs::write(shop.join("Gemfile"), "source \"https://rubygems.org\"\n").unwrap();
    std::fs::write(shop.join("app/models/order.rb"), "class Order\nend\n").unwrap();
    git(&shop, &["add", "."]);
    git(&shop, &["commit", "-q", "-m", "Orders"]);
    git(&shop, &["push", "-q", "-u", "origin", "main"]);
    git(&dir, &["clone", "-q", "remote.git", "other"]);
    let other = dir.join("other");
    std::fs::write(other.join("app/models/order.rb"), "class Order\n  has_many :refunds\nend\n").unwrap();
    git(&other, &["commit", "-q", "-am", "Refunds"]);
    git(&other, &["push", "-q"]);
    shop
}

#[test]
fn the_pull_button_pulls_the_branch() {
    let root = pull_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    h.get_by_label("Pull").click();
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(25));
        let _ = h.run_ok();
        if !h.state().pulling {
            break;
        }
    }
    let _ = h.run_ok();
    assert_eq!(toast_text(&h), "Pulled 1 commit");
    assert!(std::fs::read_to_string(root.join("app/models/order.rb")).unwrap().contains("refunds"));
    h.event(Event::PointerMoved(Pos2::new(600.0, 400.0)));
    let _ = h.run_ok();
    shot(&mut h, "50-pull");
    // The button is back for the next pull.
    h.get_by_label("Pull");
}

#[test]
fn the_branch_button_switches_branches() {
    let root = pull_fixture();
    let other = root.parent().unwrap().join("other");
    let git = |cwd: &Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .env("GIT_COMMITTER_DATE", "2030-01-01T00:00:00")
            .args(["-c", "user.name=Ana Novak", "-c", "user.email=ana@example.com", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&other, &["checkout", "-q", "-b", "feature/refunds"]);
    std::fs::write(other.join("app/models/refund.rb"), "class Refund\nend\n").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "Refund model"]);
    git(&other, &["push", "-q", "-u", "origin", "feature/refunds"]);
    git(&root, &["fetch", "-q"]);

    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    settle(&mut h);
    assert_eq!(h.state().head_name.as_deref(), Some("main"));
    h.get_by_label("main").click();
    settle(&mut h);
    assert_eq!(h.state().picker.as_ref().map(|p| p.kind), Some(crate::picker::PickerKind::SwitchBranch));
    // The newest commit first, with the initials of its author (no pull
    // request here: the author of the last commit).
    let first = h.state().picker.as_ref().unwrap().first_label().unwrap_or_default();
    assert_eq!(first, "origin/feature/refunds");
    h.get_by_label("AN");
    h.event(Event::PointerMoved(Pos2::new(600.0, 700.0)));
    shot(&mut h, "51-switch-branch");
    type_text(&mut h, "refunds");
    press(&mut h, Modifiers::NONE, Key::Enter);
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(25));
        let _ = h.run_ok();
        if !h.state().switching {
            break;
        }
    }
    let _ = h.run_ok();
    assert_eq!(toast_text(&h), "Switched to feature/refunds");
    assert_eq!(h.state().head_name.as_deref(), Some("feature/refunds"));
    assert!(root.join("app/models/refund.rb").exists());
    // The open file is read again from the new branch.
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(25));
        let _ = h.run_ok();
    }
    assert!(h.state().current().unwrap().text.contains("refunds"));

    // A change that the switch would lose: git refuses and says why.
    std::fs::write(root.join("app/models/order.rb"), "class Order\n  # mine\nend\n").unwrap();
    h.get_by_label("feature/refunds").click();
    settle(&mut h);
    type_text(&mut h, "main");
    press(&mut h, Modifiers::NONE, Key::Enter);
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(25));
        let _ = h.run_ok();
        if !h.state().switching {
            break;
        }
    }
    let _ = h.run_ok();
    assert!(toast_text(&h).starts_with("Could not switch:"), "{}", toast_text(&h));
    assert!(toast_text(&h).contains("app/models/order.rb"), "{}", toast_text(&h));
    assert_eq!(h.state().head_name.as_deref(), Some("feature/refunds"));
}

#[test]
fn diff_mode_outside_git_says_so() {
    let root = fixture_named("nogit");
    std::fs::create_dir_all(root.join("app/models")).unwrap();
    std::fs::write(root.join("app/models/order.rb"), "class Order\nend\n").unwrap();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    settle(&mut h);
    let repo = h.state().repo.clone().expect("status");
    assert!(repo.error.is_some());
    h.get_by_label("Diff: not a git repository");
    assert!(h.state().current().unwrap().rows.is_none());
    shot(&mut h, "48-diff-no-git");
    // ] says there is nothing to jump to.
    press(&mut h, Modifiers::NONE, Key::CloseBracket);
    assert!(toast_text(&h).contains("Nothing changed"), "{}", toast_text(&h));
}

/// A long file where the branch removed lines 20 to 29 of 300.
fn long_diff_fixture() -> PathBuf {
    let root = diff_fixture();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&root)
            .args(["-c", "user.name=Test Author", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
    };
    let all: Vec<String> = (1..=300).map(|n| format!("  # note {n}\n")).collect();
    git(&["stash", "-q", "-u"]);
    git(&["checkout", "-q", "main"]);
    std::fs::write(root.join("app/models/notes.rb"), all.concat()).unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Notes"]);
    git(&["checkout", "-q", "feature/order-refunds"]);
    git(&["merge", "-q", "--no-edit", "main"]);
    let kept: Vec<String> =
        all.iter().enumerate().filter(|(i, _)| !(19..29).contains(i)).map(|(_, l)| l.clone()).collect();
    std::fs::write(root.join("app/models/notes.rb"), kept.concat()).unwrap();
    git(&["commit", "-q", "-am", "Fewer notes"]);
    git(&["stash", "pop", "-q"]);
    root
}

#[test]
fn toggling_diff_mode_keeps_the_line_at_the_top() {
    let root = long_diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/notes.rb");
    for key in [Key::Num1, Key::Num5, Key::Num0, Key::G] {
        h.key_press(key);
    }
    settle(&mut h);
    let top = h.state().current().unwrap().top_line;
    // Line 150 is the 7th line in the view: exactly, as rows do not drift.
    assert_eq!(top, 143);
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    wait_for_diff(&mut h);
    let t = h.state().current().unwrap();
    assert_eq!(t.rows.as_ref().unwrap().ghosts.len(), 10);
    assert_eq!(t.top_line, top, "the same line is at the top with the ghost rows above it");
    assert_eq!(t.top_row, top + 10);
    // A click on the mark of the change on the right edge goes there.
    let view = h.state().current().unwrap().view_size;
    let (origin, ..) = h.state().current().unwrap().layout.unwrap();
    let rows = h.state().current().unwrap().row_count() as f32;
    let x = 1300.0 - crate::view::CHANGE_MAP_WIDTH / 2.0;
    let y = origin.y - 6.0 + (19.5 / rows) * view.y;
    h.event(Event::PointerMoved(Pos2::new(x, y)));
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: Pos2::new(x, y),
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    settle(&mut h);
    let t = h.state().current().unwrap();
    assert_eq!(t.change, Some(0), "the click on the mark did not go to the change");
    // The change (rows 19 to 28) shows a third of the way down the view.
    let visible = (t.view_size.y / t.layout.unwrap().1) as u32;
    assert!(t.top_row < 19 && t.top_row + visible / 2 > 19, "row {} of {visible}", t.top_row);
    shot(&mut h, "46-diff-change-map");
    // Back off: the line at the top stays.
    let top = h.state().current().unwrap().top_line;
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    settle(&mut h);
    assert_eq!(h.state().current().unwrap().top_line, top);
}

#[test]
fn ghost_rows_are_not_selected_and_explain_themselves() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    wait_for_diff(&mut h);
    // Drag from the start of line 5 to the end of line 6, over the ghost row.
    let left = Vec2::new(2.0, 0.0);
    let (from, to) = (point(&h, 5, 1) - left, point(&h, 6, 64));
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
    let selected = h.state().current().unwrap().selected_text().unwrap_or_default();
    assert_eq!(selected, "\n  enum :status, { cart: 0, paid: 1, shipped: 2, refunded: 3 }");
    // The pointer rests on the ghost row: a tooltip says why it has no links.
    let t = h.state().current().unwrap();
    let ghost_row = t.row_of_line(5) - 1;
    let (origin, row_h, char_w, first) = t.layout.unwrap();
    let ghost =
        Pos2::new(origin.x + 8.5 * char_w, origin.y + (ghost_row as usize - first) as f32 * row_h + row_h * 0.35);
    for _ in 0..15 {
        std::thread::sleep(Duration::from_millis(60));
        h.event(Event::PointerMoved(ghost));
        let _ = h.run_ok();
    }
    shot(&mut h, "47-diff-ghost-tooltip");
    h.get_by_label("Removed line, not in the index");
    h.get_by_label_contains("since the merge-base with main");
}

#[test]
fn diff_follows_the_file_and_new_commits() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    wait_for_diff(&mut h);
    // An edit on disk: the tab loads the new text, and the diff follows.
    let path = root.join("app/models/order.rb");
    std::fs::write(&path, ORDER_DISK.replace("  # The sum of all line items, in cents.\n", "")).unwrap();
    let _ = h.state().worker.tx.send(crate::worker::Msg::Changed(vec![path]));
    let has_ghost = |h: &Harness<'static, App>, text: &str| {
        h.state().current().unwrap().rows.as_ref().is_some_and(|m| m.ghosts.iter().any(|g| g.text.contains(text)))
    };
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(30));
        let _ = h.run_ok();
        if has_ghost(&h, "The sum of all line items") {
            break;
        }
    }
    assert!(has_ghost(&h, "The sum of all line items"), "{:#?}", row_text(&h));
    assert_eq!(h.state().repo.as_ref().unwrap().uncommitted_files.len(), 3);

    // A commit: HEAD moves, and the changes are on the branch now.
    let out = std::process::Command::new("git")
        .current_dir(&root)
        .args(["-c", "user.name=Test Author", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"])
        .args(["commit", "-q", "-am", "Refund rules"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut committed = false;
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(50));
        let _ = h.run_ok();
        let repo = h.state().repo.clone().unwrap();
        if repo.uncommitted_files.len() == 1 && !h.state().current().unwrap().diff_requested {
            committed = true;
            break;
        }
    }
    assert!(committed, "the commit was not seen");
    settle(&mut h);
    let rows = row_text(&h);
    assert!(rows.iter().all(|r| !r.starts_with('~') && !r.starts_with("u-")), "{rows:#?}");
    assert!(rows.contains(&"-# The sum of all line items, in cents.".to_string()), "{rows:#?}");
}

#[test]
fn a_missing_base_branch_still_shows_what_is_not_committed() {
    let root = diff_fixture();
    let mut h = harness(&root);
    h.state_mut().diff_base = Some("release/9.9".into());
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD | Modifiers::SHIFT, Key::D);
    wait_for_diff(&mut h);
    let repo = h.state().repo.clone().unwrap();
    assert!(repo.base_missing && repo.merge_base.is_none());
    h.get_by_label_contains("Diff: release/9.9 not found");
    let rows = row_text(&h);
    assert!(rows.contains(&"~16".to_string()) && !rows.contains(&"+4".to_string()), "{rows:#?}");
}

/// What the window manager sends for Super+W or the close button.
fn ask_to_close(h: &mut Harness<'static, App>) {
    let root = h.input_mut().viewports.entry(eframe::egui::ViewportId::ROOT).or_default();
    root.events.push(eframe::egui::ViewportEvent::Close);
    h.step();
}

fn held_back_close(h: &Harness<'static, App>) -> bool {
    h.output().viewport_output.values().any(|v| v.commands.contains(&eframe::egui::ViewportCommand::CancelClose))
}

#[test]
fn closing_with_nothing_running_quits_at_once() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/invoice.rb");
    ask_to_close(&mut h);
    assert!(h.state().quit);
    assert!(asked_to_close(&h));
    assert!(h.query_by_label("Closing Rubyscope").is_none());
}

#[test]
fn closing_waits_for_the_index_save_and_shows_it() {
    use std::sync::atomic::Ordering;
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/invoice.rb");
    let saving = h.state().worker.saving.clone();
    saving.store(true, Ordering::SeqCst);
    ask_to_close(&mut h);
    assert!(held_back_close(&h));
    assert!(!h.state().quit);
    // The dialog shows once closing takes a moment.
    std::thread::sleep(Duration::from_millis(200));
    // A new window area shows from its second frame.
    h.step();
    h.step();
    h.get_by_label("Closing Rubyscope");
    h.get_by_label("Saving the index…");
    shot(&mut h, "closing");
    let c = h.state().closing.as_ref().unwrap();
    assert_eq!(c.done, 2, "the tabs are saved and the watcher stopped");
    // Keys do nothing while closing.
    press(&mut h, CMD, Key::P);
    assert!(h.state().picker.is_none());

    saving.store(false, Ordering::SeqCst);
    h.step();
    assert!(h.state().quit);
    assert!(asked_to_close(&h));
}

#[test]
fn closing_again_quits_without_waiting() {
    let root = fixture();
    let mut h = harness(&root);
    h.state_mut().pulling = true;
    ask_to_close(&mut h);
    assert!(!h.state().quit);
    ask_to_close(&mut h);
    assert!(h.state().quit);
}

#[test]
fn the_blame_button_shows_and_hides_blame() {
    let root = diff_fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    h.get_by_label("Blame").click();
    h.run();
    assert!(h.state().show_blame);
    // The button and the key work together.
    press(&mut h, CMD, Key::B);
    assert!(!h.state().show_blame);
    h.get_by_label("Blame").click();
    h.run();
    assert!(h.state().show_blame);
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(25));
        let _ = h.run_ok();
        if h.state().current().is_some_and(|t| t.blame.is_some()) {
            break;
        }
    }
    shot(&mut h, "52-blame-button");
}

#[test]
fn the_folder_button_opens_go_to_file() {
    let root = fixture();
    let mut h = harness(&root);
    h.get_by_label("Go to file").click();
    h.run();
    assert_eq!(h.state().picker.as_ref().map(|p| p.kind), Some(crate::picker::PickerKind::Files));
    shot(&mut h, "53-folder-button");
}

/// The start screen, with these projects in its list.
fn start_screen(projects: &[&Path]) -> Harness<'static, App> {
    let list: Vec<crate::projects::Project> =
        projects.iter().map(|p| crate::projects::Project::new(&p.display().to_string())).collect();
    crate::app::save_projects(&list);
    let mut h =
        Harness::builder().with_size([1000.0, 700.0]).wgpu().build_eframe(|cc| App::new(&cc.egui_ctx, None, None));
    let _ = h.run_ok();
    h
}

fn listed() -> Vec<String> {
    crate::app::recent_projects().into_iter().map(|p| p.title()).collect()
}

#[test]
fn a_number_key_opens_that_project() {
    let (shop, blog) = (fixture_named("num-shop"), fixture_named("num-blog"));
    // The shop has an icon; the blog shows its first letter.
    std::fs::create_dir_all(shop.join("public")).unwrap();
    image::RgbaImage::from_pixel(32, 32, image::Rgba([220, 60, 60, 255]))
        .save(shop.join("public/favicon.png"))
        .unwrap();
    let mut h = start_screen(&[&shop, &blog]);
    shot(&mut h, "54-project-list");
    press(&mut h, Modifiers::NONE, Key::Num2);
    assert_eq!(h.state().root.as_deref(), Some(blog.as_path()));
}

#[test]
fn a_project_can_be_renamed_and_removed() {
    let (shop, blog) = (fixture_named("ren-shop"), fixture_named("ren-blog"));
    let shop_name = shop.file_name().unwrap().to_string_lossy().to_string();
    let blog_name = blog.file_name().unwrap().to_string_lossy().to_string();
    let mut h = start_screen(&[&shop, &blog]);
    h.get_by_label(&format!("More for {shop_name}")).click();
    let _ = h.run_ok();
    h.get_by_label("Rename").click();
    let _ = h.run_ok();
    type_text(&mut h, "Shop app");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(listed(), ["Shop app", blog_name.as_str()]);
    // The folder keeps its name; the list shows the new one.
    assert!(shop.is_dir());
    h.get_by_label("Shop app");
    h.get_by_label("More for Shop app").click();
    let _ = h.run_ok();
    h.get_by_label("Remove from the list").click();
    let _ = h.run_ok();
    assert_eq!(listed(), [blog_name.as_str()]);
}

#[test]
fn a_drag_moves_a_project_to_another_number() {
    let (a, b, c) = (fixture_named("drag-a"), fixture_named("drag-b"), fixture_named("drag-c"));
    let names: Vec<String> =
        [&a, &b, &c].iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
    let mut h = start_screen(&[&a, &b, &c]);
    let from = h.get_by_label(&names[2]).rect().center();
    let to = h.get_by_label(&names[0]).rect().center() - Vec2::new(0.0, 20.0);
    h.event(Event::PointerMoved(from));
    h.event(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    let _ = h.run_ok();
    for step in 1..=10 {
        h.event(Event::PointerMoved(from + (to - from) * (step as f32 / 10.0)));
        let _ = h.run_ok();
    }
    shot(&mut h, "55-project-drag");
    h.event(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    let _ = h.run_ok();
    assert_eq!(listed(), [names[2].as_str(), names[0].as_str(), names[1].as_str()]);
    assert!(h.state().root.is_none());
}

#[test]
fn the_sun_and_moon_button_switches_dark_and_light_mode() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/services/billing.rb");
    // Dark is the default.
    assert!(h.state().theme.dark);
    shot(&mut h, "55-dark-mode");
    h.get_by_label("Light mode").click();
    h.run();
    assert!(!h.state().theme.dark);
    shot(&mut h, "56-light-mode");
    // A new window starts in the mode chosen last.
    let mut again = harness(&root);
    assert!(!again.state().theme.dark);
    again.get_by_label("Dark mode").click();
    again.run();
    assert!(again.state().theme.dark);
    assert!(harness(&root).state().theme.dark);
}

#[test]
fn the_start_screen_has_the_mode_button() {
    let mut h = start_screen(&[]);
    h.get_by_label("Light mode").click();
    let _ = h.run_ok();
    assert!(!h.state().theme.dark);
    shot(&mut h, "57-start-screen-light");
}

#[test]
fn arrows_choose_a_file_in_go_to_file() {
    let root = fixture();
    let mut h = harness(&root);
    press(&mut h, CMD, Key::P);
    type_text(&mut h, "models");
    let labels = h.state_mut().picker.as_mut().unwrap().result_labels();
    assert!(labels.len() >= 3, "{labels:?}");
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowUp);
    assert_eq!(h.state().picker.as_ref().unwrap().selected, 1);
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    assert_eq!(current_rel(&h), labels[1]);
}

#[test]
fn ctrl_click_on_a_method_definition_with_one_usage_goes_there() {
    let root = fixture();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/invoice.rb");
    // `pay!` in `def pay!`, line 6. Its only usage is in billing.rb.
    let p = point(&h, 6, 8);
    ctrl_click(&mut h, p);
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/services/billing.rb");
    assert_eq!(cursor_line(&h), 4);
}

#[test]
fn method_usages_list_the_app_first_and_the_arrows_choose_one() {
    let root = fixture();
    let call = "    invoice = Invoice.find(1)\n    invoice.pay!\n";
    std::fs::create_dir_all(root.join("spec/models")).unwrap();
    std::fs::write(
        root.join("spec/models/invoice_spec.rb"),
        format!("RSpec.describe Invoice do\n  it \"pays\" do\n{call}  end\nend\n"),
    )
    .unwrap();
    std::fs::create_dir_all(root.join("app/jobs")).unwrap();
    std::fs::write(root.join("app/jobs/pay_job.rb"), format!("class PayJob\n  def perform\n{call}  end\nend\n"))
        .unwrap();
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/invoice.rb");
    let p = point(&h, 6, 8);
    ctrl_click(&mut h, p);
    settle(&mut h);
    assert_eq!(current_rel(&h), "app/models/invoice.rb", "a list, not a jump");
    let rels = match &h.state().bottom {
        Some(Bottom::Results { rows, title, selected }) => {
            assert_eq!(title, "Usages of Invoice#pay! (3)");
            assert_eq!(*selected, 0);
            rows.iter().map(|r| r.rel.clone()).collect::<Vec<_>>()
        }
        _ => panic!("no results panel"),
    };
    assert_eq!(rels[2], "spec/models/invoice_spec.rb", "the spec comes last: {rels:?}");
    assert!(h.state().results_keys);
    shot(&mut h, "58-method-usages");
    // Down twice and up once: the second row. Enter opens it, and the
    // arrows still choose in the list, not scroll the code.
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowUp);
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    assert_eq!(current_rel(&h), rels[1]);
    assert_eq!(cursor_line(&h), 4);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    assert_eq!(current_rel(&h), "spec/models/invoice_spec.rb");
    // A click in the code gives the arrows back to the code.
    let p = point(&h, 1, 2);
    h.event(Event::PointerMoved(p));
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.event(Event::PointerButton {
        pos: p,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    let _ = h.run_ok();
    assert!(!h.state().results_keys);
}
