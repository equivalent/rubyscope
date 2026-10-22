//! Screenshots for the web page in `webpage/`. A made-up shop app on disk,
//! driven the same way as the window tests.
//! `webpage/bin/shots` runs this test and crops the pictures.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use rubyscope_index::{BuildOptions, Index};

use crate::app::App;

const CMD: Modifiers = if crate::keys::MAC { Modifiers::MAC_CMD } else { Modifiers::CTRL };

/// Each step is one commit: (author, date, files).
type Step = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

const ORDER_V1: &str = r#"# An order that a customer places in the shop.
class Order < ApplicationRecord
  belongs_to :customer
  has_many :line_items, dependent: :destroy

  enum :status, { cart: 0, paid: 1, shipped: 2 }

  scope :recent, -> { order(created_at: :desc) }

  validates :email, presence: true

  # The sum of all line items, in cents.
  def total_cents
    line_items.sum(&:price_cents)
  end
end
"#;

const ORDER_V2: &str = r#"# An order that a customer places in the shop.
class Order < ApplicationRecord
  belongs_to :customer
  has_many :line_items, dependent: :destroy

  enum :status, { cart: 0, paid: 1, shipped: 2 }

  scope :recent, -> { order(created_at: :desc) }

  validates :email, presence: true
  before_save :normalize_email

  # The sum of all line items, in cents.
  def total_cents
    line_items.sum(&:price_cents)
  end

  # Marks the order as shipped and emails the customer.
  #
  # @return [Boolean]
  def ship!
    update!(status: :shipped, shipped_at: Time.current)
    OrderMailer.shipped(self).deliver_later
  end

  private

  def normalize_email
    self.email = email.to_s.strip.downcase
  end
end
"#;

const ORDER_V3: &str = r#"# An order that a customer places in the shop.
class Order < ApplicationRecord
  belongs_to :customer
  has_many :line_items, dependent: :destroy

  enum :status, { cart: 0, paid: 1, shipped: 2 }

  scope :recent, -> { order(created_at: :desc) }
  scope :ready_to_ship, -> { paid.where(shipped_at: nil) }

  validates :email, presence: true
  before_save :normalize_email

  # The sum of all line items, in cents.
  def total_cents
    line_items.sum(&:price_cents)
  end

  # Marks the order as shipped and emails the customer.
  #
  # @return [Boolean]
  def ship!
    update!(status: :shipped, shipped_at: Time.current)
    OrderMailer.shipped(self).deliver_later
  end

  private

  def normalize_email
    self.email = email.to_s.strip.downcase
  end
end
"#;

const CONTROLLER: &str = r#"class OrdersController < ApplicationController
  before_action :set_order, only: %i[show edit update]

  def index
    @orders = current_customer.orders.recent
  end

  def show
    @line_items = @order.line_items.includes(:product)
  end

  def edit
  end

  def update
    if @order.update(order_params)
      redirect_to order_path(@order), notice: t(".updated")
    else
      render :edit, status: :unprocessable_entity
    end
  end

  private

  def set_order
    @order = Order.find(params[:id])
  end

  def order_params
    params.require(:order).permit(:email, :note)
  end
end
"#;

const SHOW: &str = r#"<% content_for :title, t(".title", number: @order.id) %>

<section class="order" data-controller="order">
  <h1><%= t(".title", number: @order.id) %></h1>
  <p><%= @order.email %> · <%= @order.status.humanize %></p>

  <table>
    <%= render partial: "orders/line_item", collection: @line_items %>
  </table>

  <p class="total"><%= number_to_currency(@order.total_cents / 100.0) %></p>

  <%= link_to t(".edit"), edit_order_path(@order) %>
  <button data-action="order#ship">Ship</button>
</section>
"#;

const SHIPPING: &str = r#"# Ships every paid order that is still waiting.
class Shipping
  def initialize(warehouse)
    @warehouse = warehouse
  end

  def run
    Order.ready_to_ship.find_each do |order|
      next unless @warehouse.in_stock?(order.line_items)

      order.ship!
      Rails.logger.info("Shipped order #{order.id} to #{order.customer.name}")
    end
  end
end
"#;

