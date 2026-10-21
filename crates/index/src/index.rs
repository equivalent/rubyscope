//! The project index: every file, namespace, constant and method, joined
//! across files with full names.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use rayon::prelude::*;

use crate::cache::{self, Body, Reuse, Stamp};
use crate::discover::{self, Origin, RubyEnv, Source};
use crate::extract::{extract, extract_erb};
use crate::model::*;
use crate::rails;

pub type FileId = u32;
pub type MethodId = u32;

/// The owner of methods that are defined in a top-level block.
pub const BLOCK_OWNER: &str = "(block)";

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub source: u32,
    /// What extraction found, without `refs` and `locals`: use `refs()`
    /// and `locals()`. Shared, so a new build can take a file that did
    /// not change.
    pub raw: Arc<RawFile>,
    pub(crate) body: Arc<Body>,
    /// The size and change time when the file was read.
    pub stamp: Stamp,
    /// The full name of each scope in `raw.scopes`. The root scope is "".
    pub scope_names: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loc {
    pub file: FileId,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstKind {
    Class,
    Module,
    Value,
}

#[derive(Debug, Clone)]
pub struct ConstDef {
    pub kind: ConstKind,
    pub loc: Loc,
    /// For `Foo = Bar`: the path of `Bar` and the scope it is written in.
    pub alias_of: Option<(ConstPath, FileId, u32)>,
}

#[derive(Debug, Clone, Default)]
pub struct Namespace {
    pub is_class: bool,
    pub superclass: Option<String>,
    pub includes: Vec<String>,
    pub prepends: Vec<String>,
    pub extends: Vec<String>,
    pub extend_self: bool,
    pub is_concern: bool,
}

#[derive(Debug, Clone)]
pub struct MethodDef {
    pub owner: String,
    pub name: String,
    pub singleton: bool,
    pub kind: MethodKind,
    pub visibility: Visibility,
    pub loc: Loc,
    pub params: String,
    pub alias_of: Option<String>,
    /// The lexical scope of the definition in its file.
    pub scope: u32,
    pub doc: Option<String>,
    pub returns: Option<Expr>,
    pub delegate_to: Option<String>,
    /// For database columns: the Ruby class of the value.
    pub value_class: Option<String>,
}

impl MethodDef {
    pub fn label(&self) -> String {
        let sep = if self.singleton { "." } else { "#" };
        format!("{}{sep}{}", self.owner, self.name)
    }
}

#[derive(Debug, Default)]
pub struct OwnerMethods {
    pub instance: HashMap<String, Vec<MethodId>>,
    pub singleton: HashMap<String, Vec<MethodId>>,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub files: usize,
    pub project_files: usize,
    pub sources: usize,
    pub parse_errors: usize,
    pub bytes: u64,
    /// Files taken from the saved index or the last build, not parsed.
    pub reused: usize,
    /// True when the saved index has exactly these files, so it does not
    /// need to be written again.
    pub same_as_saved: bool,
    pub discover: Duration,
    pub parse: Duration,
    pub link: Duration,
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub gems: bool,
    pub stdlib: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self { gems: true, stdlib: true }
    }
}

