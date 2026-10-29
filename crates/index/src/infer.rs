//! Type inference: what class an expression's value has.
//!
//! The rules come in levels. Literals, `self` and constants are exact.
//! Finders, relations, associations, instance variables and return values
//! are inferred. A type from a variable's name (`invoice` → `Invoice`) is a
//! guess. The confidence travels with the type, so the window can show it.

use crate::index::{FileId, Index};
use crate::inflect;
use crate::model::*;

/// How sure a type (or a click target) is. Lower is surer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Found with Ruby's own lookup rules.
    Exact,
    /// Found through inferred types (finders, associations, return values).
    Inferred,
    /// Based on a variable's name only.
    Guess,
    /// A name match only. The list can hold wrong targets.
    NameOnly,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::Exact => "exact",
            Confidence::Inferred => "inferred",
            Confidence::Guess => "guess",
            Confidence::NameOnly => "name only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ty {
    /// The full class name.
    pub class: String,
    /// True for the class object itself (`Invoice`), false for an instance.
    pub singleton: bool,
    /// True for a collection of `class` (a relation, an association, an array).
    pub many: bool,
}

impl Ty {
    pub fn instance(class: impl Into<String>) -> Ty {
        Ty { class: class.into(), singleton: false, many: false }
    }

    fn many(class: impl Into<String>) -> Ty {
        Ty { class: class.into(), singleton: false, many: true }
    }

    /// `Invoice`, `Invoice (class)` or `[Invoice]`.
    pub fn describe(&self) -> String {
        if self.many {
            format!("[{}]", self.class)
        } else if self.singleton {
            format!("{} (class)", self.class)
        } else {
            self.class.clone()
        }
    }
}

/// Where an expression is evaluated.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    pub file: FileId,
    /// The lexical scope.
    pub scope: u32,
    pub self_ctx: SelfCtx,
    /// Where the expression is, for `let` lookups in spec files.
    pub offset: u32,
}

pub type Typed = (Ty, Confidence);

const MAX_DEPTH: u32 = 6;

/// Methods that return one record of a model.
const FINDERS: &[&str] = &[
    "find",
    "find_by",
    "find_by!",
    "first",
    "first!",
    "last",
    "last!",
    "take",
    "take!",
    "sole",
    "find_sole_by",
    "second",
    "third",
    "find_or_create_by",
    "find_or_create_by!",
    "find_or_initialize_by",
    "create",
    "create!",
    "new",
    "build",
    "create_or_find_by",
    "create_or_find_by!",
    "reload",
    "lock!",
];

/// Relation methods that return a relation of the same model.
const RELATION_METHODS: &[&str] = &[
    "where",
    "not",
    "or",
    "and",
    "order",
    "reorder",
    "in_order_of",
    "includes",
    "preload",
    "eager_load",
    "joins",
    "left_joins",
    "left_outer_joins",
    "references",
    "limit",
    "offset",
    "select",
    "distinct",
    "group",
    "having",
    "all",
    "none",
    "unscope",
    "unscoped",
    "rewhere",
    "readonly",
    "lock",
    "extending",
    "strict_loading",
    "excluding",
    "without",
    "merge",
    "reverse_order",
    "from",
    "with",
    "load",
    "reload",
    "to_a",
    "records",
    "sort_by",
    "filter",
    "reject",
    "compact",
    "uniq",
    "reverse",
    "sort",
];

/// Block methods whose first block parameter is one element.
const ITERATORS: &[&str] = &[
    "each",
    "map",
    "collect",
    "flat_map",
    "select",
    "filter",
    "filter_map",
    "reject",
    "find",
    "detect",
    "find_each",
    "each_with_index",
    "each_with_object",
    "sort_by",
    "min_by",
    "max_by",
    "sum",
    "group_by",
    "partition",
    "index_by",
    "count",
    "any?",
    "all?",
    "none?",
    "one?",
    "find_index",
    "each_slice",
    "in_batches",
    "take_while",
    "drop_while",
    "chunk_while",
    "uniq",
    "to_h",
];

/// Plain Ruby methods with a known result.
fn simple_result(name: &str, recv: &Ty) -> Option<Ty> {
    Some(match name {
        "to_s" | "inspect" | "to_str" | "humanize" | "titleize" | "downcase" | "upcase" | "strip" | "squish"
        | "parameterize" | "underscore" | "camelize" | "pluralize" | "singularize" => Ty::instance("String"),
        "to_i" | "to_int" | "count" | "size" | "length" => Ty::instance("Integer"),
        "to_f" => Ty::instance("Float"),
        "to_sym" => Ty::instance("Symbol"),
        "to_h" | "as_json" | "attributes" => Ty::instance("Hash"),
        "to_a" | "pluck" | "ids" | "keys" | "values" => Ty::instance("Array"),
        "to_d" => Ty::instance("BigDecimal"),
        "nil?" | "present?" | "blank?" | "empty?" | "any?" | "exists?" | "persisted?" | "new_record?" | "valid?"
        | "is_a?" | "kind_of?" | "include?" | "zero?" => Ty::instance("TrueClass"),
        "presence" | "dup" | "clone" | "freeze" | "tap" | "itself" => recv.clone(),
        _ => return None,
    })
}

