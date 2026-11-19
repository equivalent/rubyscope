# Rubyscope

Rubyscope is a desktop app for reading Ruby and Rails code. Hold Ctrl (Cmd
on macOS) and click a name to go to its definition. The name can be a class, module,
constant, method, route helper, translation key, partial, RSpec `let` or
factory. Rubyscope reads code only. It never changes a file.

It is for people who read large Rails apps and want go-to-definition as
good as RubyMine's, in a small native app that starts in about one second.

Rubyscope is not:

- an editor. To change a file, open it in your editor from rubyscope.
- a language server (LSP). It does not connect to an editor.
- a tool that runs your code. It reads the source and does not boot Rails.

The design plan is in `plans/001-rubyscope-v1.md`. Its "Status" section at
the end tells what is done.

## Requirements

- Linux or macOS. On Linux, Rubyscope is built and used on Wayland
  (Hyprland). The build also has X11 support, but it is not checked. On
  macOS it runs on Apple silicon.
- Rust 1.88 or newer.
- clang and libclang. The `ruby-prism` crate compiles the Prism C parser
  and makes its Rust bindings with bindgen.
- Optional, for more features:

| Tool | What it gives |
|---|---|
| `ruby` | Gems and the standard library in the index. Rubyscope asks mise or rbenv (`mise which ruby`, `rbenv which ruby`) for the Ruby of the project folder, else it runs the `ruby` on `PATH`. |
| Installed gems (`bundle install`) | Clicks that go into gem source. Only gems that are installed are indexed. |
| `git` | Git blame, diff mode and "Open on GitHub" |
| `gh` (GitHub CLI) | A click on a blame block opens the pull request of the commit, not only the commit. The branch list shows who opened the pull request of each branch. |
| `fc-match` (fontconfig) | Linux: the system monospace font for code. macOS uses Menlo. |
| `xdg-open` | Linux: opens links in the browser. macOS uses `open`. |
| Omarchy | Colors from the current Omarchy theme, when it is of the mode you chose (dark or light). Otherwise the colors are Tokyo Night or Tokyo Night Day. |

## Install from a release

Each GitHub release has a Linux build and a macOS build. This is alpha
software, so the versions start at 0.0.1. Below, `<version>` is the
number of the release, for example `0.0.1`.

Linux (x86_64, built on Arch Linux):

```
tar xzf rubyscope-<version>-linux-x86_64.tar.gz
rubyscope-<version>-linux-x86_64/install.sh
```

`install.sh` copies the files listed under "Build and install" below.
Make sure `~/.local/bin` is on your `PATH`.

macOS (Apple silicon): open `Rubyscope-<version>-macos-arm64.dmg` and drag
Rubyscope to Applications. The app is not signed, so macOS blocks the
first start. Run this once:

```
xattr -dr com.apple.quarantine /Applications/Rubyscope.app
```

For the terminal command, link the binary in the app, and copy
`rubyscope-cli` from the DMG:

```
ln -sf /Applications/Rubyscope.app/Contents/MacOS/rubyscope ~/.local/bin/rubyscope
```

To make the release files, run `bin/package` on each OS. It writes them
to `dist/`. `bin/release <version>` does a whole release: it scans the
history and the binaries for private names and secrets, sets the version,
builds on Linux and on a Mac over ssh, tags the version and makes the
GitHub release. Read the top of the script for what it needs.

## Build and install

```
bin/install
```

This builds the release binaries and installs:

- `~/.local/bin/rubyscope` (the window)
- `~/.local/bin/rubyscope-cli` (the test command)
- `~/.local/share/applications/rubyscope.desktop` (the app launcher entry,
  set as the handler of `rubyscope://` links with `xdg-mime`)
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
macOS, Rubyscope reads the `PATH` from your login shell when it starts.
Then `ruby`, `mise` and `rbenv` are found the same way as in the
terminal.

To build without installing:

```
cargo build --release
target/release/rubyscope ~/code/shop
```

## Start Rubyscope

```
rubyscope ~/code/shop                                # open a project folder
rubyscope ~/code/shop/app/models/invoice.rb:40       # open a file at line 40
rubyscope                                            # see below
```

- With a file and no folder, the project is the nearest parent folder that
  has `.git` or a `Gemfile`.
- With no arguments, Rubyscope opens the current folder when it has `.git`
  or a `Gemfile`. Otherwise it shows the start screen.