#[derive(Debug, Clone)]
pub struct Association {
    pub kind: AssocKind,
    pub name: String,
    /// The full name of the associated class, when it is known.
    pub class: Option<String>,
    pub polymorphic: bool,
    pub through: Option<String>,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct FactoryInfo {
    pub loc: Loc,
    pub class_name: Option<String>,
    pub parent: Option<String>,
}

#[derive(Debug, Default)]
pub struct Index {
    pub root: PathBuf,
    pub sources: Vec<Source>,
    pub files: Vec<FileEntry>,
    pub file_ids: HashMap<PathBuf, FileId>,
    /// Every file of the project, relative to the root, for "go to file".
    pub all_files: Vec<String>,
    pub consts: HashMap<String, Vec<ConstDef>>,
    pub namespaces: HashMap<String, Namespace>,
    pub methods: Vec<MethodDef>,
    pub owners: HashMap<String, OwnerMethods>,
    pub methods_by_name: HashMap<String, Vec<MethodId>>,
    /// Modules with no definition of their own, such as the module Zeitwerk
    /// makes for a folder.
    pub implicit_namespaces: HashSet<String>,
    /// Classes with `self.abstract_class = true` or `primary_abstract_class`.
    pub abstract_classes: HashSet<String>,
    /// Associations by owner class.
    pub associations: HashMap<String, Vec<Association>>,
    /// Database table name by model class.
    pub model_tables: HashMap<String, String>,
    pub routes: Vec<rails::Route>,
    /// Route ids by helper name (without `_path` / `_url`).
    pub route_helpers: HashMap<String, Vec<usize>>,
    pub route_links: HashMap<FileId, Vec<rails::RouteLink>>,
    /// Locale definitions by key (without the locale): (locale, place).
    pub locale_keys: HashMap<String, Vec<(String, Loc)>>,
    /// Stimulus controller files by identifier (`admin--tabs`).
    pub stimulus: HashMap<String, FileId>,
    pub shared_groups: HashMap<String, Vec<Loc>>,
    pub factories: HashMap<String, Vec<FactoryInfo>>,
    /// Traits by (factory, name); global traits have no factory.
    pub traits: HashMap<(Option<String>, String), Vec<Loc>>,
    /// Instance variable writes by (owner, name): (file, index in `ivars`).
    pub ivar_index: HashMap<(String, String), Vec<(FileId, u32)>>,
    /// References by name (method name, or last constant segment):
    /// (file, index in `refs`). Used by "find usages", and built on first
    /// use: see `refs_by_name()`.
    pub(crate) refs_by_name: OnceLock<HashMap<String, Vec<(FileId, u32)>>>,
    pub subclasses: HashMap<String, Vec<String>>,
    /// Classes and modules that include, extend or prepend a module.
    pub includers: HashMap<String, Vec<String>>,
    pub stats: Stats,
    /// The stamp of `config/initializers/inflections.rb`. Extraction uses
    /// the inflections, so no file is reused when it changes.
    pub(crate) inflections: Option<Stamp>,
    /// File contents for `from_memory` indexes.
    pub memory: HashMap<PathBuf, String>,
    ancestors_cache: RwLock<HashMap<String, Arc<Vec<String>>>>,
}

impl FileEntry {
    /// The references in the file, sorted by start.
    pub fn refs(&self) -> &[RawRef] {
        &self.body.get().refs
    }

    /// True when the references and local variables of the file are
    /// decoded and kept in memory. Most files keep them as bytes until a
    /// click needs them.
    pub fn body_is_decoded(&self) -> bool {
        self.body.is_decoded()
    }

    /// The local variable assignments in the file.
    pub fn locals(&self) -> &[LocalAssign] {
        &self.body.get().locals
    }
}

/// What one file defines, collected in parallel and then added in order.
#[derive(Default)]
struct Collected {
    consts: Vec<(String, ConstDef)>,
    /// (name, is a class, is a concern)
    namespaces: Vec<(String, bool, bool)>,
    methods: Vec<MethodDef>,
}

/// Extracts a file by its kind: Ruby, ERB, or text.
pub fn extract_path(path: &Path, bytes: &[u8]) -> RawFile {
    match discover::file_kind(path) {
        FileKind::Ruby => extract(bytes),
        FileKind::Erb => extract_erb(bytes),
        FileKind::Text => RawFile::text(bytes),
    }
}

impl Index {
    pub fn build(project: &Path, opts: &BuildOptions) -> Result<Index> {
        Self::build_reusing(project, opts, Reuse::default)
    }

    /// Builds the index, and takes each file that did not change from the
    /// index saved in `cache_dir`. Save it again with `save`.
    pub fn load(project: &Path, opts: &BuildOptions, cache_dir: &Path) -> Result<Index> {
        let project = project.canonicalize()?;
        let file = cache::file_for(cache_dir, &project);
        Self::build_reusing(&project, opts, || cache::read(&file).unwrap_or_default())
    }

    /// Writes the parsed files to `cache_dir`, for `load`.
    pub fn save(&self, cache_dir: &Path) -> Result<()> {
        cache::write(&cache::file_for(cache_dir, &self.root), self)
    }

    /// Builds again after files changed. Files that did not change are
    /// taken from this index.
    pub fn rebuild(&self, opts: &BuildOptions) -> Result<Index> {
        let reuse = Reuse::from_index(self);
        Self::build_reusing(&self.root, opts, move || reuse)
    }

