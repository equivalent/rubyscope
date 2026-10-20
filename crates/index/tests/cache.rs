//! The saved index: files that did not change are taken from it, files
//! that changed are parsed again, and the answers are the same.

use std::path::{Path, PathBuf};

use rubyscope_index::{BuildOptions, Index};

const OPTS: BuildOptions = BuildOptions { gems: false, stdlib: false };

/// A new temp folder with a project and a cache folder in it.
fn setup() -> (PathBuf, PathBuf) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("rubyscope-cache-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("app");
    write(&project, "app/models/invoice.rb", "class Invoice\n  def pay!; end\nend\n");
    write(&project, "app/services/billing.rb", "class Billing\n  def run\n    Invoice.new.pay!\n  end\nend\n");
    (project.canonicalize().unwrap(), dir.join("cache"))
}

fn write(project: &Path, rel: &str, code: &str) {
    let p = project.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, code).unwrap();
}

/// Where a click on `needle` in `rel` goes, as `label @ path:line`.
fn click(index: &Index, rel: &str, needle: &str) -> String {
    let file = index.file_id(Path::new(rel)).unwrap();
    let text = index.read_text(file).unwrap();
    let offset = text.find(needle).unwrap() as u32;
    let res = index.definition_at(file, offset).unwrap();
    let t = &res.targets[0];
    let line = index.files[t.loc.file as usize].raw.line_of(t.loc.span.start) + 1;
    format!("{} @ {}:{line}", t.label, index.display_path(t.loc.file))
}

#[test]
fn unchanged_files_come_from_the_saved_index() {
    let (project, cache) = setup();
    let first = Index::load(&project, &OPTS, &cache).unwrap();
    assert_eq!(first.stats.reused, 0);
    assert!(!first.stats.same_as_saved);
    first.save(&cache).unwrap();

    let second = Index::load(&project, &OPTS, &cache).unwrap();
    assert_eq!(second.stats.reused, second.stats.files);
    assert!(second.stats.same_as_saved);
    // References come from the saved bytes when a click needs them.
    assert_eq!(click(&second, "app/services/billing.rb", "pay!"), "Invoice#pay! @ app/models/invoice.rb:2");
    let usages =
        second.usages(&second.symbol_at(second.file_id(Path::new("app/models/invoice.rb")).unwrap(), 20).unwrap());
    assert_eq!(usages.len(), 1);

    // A changed file is parsed again; the other one is still taken.
    write(&project, "app/models/invoice.rb", "class Invoice\n\n  def pay!; end\nend\n");
    let third = Index::load(&project, &OPTS, &cache).unwrap();
    assert_eq!(third.stats.reused, third.stats.files - 1);
    assert!(!third.stats.same_as_saved);
    assert_eq!(click(&third, "app/services/billing.rb", "pay!"), "Invoice#pay! @ app/models/invoice.rb:3");

    // A saved index with the changed file, and a new file.
    third.save(&cache).unwrap();
    write(&project, "app/models/entity.rb", "class Entity\nend\n");
    let fourth = Index::load(&project, &OPTS, &cache).unwrap();
    assert_eq!(fourth.stats.files, 3);
    assert_eq!(fourth.stats.reused, 2);
    assert!(fourth.const_exists("Entity"));
}

#[test]
fn a_rebuild_takes_the_files_from_the_last_index() {
    let (project, _) = setup();
    let first = Index::build(&project, &OPTS).unwrap();
    write(&project, "app/models/invoice.rb", "class Invoice\n  def pay!; end\n  def void!; end\nend\n");
    let second = first.rebuild(&OPTS).unwrap();
    assert_eq!(second.stats.reused, 1);
    assert!(second.find_method("Invoice", false, "void!").is_some());
}

#[test]
fn new_inflections_parse_every_file_again() {
    let (project, cache) = setup();
    Index::load(&project, &OPTS, &cache).unwrap().save(&cache).unwrap();
    write(
        &project,
        "config/initializers/inflections.rb",
        "ActiveSupport::Inflector.inflections(:en) do |inflect|\n  inflect.acronym \"API\"\nend\n",
    );
    let index = Index::load(&project, &OPTS, &cache).unwrap();
    assert_eq!(index.stats.reused, 0);
}

#[test]
fn a_bad_saved_index_is_not_used() {
    let (project, cache) = setup();
    let first = Index::load(&project, &OPTS, &cache).unwrap();
    first.save(&cache).unwrap();
    let saved = std::fs::read_dir(&cache).unwrap().next().unwrap().unwrap().path();
    let mut bytes = std::fs::read(&saved).unwrap();
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&saved, bytes).unwrap();
    let index = Index::load(&project, &OPTS, &cache).unwrap();
    assert_eq!(index.stats.reused, 0);
    assert_eq!(click(&index, "app/services/billing.rb", "pay!"), "Invoice#pay! @ app/models/invoice.rb:2");
    std::fs::write(&saved, b"not an index").unwrap();
    assert_eq!(Index::load(&project, &OPTS, &cache).unwrap().stats.reused, 0);
}
