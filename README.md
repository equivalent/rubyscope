# rubyscope

rubyscope is a desktop app for reading Ruby and Rails code. Hold Ctrl (Cmd
on macOS) and click a name to go to its definition. The name can be a class, module,
constant, method, route helper, translation key, partial, RSpec `let` or
factory. rubyscope reads code only. It never changes a file.

It is for people who read large Rails apps and want go-to-definition as
good as RubyMine's, in a small native app that starts in about one second.

rubyscope is not:

- an editor. To change a file, open it in your editor from rubyscope.
- a language server (LSP). It does not connect to an editor.
- a tool that runs your code. It reads the source and does not boot Rails.

The design plan is in `plans/001-rubyscope-v1.md`. Its "Status" section at
the end tells what is done.

## Requirements

- Linux or macOS. On Linux, rubyscope is built and used on Wayland
  (Hyprland). The build also has X11 support, but it is not checked. On
  macOS it runs on Apple silicon.
- Rust 1.88 or newer.
- clang and libclang. The `ruby-prism` crate compiles the Prism C parser
  and makes its Rust bindings with bindgen.
- Optional, for more features:

| Tool | What it gives |
|---|---|
| `ruby` | Gems and the standard library in the index. rubyscope runs `ruby` in the project folder, so mise, rbenv or the `ruby` on `PATH` selects the version. |
| Installed gems (`bundle install`) | Clicks that go into gem source. Only gems that are installed are indexed. |
| `git` | Git blame and "Open on GitHub" |
| `gh` (GitHub CLI) | A click on a blame block opens the pull request of the commit, not only the commit |
| `fc-match` (fontconfig) | Linux: the system monospace font for code. macOS uses Menlo. |
| `xdg-open` | Linux: opens links in the browser. macOS uses `open`. |
| Omarchy | Colors from the current Omarchy theme. Without Omarchy, the colors are Tokyo Night. |

## Build and install

```
bin/install
```

This builds the release binaries and installs:

- `~/.local/bin/rubyscope` (the window)
- `~/.local/bin/rubyscope-cli` (the test command)
- `~/.local/share/applications/rubyscope.desktop` (the app launcher entry)
- `~/.local/share/icons/hicolor/*/apps/rubyscope.*` (the app icon, from
  `packaging/icon/`)

Make sure `~/.local/bin` is on your `PATH`.

On macOS, run this:

```
bin/install-macos
```

It builds the release binaries and installs:

- `~/Applications/rubyscope.app` (open it from Spotlight, Launchpad or the
  Dock)
- `~/.local/bin/rubyscope` (a link to the binary in the app)
- `~/.local/bin/rubyscope-cli` (the test command)

An app started from Finder or the Dock gets only the system `PATH`. So on
macOS, rubyscope reads the `PATH` from your login shell when it starts.
Then `ruby` is the Ruby of rbenv, mise or Homebrew, the same as in the
terminal.

To build without installing:

```
cargo build --release
target/release/rubyscope ~/code/shop
```

## Start rubyscope

```
rubyscope ~/code/shop                                # open a project folder
rubyscope ~/code/shop/app/models/invoice.rb:40       # open a file at line 40
rubyscope                                            # see below
```

- With a file and no folder, the project is the nearest parent folder that
  has `.git` or a `Gemfile`.
- With no arguments, rubyscope opens the current folder when it has `.git`
  or a `Gemfile`. Otherwise it shows the start screen.
- The start screen has a path field (`~/` is allowed), a "Browse…" button
  for the system folder dialog, and the recent projects. Click a recent
  project to open it.
- Ctrl+O opens the system folder dialog at any time.
- There is one window for each project. When a project is open, Ctrl+O
  opens the new folder in a new window.
- rubyscope remembers the open tabs of each project and opens them again
  at the next start.
- The app launcher entry also lets a file manager open a folder or a `.rb`
  file with rubyscope.

## First steps

1. Open a project: `rubyscope ~/code/shop`. The index builds in the
   background. The top bar shows "indexing…", then the number of files
   and the time. The first index of a large app takes a few seconds. The
   next start uses the saved index and takes about one second.
2. Open a file. Press Ctrl+P and type part of the path, or click it in the
   file tree. Press Ctrl+T to find a class, module, method or constant by
   name.
3. Hold Ctrl. The names that you can click get an underline. Click one to
   go to its definition. When there are several definitions, rubyscope
   shows a list. When a class is opened in many files, it goes to the main
   file and tells you how many files there are.