    /// `reuse` runs while Ruby is asked for its paths and the files are
    /// listed.
    fn build_reusing(project: &Path, opts: &BuildOptions, reuse: impl FnOnce() -> Reuse + Send) -> Result<Index> {
        let started = Instant::now();
        let project = project.canonicalize()?;
        let inflections_path = project.join("config/initializers/inflections.rb");
        let inflections = std::fs::metadata(&inflections_path).ok().map(|m| Stamp::of(&m));
        if let Ok(text) = std::fs::read_to_string(&inflections_path) {
            crate::inflect::configure(crate::inflect::parse_initializer(&text));
        }
        let (mut reuse, (sources, mut listed, all_files)) = rayon::join(reuse, || {
            let env = if opts.gems || opts.stdlib {
                RubyEnv::detect(&project).unwrap_or_default()
            } else {
                RubyEnv::default()
            };
            let sources: Vec<Source> = discover::sources(&project, &env)
                .into_iter()
                .filter(|s| match s.origin {
                    Origin::Project => true,
                    Origin::Gem { .. } => opts.gems,
                    Origin::Stdlib => opts.stdlib,
                })
                .collect();
            let (mut listed, all_files) = rayon::join(
                || discover::list_files(&sources[0]).into_iter().map(|p| (0, p)).collect::<Vec<(u32, PathBuf)>>(),
                || discover::list_all_files(&project),
            );
            // Templates, locale files and Stimulus controllers can be targets.
            listed.extend(
                all_files.iter().filter(|rel| discover::is_linked_text_file(rel)).map(|rel| (0, project.join(rel))),
            );
            (sources, listed, all_files)
        });
        // Gems: the saved list of files of an installed gem, else a new list.
        listed.par_extend(sources.par_iter().enumerate().skip(1).flat_map_iter(|(i, s)| {
            let files = match reuse.lists.get(&s.root).filter(|_| s.fixed) {
                Some(files) => files.clone(),
                None => discover::list_files(s),
            };
            files.into_iter().map(move |p| (i as u32, p))
        }));
        if reuse.inflections != inflections {
            reuse.files.clear();
        }
        let discover_time = started.elapsed();

        let parse_start = Instant::now();
        let mut files: Vec<(FileEntry, u64, bool)> = listed
            .into_par_iter()
            .filter_map(|(source, path)| {
                let meta = std::fs::metadata(&path).ok()?;
                let stamp = Stamp::of(&meta);
                if let Some((old, raw, body)) = reuse.files.get(&path)
                    && *old == stamp
                {
                    let (raw, body) = (raw.clone(), body.clone());
                    let entry = FileEntry { path, source, raw, body, stamp, scope_names: Vec::new() };
                    return Some((entry, meta.len(), true));
                }
                let bytes = std::fs::read(&path).ok()?;
                let (raw, body) = cache::split(extract_path(&path, &bytes));
                let entry = FileEntry { path, source, raw, body, stamp, scope_names: Vec::new() };
                Some((entry, bytes.len() as u64, false))
            })
            .collect();
        // Project files first, then gems: lower ids rank first in lists.
        files.sort_by(|a, b| (a.0.source, &a.0.path).cmp(&(b.0.source, &b.0.path)));
        let parse_time = parse_start.elapsed();

        let mut index = Index { root: project, sources, all_files, inflections, ..Default::default() };
        index.stats.bytes = files.iter().map(|f| f.1).sum();
        index.stats.reused = files.iter().filter(|f| f.2).count();
        index.stats.same_as_saved =
            reuse.saved && index.stats.reused == files.len() && reuse.files.len() == files.len();
        index.files = files.into_iter().map(|f| f.0).collect();
        let link_start = Instant::now();
        index.link();
        index.stats.files = index.files.len();
        index.stats.project_files = index.files.iter().filter(|f| f.source == 0).count();
        index.stats.sources = index.sources.len();
        index.stats.parse_errors = index.files.iter().filter(|f| f.raw.parse_errors > 0).count();
        index.stats.discover = discover_time;
        index.stats.parse = parse_time;
        index.stats.link = link_start.elapsed();
        Ok(index)
    }

    /// Builds an index from code in memory, as project files. For tests.
    pub fn from_memory(files: &[(&str, &str)]) -> Index {
        let root = PathBuf::from("/memory");
        let sources = vec![Source { root: root.clone(), origin: Origin::Project, dirs: Vec::new(), fixed: false }];
        let mut index = Index { root: root.clone(), sources, ..Default::default() };
        index.files = files
            .iter()
            .map(|(path, code)| {
                let (raw, body) = cache::split(extract_path(Path::new(path), code.as_bytes()));
                FileEntry {
                    path: root.join(path),
                    source: 0,
                    raw,
                    body,
                    stamp: Stamp::default(),
                    scope_names: Vec::new(),
                }
            })
            .collect();
        index.all_files = files.iter().map(|(p, _)| p.to_string()).collect();
        index.memory = files.iter().map(|(p, c)| (root.join(p), c.to_string())).collect();
        index.link();
        index
    }

