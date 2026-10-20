//! Rails files that are not plain Ruby classes: `db/schema.rb`, the routes
//! files and the locale YAML files.

use std::path::{Path, PathBuf};

use ruby_prism::{CallNode, Node, Visit};

use crate::inflect;
use crate::model::Span;

fn span(loc: &ruby_prism::Location<'_>) -> Span {
    Span::new(loc.start_offset(), loc.end_offset())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn sym_or_str(node: &Node<'_>) -> Option<(String, Span)> {
    if let Some(sym) = node.as_symbol_node() {
        let loc = sym.value_loc().unwrap_or_else(|| sym.location());
        return Some((text(sym.unescaped()), span(&loc)));
    }
    if let Some(s) = node.as_string_node() {
        return Some((text(s.unescaped()), span(&s.content_loc())));
    }
    None
}

struct CallArgs<'pr> {
    positional: Vec<Node<'pr>>,
    keywords: Vec<(String, Node<'pr>)>,
    /// `get "about" => "pages#about"`: the first hash rocket pair.
    rocket: Option<(Node<'pr>, Node<'pr>)>,
}

impl<'pr> CallArgs<'pr> {
    fn of(node: &CallNode<'pr>) -> CallArgs<'pr> {
        let mut out = CallArgs { positional: Vec::new(), keywords: Vec::new(), rocket: None };
        let Some(args) = node.arguments() else { return out };
        for arg in args.arguments().iter() {
            let Some(hash) = arg.as_keyword_hash_node() else {
                out.positional.push(arg);
                continue;
            };
            for el in hash.elements().iter() {
                let Some(pair) = el.as_assoc_node() else { continue };
                let key = pair.key();
                if key.as_symbol_node().is_some() {
                    if let Some((k, _)) = sym_or_str(&key) {
                        out.keywords.push((k, pair.value()));
                    }
                } else if out.rocket.is_none() {
                    out.rocket = Some((key, pair.value()));
                }
            }
        }
        out
    }

    fn get(&self, key: &str) -> Option<&Node<'pr>> {
        self.keywords.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn text(&self, key: &str) -> Option<(String, Span)> {
        self.get(key).and_then(sym_or_str)
    }

    /// `only: [:index, :show]` / `except: :destroy`
    fn list(&self, key: &str) -> Option<Vec<String>> {
        let v = self.get(key)?;
        if let Some(a) = v.as_array_node() {
            return Some(a.elements().iter().filter_map(|e| sym_or_str(&e).map(|(s, _)| s)).collect());
        }
        sym_or_str(v).map(|(s, _)| vec![s])
    }
}

// ---- db/schema.rb ----------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    /// The schema type: `string`, `integer`, `datetime`, ...
    pub sql_type: String,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub span: Span,
    pub name_span: Span,
    pub columns: Vec<Column>,
}

/// The Ruby class a column's value has.
pub fn column_class(sql_type: &str) -> Option<&'static str> {
    Some(match sql_type {
        "string" | "text" | "citext" | "uuid" | "binary" | "inet" | "cidr" | "macaddr" | "enum" => "String",
        "integer" | "bigint" | "smallint" | "primary_key" => "Integer",
        "decimal" | "numeric" => "BigDecimal",
        "float" => "Float",
        "boolean" => "TrueClass",
        "datetime" | "timestamp" | "timestamptz" | "time" => "ActiveSupport::TimeWithZone",
        "date" => "Date",
        "json" | "jsonb" | "hstore" => "Hash",
        _ => return None,
    })
}

pub fn parse_schema(source: &[u8]) -> Vec<Table> {
    let result = ruby_prism::parse(source);
    let mut v = SchemaVisitor { tables: Vec::new() };
    v.visit(&result.node());
    v.tables
}

struct SchemaVisitor {
    tables: Vec<Table>,
}