4. Go back with the mouse back button or the ← button in the top bar. Go
   forward with the mouse forward button or the → button.
5. Right-click a name and choose "Find usages". The results panel opens at
   the bottom. Click a row to go there. For a method, the "callers" button
   on a row shows who calls the method around that row.
6. Rest the pointer on a name for about half a second. A popup shows the
   signature, the docs, the definition location, the type that rubyscope
   found and how sure it is.
7. Press Ctrl+B to show git blame. Rest the pointer on a blame block to see
   the commit. Click it to open the commit, or its pull request, on GitHub.
8. Press Ctrl+K to see all commands. Some commands have no key, for example
   "Index the project again" and "Open in editor".

## The window

| Part | What it has |
|---|---|
| Top bar | Back and forward, the file path, the breadcrumb, the "Commands" button and the index status. The breadcrumb shows the class and method at the cursor. Click a part to go to it. |
| Side bar | Two tabs. "Files" is the file tree. "Outline" lists the classes, modules, methods and constants of the open file. |
| Tabs | One tab for each open file. A middle-click on a tab closes it. |
| Action bar | Only in a view file. A line above the code with a link to the controller action (or mailer method) of the view, for example `InvoicesController#edit`. Click it to go there. The line does not scroll with the code. |
| Code view | Drag to select text. Double-click selects a word. Ctrl+C copies the selection. |
| Results panel | Usages, search results and the type hierarchy. The type hierarchy shows the ancestors in method lookup order, and the subclasses and includers. Click a row or a name to go to it. |

## Mouse

| Mouse | What it does |
|---|---|
| Ctrl+click | Go to the definition (a list when there are several) |
| Ctrl+click on the name of a controller action | Go to its view (template or Phlex view) |
| Click the link above the code of a view | Go to its controller action or mailer method |
| Rest the pointer on a name | Popup with signature, docs, location, type |
| Right-click | Menu: go to view (in a controller action) or go to controller action (in a view), go to definition, all definitions, find usages, type hierarchy, copy name, copy path:line, open in editor, open on GitHub |
| Mouse back and forward buttons, the ← → buttons | Back and forward |
| Click a breadcrumb part | Go to that class or method |
| Click a blame block | Open the commit (or its pull request) on GitHub |
| "callers" in the results panel | Who calls that method (one level up the call hierarchy) |
| Middle-click a tab | Close the tab |

## Keys

On macOS, use Cmd where this README says Ctrl. Ctrl+Tab and Ctrl+Shift+Tab
stay on Ctrl, because Cmd+Tab switches apps. Ctrl+D and Ctrl+U also stay on
Ctrl, as in Vim.

| Key | What it does |
|---|---|
| Ctrl+O | Open a project folder (the system folder dialog; a new window when a project is open) |
| Ctrl+P | Go to file |
| Ctrl+T | Go to symbol (class, module, method, constant) |
| Ctrl+Shift+F | Search in all files of the project (plain text, case is ignored) |
| Ctrl+F | Find in this file (Enter / Shift+Enter: next / previous) |
| Ctrl+K | All commands |
| Ctrl+G | Go to line |
| Ctrl+W | Close tab |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+B | Show or hide git blame |
| Ctrl+Shift+B | Show or hide the side bar |
| Esc | Close a popup, then the find bar, then the results panel |

Ctrl+F and Ctrl+Shift+F start with the selected text, when it is on one
line.

These keys scroll the code when no text field has focus:

| Key | What it does |
|---|---|
| ↑ ↓, j k | Scroll 3 lines |
| ← → | Scroll sideways |
| Page Up / Page Down, Space / Shift+Space | Scroll one page |
| Ctrl+D / Ctrl+U | Scroll half a page |
| gg, Home | Go to the top |
| Shift+G, End | Go to the end |
| A number, then g or G (`500g`) | Go to that line |
| 0 | Scroll to the left edge |

The mouse wheel scrolls 3 times faster than the egui default.

### Commands without a key

These commands are in the Ctrl+K list. They have no default key, but you
can give them one (see below).

| Command | Name in `keys.toml` |
|---|---|
| Back | `back` |
| Forward | `forward` |
| Find usages of the symbol at the cursor | `find_usages` |
| Go to the view of the action, or the action of the view | `go_to_view` |
| Type hierarchy of the class at the cursor | `type_hierarchy` |
| Outline of this file | `file_outline` |
| Show or hide the results panel | `toggle_bottom_panel` |
| Open in editor (nvim) at this line | `open_in_editor` |
| Open this line on GitHub | `open_on_github` |
| Copy path:line | `copy_path` |
| Index the project again, parsing every file | `reindex` |