    /// The text of a file: from memory for tests, else from disk.
    pub fn read_text(&self, file: FileId) -> Option<String> {
        let path = &self.files[file as usize].path;
        if let Some(t) = self.memory.get(path) {
            return Some(t.clone());
        }
        std::fs::read_to_string(path).ok()
    }

    /// The text of a project file by relative path.
    pub fn read_rel(&self, rel: &str) -> Option<String> {
        let path = self.root.join(rel);
        if let Some(t) = self.memory.get(&path) {
            return Some(t.clone());
        }
        std::fs::read_to_string(path).ok()
    }

    /// The path of a project file relative to the root.
    pub fn rel_path(&self, file: FileId) -> Option<String> {
        let path = &self.files[file as usize].path;
        path.strip_prefix(&self.root).ok().map(|p| p.display().to_string())
    }

    pub fn origin(&self, file: FileId) -> &Origin {
        &self.sources[self.files[file as usize].source as usize].origin
    }

    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        let abs = if path.is_absolute() { path.to_path_buf() } else { self.root.join(path) };
        if let Some(id) = self.file_ids.get(&abs) {
            return Some(*id);
        }
        let abs = abs.canonicalize().unwrap_or(abs);
        self.file_ids.get(&abs).copied()
    }

    /// A path that is the same on every machine: relative for project files,
    /// `gem:<name>/<path in the gem>` for gems, `stdlib:<path>` for the
    /// standard library.
    pub fn stable_path(&self, file: FileId) -> String {
        let f = &self.files[file as usize];
        let source = &self.sources[f.source as usize];
        let rel = f.path.strip_prefix(&source.root).unwrap_or(&f.path).display();
        match &source.origin {
            Origin::Project => rel.to_string(),
            Origin::Gem { name, .. } => format!("gem:{name}/{rel}"),
            Origin::Stdlib => format!("stdlib:{rel}"),
        }
    }

    /// The path to show for a file: relative for project files, else absolute.
    pub fn display_path(&self, file: FileId) -> String {
        let path = &self.files[file as usize].path;
        path.strip_prefix(&self.root).unwrap_or(path).display().to_string()
    }

    /// Joins the per-file data into the global tables.
    fn link(&mut self) {
        for (i, f) in self.files.iter().enumerate() {
            self.file_ids.insert(f.path.clone(), i as FileId);
        }

        // Full names in two passes. The first pass joins names as written.
        // The second pass looks up the first segment of compact names like
        // `class Foo::Bar` in the lexical scopes, using the names from the
        // first pass.
        let naive: Vec<Vec<String>> = self.files.par_iter().map(|f| scope_names(&f.raw, None)).collect();
        // Every prefix counts as known: Zeitwerk makes a module for each
        // folder (`action_mailbox/ingresses/` → `ActionMailbox::Ingresses`),
        // even when no file defines it.
        let mut known: HashSet<String> = HashSet::new();
        for name in naive.iter().flatten().filter(|n| !n.is_empty()) {
            let mut end = 0;
            while let Some(i) = name[end..].find("::") {
                known.insert(name[..end + i].to_string());
                end += i + 2;
            }
            known.insert(name.clone());
        }
        let names: Vec<Vec<String>> = self.files.par_iter().map(|f| scope_names(&f.raw, Some(&known))).collect();
        for (f, n) in self.files.iter_mut().zip(names) {
            f.scope_names = n;
        }

        let collected: Vec<Collected> =
            (0..self.files.len() as FileId).into_par_iter().map(|id| self.collect_file(id, &known)).collect();
        for c in collected {
            self.add_collected(c);
        }
        // Namespaces that only exist as a prefix, like Zeitwerk folders.
        for name in self.consts.keys() {
            let mut end = 0;
            while let Some(i) = name[end..].find("::") {
                let prefix = &name[..end + i];
                if !self.consts.contains_key(prefix) {
                    self.implicit_namespaces.insert(prefix.to_string());
                }
                end += i + 2;
            }
        }
        for name in &self.implicit_namespaces {
            self.namespaces.entry(name.clone()).or_default();
        }
        self.resolve_hierarchy();
        self.link_extra();
    }

    /// Forgets cached ancestors, after the hierarchy changed.
    pub(crate) fn clear_ancestors(&self) {
        self.ancestors_cache.write().unwrap().clear();
    }

