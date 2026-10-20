//! Rails, RSpec, FactoryBot, Stimulus and type rules, tested on a small
//! app kept in memory.

use rubyscope_index::{Confidence, Index};

const FILES: &[(&str, &str)] = &[
    (
        "app/models/application_record.rb",
        "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
    ),
    (
        "app/models/entity.rb",
        "class Entity < ApplicationRecord\n  has_many :invoices\n  has_many :items, through: :invoices\n  scope :active, -> { where(active: true) }\n  enum :kind, { company: 0, person: 1 }, prefix: true\n  delegate :email, to: :owner\n  belongs_to :owner, class_name: \"User\"\n\n  # The newest invoice.\n  # @return [Invoice]\n  def latest_invoice\n    invoices.order(:created_at).last\n  end\nend\n",
    ),
    (
        "app/models/invoice.rb",
        "class Invoice < ApplicationRecord\n  belongs_to :entity\n  has_many :items, class_name: \"InvoiceItem\"\n  before_save :set_number, if: :draft?\n\n  def draft? = true\n\n  private\n\n  def set_number; end\nend\n",
    ),
    ("app/models/invoice_item.rb", "class InvoiceItem < ApplicationRecord\n  belongs_to :invoice\nend\n"),
    ("app/models/user.rb", "class User < ApplicationRecord\n  def email; end\nend\n"),
    (
        "db/schema.rb",
        "ActiveRecord::Schema[8.0].define(version: 1) do\n  create_table \"invoices\" do |t|\n    t.string \"number\"\n    t.decimal \"total\"\n  end\n  create_table \"entities\" do |t|\n    t.string \"name\"\n  end\nend\n",
    ),
    (
        "app/services/report.rb",
        "class Report\n  def initialize(entity)\n    @entity = entity\n  end\n\n  def call\n    invoice = Invoice.find(1)\n    invoice.number\n    invoice.entity.name\n    Entity.active.where(x: 1).each { |e| e.latest_invoice }\n    @entity.latest_invoice.total\n    @entity.kind_company?\n    @entity.email\n    Invoice.where(a: 1)\n  end\nend\n",
    ),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\n  namespace :admin do\n    resources :invoices, only: [:index, :show]\n  end\n  get \"about\", to: \"pages#about\"\nend\n",
    ),
    (
        "app/controllers/admin/invoices_controller.rb",
        "module Admin\n  class InvoicesController < ApplicationController\n    def show\n      redirect_to admin_invoices_path, notice: t(\".done\")\n      about_url\n    end\n  end\nend\n",
    ),
    ("app/controllers/pages_controller.rb", "class PagesController < ApplicationController\n  def about; end\nend\n"),
    ("config/locales/en.yml", "en:\n  admin:\n    invoices:\n      show:\n        done: \"Done\"\n"),
    (
        "app/views/invoices/show.html.erb",
        "<div data-controller=\"invoice-form\" data-action=\"click->invoice-form#save\">\n  <%= render \"form\" %>\n</div>\n",
    ),
    ("app/views/invoices/_form.html.erb", "<form></form>\n"),
    (
        "app/javascript/controllers/invoice_form_controller.js",
        "export default class extends Controller {\n  save() {}\n}\n",
    ),
    (
        "spec/factories/invoices.rb",
        "FactoryBot.define do\n  factory :invoice do\n    trait :paid do\n    end\n  end\n  factory :big_invoice, class: \"Invoice\" do\n  end\nend\n",
    ),
    (
        "spec/models/invoice_spec.rb",
        "RSpec.describe Invoice do\n  let(:invoice) { create(:invoice, :paid) }\n  def helper_here; end\n\n  it \"works\" do\n    described_class\n    invoice.number\n    helper_here\n    big = create(:big_invoice)\n    big.entity\n  end\nend\n",
    ),
];

fn index() -> Index {
    Index::from_memory(FILES)
}