const STEPS: &[Step] = &[
    (
        "Ada Novak",
        "2025-03-04T10:00:00",
        &[
            ("Gemfile", "source \"https://rubygems.org\"\n\ngem \"rails\"\n"),
            (
                "app/models/application_record.rb",
                "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
            ),
            ("app/models/order.rb", ORDER_V1),
            (
                "app/models/customer.rb",
                "# Someone who buys in the shop.\nclass Customer < ApplicationRecord\n  has_many :orders\n\n  def name\n    [first_name, last_name].compact.join(\" \")\n  end\nend\n",
            ),
            (
                "app/models/line_item.rb",
                "class LineItem < ApplicationRecord\n  belongs_to :order\n  belongs_to :product\nend\n",
            ),
            ("app/models/product.rb", "class Product < ApplicationRecord\n  has_many :line_items\nend\n"),
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base\n  def current_customer\n    @current_customer ||= Customer.find(session[:customer_id])\n  end\nend\n",
            ),
            ("app/controllers/orders_controller.rb", CONTROLLER),
            ("app/views/orders/show.html.erb", SHOW),
            ("app/views/orders/edit.html.erb", "<h1><%= t(\".title\") %></h1>\n"),
            ("app/views/orders/index.html.erb", "<h1>Orders</h1>\n"),
            (
                "app/views/orders/_line_item.html.erb",
                "<tr>\n  <td><%= line_item.product.name %></td>\n  <td><%= line_item.quantity %></td>\n</tr>\n",
            ),
            (
                "config/routes.rb",
                "Rails.application.routes.draw do\n  resources :orders, only: %i[index show edit update]\nend\n",
            ),
            (
                "config/locales/en.yml",
                "en:\n  orders:\n    show:\n      title: \"Order %{number}\"\n      edit: \"Edit order\"\n    edit:\n      title: \"Edit order\"\n    update:\n      updated: \"The order was saved.\"\n",
            ),
            (
                "db/schema.rb",
                "ActiveRecord::Schema[8.0].define(version: 2025_03_01_000000) do\n  create_table \"orders\" do |t|\n    t.bigint \"customer_id\"\n    t.string \"email\"\n    t.integer \"status\", default: 0\n    t.datetime \"shipped_at\"\n    t.text \"note\"\n    t.timestamps\n  end\n\n  create_table \"customers\" do |t|\n    t.string \"first_name\"\n    t.string \"last_name\"\n  end\n\n  create_table \"line_items\" do |t|\n    t.bigint \"order_id\"\n    t.bigint \"product_id\"\n    t.integer \"quantity\"\n    t.integer \"price_cents\"\n  end\n\n  create_table \"products\" do |t|\n    t.string \"name\"\n  end\nend\n",
            ),
        ],
    ),
    (
        "Ben Ortiz",
        "2025-09-18T15:30:00",
        &[
            ("app/models/order.rb", ORDER_V2),
            (
                "app/mailers/order_mailer.rb",
                "class OrderMailer < ApplicationMailer\n  def shipped(order)\n    @order = order\n    mail(to: order.email)\n  end\nend\n",
            ),
            ("app/views/order_mailer/shipped.html.erb", "<p>Your order is on its way.</p>\n"),
            ("app/services/shipping.rb", SHIPPING),
        ],
    ),
    ("Cleo Park", "2026-10-12T09:15:00", &[("app/models/order.rb", ORDER_V3)]),
    (
        "Dan Weiss",
        "2026-10-20T11:00:00",
        &[
            (
                "app/services/refund.rb",
                "# Gives the money of an order back.\nclass Refund\n  def call(order)\n    order.ship!\n  end\nend\n",
            ),
            ("app/services/gift_card.rb", "class GiftCard\n  # Ships a gift card.\n  def ship!\n  end\nend\n"),
            ("app/services/fulfil.rb", "class Fulfil\n  def call(item)\n    item.ship!\n  end\nend\n"),
        ],
    ),
];

fn fixture() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rubyscope-shots-{}", std::process::id())).join("shop");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str], envs: &[(&str, &str)]| {
        std::process::Command::new("git").current_dir(&dir).args(args).envs(envs.iter().copied()).output().unwrap();
    };
    git(&["init", "-q"], &[]);
    for (author, date, files) in STEPS {
        for (path, code) in *files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, code).unwrap();
        }
        let email = format!("{}@example.com", author.split(' ').next().unwrap().to_lowercase());
        let envs = [
            ("GIT_AUTHOR_NAME", *author),
            ("GIT_AUTHOR_EMAIL", email.as_str()),
            ("GIT_AUTHOR_DATE", *date),
            ("GIT_COMMITTER_NAME", *author),
            ("GIT_COMMITTER_EMAIL", email.as_str()),
            ("GIT_COMMITTER_DATE", *date),
        ];
        git(&["add", "."], &[]);
        git(&["commit", "-q", "-m", "Work on orders"], &envs);
    }
    dir.canonicalize().unwrap()
}

fn harness(root: &Path) -> Harness<'static, App> {
    let index = Arc::new(Index::build(root, &BuildOptions { gems: false, stdlib: false }).unwrap());
    let root = root.to_path_buf();
    let mut h =
        Harness::builder().with_size([1280.0, 760.0]).with_pixels_per_point(2.0).wgpu().build_eframe(move |cc| {
            let mut app = App::new(&cc.egui_ctx, None, None);
            app.open_prebuilt(root.clone(), index.clone());
            app
        });
    h.run_ok();
    h
}

fn settle(h: &mut Harness<'static, App>) {
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(40));
        let _ = h.run_ok();
    }
}

fn shot(h: &mut Harness<'static, App>, name: &str) {
    let dir = std::env::var_os("RUBYSCOPE_SHOTS").expect("set RUBYSCOPE_SHOTS");
    let img = h.render().expect("render");
    std::fs::create_dir_all(&dir).unwrap();
    img.save(PathBuf::from(dir).join(format!("{name}.png"))).unwrap();
}