    /// Adds a method to the tables.
    pub(crate) fn push_method(&mut self, def: MethodDef) -> MethodId {
        let id = self.methods.len() as MethodId;
        let table = self.owners.entry(def.owner.clone()).or_default();
        let side = if def.singleton { &mut table.singleton } else { &mut table.instance };
        side.entry(def.name.clone()).or_default().push(id);
        self.methods_by_name.entry(def.name.clone()).or_default().push(id);
        self.methods.push(def);
        id
    }

    /// The constants, namespaces and methods that one file defines.
    fn collect_file(&self, file_id: FileId, known: &HashSet<String>) -> Collected {
        let f = &self.files[file_id as usize];
        let mut out = Collected::default();
        for (i, scope) in f.raw.scopes.iter().enumerate() {
            let name = &f.scope_names[i];
            if scope.kind == ScopeKind::Root {
                continue;
            }
            if scope.kind == ScopeKind::EvalBlock {
                // `Foo.class_eval` reopens `Foo`; it does not define it.
                out.namespaces.push((name.clone(), false, false));
                continue;
            }
            let kind = match scope.kind {
                ScopeKind::Class => ConstKind::Class,
                _ => ConstKind::Module,
            };
            let loc = Loc { file: file_id, span: scope.span, name_span: scope.name_span };
            out.consts.push((name.clone(), ConstDef { kind, loc, alias_of: None }));
            out.namespaces.push((name.clone(), kind == ConstKind::Class, scope.is_concern));
        }
        for w in &f.raw.const_writes {
            let lex = lexical_name(&f.raw, &f.scope_names, w.scope);
            let name = qualify(&w.path, &lexical_chain(&f.raw, &f.scope_names, w.scope), lex, Some(known));
            let loc = Loc { file: file_id, span: w.span, name_span: w.name_span };
            let alias_of = w.alias_of.clone().map(|p| (p, file_id, w.scope));
            out.consts.push((name, ConstDef { kind: ConstKind::Value, loc, alias_of }));
        }
        for m in &f.raw.methods {
            let mut owner = owner_name(&f.scope_names, m.scope);
            if owner == "Object" && m.in_block {
                // A `def` in a top-level block (RSpec `describe do`, a Rake
                // `namespace do`) belongs to whatever runs the block.
                owner = BLOCK_OWNER.to_string();
            }
            out.methods.push(MethodDef {
                owner,
                name: m.name.clone(),
                singleton: m.singleton,
                kind: m.kind,
                visibility: m.visibility,
                loc: Loc { file: file_id, span: m.span, name_span: m.name_span },
                params: m.params.clone(),
                alias_of: m.alias_of.clone(),
                scope: m.scope,
                doc: m.doc.clone(),
                returns: m.returns.clone(),
                delegate_to: m.delegate_to.clone(),
                value_class: None,
            });
        }
        out
    }

    /// Adds what `collect_file` found to the tables, in file order.
    fn add_collected(&mut self, c: Collected) {
        for (name, is_class, is_concern) in c.namespaces {
            let ns = self.namespaces.entry(name).or_default();
            ns.is_class |= is_class;
            ns.is_concern |= is_concern;
        }
        for (name, def) in c.consts {
            self.consts.entry(name).or_default().push(def);
        }
        for def in c.methods {
            self.push_method(def);
        }
    }

    /// Resolves superclasses and mixins to full names.
    fn resolve_hierarchy(&mut self) {
        let mut updates: Vec<(String, Namespace)> = Vec::new();
        for f in &self.files {
            for (i, scope) in f.raw.scopes.iter().enumerate() {
                if matches!(scope.kind, ScopeKind::Root) {
                    continue;
                }
                let mut ns = Namespace::default();
                if let Some(sup) = &scope.superclass {
                    // The superclass is written outside the class body.
                    let outer = scope.parent.unwrap_or(0);
                    // A superclass the index does not know (a gem that is not
                    // indexed) keeps its name as written.
                    ns.superclass = self
                        .lookup_lexical(sup, &lexical_chain(&f.raw, &f.scope_names, outer))
                        .or_else(|| Some(sup.joined()));
                }
                let chain = lexical_chain(&f.raw, &f.scope_names, i as u32);
                for mixin in &scope.mixins {
                    let target = match &mixin.target {
                        MixinTarget::SelfRef => {
                            if mixin.kind == MixinKind::Extend {
                                ns.extend_self = true;
                            }
                            continue;
                        }
                        MixinTarget::Const(p) => match self.lookup_lexical(p, &chain) {
                            Some(t) => t,
                            None => p.joined(),
                        },
                    };
                    match mixin.kind {
                        MixinKind::Include => ns.includes.push(target),
                        MixinKind::Extend => ns.extends.push(target),
                        MixinKind::Prepend => ns.prepends.push(target),
                    }
                }
                updates.push((f.scope_names[i].clone(), ns));
            }
        }
        for (name, add) in updates {
            let ns = self.namespaces.entry(name).or_default();
            if ns.superclass.is_none() {
                ns.superclass = add.superclass;
            }
            for m in add.includes {
                if !ns.includes.contains(&m) {
                    ns.includes.push(m);
                }
            }
            for m in add.prepends {
                if !ns.prepends.contains(&m) {
                    ns.prepends.push(m);
                }
            }
            for m in add.extends {
                if !ns.extends.contains(&m) {
                    ns.extends.push(m);
                }
            }
            ns.extend_self |= add.extend_self;
        }
    }