/// Clicks on the `nth` (0-based) match of `needle` in `path`, `skip`
/// characters in, and returns `label @ path:line (confidence)` for the
/// first target.
fn click(index: &Index, path: &str, needle: &str, nth: usize, skip: usize) -> String {
    let (_, code) = FILES.iter().find(|(p, _)| *p == path).expect("file");
    let offset = code.match_indices(needle).nth(nth).unwrap_or_else(|| panic!("{needle} not in {path}")).0 + skip;
    let file = index.file_id(std::path::Path::new(path)).expect("file id");
    let Some(res) = index.definition_at(file, offset as u32) else { return "nothing".into() };
    let Some(t) = res.targets.first() else { return "no targets".into() };
    let line = index.files[t.loc.file as usize].raw.line_of(t.loc.span.start) + 1;
    let conf = match t.confidence {
        Confidence::Exact => "",
        Confidence::Inferred => " (inferred)",
        Confidence::Guess => " (guess)",
        Confidence::NameOnly => " (name only)",
    };
    format!("{} @ {}:{line}{conf}", t.label, index.display_path(t.loc.file))
}

#[test]
fn associations_scopes_enums_and_delegates() {
    let ix = index();
    assert_eq!(click(&ix, "app/models/entity.rb", ":invoices", 0, 1), "Invoice @ app/models/invoice.rb:1");
    assert_eq!(
        click(&ix, "app/models/invoice.rb", "\"InvoiceItem\"", 0, 1),
        "InvoiceItem @ app/models/invoice_item.rb:1"
    );
    assert_eq!(click(&ix, "app/services/report.rb", "active", 0, 0), "Entity.active @ app/models/entity.rb:4");
    assert_eq!(
        click(&ix, "app/services/report.rb", "kind_company?", 0, 0),
        "Entity#kind_company? @ app/models/entity.rb:5 (guess)"
    );
    // `delegate :email, to: :owner` shows the delegate line first.
    assert_eq!(click(&ix, "app/services/report.rb", "email", 0, 0), "Entity#email @ app/models/entity.rb:6 (guess)");
}

#[test]
fn callback_symbols_go_to_methods() {
    let ix = index();
    assert_eq!(
        click(&ix, "app/models/invoice.rb", ":set_number", 0, 1),
        "Invoice#set_number @ app/models/invoice.rb:10"
    );
    assert_eq!(click(&ix, "app/models/invoice.rb", ":draft?", 0, 1), "Invoice#draft? @ app/models/invoice.rb:6");
}

#[test]
fn types_of_variables() {
    let ix = index();
    let r = "app/services/report.rb";
    // `invoice = Invoice.find(1)`; `number` is a column.
    assert_eq!(click(&ix, r, "number", 0, 0), "Invoice#number @ db/schema.rb:3 (inferred)");
    // Through an association to a column of another table.
    assert_eq!(click(&ix, r, "name", 0, 0), "Entity#name @ db/schema.rb:7 (inferred)");
    // A block parameter of a relation.
    assert_eq!(click(&ix, r, "latest_invoice }", 0, 0), "Entity#latest_invoice @ app/models/entity.rb:11 (inferred)");
    // `@entity = entity` (a parameter, so a guess), then a YARD @return.
    assert_eq!(click(&ix, r, "total", 0, 0), "Invoice#total @ db/schema.rb:4 (guess)");
}

#[test]
fn routes_and_translations() {
    let ix = index();
    let c = "app/controllers/admin/invoices_controller.rb";
    assert_eq!(click(&ix, c, "admin_invoices_path", 0, 0), "admin_invoices_path @ config/routes.rb:3");
    assert_eq!(click(&ix, c, "about_url", 0, 0), "about_path @ config/routes.rb:5");
    assert_eq!(click(&ix, c, ".done", 0, 0), "en.admin.invoices.show.done @ config/locales/en.yml:5");
    // In the routes file: the resource goes to its controller, `to:` to the action.
    assert_eq!(
        click(&ix, "config/routes.rb", ":invoices", 0, 1),
        "Admin::InvoicesController @ app/controllers/admin/invoices_controller.rb:2"
    );
    assert_eq!(
        click(&ix, "config/routes.rb", "pages#about", 0, 7),
        "PagesController#about @ app/controllers/pages_controller.rb:2"
    );
}

#[test]
fn erb_partials_and_stimulus() {
    let ix = index();
    let v = "app/views/invoices/show.html.erb";
    assert_eq!(
        click(&ix, v, "\"form\"", 0, 1),
        "app/views/invoices/_form.html.erb @ app/views/invoices/_form.html.erb:1"
    );
    assert_eq!(
        click(&ix, v, "invoice-form", 0, 0),
        "invoice-form @ app/javascript/controllers/invoice_form_controller.js:1"
    );
    assert_eq!(
        click(&ix, v, "#save", 0, 1),
        "invoice-form#save @ app/javascript/controllers/invoice_form_controller.js:2"
    );
}

