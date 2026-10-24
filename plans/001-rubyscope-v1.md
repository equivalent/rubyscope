# Rubyscope v1 plan

Status: steps 1–6 built and tested on 2026-10-20. See "Status" at the end
for what is done, the numbers, and what is not done yet.

## What Rubyscope is

Rubyscope is a desktop app for **reading** Ruby code. It is not an editor.
You open a project folder and click around the code. Ctrl+click on a
class, module, method, constant, route helper, i18n key, partial, RSpec `let`
or factory goes to its definition. The aim is RubyMine-level depth for "go
to definition" and "find usages".

- Written in Rust.
- Runs on Omarchy (Arch Linux, Hyprland, Wayland) now. macOS comes later.
- The main test project is a large private Rails app.
- Read only, forever. There is an "open in nvim" action for editing.

## Decisions

| Topic | Decision |
|---|---|
| Name and place | `rubyscope`, in `~/git/rubyscope` |
| App type | Native window, Rust + `egui` (`eframe`) |
| Ruby parser | `ruby-prism` crate (Prism, the parser Ruby 4 uses) |
| Projects | Any folder, one window per folder: `rubyscope <folder>`. Worktrees work the same way. |
| Gems | Indexed too. Clicks go into gem source, for example Rails. |
| Rails | Zeitwerk constants, DSL methods, variable types, routes, i18n, partials |
| RSpec | Full: `let`, `subject`, `described_class`, shared examples, FactoryBot |
| Other files | Open with colors. No links, except Stimulus (see below). |
| Editing | Never. Read only. |
| Keys | Short list (below). Everything else is done with the mouse and the Ctrl+K command list. |
| Git blame | Side panel on the left of the code, Ctrl+B shows or hides it |
| Git | Local repo first. |
| Permission | Rubyscope development may run `bundle install` and read-only `bin/rails runner` in the test project |

## Facts about large Rails apps that the design must handle

- Thousands of Ruby files, about half of them in `spec/`.
- Ruby 4, Rails 8, Zeitwerk, Phlex. No Sorbet types.
- Views and components can be Phlex classes, with only a few ERB files.
- Initializers can change the Zeitwerk setup with `push_dir(..., namespace:)`,
  for example `app/views` → `Views` and `app/components/ui` → `UI`. Some
  folders are ignored. `config.autoload_lib_once(ignore: ...)` loads `lib/`.
  **So the file → constant rules can not come from Rails defaults only.**
- Phlex kits (`extend Phlex::Kit`): inside a kit, `Button(...)` renders
  `UI::Button`. That call must be a link to the component class.
- Routes: `config/routes.rb` can load more files from `config/routes/` with
  `draw :name`.
- Hundreds of locale YAML files in `config/locales/`.
- Many Stimulus controllers in `app/javascript/controllers/`, some in
  subfolders (`admin/reports_controller.js` → `admin--reports`).
- Many factory files in `spec/factories/`.
- Some gems may not be installed yet (git gems too). Run `bundle install`
  first.

## Architecture

One Cargo workspace with three crates:

```
rubyscope/
  crates/
    index/   parse, build the index, resolve clicks. No GUI code.
    app/     the egui window. Depends on index.
    cli/     test command. Depends on index.
  probe/
    rails_probe.rb   script that runs inside booted Rails (see "Rails probe")
  plans/
```

The `index` crate has no GUI code, so all the hard logic is tested
from the CLI and from Rust tests, without a window.

### Main libraries