    pub fn const_exists(&self, name: &str) -> bool {
        self.consts.contains_key(name) || self.implicit_namespaces.contains(name)
    }

    /// Constant lookup with the lexical scopes and the top level only. The
    /// indexer uses it before the ancestors are known.
    fn lookup_lexical(&self, path: &ConstPath, chain: &[String]) -> Option<String> {
        let first = path.segments.first()?;
        let mut cur = if path.absolute {
            self.const_exists(first).then(|| first.clone())?
        } else {
            chain
                .iter()
                .map(|c| format!("{c}::{first}"))
                .find(|n| self.const_exists(n))
                .or_else(|| self.const_exists(first).then(|| first.clone()))?
        };
        for seg in &path.segments[1..] {
            let next = format!("{cur}::{seg}");
            if !self.const_exists(&next) {
                return None;
            }
            cur = next;
        }
        Some(cur)
    }

    /// Ruby's constant lookup: the lexical scopes (innermost first), then the
    /// ancestors of the innermost scope, then the top level.
    pub fn lookup_const(&self, path: &ConstPath, chain: &[String]) -> Option<String> {
        let first = path.segments.first()?;
        let mut cur = if path.absolute {
            self.const_exists(first).then(|| first.clone())?
        } else {
            let lexical = chain.iter().map(|c| format!("{c}::{first}")).find(|n| self.const_exists(n));
            let inherited = || {
                let cref = chain.first()?;
                self.ancestors(cref)
                    .iter()
                    .skip(1)
                    .filter(|a| a.as_str() != "Object")
                    .map(|a| format!("{a}::{first}"))
                    .find(|n| self.const_exists(n))
            };
            lexical.or_else(inherited).or_else(|| self.const_exists(first).then(|| first.clone()))?
        };
        for seg in &path.segments[1..] {
            cur = self.lookup_in(&cur, seg, 0)?;
        }
        Some(cur)
    }

    /// Looks up `container::seg`, also in the container's ancestors and
    /// through constant aliases (`Foo = Bar`).
    fn lookup_in(&self, container: &str, seg: &str, depth: u32) -> Option<String> {
        let direct = format!("{container}::{seg}");
        if self.const_exists(&direct) {
            return Some(direct);
        }
        for anc in self.ancestors(container).iter().skip(1) {
            if anc == "Object" || anc == "BasicObject" || anc == "Kernel" {
                continue;
            }
            let n = format!("{anc}::{seg}");
            if self.const_exists(&n) {
                return Some(n);
            }
        }
        if depth < 4
            && let Some(target) = self.alias_target(container)
        {
            return self.lookup_in(&target, seg, depth + 1);
        }
        None
    }

    /// For `Foo = Bar`, returns the full name of `Bar`.
    pub fn alias_target(&self, name: &str) -> Option<String> {
        let defs = self.consts.get(name)?;
        let (path, file, scope) = defs.iter().find_map(|d| d.alias_of.clone())?;
        let f = &self.files[file as usize];
        self.lookup_const(&path, &lexical_chain(&f.raw, &f.scope_names, scope))
    }

    /// Like `alias_target`, but without ancestor lookups, so it can run
    /// while the ancestors are computed.
    fn alias_target_lexical(&self, name: &str) -> Option<String> {
        let defs = self.consts.get(name)?;
        let (path, file, scope) = defs.iter().find_map(|d| d.alias_of.clone())?;
        let f = &self.files[file as usize];
        self.lookup_lexical(&path, &lexical_chain(&f.raw, &f.scope_names, scope))
    }