"Open in editor" uses `omarchy-launch-editor` when it is on `PATH`.
Otherwise it opens nvim in a terminal with `xdg-terminal-exec`, or runs
`nvim`. On macOS it opens nvim in a new Terminal window. The first time,
macOS asks if rubyscope can control Terminal. "Open on GitHub" needs an `origin` remote on github.com. It opens
the line at the commit that `HEAD` points to.

### Change the keys

Put the keys in `~/.config/rubyscope/keys.toml`. A key in this file
replaces the default key of that command. The other commands keep their
defaults.

```toml
toggle_blame = "cmd+alt+b"
find_usages = "alt+u"
back = "alt+left"
forward = "alt+right"
reindex = "f5"
```

A key is modifiers and one key, joined with `+`. The modifiers are `cmd`,
`ctrl`, `shift` and `alt`. `cmd` is Cmd on macOS and Ctrl on Linux, so one
file works on both. `ctrl` is always the Ctrl key. The key can be a letter, a digit, `esc`, `tab`, `enter`,
`space`, `left`, `right`, `up`, `down` or `f1` to `f12`. rubyscope reads
this file when it starts.

The names of the commands that have a default key:

| Name | Default |
|---|---|
| `open_folder` | Ctrl+O |
| `go_to_file` | Ctrl+P |
| `go_to_symbol` | Ctrl+T |
| `search_project` | Ctrl+Shift+F |
| `find_in_file` | Ctrl+F |
| `commands` | Ctrl+K |
| `go_to_line` | Ctrl+G |
| `close_tab` | Ctrl+W |
| `next_tab` | Ctrl+Tab |
| `prev_tab` | Ctrl+Shift+Tab |
| `toggle_blame` | Ctrl+B |
| `toggle_sidebar` | Ctrl+Shift+B |
| `close_popup` | Esc |

## What a click understands

- Ruby's constant lookup (lexical scopes, ancestors, top level), compact
  names (`class A::B`), constant aliases, Zeitwerk folder namespaces.
- Method lookup in Ruby's order: prepends, the class, includes, the
  superclass. Class methods, `class << self`, `extend`, `module_function`,
  concerns (`class_methods do`, `included do`), `class_eval`,
  `ActiveSupport.on_load`, `super`.
- Methods from Rails DSLs: associations, scopes, enums, `delegate`,
  `class_attribute` / `mattr_*`, `attr_*`, `attribute`, `store_accessor`,
  attachments, `alias_attribute`, and database columns from `db/schema.rb`.
- Symbols that name methods: `before_action :load`, `validate :check`,
  `if: :paid?`, `send(:x)`.
- Types of variables: `Model.find`, relations and scopes, associations,
  block parameters, instance variables, return values, YARD `@return`,
  FactoryBot `create(:x)`, and as a last guess the variable's name. The
  hover shows how sure each answer is: exact, inferred, guess, name only.
- Phlex kit calls (`Button(...)` → `UI::Button`).
- Controller actions and their views, both ways: `InvoicesController#edit`
  ↔ `app/views/invoices/edit.*`, variant folders such as
  `app/views/invoices/native/edit.rb`, and `Views::Invoices::Edit`.
  Mailer methods and their templates the same way.
  A view file shows a link to its action above the code.
- Route helpers (`admin_invoice_path` → the route line), and in routes
  files, `resources :x` → the controller, `to: "a#b"` → the action.
- `t("a.b")` and lazy `t(".x")` (controllers, Phlex, templates) → the
  locale YAML line, for every locale.
- `render "x/y"` → the partial. In ERB templates, the Ruby inside the tags.
- RSpec `let` / `subject`, `described_class`, shared examples, methods
  in `describe` blocks, FactoryBot factories and traits.
- Stimulus: `data-controller` / `data-action` (in ERB and Phlex hashes) →
  the controller file and method.
- Gems from `Gemfile.lock` and the Ruby standard library.

## How it works

1. **Find files.** rubyscope lists the project files and respects
   `.gitignore`. It reads `Gemfile.lock` and asks `ruby` where the gems
   and the standard library are. From each installed gem it reads `lib/`
   and `app/`.
2. **Parse.** It parses every Ruby file with Prism, on all CPU cores. From
   each file it takes the classes, modules, methods, constants and
   references.
3. **Link.** It joins all files into one table with full names, and adds
   the Rails, RSpec, FactoryBot and Stimulus rules.