fn worse(a: Confidence, b: Confidence) -> Confidence {
    a.max(b)
}

impl Index {
    /// The type of an expression.
    pub fn type_of(&self, expr: &Expr, ctx: &Ctx) -> Option<Typed> {
        self.type_depth(expr, ctx, 0)
    }

    fn type_depth(&self, expr: &Expr, ctx: &Ctx, depth: u32) -> Option<Typed> {
        if depth > MAX_DEPTH {
            return None;
        }
        let d = depth + 1;
        match expr {
            Expr::Unknown => None,
            Expr::SelfRef => {
                let owner = self.self_owner(ctx.file, ctx.scope);
                Some((
                    Ty { class: owner, singleton: ctx.self_ctx == SelfCtx::Singleton, many: false },
                    Confidence::Exact,
                ))
            }
            Expr::Lit(lit) => Some((Ty::instance(lit.class_name()), Confidence::Exact)),
            Expr::Const(path) => {
                let name = self.lookup_const(path, &self.chain(ctx.file, ctx.scope))?;
                let name = self.alias_target(&name).unwrap_or(name);
                if self.namespaces.contains_key(&name) {
                    Some((Ty { class: name, singleton: true, many: false }, Confidence::Exact))
                } else {
                    None
                }
            }
            Expr::Local { name, lscope, offset } => self.local_type(name, *lscope, *offset, ctx, d),
            Expr::Param { name } => self.guess_by_name(name, ctx),
            Expr::Ivar { name } => self.ivar_type(name, ctx, d),
            Expr::Call { recv, name } => {
                let recv_ty = match recv {
                    Some(r) => self.type_depth(r, ctx, d)?,
                    None => self.type_depth(&Expr::SelfRef, ctx, d)?,
                };
                // A call with no receiver can be a `let` in a spec file.
                if recv.is_none()
                    && let Some(t) = self.let_type(name, ctx, d)
                {
                    return Some(t);
                }
                match self.call_result(&recv_ty.0, name, d) {
                    Some((ty, conf)) => Some((ty, worse(conf, recv_ty.1))),
                    // `blob` in a partial, `invoice` from a helper: a call
                    // with no receiver and no known method.
                    None if recv.is_none()
                        && self.find_method(&recv_ty.0.class, recv_ty.0.singleton, name).is_none() =>
                    {
                        self.guess_by_name(name, ctx)
                    }
                    None => None,
                }
            }
            Expr::BlockParam { recv, method, index } => {
                let (recv_ty, conf) = self.type_depth(recv, ctx, d)?;
                match method.as_str() {
                    "tap" | "then" | "yield_self" if *index == 0 => Some((recv_ty, conf)),
                    m if ITERATORS.contains(&m) && *index == 0 && recv_ty.many => {
                        Some((Ty::instance(recv_ty.class), worse(conf, Confidence::Inferred)))
                    }
                    "find_in_batches" | "in_batches" if recv_ty.many => {
                        Some((Ty::many(recv_ty.class), worse(conf, Confidence::Inferred)))
                    }
                    _ => None,
                }
            }
            Expr::Factory { name, many } => {
                let class = self.factory_class(name)?;
                Some((Ty { class, singleton: false, many: *many }, Confidence::Inferred))
            }
            Expr::Union(parts) => parts
                .iter()
                .filter_map(|p| self.type_depth(p, ctx, d))
                .find(|(t, _)| t.class != "NilClass")
                .or_else(|| parts.iter().find_map(|p| self.type_depth(p, ctx, d))),
        }
    }

    /// The class a factory builds: its `class:` option, its parent
    /// factory's class, or the class named like the factory.
    pub fn factory_class(&self, factory: &str) -> Option<String> {
        let mut current = factory.to_string();
        for _ in 0..10 {
            let info = self.factories.get(&current).and_then(|l| l.first());
            if let Some(cn) = info.and_then(|i| i.class_name.clone()) {
                return ConstPath::parse(&cn).and_then(|p| self.lookup_const(&p, &[]));
            }
            match info.and_then(|i| i.parent.clone()) {
                Some(parent) => current = parent,
                None => break,
            }
        }
        let path = ConstPath::parse(&inflect::camelize(&current))?;
        self.lookup_const(&path, &[])
    }