    /// The lexical scope names around a scope in a file, innermost first.
    pub fn chain(&self, file: FileId, scope: u32) -> Vec<String> {
        let f = &self.files[file as usize];
        lexical_chain(&f.raw, &f.scope_names, scope)
    }

    /// The ancestors of a class or module in Ruby's method lookup order:
    /// prepended modules, itself, included modules (last included first),
    /// then the superclass and its ancestors.
    pub fn ancestors(&self, name: &str) -> Arc<Vec<String>> {
        if let Some(a) = self.ancestors_cache.read().unwrap().get(name) {
            return a.clone();
        }
        let mut visiting = HashSet::new();
        let list = Arc::new(self.compute_ancestors(name, &mut visiting));
        self.ancestors_cache.write().unwrap().insert(name.to_string(), list.clone());
        list
    }

    fn compute_ancestors(&self, name: &str, visiting: &mut HashSet<String>) -> Vec<String> {
        let ns = self.namespaces.get(name);
        // A name that the index does not know is a class only when it is
        // used as a superclass; then `walk` is called with `as_module` false.
        let as_module = ns.is_some_and(|ns| !ns.is_class);
        let mut out = Vec::new();
        self.walk(name, as_module, visiting, &mut out);
        // When a module shows up twice, Ruby keeps the later place: an
        // `include` of a module that is already in the chain does nothing.
        let mut seen = HashSet::new();
        let mut kept: Vec<String> = out.into_iter().rev().filter(|a| seen.insert(a.clone())).collect();
        kept.reverse();
        kept
    }

    fn walk(&self, name: &str, as_module: bool, visiting: &mut HashSet<String>, out: &mut Vec<String>) {
        if !visiting.insert(name.to_string()) {
            return;
        }
        // `T = Translate`: an include of `T` includes `Translate`.
        if !self.namespaces.contains_key(name)
            && let Some(target) = self.alias_target_lexical(name)
        {
            self.walk(&target, as_module, visiting, out);
            visiting.remove(name);
            return;
        }
        let ns = self.namespaces.get(name);
        if let Some(ns) = ns {
            for p in ns.prepends.iter().rev() {
                self.walk(p, true, visiting, out);
            }
        }
        out.push(name.to_string());
        if let Some(ns) = ns {
            for m in ns.includes.iter().rev() {
                self.walk(m, true, visiting, out);
            }
        }
        if !as_module {
            let sup = match (ns.and_then(|ns| ns.superclass.clone()), name) {
                (Some(s), _) => Some(s),
                (None, "BasicObject") => None,
                (None, "Object") => Some("BasicObject".to_string()),
                (None, _) => Some("Object".to_string()),
            };
            if name == "Object" {
                out.push("Kernel".to_string());
            }
            if let Some(sup) = sup {
                self.walk(&sup, false, visiting, out);
            }
        }
        visiting.remove(name);
    }

    /// Where to look for a class method of `name`, in order. Each item is
    /// an owner and whether to use its singleton (`def self.x`) methods.
    pub fn singleton_chain(&self, name: &str) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        let ancestors = self.ancestors(name);
        let is_class = self.namespaces.get(name).is_none_or(|ns| ns.is_class);
        let classes: Vec<&String> = if is_class {
            ancestors.iter().filter(|a| self.namespaces.get(*a).is_none_or(|ns| ns.is_class)).collect()
        } else {
            vec![&ancestors[0]]
        };
        for (ci, class) in classes.iter().enumerate() {
            out.push(((*class).clone(), true));
            if let Some(ns) = self.namespaces.get(*class) {
                for e in ns.extends.iter().rev() {
                    out.extend(self.ancestors(e).iter().map(|a| (a.clone(), false)));
                }
                if ns.extend_self {
                    out.push(((*class).clone(), false));
                }
            }
            // Modules included between this class and the next one in the
            // chain. Their `ClassMethods` (concerns, and the old
            // `base.extend(ClassMethods)` pattern) become class methods.
            let next = classes.get(ci + 1);
            let own = self.ancestors(class);
            for m in own.iter().take_while(|a| Some(a) != next) {
                let cm = format!("{m}::ClassMethods");
                if self.namespaces.contains_key(&cm) {
                    out.extend(self.ancestors(&cm).iter().map(|a| (a.clone(), false)));
                }
            }
        }
        for core in ["Class", "Module", "Object", "Kernel", "BasicObject"] {
            out.push((core.to_string(), false));
        }
        let mut seen = HashSet::new();
        out.retain(|a| seen.insert(a.clone()));
        out
    }

    /// Finds a method the way Ruby would call it on an instance of `owner`
    /// (or on `owner` itself when `singleton` is true).
    pub fn find_method(&self, owner: &str, singleton: bool, name: &str) -> Option<&[MethodId]> {
        let chain: Vec<(String, bool)> = if singleton {
            self.singleton_chain(owner)
        } else {
            self.ancestors(owner).iter().map(|a| (a.clone(), false)).collect()
        };
        for (o, sing) in chain {
            let Some(table) = self.owners.get(&o) else { continue };
            let side = if sing { &table.singleton } else { &table.instance };
            if let Some(ids) = side.get(name) {
                return Some(ids);
            }
        }
        None
    }
}

