//! Go-to-definition rules, tested on small Ruby snippets.
//!
//! Each test builds an index from code in memory, "clicks" on the first
//! match of a marker text, and checks where the click goes.

use rubyscope_index::{Confidence, Index};

struct Project {
    files: Vec<(&'static str, &'static str)>,
    index: Index,
}

impl Project {
    fn new(files: &[(&'static str, &'static str)]) -> Self {
        Self { files: files.to_vec(), index: Index::from_memory(files) }
    }

    /// Clicks on `needle` in `path` (on its first character, plus `skip`
    /// characters) and returns each target as `label @ path:line`.
    fn click(&self, path: &str, needle: &str, skip: usize) -> Vec<String> {
        let (_, code) = self.files.iter().find(|(p, _)| *p == path).expect("file");
        let offset = code.find(needle).unwrap_or_else(|| panic!("{needle} not in {path}")) + skip;
        let file = self.index.file_id(std::path::Path::new(path)).expect("file id");
        let Some(res) = self.index.definition_at(file, offset as u32) else { return vec![] };
        res.targets
            .iter()
            .map(|t| {
                let f = &self.index.files[t.loc.file as usize];
                let (line, _) = f.raw.line_col(t.loc.span.start);
                let guess = if t.confidence == Confidence::NameOnly { " (name only)" } else { "" };
                format!("{} @ {}:{}{guess}", t.label, self.index.display_path(t.loc.file), line + 1)
            })
            .collect()
    }

    fn first(&self, path: &str, needle: &str, skip: usize) -> String {
        self.click(path, needle, skip).into_iter().next().unwrap_or_else(|| "nothing".into())
    }
}

#[test]
fn constant_in_the_lexical_scope_wins_over_the_top_level() {
    let p = Project::new(&[
        ("a.rb", "class Invoice; end\nmodule Billing\n  class Invoice; end\nend\n"),
        ("b.rb", "module Billing\n  class Job\n    def run = Invoice.new\n  end\nend\n"),
    ]);
    assert_eq!(p.first("b.rb", "Invoice.new", 0), "Billing::Invoice @ a.rb:3");
}

#[test]
fn compact_class_names_look_up_their_first_segment() {
    let p = Project::new(&[
        ("a.rb", "module Admin\n  module Reports; end\nend\n"),
        ("b.rb", "module Admin\n  class Reports::Daily\n    X = 1\n  end\nend\n"),
        ("c.rb", "Admin::Reports::Daily::X\n"),
    ]);
    assert_eq!(p.first("c.rb", "Daily", 0), "Admin::Reports::Daily @ b.rb:2");
    assert_eq!(p.first("c.rb", "X", 0), "Admin::Reports::Daily::X @ b.rb:3");
}

#[test]
fn compact_scope_does_not_see_the_outer_namespace() {
    // Inside `class Admin::Report`, the lexical scope is only
    // `Admin::Report`, so `Helper` is the top-level one.
    let p = Project::new(&[
        ("a.rb", "class Helper; end\nmodule Admin\n  class Helper; end\nend\n"),
        ("b.rb", "class Admin::Report\n  def run = Helper\nend\n"),
    ]);
    assert_eq!(p.first("b.rb", "Helper", 0), "Helper @ a.rb:1");
}

#[test]
fn constants_are_found_through_the_superclass() {
    let p = Project::new(&[
        ("a.rb", "class Base\n  LIMIT = 10\nend\n"),
        ("b.rb", "class Child < Base\n  def max = LIMIT\nend\n"),
    ]);
    assert_eq!(p.first("b.rb", "LIMIT", 0), "Base::LIMIT @ a.rb:2");
}

#[test]
fn constant_alias_is_followed() {
    let p =
        Project::new(&[("a.rb", "module Real\n  class Thing; end\nend\nAlias = Real\n"), ("b.rb", "Alias::Thing\n")]);
    assert_eq!(p.first("b.rb", "Thing", 0), "Real::Thing @ a.rb:2");
}

#[test]
fn implicit_call_goes_to_the_method_in_the_superclass() {
    let p = Project::new(&[
        ("a.rb", "class Base\n  def helper; end\nend\n"),
        ("b.rb", "class Child < Base\n  def run\n    helper\n  end\nend\n"),
    ]);
    assert_eq!(p.first("b.rb", "helper", 0), "Base#helper @ a.rb:2");
}

#[test]
fn included_module_comes_before_the_superclass_and_last_include_wins() {
    let p = Project::new(&[
        ("a.rb", "class Base\n  def name; end\nend\nmodule A\n  def name; end\nend\nmodule B\n  def name; end\nend\n"),
        ("b.rb", "class Child < Base\n  include A\n  include B\n  def run = name\nend\n"),
    ]);
    assert_eq!(p.first("b.rb", "name\nend", 0), "B#name @ a.rb:8");
}

#[test]
fn prepended_module_comes_before_the_class() {
    let p = Project::new(&[
        ("a.rb", "module Loud\n  def say; end\nend\n"),
        ("b.rb", "class Dog\n  prepend Loud\n  def say; end\n  def run = say\nend\n"),
    ]);
    assert_eq!(p.first("b.rb", "say\nend", 0), "Loud#say @ a.rb:2");
}

#[test]
fn class_method_calls_use_the_singleton_side() {
    let p = Project::new(&[(
        "a.rb",
        "class Invoice\n  def self.build; end\n  def build; end\n  class << self\n    def make; end\n  end\nend\nInvoice.build\nInvoice.make\n",
    )]);
    assert_eq!(p.first("a.rb", "build\nInvoice", 0), "Invoice.build @ a.rb:2");
    assert_eq!(p.first("a.rb", "make\n", 0), "Invoice.make @ a.rb:5");
}