4. **Save.** It saves the parsed files, so the next start parses only the
   files that changed.
5. **Watch.** While the window is open, a file watcher sees changes in
   `app/`, `config/`, `db/`, `lib/`, `spec/`, `test/`, `Gemfile` and
   `Gemfile.lock`. Open tabs load the new text, and the index builds again
   from the changed files.

A click asks the index what is at that position. The answer is a list of
definitions, each with how sure it is.

### Saved index

The saved index is in `~/.cache/rubyscope/<project>-<hash>.bin`, or in
`$XDG_CACHE_HOME/rubyscope/` when `XDG_CACHE_HOME` is set. For a large
Rails app with its gems the file is about 125 MB.

At the next start rubyscope parses only the files whose size or change
time is different. Then it links everything again. For a large app (about
7,000 Ruby files and 340 gems) the index is ready in less than 1 s with
the saved index, and in about 2.3 s without it.

rubyscope does not use the saved index when:

- the index code is different (a new build with new parse rules). The
  saved file has a fingerprint of that code.
- `config/initializers/inflections.rb` changed. Then every file is parsed
  again.

"Index the project again" (Ctrl+K) parses every file and writes a new
saved index.

## Limits

- Methods with names that exist only at runtime (`define_method` with a
  variable name, `method_missing`, `class_eval` with a string) cannot be
  found by reading the code. For these, rubyscope shows the list of
  methods with that name ("name only").
- Some variable types are guesses. The hover always shows when a type is
  a guess.
- RBS signatures for core classes are not read yet. For example,
  `"x".strip` gives a name-only list, because `String` is written in C.
- Custom Zeitwerk setups are read from the initializers only, so some
  unusual setups can be missed.
- Files outside the watched folders (see "How it works") do not update
  the index while the window is open. Use "Index the project again".
- After a file changes, linking runs again for the whole project (about
  0.6 s for a large app).
- A large app with all its gems uses about 700 MB of memory. "Find usages"
  decodes all references, and this adds memory.

## Files that rubyscope writes

rubyscope never writes into the project folder.

| Path | What it holds |
|---|---|
| `~/.cache/rubyscope/` (or `$XDG_CACHE_HOME/rubyscope/`) | The saved index of each project |
| `~/.local/state/rubyscope/recent.json` | The recent projects (up to 12) |
| `~/.local/state/rubyscope/session*.json` | The open tabs of each project |

It reads `~/.config/rubyscope/keys.toml` (keys) and
`~/.local/state/omarchy/current/theme/colors.toml` (colors).

## Environment variables

| Variable | What it does |
|---|---|
| `XDG_CACHE_HOME` | The folder for the saved index (`$XDG_CACHE_HOME/rubyscope`). It must be an absolute path. |
| `RUBYSCOPE_APP_ID` | The app id of the window, which window manager rules match. The default is `rubyscope`. Set it to give a test window its own window rules. |
| `RUBYSCOPE_SHOTS` | Tests only: the folder for window test screenshots |
| `RUBYSCOPE_PROJECT` | Tests only: the project for the `real_project_smoke` test |

## Troubleshooting

| Problem | What to do |
|---|---|
| A click goes to an old line, or a new method is not found | Press Ctrl+K and choose "Index the project again". If this does not help, close rubyscope and delete `~/.cache/rubyscope/`. |
| A click on a gem name or a Rails method does nothing | Run `ruby -v` and `bundle install` in the project folder. rubyscope indexes only gems that are installed for the Ruby that the project uses. |
| Blame does not show | The project must be a git repository, and `git` must be on `PATH`. |
| "This project has no GitHub remote" | The `origin` remote must be on github.com. |
| A blame click opens the commit, not the pull request | Install the GitHub CLI (`gh`) and log in. |
| You want to see what the index does | Start rubyscope from a terminal. It prints a line such as `rubyscope: indexed 9000 files in 0.71 s, 9000 from the saved index`. |

## Command-line test tool

`rubyscope-cli` builds the same index as the window and answers questions
about it. Use it to test the index without the window. Positions are
`FILE:LINE:COL`, 1-based.