    /// The type of a local variable at `offset`: its last assignment before
    /// that point.
    fn local_type(&self, name: &str, lscope: u32, offset: u32, ctx: &Ctx, depth: u32) -> Option<Typed> {
        let f = &self.files[ctx.file as usize];
        let assign = f.locals().iter().rfind(|l| l.lscope == lscope && l.name == name && l.name_span.start < offset);
        match assign {
            Some(a) => {
                let local_ctx = Ctx { offset: a.name_span.start, ..*ctx };
                self.type_depth(&a.value, &local_ctx, depth)
            }
            None => self.let_type(name, &Ctx { offset, ..*ctx }, depth),
        }
    }

    /// The type of a `let` (or `subject`) visible at `ctx.offset`.
    fn let_type(&self, name: &str, ctx: &Ctx, depth: u32) -> Option<Typed> {
        let l = self.let_at(ctx.file, ctx.offset, name)?;
        let f = &self.files[ctx.file as usize];
        let def = &f.raw.lets[l];
        let let_ctx = Ctx { offset: def.span.start, ..*ctx };
        self.type_depth(&def.value, &let_ctx, depth).or_else(|| self.guess_by_name(name, ctx))
    }

    /// The `let` named `name` that code at `offset` sees: the one in the
    /// innermost enclosing group.
    pub fn let_at(&self, file: FileId, offset: u32, name: &str) -> Option<usize> {
        let f = &self.files[file as usize];
        if f.raw.lets.is_empty() {
            return None;
        }
        let blocks = &f.raw.spec_blocks;
        let mut best: Option<(usize, u32)> = None;
        for (i, l) in f.raw.lets.iter().enumerate() {
            if l.name != name {
                continue;
            }
            let (visible, size) = match l.block {
                None => (true, u32::MAX),
                Some(b) => {
                    let span = blocks[b as usize].span;
                    (span.contains(offset), span.len())
                }
            };
            if visible && best.is_none_or(|(_, s)| size <= s) {
                best = Some((i, size));
            }
        }
        best.map(|(i, _)| i)
    }

    /// The type of an instance variable: the first of its assignments in
    /// the class (in any file) that has a known type.
    fn ivar_type(&self, name: &str, ctx: &Ctx, depth: u32) -> Option<Typed> {
        let owner = self.self_owner(ctx.file, ctx.scope);
        let mut owners = vec![owner.clone()];
        owners.extend(self.ancestors(&owner).iter().skip(1).take(6).cloned());
        for o in owners {
            let Some(list) = self.ivar_index.get(&(o, name.to_string())) else { continue };
            for (file, i) in list {
                let iv = &self.files[*file as usize].raw.ivars[*i as usize];
                let self_ctx = if iv.singleton { SelfCtx::Singleton } else { SelfCtx::Instance };
                let ivar_ctx = Ctx { file: *file, scope: iv.scope, self_ctx, offset: iv.name_span.start };
                if let Some((t, c)) = self.type_depth(&iv.value, &ivar_ctx, depth) {
                    return Some((t, worse(c, Confidence::Inferred)));
                }
            }
        }
        self.guess_by_name(name.trim_start_matches('@'), ctx)
    }

    /// A type from a variable name: `invoice` → `Invoice`, `invoices` →
    /// `[Invoice]`, `current_user` → `User`.
    pub fn guess_by_name(&self, name: &str, ctx: &Ctx) -> Option<Typed> {
        let base = name.trim_start_matches('@').trim_start_matches('_').trim_end_matches(['?', '!']);
        let base = base.strip_prefix("current_").unwrap_or(base);
        if base.is_empty() || base.ends_with("_id") || base.ends_with("_ids") {
            return None;
        }
        let chain = self.chain(ctx.file, ctx.scope);
        let singular = inflect::singularize(base);
        let many = singular != base;
        let mut candidates = vec![singular.clone()];
        // `order_item` → `OrderItem`, then `Item`.
        if let Some((_, last)) = singular.split_once('_') {
            candidates.push(last.to_string());
        }
        for c in candidates {
            let path = ConstPath::parse(&inflect::camelize(&c))?;
            if let Some(class) = self.lookup_const(&path, &chain)
                && self.namespaces.get(&class).is_some_and(|ns| ns.is_class)
            {
                let ty = Ty { class, singleton: false, many };
                return Some((ty, Confidence::Guess));
            }
        }
        None
    }