fn point(h: &Harness<'static, App>, line: usize, col: usize) -> Pos2 {
    let t = h.state().current().expect("a tab");
    let (origin, row_h, char_w, first) = t.layout.expect("drawn");
    Pos2::new(origin.x + (col as f32 - 0.5) * char_w, origin.y + (line - 1 - first) as f32 * row_h + row_h * 0.35)
}

fn open(h: &mut Harness<'static, App>, root: &Path, rel: &str) {
    h.state_mut().open_path(&root.join(rel), None, Some(0), true);
    settle(h);
}

/// Holds Ctrl over a name, so it gets the underline.
fn ctrl_hover(h: &mut Harness<'static, App>, line: usize, col: usize) {
    let pos = point(h, line, col);
    h.event(Event::ModifiersChanged(CMD));
    for _ in 0..4 {
        h.event(Event::PointerMoved(pos));
        let _ = h.run_ok();
    }
}

fn ctrl_click(h: &mut Harness<'static, App>, line: usize, col: usize) {
    let pos = point(h, line, col);
    ctrl_hover(h, line, col);
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: CMD });
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: CMD });
    let _ = h.run_ok();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.event(Event::PointerGone);
    settle(h);
}

fn right_click(h: &mut Harness<'static, App>, line: usize, col: usize) {
    let pos = point(h, line, col);
    h.event(Event::PointerMoved(pos));
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    let _ = h.run_ok();
    let _ = h.run_ok();
}

fn hover(h: &mut Harness<'static, App>, line: usize, col: usize) {
    let pos = point(h, line, col);
    for _ in 0..15 {
        std::thread::sleep(Duration::from_millis(60));
        h.event(Event::PointerMoved(pos));
        let _ = h.run_ok();
    }
}

fn press(h: &mut Harness<'static, App>, mods: Modifiers, key: Key) {
    h.key_press_modifiers(mods, key);
    let _ = h.run_ok();
}

fn type_text(h: &mut Harness<'static, App>, text: &str) {
    h.event(Event::Text(text.to_string()));
    let _ = h.run_ok();
}

#[test]
#[ignore = "writes the web page screenshots"]
fn shots() {
    let root = fixture();

    // Ctrl over a class name in a service: the name gets an underline.
    let mut h = harness(&root);
    open(&mut h, &root, "app/services/shipping.rb");
    ctrl_hover(&mut h, 8, 6);
    shot(&mut h, "ctrl-hover");

    // The click lands on the class.
    ctrl_click(&mut h, 8, 6);
    shot(&mut h, "class-definition");

    // A method on a block variable: the hover knows its type and docs.
    let mut h = harness(&root);
    open(&mut h, &root, "app/services/shipping.rb");
    hover(&mut h, 11, 14);
    shot(&mut h, "hover");

    // Ctrl+click on `ship!` goes to the method.
    ctrl_click(&mut h, 11, 14);
    shot(&mut h, "method-definition");

    // A receiver with no known type: a list to choose from.
    let mut h = harness(&root);
    open(&mut h, &root, "app/services/fulfil.rb");
    ctrl_click(&mut h, 3, 11);
    shot(&mut h, "choose");

    // Controller action to its view, and back.
    let mut h = harness(&root);
    open(&mut h, &root, "app/controllers/orders_controller.rb");
    ctrl_hover(&mut h, 8, 8);
    shot(&mut h, "action-hover");
    ctrl_click(&mut h, 8, 8);
    shot(&mut h, "view-from-action");
    right_click(&mut h, 4, 6);
    shot(&mut h, "view-menu");

    // A lazy translation key.
    let mut h = harness(&root);
    open(&mut h, &root, "app/controllers/orders_controller.rb");
    ctrl_hover(&mut h, 17, 53);
    shot(&mut h, "i18n-hover");
    ctrl_click(&mut h, 17, 53);
    shot(&mut h, "i18n");

    // Ctrl+click on the class name in its definition lists its usages.
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    ctrl_click(&mut h, 2, 9);
    shot(&mut h, "usages");

    // Git blame.
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD, Key::B);
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(60));
        let _ = h.run_ok();
    }
    shot(&mut h, "blame");

    // Go to symbol and the command list.
    let mut h = harness(&root);
    open(&mut h, &root, "app/models/order.rb");
    press(&mut h, CMD, Key::T);
    type_text(&mut h, "ship");
    settle(&mut h);
    shot(&mut h, "go-to-symbol");
    press(&mut h, Modifiers::NONE, Key::Escape);
    press(&mut h, CMD, Key::K);
    settle(&mut h);
    shot(&mut h, "commands");
    press(&mut h, Modifiers::NONE, Key::Escape);
    press(&mut h, CMD, Key::P);
    type_text(&mut h, "orders show");
    settle(&mut h);
    shot(&mut h, "go-to-file");
}
