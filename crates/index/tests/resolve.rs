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
fn self_class_calls_go_to_the_class_method() {
    let p = Project::new(&[(
        "a.rb",
        "class Notice\n  def self.messages(x) = []\n  def messages; end\n  def run\n    self.class.messages(1)\n    div do\n      self.class.messages(2)\n    end\n  end\nend\n",
    )]);
    assert_eq!(p.click("a.rb", "messages(1)", 0), vec!["Notice.messages @ a.rb:2"]);
    assert_eq!(p.click("a.rb", "messages(2)", 0), vec!["Notice.messages @ a.rb:2"]);
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
fn phlex_kit_calls_go_to_the_component() {
    // `extend Phlex::Kit` makes a method for each component of the kit, so
    // `ShopUI::Page(...)` and `Page(...)` render `ShopUI::Page`.
    let p = Project::new(&[
        ("config/initializers/phlex.rb", "module ShopUI\n  extend Phlex::Kit\nend\n"),
        ("app/components/shop_ui/base.rb", "module ShopUI\n  class Base < Phlex::HTML; end\nend\n"),
        (
            "app/components/shop_ui/page.rb",
            "module ShopUI\n  class Page < ShopUI::Base\n    def view_template = Header(title: 1)\n  end\nend\n",
        ),
        ("app/components/shop_ui/header.rb", "module ShopUI\n  class Header < ShopUI::Base; end\nend\n"),
        (
            "app/views/orders/index.rb",
            "module Views\n  class Index\n    include ShopUI\n    def view_template\n      ShopUI::Page(title: 1) do\n        ::ShopUI::Header()\n        Page()\n      end\n    end\n  end\nend\n",
        ),
    ]);
    let view = "app/views/orders/index.rb";
    assert_eq!(p.first(view, "Page(title", 0), "ShopUI::Page @ app/components/shop_ui/page.rb:2");
    assert_eq!(p.first(view, "Header()", 0), "ShopUI::Header @ app/components/shop_ui/header.rb:2");
    assert_eq!(p.first(view, "Page()", 0), "ShopUI::Page @ app/components/shop_ui/page.rb:2");
    assert_eq!(
        p.first("app/components/shop_ui/page.rb", "Header(", 0),
        "ShopUI::Header @ app/components/shop_ui/header.rb:2"
    );
}

#[test]
fn phlex_kit_call_in_a_module_uses_the_classes_that_include_it() {
    let p = Project::new(&[
        ("config/initializers/phlex.rb", "module ShopUI\n  extend Phlex::Kit\nend\n"),
        ("app/components/shop_ui/badge.rb", "module ShopUI\n  class Badge < Phlex::HTML; end\nend\n"),
        ("app/components/concerns/badges.rb", "module Badges\n  def status_cell = Badge(:sm)\nend\n"),
        (
            "app/components/application_component.rb",
            "class ApplicationComponent < Phlex::HTML\n  include ShopUI\n  include Badges\nend\n",
        ),
    ]);
    assert_eq!(
        p.click("app/components/concerns/badges.rb", "Badge(", 0),
        vec!["ShopUI::Badge @ app/components/shop_ui/badge.rb:2".to_string()]
    );
}

#[test]
fn phlex_kit_call_on_an_unknown_receiver_lists_kit_components() {
    let p = Project::new(&[
        ("config/initializers/phlex.rb", "module ShopUI\n  extend Phlex::Kit\nend\n"),
        ("app/components/shop_ui/submit.rb", "module ShopUI\n  class Submit < Phlex::HTML; end\nend\n"),
        ("app/components/order_form.rb", "class OrderForm\n  def footer(f) = f.Submit(:primary)\nend\n"),
    ]);
    assert_eq!(
        p.click("app/components/order_form.rb", "Submit", 0),
        vec!["ShopUI::Submit @ app/components/shop_ui/submit.rb:2 (name only)".to_string()]
    );
}

#[test]
fn folder_namespace_lists_its_classes() {
    // No file defines `module Audit`; Zeitwerk makes it for the folder.
    let p = Project::new(&[
        ("app/events/audit/read.rb", "class Audit::Read < Event; end\n"),
        ("app/events/audit/created.rb", "class Audit::Created < Event; end\n"),
        ("app/models/order.rb", "class Order\n  def log = Audit::Read.record\nend\n"),
    ]);
    assert_eq!(
        p.click("app/models/order.rb", "Audit", 0),
        vec![
            "Audit::Created @ app/events/audit/created.rb:1".to_string(),
            "Audit::Read @ app/events/audit/read.rb:1".to_string()
        ]
    );
}

#[test]
fn private_def_keeps_the_method() {
    let p = Project::new(&[("a.rb", "class A\n  def run = secret\n  private def secret; end\nend\n")]);
    assert_eq!(p.first("a.rb", "secret\n", 0), "A#secret @ a.rb:3");
    let idx = &p.index;
    let m = idx.methods.iter().find(|m| m.name == "secret").unwrap();
    assert_eq!(m.visibility, rubyscope_index::model::Visibility::Private);
}

impl Project {
    /// The usages of the definition named at `needle`, as `path:line`.
    fn declaration_usages(&self, path: &str, needle: &str, skip: usize) -> Option<Vec<String>> {
        let (_, code) = self.files.iter().find(|(p, _)| *p == path).expect("file");
        let offset = code.find(needle).unwrap_or_else(|| panic!("{needle} not in {path}")) + skip;
        let file = self.index.file_id(std::path::Path::new(path)).expect("file id");
        let sym = self.index.declaration_at(file, offset as u32)?;
        let usages = self.index.usages(&sym);
        Some(
            usages
                .iter()
                .map(|u| {
                    let line = self.index.files[u.file as usize].raw.line_of(u.span.start) + 1;
                    format!("{}:{line}", self.index.display_path(u.file))
                })
                .collect(),
        )
    }
}

#[test]
fn the_name_of_a_definition_lists_its_usages() {
    let p = Project::new(&[
        ("app/models/order.rb", "class Order\n  TOPIC_PREFIX = \"ai:\"\n\n  def topic = \"#{TOPIC_PREFIX}x\"\nend\n"),
        ("app/models/shop.rb", "module Shop\n  class Cart\n  end\nend\nclass Shop::Basket\nend\n"),
        (
            "app/services/checkout.rb",
            "class Checkout\n  def run\n    Order::TOPIC_PREFIX\n    Shop::Cart.new\n    Order.new\n  end\nend\n",
        ),
    ]);
    // A constant: its own file and other files.
    assert_eq!(
        p.declaration_usages("app/models/order.rb", "TOPIC_PREFIX", 3),
        Some(vec!["app/models/order.rb:4".to_string(), "app/services/checkout.rb:3".to_string()])
    );
    // A class and a module. The definition line itself is not a usage.
    assert_eq!(
        p.declaration_usages("app/models/order.rb", "Order", 0),
        Some(vec!["app/services/checkout.rb:3".to_string(), "app/services/checkout.rb:5".to_string()])
    );
    assert_eq!(
        p.declaration_usages("app/models/shop.rb", "Shop", 1),
        Some(vec!["app/models/shop.rb:5".to_string(), "app/services/checkout.rb:4".to_string()])
    );
    assert_eq!(
        p.declaration_usages("app/models/shop.rb", "Cart", 0),
        Some(vec!["app/services/checkout.rb:4".to_string()])
    );
    // `Shop` in `class Shop::Basket` is a reference, so a click goes to Shop.
    assert_eq!(p.declaration_usages("app/models/shop.rb", "Shop::Basket", 1), None);
    assert_eq!(p.first("app/models/shop.rb", "Shop::Basket", 1), "Shop @ app/models/shop.rb:1");
    assert_eq!(p.declaration_usages("app/models/shop.rb", "Basket", 0), Some(vec![]));
    // A name inside a method body is not a definition.
    assert_eq!(p.declaration_usages("app/services/checkout.rb", "Order.new", 0), None);
}

#[test]
fn the_name_of_a_method_definition_lists_its_usages_app_first() {
    let p = Project::new(&[
        ("spec/models/order_spec.rb", "RSpec.describe Order do\n  it { Order.new.total }\nend\n"),
        ("app/models/order.rb", "class Order\n  def total = 1\nend\n"),
        ("test/models/order_test.rb", "class OrderTest\n  def test_it = Order.new.total\nend\n"),
        ("app/services/checkout.rb", "class Checkout\n  def run = Order.new.total\nend\n"),
    ]);
    let file = |path: &str| p.index.file_id(std::path::Path::new(path)).unwrap();
    let order = file("app/models/order.rb");
    let (sym, views) = p.index.method_declaration_at(order, 18).expect("the name of `def total`");
    assert_eq!(sym.label(), "Order#total");
    assert!(views.is_empty());
    let rels: Vec<String> = p.index.usages(&sym).iter().map(|u| p.index.display_path(u.file)).collect();
    assert_eq!(rels, ["app/services/checkout.rb", "spec/models/order_spec.rb", "test/models/order_test.rb"]);
    // `new` in a method body is a reference, not a definition.
    assert!(p.index.method_declaration_at(file("app/services/checkout.rb"), 34).is_none());
}