#[test]
fn rspec_and_factories() {
    let ix = index();
    let s = "spec/models/invoice_spec.rb";
    assert_eq!(click(&ix, s, "described_class", 0, 0), "Invoice @ app/models/invoice.rb:1");
    assert_eq!(click(&ix, s, "invoice.number", 0, 0), "let(:invoice) @ spec/models/invoice_spec.rb:2");
    assert_eq!(click(&ix, s, "number", 0, 0), "Invoice#number @ db/schema.rb:3 (inferred)");
    assert_eq!(click(&ix, s, ":invoice,", 0, 1), "factory :invoice @ spec/factories/invoices.rb:2");
    assert_eq!(click(&ix, s, ":paid", 0, 1), "trait :paid @ spec/factories/invoices.rb:3");
    assert_eq!(click(&ix, s, "helper_here\n", 0, 0), "(block)#helper_here @ spec/models/invoice_spec.rb:3");
    // `create(:big_invoice)` uses the factory's `class:`.
    assert_eq!(click(&ix, s, "big.entity", 0, 4), "Invoice#entity @ app/models/invoice.rb:2 (inferred)");
}

#[test]
fn usages_of_a_method() {
    let ix = index();
    let file = ix.file_id(std::path::Path::new("app/models/entity.rb")).unwrap();
    let code = FILES.iter().find(|(p, _)| *p == "app/models/entity.rb").unwrap().1;
    let offset = code.find("latest_invoice").unwrap() as u32;
    let sym = ix.symbol_at(file, offset).expect("symbol");
    let usages = ix.usages(&sym);
    assert_eq!(usages.len(), 2, "{usages:?}");
}

#[test]
fn controller_actions_and_their_views() {
    let ix = Index::from_memory(&[
        (
            "app/controllers/invoices_controller.rb",
            "class InvoicesController < ApplicationController\n  def edit\n    load_it\n  end\n\n  def show; end\nend\n",
        ),
        (
            "app/controllers/invoices/payments_controller.rb",
            "class Invoices::PaymentsController\n  def edit; end\nend\n",
        ),
        ("app/views/invoices/edit.html.erb", "<p>edit</p>\n"),
        ("app/views/invoices/native/edit.rb", "module Views\n  class Invoices::Native::Edit\n  end\nend\n"),
        ("app/views/invoices/payments/edit.rb", "class Views::Invoices::Payments::Edit\nend\n"),
        ("app/views/invoices/_row.html.erb", "<tr></tr>\n"),
        ("app/views/invoices/row.rb", "class Views::Invoices::Row\nend\n"),
    ]);
    let file = |p: &str| ix.file_id(std::path::Path::new(p)).unwrap();
    let ctrl = file("app/controllers/invoices_controller.rb");
    // Anywhere inside the action: its template and its variant, not the
    // view of `Invoices::PaymentsController#edit`.
    let (label, views) = ix.related_at(ctrl, 66).unwrap();
    assert_eq!(label, "Go to view");
    let rels: Vec<String> = views.iter().map(|t| ix.display_path(t.loc.file)).collect();
    assert_eq!(rels, ["app/views/invoices/edit.html.erb", "app/views/invoices/native/edit.rb"]);
    // Ctrl+click on the action name goes to the views too.
    let res = ix.definition_at(ctrl, 56).unwrap();
    assert_eq!(res.targets.len(), 2);
    // An action with no view: nothing related, and the click shows the method.
    assert!(ix.related_at(ctrl, 86).is_none());

    let (label, actions) = ix.related_at(file("app/views/invoices/native/edit.rb"), 0).unwrap();
    assert_eq!(label, "Go to controller action");
    assert_eq!(actions[0].label, "InvoicesController#edit");
    let (_, actions) = ix.related_at(file("app/views/invoices/payments/edit.rb"), 0).unwrap();
    assert_eq!(actions[0].label, "Invoices::PaymentsController#edit");
    // A view with no action of its name goes to the controller.
    assert_eq!(ix.related_at(file("app/views/invoices/row.rb"), 0).unwrap().0, "Go to controller");
    assert!(ix.related_at(file("app/views/invoices/_row.html.erb"), 0).is_none());
}