- The start screen has a path field (`~/` is allowed), a "Browse…" button
  for the system folder dialog, and your projects. Click a project, or
  press its number (1 to 9), to open it. Drag a project to change its
  number. The "…" button of a project renames it in the list (the folder
  keeps its name) or removes it from the list. The list shows the icon of
  a project when it has one, such as `public/favicon.ico`. The sun and
  moon button at the top right switches dark and light mode (see "Dark
  and light mode").
- Ctrl+O opens the system folder dialog at any time.
- Click the project name in the top bar to go back to the project list,
  or to open a recent project in this window or in a new window.
- There is one window for each project. When a project is open, Ctrl+O
  opens the new folder in a new window.
- When the project of a file is already open in a window,
  `rubyscope <file>:<line>` (or a link, see below) opens the file in that
  window and brings it to the front, instead of a second window.
- Rubyscope remembers the open tabs of each project and opens them again
  at the next start.
- The app launcher entry also lets a file manager open a folder or a `.rb`
  file with rubyscope.

### Links

A `rubyscope://` link opens a file at a line, from a browser, a chat or a
report:

```
rubyscope://open?file=/abs/path/to/shop/app/models/order.rb&line=40
rubyscope://open?repo=shop&path=app/models/order.rb&line=40
```

- `file=` is an absolute path.
- `repo=` with `path=` finds the file in `~/git/<repo>` on the computer that
  opens the link, so one link works on every computer.
- `line=` is optional and 1-based.
- The link is data from a web page, so Rubyscope checks it and runs nothing
  from it. It refuses the link, with a desktop notification, when the line
  is not a number, the file does not exist, or the `path=` leaves
  `~/git/<repo>` (with `..` or a symbolic link).
- On Linux, `bin/install` makes Rubyscope the handler of these links. Test
  one with `xdg-open 'rubyscope://open?repo=shop&path=app/models/order.rb&line=40'`.
  From a terminal, `rubyscope '<link>'` works on Linux and macOS.
- macOS does not send these links to Rubyscope yet: the app does not
  register the scheme.

## First steps

1. Open a project: `rubyscope ~/code/shop`. The index builds in the
   background. The top bar shows "indexing…", then the number of files
   and the time. The first index of a large app takes a few seconds. The
   next start uses the saved index and takes about one second.
2. Open a file. Press Ctrl+P and type part of the path, or click it in the
   file tree. Press Ctrl+T to find a class, module, method or constant by
   name.
3. Hold Ctrl. The names that you can click get an underline. Click one to
   go to its definition. When there are several definitions, Rubyscope
   shows a list. When a class is opened in many files, it goes to the main
   file and tells you how many files there are. A click on the name where
   a class, module or constant is defined (`class Order`, `PREFIX = "x"`)
   lists the places that use it. A click on the name in `def total` goes
   to the place that calls the method, or lists them when there are more.
4. Go back with the mouse back button or the ← button in the top bar. Go
   forward with the mouse forward button or the → button.
5. Right-click a name and choose "Find usages". The results panel opens at
   the bottom. Click a row to go there, or choose one with the up and down
   arrows and press Enter. The app comes first, then specs and tests, then
   gems. For a method, the "callers" button on a row shows who calls the
   method around that row.
6. Rest the pointer on a name for about half a second. A popup shows the
   signature, the docs, the definition location, the type that Rubyscope
   found and how sure it is.
7. Press Ctrl+B (or click Blame in the top bar) to show git blame. Each line shows the date and the last name
   of the author, as in RubyMine. The newest commits have the strongest blue.
   Rest the pointer on a blame line to see the commit. Click it to open the commit, or its pull request, on GitHub.
8. Press Ctrl+K to see all commands. Some commands have no key, for example
   "Index the project again" and "Open in editor".
9. Press Ctrl+Shift+D (or click Diff at the top right) for diff mode: what
   your branch changed, as in a pull request, and what is not committed yet.
   Press ] and [ to go from change to change. See "Diff mode" below.

## The window

| Part | What it has |
|---|---|
| Top bar | From the left: the folder button (Go to file, like Ctrl+P), back and forward, the project name (a menu: back to the project list, or another project here or in a new window), the file path and the breadcrumb. The breadcrumb shows the class and method at the cursor. Click a part to go to it. On the right: the diff bar in diff mode (see "Diff mode"), the index numbers and the RAM that Rubyscope uses, the "Diff" and "Blame" buttons (blue while on), the branch button, the pull button, "Commands" and the sun and moon button. Each number and button explains itself on hover. |
| Side bar | Two tabs. "Files" is the file tree. "Outline" lists the classes, modules, methods and constants of the open file. In diff mode a third tab, "Changes", lists the changed files. |
| Tabs | One tab for each open file. A middle-click on a tab closes it. |
| Action bar | Only in a view file. A line above the code with a link to the controller action (or mailer method) of the view, for example `InvoicesController#edit`. Click it to go there. The line does not scroll with the code. |
| Code view | Drag to select text. Double-click selects a word. Ctrl+C copies the selection. |
| Results panel | Usages, search results and the type hierarchy. The type hierarchy shows the ancestors in method lookup order, and the subclasses and includers. Click a row or a name to go to it. While the panel is new or was clicked last, the up and down arrows choose a row and Enter opens it; a click in the code gives the arrows back to the code. |

## Mouse

| Mouse | What it does |
|---|---|
| Ctrl+click | Go to the definition (a list when there are several) |
| Ctrl+click on the name in a class, module or constant definition (`class Order`, `PREFIX = "x"`) | List the usages of that class, module or constant in the results panel |
| Ctrl+click on the name in a method definition (`def total`) | Go to the only usage, or list the usages in the results panel |
| Ctrl+click on the name of a controller action | Go to its view (template or Phlex view) |
| Click the link above the code of a view | Go to its controller action or mailer method |
| Rest the pointer on a name | Popup with signature, docs, location, type |
| Right-click | Menu: go to view (in a controller action) or go to controller action (in a view), go to definition, all definitions, find usages, type hierarchy, copy name, copy path:line, open in editor, open on GitHub |
| Mouse back and forward buttons, the ← → buttons | Back and forward |
| Click a breadcrumb part | Go to that class or method |
| Click a blame block | Open the commit (or its pull request) on GitHub |
| "callers" in the results panel | Who calls that method (one level up the call hierarchy) |
| Middle-click a tab | Close the tab |
| Click the diff bar in the top bar | Diff against another branch |
| Click a mark on the right edge (diff mode) | Go to that change |
| The folder button at the top left | Go to file (like Ctrl+P) |
| Click the project name in the top bar | Back to the project list, or open a recent project |
| "Diff" and "Blame" in the top bar | Turn diff mode or git blame on and off |
| The branch button | Switch branches (see "Switch branches") |
| The round arrow | `git pull` for the branch |
| The sun or the moon | Switch to light or dark mode |

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
| Ctrl+Shift+D | Diff mode on or off |
| Ctrl+Shift+B | Show or hide the side bar |
| Ctrl+Q | Close the project and go back to the project list; on the project list, quit |
| Esc | Close a popup, then the find bar, then the frame of the current change, then the results panel |
| 1 to 9 | On the start screen: open that project |
| Tab | In Ctrl+P, in diff mode: all files or only the changed files |
| Up and down arrows, Enter | In Ctrl+P, Ctrl+T, Ctrl+K and the results panel: choose a row and open it |

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
| ] / [, Alt+↓ / Alt+↑ | Diff mode: next / previous change |

The mouse wheel scrolls the code 12 times faster than the egui default,
the side bar 3.75 times faster, and every other list 1.5 times faster.

| Wheel | What it does |
|---|---|
| Ctrl + wheel (Cmd + wheel on macOS) | Scroll 3 times faster: the code, the side bar and every list |
| Shift + wheel | Scroll the code one page per wheel step |

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
| Diff against branch… | `diff_against_branch` |
| Go to changed file | `go_to_changed_file` |
| Switch branch… | `switch_branch` |

"Open in editor" uses `omarchy-launch-editor` when it is on `PATH`.
Otherwise it opens nvim in a terminal with `xdg-terminal-exec`, or runs
`nvim`. On macOS it opens nvim in a new Terminal window. The first time,
macOS asks if Rubyscope can control Terminal. "Open on GitHub" needs an `origin` remote on github.com. It opens
the line at the commit that `HEAD` points to.

### Change the keys

Put the keys in `~/.config/rubyscope/keys.toml`. A key in this file
replaces the default key of that command. The other commands keep their
defaults.

```toml
toggle_blame = "cmd+alt+b"
next_change = ["n", "alt+down"]
find_usages = "alt+u"
back = "alt+left"
forward = "alt+right"
reindex = "f5"
```

A key is modifiers and one key, joined with `+`. The modifiers are `cmd`,
`ctrl`, `shift` and `alt`. `cmd` is Cmd on macOS and Ctrl on Linux, so one
file works on both. `ctrl` is always the Ctrl key. The key can be a letter, a digit, `esc`, `tab`, `enter`,
`space`, `left`, `right`, `up`, `down`, `[`, `]` or `f1` to `f12`. A
command can have several keys: give a list, as for `next_change` above.
Rubyscope reads this file when it starts.

The names of the commands that have a default key:

| Name | Default |
|---|---|
| `open_folder` | Ctrl+O |
| `close_project` | Ctrl+Q |
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
| `toggle_diff` | Ctrl+Shift+D |
| `next_change` | ], Alt+↓ |
| `prev_change` | [, Alt+↑ |
| `close_popup` | Esc |

`next_change` and `prev_change` work only when no text field has focus, so
`]` and `[` can still be typed in the find bar and the pickers.

## Diff mode

Press Ctrl+Shift+D, click the Diff button at the top right, or choose
"Toggle diff mode" in Ctrl+K. The button is blue while diff mode is on. The
code view shows what your branch changed and what is not committed yet:

| Color | What it is |
|---|---|
| Green background, green bar | A line added or changed on the branch (committed) |
| Red row, struck through | A line removed on the branch: a "ghost row" |
| Amber background, amber bar | A line added or changed, not committed |
| Amber row, struck through, dashed bar | A line removed, not committed |

A changed line is a ghost row with the old text, then the new line. Rest
the pointer on the diff bar in the top bar to see this list.

The branch diff starts at the merge-base of the base branch and `HEAD`, as
in a GitHub pull request. So new commits on the base branch are not shown
as changes. "Not committed" is the file on disk against `HEAD`, staged or
not. A new file that git does not know yet is all amber.

The code view is still the file on disk. Ctrl+click, the hover, find, go
to line and the selection work as before. Ghost rows have no line number
(a dim `−`), no links and no blame, and they are not selected or copied.
Rest the pointer on a ghost row to see why.

- The diff bar in the top bar says what is compared, for example
  `Diff: main…feature/order-refunds +77 −3, 3 uncommitted`: the base
  branch, your branch, the lines added and removed on the branch, and the
  number of files with changes that are not committed.
- Click the diff bar, or choose "Diff against branch…" in Ctrl+K, to pick
  another base branch from the local and remote branches. The current base
  and the default (`main`, else `master`, else what `origin/HEAD` points
  to) are marked. Rubyscope remembers the choice for each project. Pick
  your own pushed branch (`origin/feature/x`) to see what is not pushed
  yet.
- ] and [ (or Alt+↓ and Alt+↑) go to the next and previous change. The
  change gets a frame, and a chip in the corner of the code view says
  `change 2/6`. At the last change, ] says so; press it again to open the
  next changed file at its first change. [ works the same way backwards.
- The marks on the right edge show where the changes are in the file.
  Click a mark to go to that change.
- The file tree marks changed files: a green A for a file added on the
  branch, a blue M for a changed file, an amber dot for changes that are
  not committed. A closed folder with changed files has a dot. The tabs
  have the same marks.
- In Ctrl+P, Tab switches between all files and the changed files.
  "Go to changed file" in Ctrl+K opens the changed files list.
- The "Changes" tab in the side bar lists the files changed on the branch
  (against the base) and the files that are not committed (against
  `HEAD`), with the lines added and removed.
- When diff mode turns on, the file tree opens the folders that hold
  changed files.
- Blame (Ctrl+B) works in diff mode too. Lines that are not committed say
  "not committed" in amber, and ghost rows have an empty, striped cell.

The diffs are made with `git` in the background. They are made again when
an open file changes on disk, and when `HEAD` or the base branch moves (a
commit, a checkout, a fetch; Rubyscope looks every 4 seconds). Outside a
git repository, the diff bar says so and nothing is marked. When the base
branch does not exist any more, the diff bar says so and only the changes
that are not committed are shown.

The round arrow next to "Diff" runs `git pull` for the branch that is
checked out. It follows your `pull.rebase` setting, and merges when there
is none. A message at the bottom says how many commits came in. When the
pull stops on conflicts, Rubyscope does not fix or abort anything: the
message names the files, and you fix them in git or run
`git merge --abort` (or `git rebase --abort`). When git refuses (no
upstream branch, changes that the pull would overwrite), the message
shows why.

## Switch branches

The button with the branch name, next to the round arrow, opens a list of
branches ("Switch branch…" in Ctrl+K opens it too). The branch with the
newest commit is first. The list has the local branches, and the remote
branches that have no local branch of the same name. A remote branch
gets a local branch that tracks it.

Next to each branch are the initials of the author of its open pull
request, with the number and the name on the right. Rubyscope gets them
with `gh pr list`, so they show only when `gh` is logged in. A branch
without a pull request shows the initials of the author of its last
commit, dimmed.

Rubyscope runs `git switch`. Changes that are not committed stay. When
the switch would lose them, git refuses and the message says why. After
a switch, the open files are read again and the project is indexed
again. The button also follows a switch made outside Rubyscope, within a
few seconds.

## Dark and light mode

Rubyscope starts dark. The sun button at the right end of the top bar,
or at the top right of the start screen, switches to light mode; the moon
switches back. The choice is kept for the next start.

On Omarchy, Rubyscope uses the colors of the current theme when the theme
is of the chosen mode. Otherwise it uses Tokyo Night in dark mode and
Tokyo Night Day in light mode.

## Close and quit

Ctrl+Q closes the project and goes back to the project list. On the
project list, Ctrl+Q quits.

Closing the window first saves the open tabs, stops the file watcher, and
waits for an index save, a pull or a branch switch that still runs. When
that takes longer than 150 ms, a dialog shows the step and a progress
bar. Close the window again to quit at once. After 10 seconds Rubyscope
quits anyway. It then ends without freeing its memory piece by piece; the
system does that.

## What a click understands

- Ruby's constant lookup (lexical scopes, ancestors, top level), compact
  names (`class A::B`), constant aliases, Zeitwerk folder namespaces
  (a click on one lists the classes in it).
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
- Phlex kit calls: `Button(...)` and `UI::Button(...)` → `UI::Button`,
  also in a module (such as a concern) that a class with the kit includes.
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

1. **Find files.** Rubyscope lists the project files and respects
   `.gitignore`. It reads `Gemfile.lock` and asks the project's Ruby where
   the gems and the standard library are. From each installed gem it reads `lib/`
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
Rails app with its gems the file is about 125 MB. Rubyscope maps the file
into memory and does not read all of it. The references of a file are read
from it only when a click or "find usages" needs them.

At the next start Rubyscope parses only the files whose size or change
time is different. Then it links everything again. For a large Rails app the index is ready in less than 1 s with the saved
index, and in about 2.3 s without it.

Rubyscope does not use the saved index when:

- the index code is different (a new build with new parse rules). The
  saved file has a fingerprint of that code.
- `config/initializers/inflections.rb` changed. Then every file is parsed
  again.

"Index the project again" (Ctrl+K) parses every file and writes a new
saved index.

## Limits

- Methods with names that exist only at runtime (`define_method` with a
  variable name, `method_missing`, `class_eval` with a string) cannot be
  found by reading the code. For these, Rubyscope shows the list of
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
- A large app with all its gems uses about 560 MB of memory, the same
  after a new index as with the saved index. The first "find usages"
  adds about 100 MB for the list of names.

## Files that Rubyscope writes

Rubyscope never writes into the project folder.

| Path | What it holds |
|---|---|
| `~/.cache/rubyscope/` (or `$XDG_CACHE_HOME/rubyscope/`) | The saved index of each project |
| `~/.local/state/rubyscope/recent.json` | The projects of the start screen, in your order, with the names you gave them |
| `~/.local/state/rubyscope/session*.json` | The open tabs of each project |
| `~/.local/state/rubyscope/diff*.json` | The base branch of diff mode for each project |
| `~/.local/state/rubyscope/mode` | Dark or light mode |

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
| A click goes to an old line, or a new method is not found | Press Ctrl+K and choose "Index the project again". If this does not help, close Rubyscope and delete `~/.cache/rubyscope/`. |
| A click on a gem name or a Rails method does nothing | Run `ruby -v` and `bundle install` in the project folder. Rubyscope indexes only gems that are installed for the Ruby that the project uses. |
| Blame does not show | The project must be a git repository, and `git` must be on `PATH`. |
| Diff mode marks nothing | The diff bar in the top bar tells why: not a git repository, or the base branch is not found. Click it to choose a base branch. |
| "This project has no GitHub remote" | The `origin` remote must be on github.com. |
| A blame click opens the commit, not the pull request | Install the GitHub CLI (`gh`) and log in. |
| You want to see what the index does | Start Rubyscope from a terminal. It prints a line such as `rubyscope: indexed 9000 files in 0.71 s, 9000 from the saved index`. |

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
| `bin/install` | Builds and installs Rubyscope for this user |
| `bin/package` | Builds the release file for this OS in `dist/` |
| `bin/release` | Makes a GitHub release for Linux and macOS |
| `bin/ci` | Runs every check before a push: formatting, clippy, tests, private paths |
| `packaging/rubyscope.desktop` | The app launcher entry |
| `webpage/` | The web page and the docs, plain HTML and CSS (see `webpage/README.md`) |

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

## License

MIT. See [LICENSE](LICENSE).