| Need | Crate |
|---|---|
| Ruby parsing | `ruby-prism` (needs clang, which is installed) |
| Window | `eframe` / `egui` (wgpu, Wayland) |
| Parallel parsing | `rayon` |
| Saved index | `bincode` (or `rkyv` if loading is too slow) |
| File watcher | `notify` |
| YAML with line numbers | `saphyr` (or another YAML parser that keeps positions) |
| Colors for non-Ruby files | `syntect` |
| Fuzzy search | `nucleo` (the matcher Helix uses) |
| Text search | `grep-searcher` + `ignore` (ripgrep's crates) |
| Git blame | `git blame --porcelain` in the background (move to `gix` if it is too slow) |

### Saved index

- Stored in `~/.cache/rubyscope/<hash of project path>/`.
- Each file entry stores its size, its modification time and a content hash.
  At start, only changed files are parsed again.
- Gem folders do not change for a given version, so the gem index is saved
  once per gem version and shared by all projects that use that version.
- The file watcher updates the index when files change, for example after a
  `git pull` or a branch switch.

## The index (data model)

For every Ruby file, Prism gives a syntax tree. One pass over the tree
collects:

- **Namespaces:** every `class` and `module`, with each place it is opened
  (a class can be opened in many files), its superclass, and its
  `include` / `extend` / `prepend` lists.
- **Constants:** `FOO = ...`, classes and modules, with their location.
- **Methods:** `def x`, `def self.x`, `class << self`, with parameters,
  visibility (`private`, `protected`, `private def`), the comment above the
  method, and the location.
- **Generated methods:** methods that a DSL makes (see "Rails DSL rules").
  Each one points to the DSL line, for example to the `has_many` line.
- **References:** every constant read and every method call in the file,
  with its byte range and its lexical scope (the nested `module` / `class`
  blocks around it). A click at a position finds its reference fast.
- **Local facts for type inference:** variable assignments, block
  parameters, instance variable assignments, and the last expression of each
  method.

Everything is keyed by id, so "find usages" is a lookup and does not need a
new parse.

## How a click is resolved

### 1. Constants

Ruby's real lookup order:

1. The lexical scopes, from the innermost `module` / `class` block outward.
2. The ancestors of the innermost class or module.
3. The top level (`Object`).

`Foo::Bar` resolves `Foo` first, then looks for `Bar` inside it. `::Foo`
starts at the top level.

When a constant is not defined anywhere in the parsed code, Zeitwerk's
file rules are the backup. They use the real autoload roots from the Rails
probe, including `push_dir(..., namespace:)`, collapsed folders and ignored
folders.

### 2. Methods

1. Find the type of the receiver (next section). No receiver means `self`.
2. Walk the ancestors in Ruby's order: prepended modules, the class itself,
   included modules (the last one included comes first), then the
   superclass, and the same again for it.
3. Class methods use the singleton side: `def self.x`, `class << self`,
   `extend`ed modules, and `class_methods do` in concerns.
4. The first match is the definition. `super` goes to the next match up the
   chain.

### 3. Types of variables (built in levels)

Each level adds cases. The app shows a small "guess" mark when the answer
comes from levels 5 or 6.

1. `self`, constants, literals (`"x"` is a `String`, `[]` is an `Array`).
2. Constructors and finders: `X.new`, `X.find`, `X.find_by`, `X.create!`,
   `X.first`, `X.take` give `X`. `X.where`, `X.order`, `X.all`, scopes and
   other relation methods give "relation of X". `.each { |x| }`, `.map`,
   `.find_each` on a relation make `x` an `X`.
3. Associations: `has_many :invoices` gives "relation of Invoice",
   `belongs_to :user` gives `User`. `class_name:` and `through:` are read.
4. Instance variables: `@invoice = ...` anywhere in the same class. In Phlex
   components, `initialize(invoice:)` followed by `@invoice = invoice`.
5. Return types: when the last expression of a method has a known type,
   the method returns that type. The depth is limited, so a loop can not
   run forever. YARD `@return [Type]` and `@param` comments, and RBS files
   from gems, are also used.
6. Names: a variable named `invoice` or `invoices` probably holds an
   `Invoice` (only when an `Invoice` constant exists).

When the type is still unknown, the app shows a list of all methods with
that name. Methods in the same namespace come first, then project methods,
then gem methods. This is the same as RubyMine's "choose declaration" list.

### 4. Rails DSL rules

Each rule says which methods a DSL call makes:

- `has_many`, `has_one`, `belongs_to`, `has_and_belongs_to_many`
  (readers, writers, `build_x`, `x_ids`, ...)
- `scope`, `enum` (predicate and bang methods, scopes, the class method)
- `attr_reader`, `attr_writer`, `attr_accessor`, `attribute`
- `delegate :x, to: :y` (goes to `y`'s `x` when the type of `y` is known)
- `alias_method`, `alias`
- `define_method(:literal_name)`
- `ActiveSupport::Concern`: `included do`, `class_methods do`
- Callbacks and validations with symbols: `before_action :load_invoice`,
  `validate :check_total`, `after_commit :notify`. The symbol is a link to
  the method.
- `send(:x)` and `public_send(:x)` with a literal symbol.
- Database columns: Rubyscope reads `db/schema.rb` (or `structure.sql`), so
  `invoice.total_cents` goes to the column line.
- Phlex kits: `Button(...)` inside a kit goes to `UI::Button`.

### 5. Routes

- Rubyscope reads `config/routes.rb` and follows `draw :name` into
  `config/routes/<name>.rb`.
- It understands `resources`, `resource`, `namespace`, `scope`, `member`,
  `collection`, `get` / `post` / `patch` / `put` / `delete`, `root`, `as:`,
  `only:`, `except:`, `module:`, `controller:`, `to: "c#a"`, `constraints`
  blocks and `mount`.
- From these it builds the helper names (`invoice_path`, `invoice_url`,
  `edit_admin_invoice_path`, ...). A helper goes to its route line. The
  route line goes to `controller#action`.
- The Rails probe can print the real `Rails.application.routes` list. The
  tests compare the two.

### 6. i18n

- Rubyscope reads all locale YAML files with line numbers and flattens the
  keys (`en.invoices.show.title` → file and line).
- `t("a.b")` and `I18n.t("a.b")` go to the key in every locale. A popup
  lets you choose the locale.
- Lazy keys `t(".title")` use the view, component or controller path, the
  same way Rails does.

### 7. Partials and templates

- `render "x/y"` and `render partial: "x/y"` go to `app/views/x/_y.*`.
- `render Components::Foo.new(...)` is a normal constant link.

### 8. RSpec

- `invoice` inside a spec goes to `let(:invoice)` or `let!(:invoice)` in the
  nearest `describe` / `context` block that defines it. `subject` works the
  same way.
- `described_class` goes to the class in the top `describe`.
- `it_behaves_like "x"` and `include_examples "x"` go to
  `shared_examples "x"`.
- `create(:invoice)`, `build(:invoice, :paid)` go to the factory and the
  trait in `spec/factories/`.

### 9. Stimulus

- In Ruby and ERB, `data-controller="admin--sales-charts"` (also as a
  Phlex hash, `data: { controller: "..." }`) goes to
  `app/javascript/controllers/admin/sales_charts_controller.js`.
- `data-action="click->tabs#select"` goes to the `select` method in that
  JS file. This is a simple text search in the JS file, not a JS parser.

### 10. Gems and core Ruby

- Rubyscope reads `Gemfile.lock` and finds each gem folder in the gem
  directory, including git gems under `bundler/gems`. It indexes `lib/`
  and `app/` (for engines).
- Core classes (`String`, `Hash`, ...) are written in C. Their methods go to
  the RBS signature files from the `rbs` gem, which are read only.

## Rails probe

`probe/rails_probe.rb` runs inside booted Rails with
`bin/rails runner`. It prints JSON with:

- The autoload roots and their namespaces, collapsed folders and ignored
  folders (`Rails.autoloaders.main`).
- The routes (`Rails.application.routes.routes`).
- For checks only: the real source location of every constant
  (`Module.const_source_location`) and every method
  (`instance_method(:x).source_location`), after `eager_load!`.

The probe is optional. When it can not run (for example, no Ruby or no
database config), Rubyscope reads the `push_dir` calls in the initializers
and uses Rails defaults.

The probe does not write to the database.

## The window

```
┌ rubyscope — shop ─────────────────────────────────────────────────┐
│ ← →  Invoices::PdfGenerator > #call                       Ctrl+K │
├──────────────┬──────────┬─────────────────────────────────────────┤
│ app/         │ blame    │ module Invoices                         │
│  models/     │ Ben    3m│   class PdfGenerator                    │
│   invoice.rb │ "Add VAT"│     def call                            │
│  services/   │          │       invoice = Invoice.find(id)        │
│              │ Ana    1y│       invoice.total   ← Ctrl+click      │
│              │          ├─────────────────────────────────────────┤
│              │          │ Usages of Invoice#total (12)            │
└──────────────┴──────────┴─────────────────────────────────────────┘
```

- **Top bar:** back and forward buttons, a breadcrumb you can click
  (`Invoices::PdfGenerator > #call`), and a button for the command list.
- **Left:** the file tree. Ctrl+Shift+B shows or hides it.
- **Blame panel:** see "Git blame".
- **Middle:** the code, in tabs, with colors. Code that is a link shows an
  underline while Ctrl is held down.
- **Bottom:** usage lists and search results. It opens only when needed.
- **Theme:** colors and font come from the current Omarchy theme
  (`~/.config/omarchy/current/theme`). There is a dark default.

### Mouse

| Action | What it does |
|---|---|
| Ctrl+click on a symbol | Go to the definition (or a choose list when there are many) |
| Hover over a symbol | Docs, signature, and the type that Rubyscope found |
| Right-click on a symbol | Menu: find usages, type hierarchy, call hierarchy, copy name, open in nvim |
| Mouse side buttons, ← → buttons | Back and forward |
| Click on the breadcrumb | Go to that class or method |
| Click in the file tree, tabs, usage list, search results | Open that file or line |

### Keys

| Key | What it does |
|---|---|
| Ctrl+P | Find a file by name (go to file) |
| Ctrl+T | Find a symbol by name (class, module, method, constant) |
| Ctrl+Shift+F | Search all text in the project |
| Ctrl+F | Find text in the current file |
| Ctrl+K | Show all commands |
| Ctrl+G | Go to a line |
| Ctrl+W | Close the tab |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+B | Show or hide the git blame panel |
| Ctrl+Shift+B | Show or hide the side bar (file tree) |
| Esc | Close a popup |

The keys are in a config file (`~/.config/rubyscope/keys.toml`). No key in
this list is used by Omarchy or Hyprland (checked 2026-10-20). More keys
can be added later. Every action is also in the Ctrl+K command list.

### Git blame

- A narrow column on the left of the code shows the author, how long ago
  (for example "3 months ago") and the start of the commit message.
- Lines from the same commit form one block. Newer changes have a stronger
  color than old ones.
- Hover shows the full commit message, the date and the commit hash.
- Click opens the commit on GitHub, or its PR when there is one.
- Blame runs in the background. The file opens at once, and the blame
  shows a moment later. The result is kept until the file or the branch
  changes.

## The CLI

The CLI is for tests and for checking resolution without the window:

```
rubyscope-cli index ~/code/shop                 # build the index, print counts and time
rubyscope-cli def   app/models/invoice.rb:12:5  # where the symbol at this position is defined
rubyscope-cli refs  app/models/invoice.rb:12:5  # all usages
rubyscope-cli type  app/models/invoice.rb:12:5  # the type rubyscope found, and from which level
rubyscope-cli check probe.json                  # compare the index with the Rails probe
```

## How correctness is checked

1. **Rails probe check.** The probe gives the real location of every
   constant and method in the project. `rubyscope-cli check` prints the percentage
   that Rubyscope finds at the same file and line, and lists the misses.
2. **Golden click tests.** A golden file has real click points from a
   project with the expected target: one line per click,
   `file:line:col → file:line`. They cover constants, DSL methods, variable
   types, routes, i18n, RSpec, factories and Stimulus. The list grows each
   time a click goes to the wrong place.
3. **Rust unit tests** in `index` for each rule, on small Ruby snippets.

## Speed targets

| What | Target |
|---|---|
| First full index (a large app and its gems) | under 10 seconds |
| Start with a saved index | under 1 second |
| From Ctrl+click to the definition on screen | under 50 ms |
| Find usages of a common method | under 200 ms |
| Open a 5,000-line file and scroll | smooth, 60 frames per second |

## Steps

Each step ends with something that works.

### Step 1: index and CLI

- Cargo workspace, `ruby-prism` build, file discovery (respect
  `.gitignore`).
- Parse the whole project in parallel. Collect namespaces, constants, methods
  and references.
- Constant resolution with lexical scopes and ancestors.
- Rails probe script, and `rubyscope-cli check` for constants.
- **Done when:** `rubyscope-cli def` finds constants in the project correctly,
  the probe check shows the constant match rate, and a full parse of the
  project takes a few seconds.

### Step 2: the window

- `eframe` app: file tree, tabs, code view with colors, Ctrl+click on
  constants and simple method calls (`self` and constant receivers).
- Back and forward, breadcrumb.
- Ctrl+P, Ctrl+T, Ctrl+F, Ctrl+G, Ctrl+W, Ctrl+Tab, Ctrl+K, Esc,
  Ctrl+Shift+B.
- **Done when:** you can open the project and click from a controller to a model
  to a concern and back.

### Step 3: gems, DSL methods, usages, saved index

- Gem discovery from `Gemfile.lock`, gem index shared per gem version.
- Rails DSL rules, `db/schema.rb` columns, Phlex kits.
- Find usages (bottom panel), Ctrl+Shift+F text search.
- Saved index and file watcher.
- **Done when:** Ctrl+click on `has_many` goes into Rails source, a click on
  an association method goes to its `has_many` line, and a second start
  takes under 1 second.

### Step 4: types of variables

- Type inference levels 1 to 6.
- "Guess" mark and the choose list.
- `rubyscope-cli type`.
- **Done when:** the method match rate from the probe check is high, and
  the golden tests for variable types pass.

### Step 5: Rails extras, RSpec, Stimulus

- Routes, i18n, partials, symbol arguments.
- RSpec `let` / `subject` / `described_class` / shared examples, FactoryBot.
- Stimulus links.
- **Done when:** the golden tests for these pass.

### Step 6: finish

- Git blame panel (Ctrl+B).
- Hover docs, type hierarchy, call hierarchy.
- Omarchy theme colors, desktop launcher (`.desktop` file), open in nvim.
- **Done when:** Rubyscope is the tool used every day to read the project.

### Later (not in v1)

- macOS support (mostly paths, fonts and keys; `egui` already runs there).
- More keys, optional vim keys.
- More languages for links (JS/TS).

## Limits

- Methods with names that only exist at runtime (`define_method` with a
  variable name, `method_missing`, `class_eval` with a string) can not be
  found by reading the code. RubyMine can not find them either. Rubyscope
  shows the name-search list.
- Some variable types are guesses. The app always shows when it guesses.
- Without the Rails probe, custom Zeitwerk setups are read from the
  initializers only, so some unusual setups can be missed.

## Status (2026-10-20)

All six steps are built. Checked on a large private Rails app (about
7,000 Ruby files, 340 gems).

| Check | Result |
|---|---|
| Rails probe: project constants at the right file and line | 100% |
| Rails probe: project methods (through Ruby's lookup) | 98.6%; the rest are made at runtime |
| Rails probe: gem methods | 95.8% |
| Golden clicks (kept outside the repo) | 44 / 44 |
| Index rule tests (in memory, saved index) | 46 pass |
| Window tests (egui_kittest, real keys and clicks) | 24 pass |
| Full index of the app + its gems + stdlib | about 2.3 s |
| Start with the saved index (real app) | 0.6–0.85 s |
| Real window on Hyprland / Wayland | starts, restores tabs, idles at 0% CPU |

Not done yet, compared with this plan:

- **Saved index: done, with changes from this plan.** One file per
  project, `~/.cache/rubyscope/<project>-<hash>.bin` (postcard). Each file
  has a size and change time, not a content hash. Gem files are not
  shared between projects. Link results are not saved: linking runs at
  each start (about 0.42 s). References and local variables stay as bytes
  until a click or "find usages" needs them.
- **RBS for core classes.** `"x".strip` gives a name-only list, because
  `String` is written in C.
- **Zeitwerk roots from the probe** are not used: definitions are found by
  reading the code, which covered every project constant.
- **Memory:** about 560 MB for that app with all gems, from the saved
  index or from a new index (before: 685 MB from the saved index and
  1.2 GB from a new index). The references of each file stay as bytes until
  a click needs them, also after a new index. The saved index is mapped,
  not read. The first "find usages" adds about 100 MB (before: about
  400 MB, because it kept every file's references decoded).
- **Re-index after a change:** only changed files are parsed, but linking
  runs again for the whole project (about 0.6 s for that app).
- **macOS** (planned for later).