    /// The type of `recv.name`.
    fn call_result(&self, recv: &Ty, name: &str, depth: u32) -> Option<Typed> {
        let model_like = |class: &str| self.is_model(class);
        if recv.many {
            if FINDERS.contains(&name) && !matches!(name, "reload") {
                return Some((Ty::instance(&recv.class), Confidence::Inferred));
            }
            if RELATION_METHODS.contains(&name) {
                return Some((recv.clone(), Confidence::Inferred));
            }
            // A scope or class method called on a relation.
            if let Some(ids) = self.find_method(&recv.class, true, name) {
                let m = &self.methods[ids[0] as usize];
                if m.kind == MethodKind::Scope {
                    return Some((recv.clone(), Confidence::Inferred));
                }
                return self.method_return(ids[0], &Ty { singleton: true, many: false, ..recv.clone() }, depth);
            }
            return simple_result(name, recv).map(|t| (t, Confidence::Inferred));
        }
        if recv.singleton {
            if name == "new" || (FINDERS.contains(&name) && model_like(&recv.class)) {
                return Some((Ty::instance(&recv.class), Confidence::Inferred));
            }
            if RELATION_METHODS.contains(&name) && model_like(&recv.class) {
                return Some((Ty::many(&recv.class), Confidence::Inferred));
            }
            if let Some(ids) = self.find_method(&recv.class, true, name) {
                let m = &self.methods[ids[0] as usize];
                if m.kind == MethodKind::Scope {
                    return Some((Ty::many(&recv.class), Confidence::Inferred));
                }
                return self.method_return(ids[0], recv, depth);
            }
            return None;
        }
        // An instance.
        if name == "class" {
            return Some((Ty { class: recv.class.clone(), singleton: true, many: false }, Confidence::Exact));
        }
        if let Some(assoc) = self.association(&recv.class, name) {
            let class = self.association_class(&recv.class, assoc)?;
            let ty = if assoc.kind.is_collection() { Ty::many(class) } else { Ty::instance(class) };
            return Some((ty, Confidence::Inferred));
        }
        if let Some(ids) = self.find_method(&recv.class, false, name)
            && let Some(t) = self.method_return(ids[0], recv, depth)
        {
            return Some(t);
        }
        if matches!(name, "reload" | "lock!") && model_like(&recv.class) {
            return Some((recv.clone(), Confidence::Inferred));
        }
        simple_result(name, recv).map(|t| (t, Confidence::Inferred))
    }

    /// What a method returns when called on `recv`.
    pub fn method_return(&self, id: u32, recv: &Ty, depth: u32) -> Option<Typed> {
        let m = &self.methods[id as usize];
        if let Some(vc) = &m.value_class {
            return Some((Ty::instance(vc), Confidence::Inferred));
        }
        // YARD: `# @return [Invoice]` or `# @return [Array<Invoice>]`.
        if let Some(doc) = &m.doc
            && let Some(t) = self.yard_return(doc, m.loc.file, m.scope)
        {
            return Some((t, Confidence::Inferred));
        }
        if m.kind == MethodKind::Delegate
            && let (Some(to), Some(orig)) = (&m.delegate_to, &m.alias_of)
        {
            let ctx = Ctx { file: m.loc.file, scope: m.scope, self_ctx: SelfCtx::Instance, offset: m.loc.span.start };
            let target = if to.starts_with('@') {
                Expr::Ivar { name: to.clone() }
            } else {
                Expr::Call { recv: None, name: to.clone() }
            };
            let (t, c) = self.type_depth(&target, &ctx, depth + 1)?;
            let (r, c2) = self.call_result(&t, orig, depth + 1)?;
            return Some((r, worse(c, c2)));
        }
        let returns = m.returns.as_ref()?;
        let self_ctx = if recv.singleton { SelfCtx::Singleton } else { SelfCtx::Instance };
        let ctx = Ctx { file: m.loc.file, scope: m.scope, self_ctx, offset: m.loc.span.end };
        let (t, c) = self.type_depth(returns, &ctx, depth + 1)?;
        // `self` inside the method means the receiver's class, which can be
        // a subclass of the method's owner.
        let t = if t.class == m.owner && !t.many { Ty { class: recv.class.clone(), ..t } } else { t };
        Some((t, worse(c, Confidence::Inferred)))
    }

    fn yard_return(&self, doc: &str, file: FileId, scope: u32) -> Option<Ty> {
        let line = doc.lines().find(|l| l.trim_start().starts_with("@return"))?;
        let inner = line.split_once('[')?.1.split_once(']')?.0;
        let first = inner.split(',').next()?.trim();
        let (many, class) = match first.strip_prefix("Array<").and_then(|r| r.strip_suffix('>')) {
            Some(c) => (true, c),
            None => match first.strip_prefix("ActiveRecord::Relation<").and_then(|r| r.strip_suffix('>')) {
                Some(c) => (true, c),
                None => (false, first),
            },
        };
        let path = ConstPath::parse(class)?;
        let name = self.lookup_const(&path, &self.chain(file, scope))?;
        Some(Ty { class: name, singleton: false, many })
    }
}
