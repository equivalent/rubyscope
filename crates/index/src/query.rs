//! Questions the window asks the index: usages, the outline of a file,
//! the breadcrumb, hover info, the type hierarchy, text search and the
//! symbol list for "go to symbol".

use rayon::prelude::*;

use crate::index::{BLOCK_OWNER, ConstKind, FileId, Index, Loc};
use crate::infer::Confidence;
use crate::model::*;

/// Something that has definitions and usages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Symbol {
    /// A method: every definition with this owner, name and side.
    Method {
        owner: String,
        name: String,
        singleton: bool,
    },
    Const {
        name: String,
    },
}

impl Symbol {
    pub fn label(&self) -> String {
        match self {
            Symbol::Method { owner, name, singleton } => {
                format!("{owner}{}{name}", if *singleton { "." } else { "#" })
            }
            Symbol::Const { name } => name.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub file: FileId,
    pub span: Span,
    pub confidence: Confidence,
    /// The method or class the usage is in.
    pub context: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineKind {
    Class,
    Module,
    Method,
    SingletonMethod,
    Constant,
    Generated,
}

#[derive(Debug, Clone)]
pub struct OutlineItem {
    pub label: String,
    pub kind: OutlineKind,
    pub span: Span,
    pub name_span: Span,
    pub depth: u32,
}

#[derive(Debug, Clone)]
pub struct Hover {
    /// `Invoice#total`, `Invoice`, `en.admin.title`
    pub title: String,
    /// `def total(currency: nil)`, or the kind of target.
    pub signature: Option<String>,
    pub doc: Option<String>,
    /// `receiver: Invoice (inferred)`
    pub receiver: Option<String>,
    pub location: String,
    pub confidence: Confidence,
    /// More targets than the first one.
    pub more: usize,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub rel: String,
    /// 0-based line.
    pub line: u32,
    /// Byte columns of the match in the line.
    pub start: u32,
    pub end: u32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct SymbolEntry {
    pub label: String,
    pub detail: &'static str,
    pub loc: Loc,
    pub project: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Hierarchy {
    pub ancestors: Vec<String>,
    /// Direct subclasses, or the classes that include a module.
    pub descendants: Vec<String>,
}

impl Index {
    /// The symbol at a position: what a click there goes to, or the
    /// definition the cursor is on.
    pub fn symbol_at(&self, file: FileId, offset: u32) -> Option<Symbol> {
        let res = self.definition_at(file, offset)?;
        let t = res.targets.first()?;
        self.symbol_of_loc(t.loc).or_else(|| {
            // A constant target.
            self.consts.contains_key(&t.label).then(|| Symbol::Const { name: t.label.clone() })
        })
    }

    fn symbol_of_loc(&self, loc: Loc) -> Option<Symbol> {
        let m = self.methods.iter().find(|m| m.loc == loc)?;
        Some(Symbol::Method { owner: m.owner.clone(), name: m.name.clone(), singleton: m.singleton })
    }

    /// The definitions of a symbol.
    pub fn symbol_defs(&self, sym: &Symbol) -> Vec<Loc> {
        match sym {
            Symbol::Method { owner, name, singleton } => self
                .owners
                .get(owner)
                .and_then(|o| if *singleton { o.singleton.get(name) } else { o.instance.get(name) })
                .map(|ids| ids.iter().map(|id| self.methods[*id as usize].loc).collect())
                .unwrap_or_default(),
            Symbol::Const { name } => {
                self.consts.get(name).map(|d| d.iter().map(|d| d.loc).collect()).unwrap_or_default()
            }
        }
    }

    /// Every place that uses a symbol. Each candidate reference with the
    /// same name is resolved, and kept when it goes to the symbol.
    pub fn usages(&self, sym: &Symbol) -> Vec<Usage> {
        let defs = self.symbol_defs(sym);
        let (name, is_const) = match sym {
            Symbol::Method { name, .. } => (name.clone(), false),
            Symbol::Const { name } => (name.rsplit("::").next().unwrap_or(name).to_string(), true),
        };
        let Some(candidates) = self.refs_by_name().get(&name) else { return Vec::new() };
        let mut out: Vec<Usage> = candidates
            .par_iter()
            .filter_map(|(file, i)| {
                let r = &self.files[*file as usize].refs()[*i as usize];
                let conf = match (&r.kind, sym) {
                    (RefKind::Const(path), Symbol::Const { name: full }) => {
                        let chain = self.chain(*file, r.scope);
                        (self.lookup_const(path, &chain).as_deref() == Some(full.as_str()))
                            .then_some(Confidence::Exact)?
                    }
                    _ if is_const && !matches!(r.kind, RefKind::Call { .. }) => return None,
                    _ => {
                        let (targets, _) = self.resolve_ref(*file, r);
                        let hit = targets.iter().find(|t| defs.contains(&t.loc))?;
                        hit.confidence
                    }
                };
                Some(Usage {
                    file: *file,
                    span: r.span,
                    confidence: conf,
                    context: self.context_at(*file, r.span.start),
                })
            })
            .collect();
        out.sort_by_key(|u| (u.confidence, !self.origin(u.file).is_project(), u.file, u.span.start));
        out
    }

    /// The method that an offset is inside of, as a symbol.
    pub fn method_symbol_at(&self, file: FileId, offset: u32) -> Option<Symbol> {
        let f = &self.files[file as usize];
        let m = f
            .raw
            .methods
            .iter()
            .filter(|m| m.span.contains(offset) && m.kind == MethodKind::Def)
            .min_by_key(|m| m.span.len())?;
        let owner = self.self_owner(file, m.scope);
        let owner = if owner == "Object" && m.in_block { BLOCK_OWNER.to_string() } else { owner };
        Some(Symbol::Method { owner, name: m.name.clone(), singleton: m.singleton })
    }

    /// The innermost method (or class) around an offset, as a label.
    pub fn context_at(&self, file: FileId, offset: u32) -> String {
        let f = &self.files[file as usize];
        let m = f.raw.methods.iter().filter(|m| m.span.contains(offset)).min_by_key(|m| m.span.len());
        if let Some(m) = m {
            let owner = self.self_owner(file, m.scope);
            let owner = if owner == BLOCK_OWNER { String::new() } else { owner };
            return format!("{owner}{}{}", if m.singleton { "." } else { "#" }, m.name);
        }
        let s = f
            .raw
            .scopes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind != ScopeKind::Root && s.span.contains(offset))
            .min_by_key(|(_, s)| s.span.len());
        match s {
            Some((i, _)) => f.scope_names[i].clone(),
            None => self.rel_path(file).unwrap_or_default(),
        }
    }

    /// The classes, modules, methods and constants of a file, in order.
    pub fn outline(&self, file: FileId) -> Vec<OutlineItem> {
        let f = &self.files[file as usize];
        let mut items = Vec::new();
        let depth_of = |span: Span| -> u32 {
            f.raw
                .scopes
                .iter()
                .filter(|s| {
                    matches!(s.kind, ScopeKind::Class | ScopeKind::Module) && s.span.covers(span) && s.span != span
                })
                .count() as u32
        };
        for (i, s) in f.raw.scopes.iter().enumerate() {
            let kind = match s.kind {
                ScopeKind::Class => OutlineKind::Class,
                ScopeKind::Module => OutlineKind::Module,
                _ => continue,
            };
            items.push(OutlineItem {
                label: f.scope_names[i].clone(),
                kind,
                span: s.span,
                name_span: s.name_span,
                depth: depth_of(s.span),
            });
        }
        let mut seen = std::collections::HashSet::new();
        for m in &f.raw.methods {
            // `module_function` copies share a place with the method, and
            // one DSL call (`belongs_to :entity`) makes many methods: show
            // each place once.
            let key = if m.kind == MethodKind::Def {
                (m.name_span.start, m.name.clone())
            } else {
                (m.name_span.start, String::new())
            };
            if !seen.insert(key) {
                continue;
            }
            let kind = match (m.kind, m.singleton) {
                (MethodKind::Def, false) => OutlineKind::Method,
                (MethodKind::Def, true) => OutlineKind::SingletonMethod,
                _ => OutlineKind::Generated,
            };
            let label =
                if m.singleton { format!("self.{}{}", m.name, m.params) } else { format!("{}{}", m.name, m.params) };
            items.push(OutlineItem { label, kind, span: m.span, name_span: m.name_span, depth: depth_of(m.span) });
        }
        for w in &f.raw.const_writes {
            items.push(OutlineItem {
                label: w.path.joined(),
                kind: OutlineKind::Constant,
                span: w.span,
                name_span: w.name_span,
                depth: depth_of(w.span),
            });
        }
        items.sort_by_key(|i| (i.span.start, std::cmp::Reverse(i.span.end)));
        items
    }

    /// The classes and the method around an offset, outermost first, each
    /// with the place to go to when it is clicked.
    pub fn breadcrumb(&self, file: FileId, offset: u32) -> Vec<(String, Loc)> {
        let f = &self.files[file as usize];
        let mut out = Vec::new();
        let mut scopes: Vec<(usize, &RawScope)> = f
            .raw
            .scopes
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.kind, ScopeKind::Class | ScopeKind::Module) && s.span.contains(offset))
            .collect();
        scopes.sort_by_key(|(_, s)| s.span.start);
        if let Some((i, s)) = scopes.last() {
            out.push((f.scope_names[*i].clone(), Loc { file, span: s.span, name_span: s.name_span }));
        }
        if let Some(m) = f.raw.methods.iter().filter(|m| m.span.contains(offset)).min_by_key(|m| m.span.len()) {
            let label = format!("{}{}", if m.singleton { "." } else { "#" }, m.name);
            out.push((label, Loc { file, span: m.span, name_span: m.name_span }));
        }
        out
    }

    /// What to show when the pointer rests on a symbol.
    pub fn hover(&self, file: FileId, offset: u32) -> Option<Hover> {
        let res = self.definition_at(file, offset)?;
        let t = res.targets.first()?;
        let line = self.files[t.loc.file as usize].raw.line_of(t.loc.span.start) + 1;
        let location = format!("{}:{line}", self.stable_path(t.loc.file));
        let receiver = res.receiver_type.as_ref().map(|(ty, c)| format!("receiver: {ty} ({})", c.label()));
        let method = self.methods.iter().find(|m| m.loc == t.loc);
        let (signature, doc) = match method {
            Some(m) => {
                let sig = match m.kind {
                    MethodKind::Def => format!("def {}{}{}", if m.singleton { "self." } else { "" }, m.name, m.params),
                    _ => format!("{} ({})", m.name, t.detail.clone().unwrap_or_default()),
                };
                (Some(sig), m.doc.clone())
            }
            None => {
                let doc = self.consts.get(&t.label).and_then(|defs| {
                    defs.iter().find_map(|d| {
                        let f = &self.files[d.loc.file as usize];
                        f.raw.scopes.iter().find(|s| s.span == d.loc.span).and_then(|s| s.doc.clone())
                    })
                });
                let sig = self.consts.get(&t.label).and_then(|defs| defs.first()).map(|d| {
                    let kind = match d.kind {
                        ConstKind::Class => "class",
                        ConstKind::Module => "module",
                        ConstKind::Value => "constant",
                    };
                    match (d.kind, self.namespaces.get(&t.label).and_then(|n| n.superclass.clone())) {
                        (ConstKind::Class, Some(sup)) => format!("class {} < {sup}", t.label),
                        _ => format!("{kind} {}", t.label),
                    }
                });
                (sig.or_else(|| t.detail.clone()), doc)
            }
        };
        Some(Hover {
            title: t.label.clone(),
            signature,
            doc,
            receiver,
            location,
            confidence: t.confidence,
            more: res.targets.len().saturating_sub(1),
        })
    }

    pub fn hierarchy(&self, name: &str) -> Hierarchy {
        let is_module = self.namespaces.get(name).is_some_and(|ns| !ns.is_class);
        let mut descendants = self.subclasses.get(name).cloned().unwrap_or_default();
        if is_module {
            descendants.extend(self.includers.get(name).cloned().unwrap_or_default());
        }
        descendants.retain(|d| !d.starts_with('('));
        descendants.sort();
        descendants.dedup();
        Hierarchy { ancestors: self.ancestors(name).iter().cloned().collect(), descendants }
    }

    /// Searches the text of every project file.
    pub fn search(
        &self,
        pattern: &str,
        regex: bool,
        case_sensitive: bool,
        limit: usize,
    ) -> Result<Vec<SearchHit>, String> {
        let source = if regex { pattern.to_string() } else { regex::escape(pattern) };
        let re =
            regex::RegexBuilder::new(&source).case_insensitive(!case_sensitive).build().map_err(|e| e.to_string())?;
        let mut hits: Vec<SearchHit> = self
            .all_files
            .par_iter()
            .filter_map(|rel| {
                let text = self.read_rel(rel)?;
                if text.len() > 2_000_000 || text.as_bytes().contains(&0) {
                    return None;
                }
                let mut found = Vec::new();
                for (line, l) in text.lines().enumerate() {
                    for m in re.find_iter(l) {
                        found.push(SearchHit {
                            rel: rel.clone(),
                            line: line as u32,
                            start: m.start() as u32,
                            end: m.end() as u32,
                            text: l.chars().take(300).collect(),
                        });
                        if found.len() >= 200 {
                            break;
                        }
                    }
                }
                Some(found)
            })
            .flatten()
            .collect();
        hits.sort_by(|a, b| (&a.rel, a.line).cmp(&(&b.rel, b.line)));
        hits.truncate(limit);
        Ok(hits)
    }

    /// Every class, module, constant and method, for "go to symbol".
    pub fn symbol_entries(&self) -> Vec<SymbolEntry> {
        let mut out = Vec::with_capacity(self.consts.len() + self.methods.len());
        for (name, defs) in &self.consts {
            if let Some(d) = defs.first() {
                let detail = match d.kind {
                    ConstKind::Class => "class",
                    ConstKind::Module => "module",
                    ConstKind::Value => "constant",
                };
                out.push(SymbolEntry {
                    label: name.clone(),
                    detail,
                    loc: d.loc,
                    project: self.origin(d.loc.file).is_project(),
                });
            }
        }
        for m in &self.methods {
            if m.owner.starts_with('(') {
                continue;
            }
            out.push(SymbolEntry {
                label: m.label(),
                detail: if m.kind == MethodKind::Def { "method" } else { "generated" },
                loc: m.loc,
                project: self.origin(m.loc.file).is_project(),
            });
        }
        out
    }
}