```
rubyscope-cli -r ~/code/shop index                       # build the index, print counts and times
rubyscope-cli -r ~/code/shop def app/models/x.rb:12:5    # where the name at LINE:COL goes
rubyscope-cli -r ~/code/shop refs app/models/x.rb:12:5   # every usage (the first 40)
rubyscope-cli -r ~/code/shop hover app/models/x.rb:12:5  # what the hover popup shows
rubyscope-cli -r ~/code/shop related app/controllers/invoices_controller.rb:20:7   # the view of an action, or the action of a view
rubyscope-cli -r ~/code/shop outline app/models/x.rb     # the outline of a file
rubyscope-cli -r ~/code/shop search "refresh_for"        # text search (the first 50 hits)
rubyscope-cli -r ~/code/shop search --regex "def (create|update)"
rubyscope-cli -r ~/code/shop ancestors ApplicationRecord # the ancestors of a class or module
rubyscope-cli -r ~/code/shop golden clicks.txt           # golden click tests
rubyscope-cli -r ~/code/shop check probe.json            # compare with booted Rails
```

Options (before the command):

| Option | What it does |
|---|---|
| `-r`, `--root FOLDER` | The project folder. The default is the current folder. |
| `--no-gems` | Do not index gems |
| `--no-stdlib` | Do not index the Ruby standard library |
| `--no-cache` | Do not read or write the saved index |

`check` also takes `--show N`: how many misses to list for each group (the
default is 15).

A golden file has one click on each line:

```
app/controllers/invoices_controller.rb:12:5 => app/models/invoice.rb:1   # a note
```

The target path is relative for project files, `gem:<name>/<path>` for
gems and `stdlib:<path>` for the standard library. A click passes when
its first target is that file and line. `golden` exits with status 1 when
a click fails.

## Checking against booted Rails

`probe/rails_probe.rb` runs inside the app and writes the real source
location of every constant and method to a JSON file. It only reads.

```
cd ~/code/shop
bin/rails runner ~/src/rubyscope/probe/rails_probe.rb /tmp/probe.json
rubyscope-cli -r ~/code/shop check /tmp/probe.json
```

It works without a database. When a class needs the database to load, the
probe skips that constant and tells how many constants it skipped.

## Tests

Run every check before a push, the same way a CI server would:

```
bin/ci
```

It checks formatting (`cargo fmt`), runs clippy with warnings as errors,
runs all the tests below, checks the Ruby syntax of the probe, and looks for
private local paths in the repo. It stops at the first step that fails.

To run only some of the tests:

```
cargo test                                           # index rules, a Rails app in memory, saved index, window tests
RUBYSCOPE_SHOTS=/tmp/shots cargo test -p rubyscope   # also save window screenshots
RUBYSCOPE_PROJECT=~/code/shop cargo test -p rubyscope real_project_smoke -- --nocapture
```

- `crates/index/tests/` tests the go-to-definition rules on small Ruby
  snippets, the Rails, RSpec, FactoryBot and Stimulus rules on a small app
  in memory, and the saved index.
- The window tests (`crates/app/src/ui_tests.rs`) drive the real window
  with key presses and clicks through egui_kittest, and render it with
  wgpu. They use a temporary state folder and no saved index, so they do
  not change your own state.
- `real_project_smoke` opens a real project, goes to `ApplicationRecord`
  with Ctrl+T, shows blame and finds usages. Without `RUBYSCOPE_PROJECT`
  it does nothing.

## Project layout

| Path | What it holds |
|---|---|
| `crates/index` | The index (no window): finds files, parses with Prism, links, resolves clicks, saves the index |
| `crates/cli` | `rubyscope-cli`, the test command |
| `crates/app` | The window (egui), binary `rubyscope` |
| `probe/rails_probe.rb` | The script that gets real locations from booted Rails |
| `plans/001-rubyscope-v1.md` | The design plan, with its status at the end |
| `bin/install` | Builds and installs rubyscope for this user |
| `bin/ci` | Runs every check before a push: formatting, clippy, tests, private paths |
| `packaging/rubyscope.desktop` | The app launcher entry |

Main files in `crates/index/src`:

| File | What it does |
|---|---|
| `discover.rs` | Finds the project files, the gems and the standard library |
| `extract.rs` | Reads the Prism syntax tree of one file |
| `index.rs` | Joins all files into one table with full names |
| `link.rs` | Rails models, routes, locales, RSpec, FactoryBot, Stimulus |
| `rails.rs` | `db/schema.rb`, routes files and locale YAML files |
| `resolve.rs` | Turns a position into its definitions |
| `infer.rs` | Finds the class of a value (type inference) |
| `inflect.rs` | ActiveSupport-style inflections |
| `query.rs` | Usages, outline, breadcrumb, hover, type hierarchy, search |
| `cache.rs` | The saved index |