impl<'pr> Visit<'pr> for SchemaVisitor {
    fn visit_call_node(&mut self, node: &CallNode<'pr>) {
        let name = node.name().as_slice();
        if name == b"create_table"
            && let Some((table, table_span)) = CallArgs::of(node).positional.first().and_then(sym_or_str)
        {
            let mut t = Table { name: table, span: span(&node.location()), name_span: table_span, columns: Vec::new() };
            let args = CallArgs::of(node);
            // `id: false` means no primary key; `id: :uuid` sets its type.
            if args.get("id").is_none_or(|v| v.as_false_node().is_none()) {
                let sql_type = args.text("id").map(|(s, _)| s).unwrap_or_else(|| "bigint".into());
                t.columns.push(Column { name: "id".into(), sql_type, span: t.span, name_span: table_span });
            }
            if let Some(body) = node.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
                && let Some(stmts) = body.as_statements_node()
            {
                for stmt in stmts.body().iter() {
                    let Some(call) = stmt.as_call_node() else { continue };
                    let kind = text(call.name().as_slice());
                    let args = CallArgs::of(&call);
                    let call_span = span(&call.location());
                    match kind.as_str() {
                        "index" | "check_constraint" | "exclusion_constraint" | "unique_constraint" => {}
                        "timestamps" => {
                            for c in ["created_at", "updated_at"] {
                                t.columns.push(Column {
                                    name: c.into(),
                                    sql_type: "datetime".into(),
                                    span: call_span,
                                    name_span: span(&call.message_loc().unwrap_or_else(|| call.location())),
                                });
                            }
                        }
                        "references" | "belongs_to" => {
                            for arg in &args.positional {
                                if let Some((r, r_span)) = sym_or_str(arg) {
                                    t.columns.push(Column {
                                        name: format!("{r}_id"),
                                        sql_type: "bigint".into(),
                                        span: call_span,
                                        name_span: r_span,
                                    });
                                    if args.get("polymorphic").is_some_and(|v| v.as_true_node().is_some()) {
                                        t.columns.push(Column {
                                            name: format!("{r}_type"),
                                            sql_type: "string".into(),
                                            span: call_span,
                                            name_span: r_span,
                                        });
                                    }
                                }
                            }
                        }
                        _ => {
                            for arg in &args.positional {
                                if let Some((c, c_span)) = sym_or_str(arg) {
                                    t.columns.push(Column {
                                        name: c,
                                        sql_type: kind.clone(),
                                        span: call_span,
                                        name_span: c_span,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            self.tables.push(t);
            return;
        }
        ruby_prism::visit_call_node(self, node);
    }
}

// ---- routes ----------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Route {
    /// The helper name without `_path` / `_url`, if the route has one.
    pub name: Option<String>,
    pub verb: String,
    pub path: String,
    /// `admin/invoices`
    pub controller: Option<String>,
    pub action: Option<String>,
    pub file: PathBuf,
    pub span: Span,
}

/// A link inside a routes file: `resources :invoices` goes to the
/// controller, `to: "pages#about"` goes to the action.
#[derive(Debug, Clone)]
pub struct RouteLink {
    pub file: PathBuf,
    pub span: Span,
    pub controller: String,
    pub action: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Routes {
    pub routes: Vec<Route>,
    pub links: Vec<RouteLink>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Plain,
    /// Inside `resources :x do ... end` (nested routes).
    Nested,
    Member,
    Collection,
}

#[derive(Debug, Clone)]
struct Resource {
    /// `invoices` (or `profile` for a singular resource)
    plural: String,
    singular: String,
    singular_resource: bool,
}

#[derive(Debug, Clone)]
struct Scope {
    path: String,
    /// The controller namespace: `admin/`.
    module: String,
    /// The name prefix: `admin`.
    name_prefix: Vec<String>,
    controller: Option<String>,
    resource: Option<Resource>,
    level: Level,
}

/// Reads a file's bytes; the index passes one that also knows files kept
/// in memory for tests.
pub type ReadFile<'a> = &'a dyn Fn(&Path) -> Option<Vec<u8>>;

/// Parses `config/routes.rb` and every file it loads with `draw :name`.
pub fn parse_routes(root: &Path, read: ReadFile<'_>) -> Routes {
    let mut out = Routes::default();
    let main = root.join("config/routes.rb");
    let scope = Scope {
        path: String::new(),
        module: String::new(),
        name_prefix: Vec::new(),
        controller: None,
        resource: None,
        level: Level::Plain,
    };
    parse_routes_file(root, &main, vec![scope], &mut out, 0, read);
    out
}

fn parse_routes_file(root: &Path, file: &Path, stack: Vec<Scope>, out: &mut Routes, depth: u32, read: ReadFile<'_>) {
    if depth > 8 {
        return;
    }
    let Some(source) = read(file) else { return };
    let result = ruby_prism::parse(&source);
    let mut v = RoutesVisitor { root, file: file.to_path_buf(), stack, out, depth, read };
    v.visit(&result.node());
}

struct RoutesVisitor<'a> {
    root: &'a Path,
    file: PathBuf,
    stack: Vec<Scope>,
    out: &'a mut Routes,
    depth: u32,
    read: ReadFile<'a>,
}

/// Joins route path parts with one `/` between them.
fn join_path(base: &str, part: &str) -> String {
    let part = part.trim_matches('/');
    if part.is_empty() {
        return base.to_string();
    }
    format!("{}/{part}", base.trim_end_matches('/'))
}

/// Rails' `Mapper.normalize_name`: `foo/bar-baz` → `foo_bar_baz`.
fn normalize_name(s: &str) -> String {
    s.trim_start_matches('/').replace(['/', '-'], "_").replace(['(', ')', ':', '.', '*'], "")
}

impl RoutesVisitor<'_> {
    fn top(&self) -> &Scope {
        self.stack.last().expect("route scope stack is never empty")
    }

    fn with_scope(&mut self, scope: Scope, node: &CallNode<'_>) {
        self.stack.push(scope);
        if let Some(block) = node.block().and_then(|b| b.as_block_node())
            && let Some(body) = block.body()
        {
            self.visit(&body);
        }
        self.stack.pop();
    }

    fn link(&mut self, span: Span, controller: &str, action: Option<String>) {
        self.out.links.push(RouteLink { file: self.file.clone(), span, controller: controller.to_string(), action });
    }

    fn add(
        &mut self,
        name: Option<String>,
        verb: &str,
        path: String,
        controller: Option<String>,
        action: Option<String>,
        span: Span,
    ) {
        let name =
            name.filter(|n| !n.is_empty() && n.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_'));
        let path = if path.is_empty() || path.starts_with('/') { path } else { format!("/{path}") };
        self.out.routes.push(Route {
            name,
            verb: verb.to_string(),
            path,
            controller,
            action,
            file: self.file.clone(),
            span,
        });
    }

    /// The route name for an action, the way Rails' `name_for_action` does.
    fn name_for(&self, prefix: Option<String>, new: bool) -> Option<String> {
        let s = self.top();
        let mut parts: Vec<String> = Vec::new();
        let member = s.resource.as_ref().map(|r| r.singular.clone());
        let collection =
            s.resource.as_ref().map(|r| if r.singular_resource { r.singular.clone() } else { r.plural.clone() });
        match s.level {
            Level::Member => {
                parts.extend(prefix);
                parts.extend(s.name_prefix.iter().cloned());
                parts.extend(member);
            }
            Level::Collection => {
                parts.extend(prefix);
                parts.extend(s.name_prefix.iter().cloned());
                parts.extend(collection);
            }
            Level::Nested => {
                parts.extend(s.name_prefix.iter().cloned());
                parts.extend(member);
                parts.extend(prefix);
            }
            Level::Plain if new => {
                parts.extend(prefix);
                parts.push("new".into());
                parts.extend(s.name_prefix.iter().cloned());
            }
            Level::Plain => {
                parts.extend(s.name_prefix.iter().cloned());
                parts.extend(prefix);
            }
        }
        let name = parts.join("_");
        (!name.is_empty()).then_some(name)
    }

    fn resources(&mut self, node: &CallNode<'_>, singular_resource: bool) {
        let args = CallArgs::of(node);
        let call_span = span(&node.location());
        let names: Vec<(String, Span)> = args.positional.iter().filter_map(sym_or_str).collect();
        for (name, name_span) in names {
            let s = self.top().clone();
            let plural = if singular_resource { inflect::pluralize(&name) } else { name.clone() };
            let singular = if singular_resource { name.clone() } else { inflect::singularize(&name) };
            let as_name = args.text("as").map(|(a, _)| a);
            let (res_plural, res_singular) = match &as_name {
                Some(a) if singular_resource => (a.clone(), a.clone()),
                Some(a) => (a.clone(), inflect::singularize(a)),
                None => (if singular_resource { singular.clone() } else { plural.clone() }, singular.clone()),
            };
            let module = match args.text("module") {
                Some((m, _)) => format!("{}{m}/", s.module),
                None => s.module.clone(),
            };
            let controller = match args.text("controller") {
                Some((c, _)) => format!("{module}{c}"),
                None => format!("{module}{plural}"),
            };
            self.link(name_span, &controller, None);
            let path_seg = args.text("path").map(|(p, _)| p).unwrap_or_else(|| name.clone());
            // Nested inside another resource: `/entities/:entity_id/invoices`.
            let mut name_prefix = s.name_prefix.clone();
            let mut base_path = s.path.clone();
            if let Some(parent) = &s.resource
                && matches!(s.level, Level::Nested)
            {
                name_prefix.push(parent.singular.clone());
                if !parent.singular_resource {
                    base_path = join_path(&base_path, &format!(":{}_id", parent.singular));
                }
            }
            let path = join_path(&base_path, &path_seg);
            let all: &[&str] = if singular_resource {
                &["show", "create", "new", "edit", "update", "destroy"]
            } else {
                &["index", "create", "new", "edit", "show", "update", "destroy"]
            };
            let only = args.list("only");
            let except = args.list("except").unwrap_or_default();
            let prefix_name = |extra: &[&str], last: &str| -> String {
                let mut p: Vec<String> = extra.iter().map(|x| x.to_string()).collect();
                p.extend(name_prefix.iter().cloned());
                p.push(last.to_string());
                p.join("_")
            };
            for action in all {
                if only.as_ref().is_some_and(|o| !o.iter().any(|x| x == action)) || except.iter().any(|x| x == action) {
                    continue;
                }
                let member_path = if singular_resource { path.clone() } else { join_path(&path, ":id") };
                let (verb, route_path, route_name) = match *action {
                    "index" => ("GET", path.clone(), Some(prefix_name(&[], &res_plural))),
                    "create" => ("POST", path.clone(), None),
                    "new" => ("GET", join_path(&path, "new"), Some(prefix_name(&["new"], &res_singular))),
                    "edit" => ("GET", join_path(&member_path, "edit"), Some(prefix_name(&["edit"], &res_singular))),
                    "show" => ("GET", member_path.clone(), Some(prefix_name(&[], &res_singular))),
                    "update" => ("PATCH", member_path.clone(), None),
                    _ => ("DELETE", member_path.clone(), None),
                };
                self.add(route_name, verb, route_path, Some(controller.clone()), Some(action.to_string()), call_span);
            }
            let resource = Resource { plural: res_plural.clone(), singular: res_singular.clone(), singular_resource };
            let inner = Scope {
                path: path.clone(),
                module: module.clone(),
                name_prefix,
                controller: Some(controller.clone()),
                resource: Some(resource),
                level: Level::Nested,
            };
            self.with_scope(inner, node);
        }
    }

    fn verb(&mut self, node: &CallNode<'_>, verb: &str) {
        let args = CallArgs::of(node);
        let call_span = span(&node.location());
        let s = self.top().clone();
        // The path: `get "about"`, `get :preview` or `get "about" => "pages#about"`.
        let (path_text, path_span, rocket_to) = match (&args.rocket, args.positional.first()) {
            (Some((k, v)), _) => {
                let (p, ps) = sym_or_str(k).unwrap_or_default();
                (p, ps, sym_or_str(v))
            }
            (None, Some(first)) => {
                let (p, ps) = sym_or_str(first).unwrap_or_default();
                (p, ps, None)
            }
            (None, None) => return,
        };
        let on = args.text("on").map(|(o, _)| o);
        let level = match on.as_deref() {
            Some("member") => Level::Member,
            Some("collection") => Level::Collection,
            _ => s.level,
        };
        let to = args.text("to").or(rocket_to);
        let mut controller = args.text("controller").map(|(c, _)| format!("{}{c}", s.module)).or(s.controller.clone());
        let mut action = args.text("action").map(|(a, _)| a);
        if let Some((to_text, to_span)) = &to
            && let Some((c, a)) = to_text.split_once('#')
        {
            let c = if c.is_empty() { controller.clone().unwrap_or_default() } else { format!("{}{c}", s.module) };
            controller = Some(c.clone());
            action = Some(a.to_string());
            self.link(*to_span, &c, Some(a.to_string()));
        }
        let simple = !path_text.is_empty()
            && path_text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/'));
        if action.is_none() && simple && !path_text.contains('/') {
            action = Some(path_text.replace('-', "_"));
        }
        if let (Some(c), Some(a)) = (&controller, &action)
            && to.is_none()
            && !path_span.is_empty()
            && simple
        {
            self.link(path_span, c, Some(a.clone()));
        }
        let explicit_as = args.text("as").map(|(a, _)| a);
        let no_name = args.get("as").is_some_and(|v| v.as_nil_node().is_some() || v.as_false_node().is_some());
        let prefix = explicit_as.clone().or_else(|| simple.then(|| normalize_name(&path_text)));
        let saved_level = self.stack.last().map(|t| t.level);
        if let Some(t) = self.stack.last_mut() {
            t.level = level;
        }
        let name = if no_name { None } else { self.name_for(prefix, false) };
        if let (Some(t), Some(l)) = (self.stack.last_mut(), saved_level) {
            t.level = l;
        }
        let base = match level {
            Level::Member => s
                .resource
                .as_ref()
                .map(|r| if r.singular_resource { s.path.clone() } else { join_path(&s.path, ":id") }),
            _ => None,
        }
        .unwrap_or_else(|| s.path.clone());
        let full_path = join_path(&base, &path_text);
        self.add(name, verb, full_path, controller, action, call_span);
    }

    fn scope_call(&mut self, node: &CallNode<'_>, name: &str) {
        let args = CallArgs::of(node);
        let s = self.top().clone();
        let mut next = Scope { level: Level::Plain, resource: None, ..s.clone() };
        match name {
            "namespace" => {
                let Some((ns, _)) = args.positional.first().and_then(sym_or_str) else { return };
                let path = args.text("path").map(|(p, _)| p).unwrap_or_else(|| ns.clone());
                next.path = join_path(&s.path, &path);
                next.module = match args.text("module") {
                    Some((m, _)) => format!("{}{m}/", s.module),
                    None => format!("{}{ns}/", s.module),
                };
                next.name_prefix.push(args.text("as").map(|(a, _)| a).unwrap_or(ns));
            }
            "scope" => {
                if let Some((p, _)) = args.positional.first().and_then(sym_or_str) {
                    next.path = join_path(&s.path, &p);
                }
                if let Some((p, _)) = args.text("path") {
                    next.path = join_path(&s.path, &p);
                }
                if let Some((m, _)) = args.text("module") {
                    next.module = format!("{}{m}/", s.module);
                }
                if let Some((a, _)) = args.text("as") {
                    next.name_prefix.push(a);
                }
                if let Some((c, _)) = args.text("controller") {
                    next.controller = Some(format!("{}{c}", next.module));
                }
            }
            "controller" => {
                if let Some((c, c_span)) = args.positional.first().and_then(sym_or_str) {
                    let full = format!("{}{c}", s.module);
                    self.link(c_span, &full, None);
                    next.controller = Some(full);
                }
            }
            "member" | "collection" => {
                next = Scope { level: if name == "member" { Level::Member } else { Level::Collection }, ..s.clone() };
            }
            _ => {}
        }
        self.with_scope(next, node);
    }
}

impl<'pr> Visit<'pr> for RoutesVisitor<'_> {
    fn visit_call_node(&mut self, node: &CallNode<'pr>) {
        let name = text(node.name().as_slice());
        if node.receiver().is_some() && name != "draw" {
            ruby_prism::visit_call_node(self, node);
            return;
        }
        match name.as_str() {
            "draw" => {
                // `Rails.application.routes.draw do`, or `draw :admin`.
                if let Some((file, _)) = CallArgs::of(node).positional.first().and_then(sym_or_str) {
                    let path = self.root.join("config/routes").join(format!("{file}.rb"));
                    let stack = self.stack.clone();
                    parse_routes_file(self.root, &path, stack, self.out, self.depth + 1, self.read);
                } else {
                    ruby_prism::visit_call_node(self, node);
                }
            }
            "resources" => self.resources(node, false),
            "resource" => self.resources(node, true),
            "get" | "post" | "put" | "patch" | "delete" | "match" | "options" => {
                let verb = name.to_uppercase();
                self.verb(node, &verb);
            }
            "root" => {
                let args = CallArgs::of(node);
                let to = args.text("to").or_else(|| args.positional.first().and_then(sym_or_str));
                let s = self.top().clone();
                let (mut controller, mut action) = (None, None);
                if let Some((t, t_span)) = to
                    && let Some((c, a)) = t.split_once('#')
                {
                    let c = format!("{}{c}", s.module);
                    self.link(t_span, &c, Some(a.to_string()));
                    controller = Some(c);
                    action = Some(a.to_string());
                }
                let mut parts = s.name_prefix.clone();
                parts.push(args.text("as").map(|(a, _)| a).unwrap_or("root".into()));
                let path = if s.path.is_empty() { "/".to_string() } else { s.path.clone() };
                let path = if path.starts_with('/') { path } else { format!("/{path}") };
                self.add(Some(parts.join("_")), "GET", path, controller, action, span(&node.location()));
            }
            "mount" => {
                let args = CallArgs::of(node);
                if let Some((a, _)) = args.text("as") {
                    let mut parts = self.top().name_prefix.clone();
                    parts.push(a);
                    let at = args.text("at").map(|(p, _)| p).unwrap_or_default();
                    self.add(Some(parts.join("_")), "MOUNT", at, None, None, span(&node.location()));
                }
                ruby_prism::visit_call_node(self, node);
            }
            "direct" => {
                if let Some((n, _)) = CallArgs::of(node).positional.first().and_then(sym_or_str) {
                    self.add(Some(n), "DIRECT", String::new(), None, None, span(&node.location()));
                }
            }
            "namespace" | "scope" | "controller" | "member" | "collection" => self.scope_call(node, &name),
            _ => ruby_prism::visit_call_node(self, node),
        }
    }
}

/// The controller class for a route controller path: `admin/invoices` →
/// `Admin::InvoicesController`.
pub fn controller_class(controller: &str) -> String {
    format!("{}Controller", inflect::camelize(controller))
}

// ---- locales ---------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LocaleKey {
    pub locale: String,
    /// The key without the locale: `admin.invoices.show.title`.
    pub key: String,
    pub span: Span,
}

/// Reads the keys of a Rails locale file. Rails locale files are plain
/// nested maps, so a line scanner is enough: it tracks indentation and
/// skips block scalars (`|`, `>`), list items and comments.
pub fn parse_locale(source: &str) -> Vec<LocaleKey> {
    let mut out = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut block_indent: Option<usize> = None;
    let mut offset = 0usize;
    for line in source.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        let indent = content.len() - content.trim_start().len();
        let trimmed = content.trim_start();
        if let Some(bi) = block_indent {
            if trimmed.is_empty() || indent > bi {
                continue;
            }
            block_indent = None;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') || trimmed.starts_with("---") {
            continue;
        }
        let Some((key, key_start, rest)) = split_key(trimmed) else { continue };
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let rest = rest.trim();
        if rest.starts_with('|') || rest.starts_with('>') {
            block_indent = Some(indent);
        }
        stack.push((indent, key.clone()));
        if stack.len() >= 2 {
            let locale = stack[0].1.clone();
            let full = stack[1..].iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>().join(".");
            let start = line_start + indent + key_start;
            out.push(LocaleKey { locale, key: full, span: Span::new(start, start + key.len()) });
        }
        // A leaf value: pop it again on the next sibling (the stack check
        // above does that through the indent).
    }
    out
}

/// Splits `key: value` (with optional quotes around the key). Returns the
/// key, where it starts in the trimmed line, and the rest.
fn split_key(trimmed: &str) -> Option<(String, usize, &str)> {
    let (key, start, after) = if let Some(q) = trimmed.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let end = trimmed[1..].find(q)? + 1;
        (trimmed[1..end].to_string(), 1, &trimmed[end + 1..])
    } else {
        let colon = trimmed.find(':')?;
        (trimmed[..colon].trim_end().to_string(), 0, &trimmed[colon..])
    };
    let rest = after.strip_prefix(':')?;
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    (!key.is_empty()).then_some((key, start, rest))
}

/// The lazy-lookup prefix for `t(".x")` in a file. `class_name` is the
/// class the call is in, `method` the method it is in.
pub fn lazy_prefixes(rel_path: &str, class_name: Option<&str>, method: Option<&str>, is_phlex: bool) -> Vec<String> {
    let mut out = Vec::new();
    if is_phlex && let Some(c) = class_name {
        // phlex-rails: the class name, `::` → `.`, CamelCase → snake_case.
        let mut s = c.replace("::", ".");
        let re = regex::Regex::new("([a-z])([A-Z])").expect("valid");
        s = re.replace_all(&s, "${1}_${2}").to_lowercase();
        out.push(s);
    }
    if let Some(view) = rel_path.strip_prefix("app/views/").filter(|_| !rel_path.ends_with(".rb")) {
        // `admin/invoices/_form.html.erb` → `admin.invoices.form`
        let no_ext = view.split('.').next().unwrap_or(view);
        let parts: Vec<&str> = no_ext.split('/').collect();
        let mut keys: Vec<String> = parts.iter().map(|p| p.trim_start_matches('_').to_string()).collect();
        if let Some(last) = keys.last_mut() {
            *last = last.trim_start_matches('_').to_string();
        }
        out.push(keys.join("."));
    }
    if let (Some(c), Some(m)) = (class_name, method) {
        if let Some(base) = c.strip_suffix("Controller") {
            out.push(format!("{}.{m}", inflect::underscore(base).replace('/', ".")));
        }
        if c.ends_with("Mailer") {
            out.push(format!("{}.{m}", inflect::underscore(c).replace('/', ".")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routes(main: &str, extra: &[(&str, &str)]) -> Routes {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("rubyscope-routes-{}-{n}", std::process::id()));
        std::fs::create_dir_all(dir.join("config/routes")).unwrap();
        std::fs::write(dir.join("config/routes.rb"), main).unwrap();
        for (name, code) in extra {
            std::fs::write(dir.join("config/routes").join(format!("{name}.rb")), code).unwrap();
        }
        let r = parse_routes(&dir, &|p: &Path| std::fs::read(p).ok());
        std::fs::remove_dir_all(&dir).ok();
        r
    }

    fn names(r: &Routes) -> Vec<String> {
        r.routes.iter().filter_map(|r| r.name.clone()).collect()
    }

    #[test]
    fn resources_make_the_rails_helper_names() {
        let r = routes(
            "Rails.application.routes.draw do\n  resources :invoices do\n    member do\n      get :preview\n    end\n    get :search, on: :collection\n    resources :items, only: [:index, :show]\n  end\n  resource :profile, only: [:show, :edit]\n  get \"about\", to: \"pages#about\"\n  root \"home#index\"\nend\n",
            &[],
        );
        let n = names(&r);
        for expected in [
            "invoices",
            "new_invoice",
            "edit_invoice",
            "invoice",
            "preview_invoice",
            "search_invoices",
            "invoice_items",
            "invoice_item",
            "profile",
            "edit_profile",
            "about",
            "root",
        ] {
            assert!(n.contains(&expected.to_string()), "missing {expected} in {n:?}");
        }
        let about = r.routes.iter().find(|r| r.name.as_deref() == Some("about")).unwrap();
        assert_eq!(about.controller.as_deref(), Some("pages"));
        assert_eq!(about.action.as_deref(), Some("about"));
    }

    #[test]
    fn namespaces_and_draw() {
        let r = routes(
            "Rails.application.routes.draw do\n  draw :admin\nend\n",
            &[(
                "admin",
                "namespace :admin do\n  resources :entities do\n    resources :members, only: :index\n  end\nend\n",
            )],
        );
        let n = names(&r);
        assert!(n.contains(&"admin_entities".to_string()), "{n:?}");
        assert!(n.contains(&"edit_admin_entity".to_string()), "{n:?}");
        assert!(n.contains(&"admin_entity_members".to_string()), "{n:?}");
        let ent = r.routes.iter().find(|r| r.name.as_deref() == Some("admin_entities")).unwrap();
        assert_eq!(ent.controller.as_deref(), Some("admin/entities"));
        assert_eq!(controller_class("admin/entities"), "Admin::EntitiesController");
    }

    #[test]
    fn locale_keys_with_lines() {
        let yml = "en:\n  admin:\n    title: \"Admin\"\n    body: |\n      text: not a key\n    list:\n      - a\n    \"quoted\": x\n  other: y\n";
        let keys = parse_locale(yml);
        let k: Vec<_> = keys.iter().map(|k| format!("{}:{}", k.locale, k.key)).collect();
        assert_eq!(
            k,
            vec!["en:admin", "en:admin.title", "en:admin.body", "en:admin.list", "en:admin.quoted", "en:other"]
        );
        let title = &keys[1];
        assert_eq!(&yml[title.span.start as usize..title.span.end as usize], "title");
    }

    #[test]
    fn schema_columns() {
        let tables = parse_schema(
            b"ActiveRecord::Schema[8.0].define(version: 1) do\n  create_table \"invoices\", id: :uuid do |t|\n    t.string \"number\", null: false\n    t.references \"entity\"\n    t.timestamps\n    t.index [\"number\"]\n  end\nend\n",
        );
        let cols: Vec<_> = tables[0].columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(cols, vec!["id", "number", "entity_id", "created_at", "updated_at"]);
        assert_eq!(tables[0].columns[0].sql_type, "uuid");
    }

    #[test]
    fn lazy_lookup_prefixes() {
        assert_eq!(
            lazy_prefixes("app/views/admin/x.rb", Some("Views::Admin::SalesReports::Card"), None, true),
            vec!["views.admin.sales_reports.card".to_string()]
        );
        assert_eq!(
            lazy_prefixes("app/views/admin/invoices/_form.html.erb", None, None, false),
            vec!["admin.invoices.form"]
        );
        assert_eq!(
            lazy_prefixes("app/controllers/admin/x.rb", Some("Admin::InvoicesController"), Some("show"), false),
            vec!["admin.invoices.show"]
        );
    }
}