/// The full names of the lexical scopes around `scope`, innermost first.
/// `class_methods do` blocks and the root are skipped.
pub fn lexical_chain(raw: &RawFile, names: &[String], scope: u32) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = Some(scope);
    while let Some(i) = cur {
        let s = &raw.scopes[i as usize];
        if matches!(s.kind, ScopeKind::Class | ScopeKind::Module) {
            out.push(names[i as usize].clone());
        }
        cur = s.parent;
    }
    out
}

/// The full name of the innermost class or module around `scope` ("" at
/// the top level).
pub(crate) fn lexical_name<'a>(raw: &RawFile, names: &'a [String], scope: u32) -> &'a str {
    let mut cur = Some(scope);
    while let Some(i) = cur {
        let s = &raw.scopes[i as usize];
        if matches!(s.kind, ScopeKind::Class | ScopeKind::Module) {
            return &names[i as usize];
        }
        cur = s.parent;
    }
    ""
}

/// The owner of methods defined in `scope`: its full name, or `Object` at
/// the top level.
pub(crate) fn owner_name(names: &[String], scope: u32) -> String {
    let n = &names[scope as usize];
    if n.is_empty() { "Object".to_string() } else { n.clone() }
}

/// The full name that a definition of `path` gets inside the lexical
/// `chain`. `class Bar` inside `module A` defines `A::Bar`. For compact
/// paths like `class Foo::Bar`, the first segment is looked up in the
/// lexical scopes when `known` is given.
fn qualify(path: &ConstPath, chain: &[String], lexical: &str, known: Option<&HashSet<String>>) -> String {
    let joined = path.joined();
    if path.absolute {
        return joined;
    }
    if path.segments.len() == 1 || known.is_none() {
        return if lexical.is_empty() { joined } else { format!("{lexical}::{joined}") };
    }
    let known = known.unwrap();
    let first = &path.segments[0];
    let rest = path.segments[1..].join("::");
    let head =
        chain.iter().map(|c| format!("{c}::{first}")).find(|n| known.contains(n)).unwrap_or_else(|| first.clone());
    format!("{head}::{rest}")
}

/// The full name of every scope in a file.
fn scope_names(raw: &RawFile, known: Option<&HashSet<String>>) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(raw.scopes.len());
    for scope in &raw.scopes {
        let name = match scope.kind {
            ScopeKind::Root => String::new(),
            ScopeKind::ClassMethodsBlock => {
                let parent = scope.parent.unwrap_or(0);
                let outer = lexical_name(raw, &names, parent);
                if outer.is_empty() { "ClassMethods".to_string() } else { format!("{outer}::ClassMethods") }
            }
            ScopeKind::Class | ScopeKind::Module => {
                let parent = scope.parent.unwrap_or(0);
                let chain = lexical_chain(raw, &names, parent);
                let lexical = lexical_name(raw, &names, parent);
                qualify(&scope.path, &chain, lexical, known)
            }
            ScopeKind::EvalBlock => {
                // `Foo.class_eval` looks `Foo` up; it does not define it in
                // the lexical scope.
                let parent = scope.parent.unwrap_or(0);
                let chain = lexical_chain(raw, &names, parent);
                let first = &scope.path.segments[0];
                let head = match known {
                    Some(known) if !scope.path.absolute => chain
                        .iter()
                        .map(|c| format!("{c}::{first}"))
                        .find(|n| known.contains(n))
                        .unwrap_or_else(|| first.clone()),
                    _ => first.clone(),
                };
                std::iter::once(head).chain(scope.path.segments[1..].iter().cloned()).collect::<Vec<_>>().join("::")
            }
        };
        names.push(name);
    }
    names
}