#[test]
fn new_goes_to_initialize() {
    let p = Project::new(&[
        ("a.rb", "class Base\n  def initialize(x); end\nend\nclass Child < Base; end\n"),
        ("b.rb", "Child.new(1)\n"),
    ]);
    assert_eq!(p.first("b.rb", "new", 0), "Base#initialize @ a.rb:2");
}

#[test]
fn concern_class_methods_are_class_methods_of_the_includer() {
    let p = Project::new(&[
        (
            "a.rb",
            "module Searchable\n  extend ActiveSupport::Concern\n  class_methods do\n    def search(q); end\n  end\nend\n",
        ),
        ("b.rb", "class Invoice\n  include Searchable\nend\nInvoice.search('x')\n"),
    ]);
    assert_eq!(p.first("b.rb", "search", 0), "Searchable::ClassMethods#search @ a.rb:4");
}

#[test]
fn old_style_class_methods_module_is_found_too() {
    let p = Project::new(&[
        ("a.rb", "module Taggable\n  module ClassMethods\n    def tagged; end\n  end\nend\n"),
        ("b.rb", "class Post\n  include Taggable\nend\nPost.tagged\n"),
    ]);
    assert_eq!(p.first("b.rb", "tagged", 0), "Taggable::ClassMethods#tagged @ a.rb:3");
}

#[test]
fn extend_makes_class_methods() {
    let p = Project::new(&[
        ("a.rb", "module Finders\n  def find_all; end\nend\n"),
        ("b.rb", "class Repo\n  extend Finders\n  def self.run = find_all\nend\n"),
    ]);
    assert_eq!(p.first("b.rb", "find_all\n", 0), "Finders#find_all @ a.rb:2");
}

#[test]
fn module_function_makes_a_class_method_copy() {
    let p = Project::new(&[
        ("a.rb", "module Util\n  module_function\n  def slug(s); end\nend\n"),
        ("b.rb", "Util.slug('A')\n"),
    ]);
    assert_eq!(p.first("b.rb", "slug", 0), "Util.slug @ a.rb:3");
}

#[test]
fn attr_and_alias_and_define_method_are_methods() {
    let p = Project::new(&[(
        "a.rb",
        "class User\n  attr_reader :email\n  alias handle email\n  define_method(:shout) { }\n  def run\n    email\n    handle\n    shout\n  end\nend\n",
    )]);
    assert_eq!(p.first("a.rb", "email\n    handle", 0), "User#email @ a.rb:2");
    assert_eq!(p.first("a.rb", "handle\n    shout", 0), "User#handle @ a.rb:3");
    assert_eq!(p.first("a.rb", "shout\n  end", 0), "User#shout @ a.rb:4");
}

#[test]
fn class_new_and_struct_new_define_classes() {
    let p = Project::new(&[
        ("a.rb", "Error = Class.new(StandardError)\nPoint = Struct.new(:x, :y) do\n  def len; end\nend\n"),
        ("b.rb", "Error\nPoint.new(1, 2)\n"),
    ]);
    assert_eq!(p.first("b.rb", "Error", 0), "Error @ a.rb:1");
    assert_eq!(p.first("b.rb", "Point", 0), "Point @ a.rb:2");
}

#[test]
fn class_eval_adds_methods_to_the_class() {
    let p = Project::new(&[
        ("a.rb", "class Invoice; end\nInvoice.class_eval do\n  def paid?; end\nend\n"),
        ("b.rb", "class Invoice\n  def run = paid?\nend\n"),
    ]);
    assert_eq!(p.first("b.rb", "paid?", 0), "Invoice#paid? @ a.rb:3");
}

#[test]
fn singleton_class_of_a_constant() {
    let p = Project::new(&[
        ("a.rb", "class Time\n  class << Time\n    def parse(s); end\n  end\nend\n"),
        ("b.rb", "Time.parse('x')\n"),
    ]);
    assert_eq!(p.first("b.rb", "parse", 0), "Time.parse @ a.rb:3");
}

#[test]
fn unknown_receiver_falls_back_to_a_name_list() {
    let p = Project::new(&[
        ("a.rb", "class A\n  def total; end\nend\nclass B\n  def total; end\nend\n"),
        ("b.rb", "def run(x) = x.total\n"),
    ]);
    assert_eq!(
        p.click("b.rb", "total", 0),
        vec!["A#total @ a.rb:2 (name only)".to_string(), "B#total @ a.rb:5 (name only)".to_string()]
    );
}

#[test]
fn zeitwerk_folder_namespace_counts_as_known() {
    // No file defines `module Ingresses`; only a nested class shows it.
    let p = Project::new(&[
        ("a.rb", "module ActionMailbox\n  class Ingresses::Mailgun::Controller; end\nend\n"),
        ("b.rb", "ActionMailbox::Ingresses::Mailgun::Controller\n"),
    ]);
    assert_eq!(p.first("b.rb", "Controller", 0), "ActionMailbox::Ingresses::Mailgun::Controller @ a.rb:2");
}

#[test]
fn private_def_keeps_the_method() {
    let p = Project::new(&[("a.rb", "class A\n  def run = secret\n  private def secret; end\nend\n")]);
    assert_eq!(p.first("a.rb", "secret\n", 0), "A#secret @ a.rb:3");
    let idx = &p.index;
    let m = idx.methods.iter().find(|m| m.name == "secret").unwrap();
    assert_eq!(m.visibility, rubyscope_index::model::Visibility::Private);
}
