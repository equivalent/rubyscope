//! Turns a position in a file into the definitions it points to.

use std::collections::HashSet;

use crate::index::{BLOCK_OWNER, ConstKind, FileId, Index, Loc, MethodId};
pub use crate::infer::Confidence;
use crate::infer::{Ctx, Ty};
use crate::inflect;
use crate::link::VIEW_OWNER;
use crate::model::*;
use crate::query::Symbol;
use crate::rails;

#[derive(Debug, Clone)]
pub struct Target {
    pub loc: Loc,
    pub label: String,
    pub confidence: Confidence,
    /// More about the target: the method kind, the locale, the route.
    pub detail: Option<String>,
}

/// What is under the cursor, and what it resolves to.
#[derive(Debug, Clone)]
pub struct Resolution {
    /// The clicked text.
    pub span: Span,
    pub targets: Vec<Target>,
    /// The receiver's type, when a type was worked out (`Invoice`, guess).
    pub receiver_type: Option<(String, Confidence)>,
}

/// The most targets a name-only list returns.
const NAME_ONLY_LIMIT: usize = 50;

/// `Button`, not `button`: a method with this name can be a Phlex kit call.
fn is_const_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// Rails URL helper suffixes.
fn route_helper_base(name: &str) -> Option<&str> {
    name.strip_suffix("_path").or_else(|| name.strip_suffix("_url"))
}

impl Index {
    /// The reference at an offset: the smallest one that contains it.
    pub fn ref_at(&self, file: FileId, offset: u32) -> Option<&RawRef> {
        let refs = self.files[file as usize].refs();
        let end = refs.partition_point(|r| r.span.start <= offset);
        refs[..end].iter().filter(|r| r.span.contains(offset)).min_by_key(|r| r.span.len())
    }

    /// The span of anything clickable at an offset (for underlining).
    pub fn clickable_at(&self, file: FileId, offset: u32) -> Option<Span> {
        if let Some(link) = self.route_links.get(&file).and_then(|l| l.iter().find(|l| l.span.contains(offset))) {
            return Some(link.span);
        }
        if let Some(r) = self.ref_at(file, offset) {
            return Some(r.span);
        }
        if let Some(name) = self.const_write_name_at(file, offset) {
            return Some(name);
        }
        self.methods.iter().find(|m| m.loc.file == file && m.loc.name_span.contains(offset)).map(|m| m.loc.name_span)
    }

    pub fn definition_at(&self, file: FileId, offset: u32) -> Option<Resolution> {
        // Links inside routes files.
        if let Some(link) = self.route_links.get(&file).and_then(|l| l.iter().find(|l| l.span.contains(offset))) {
            let class = rails::controller_class(&link.controller);
            let targets = match &link.action {
                Some(action) => self
                    .methods_for(&class, false, action)
                    .unwrap_or_else(|| self.const_targets(&class, Confidence::Exact)),
                None => self.const_targets(&class, Confidence::Exact),
            };
            return Some(Resolution { span: link.span, targets, receiver_type: None });
        }
        if let Some(r) = self.ref_at(file, offset) {
            let (targets, receiver_type) = self.resolve_ref(file, r);
            return Some(Resolution { span: r.span, targets, receiver_type });
        }
        // A click on the name of a method definition shows the method itself,
        // or for a controller action, its views.
        let m = self.methods.iter().position(|m| m.loc.file == file && m.loc.name_span.contains(offset))?;
        let def = &self.methods[m];
        let views = if def.singleton { Vec::new() } else { self.action_views(&def.owner, &def.name) };
        let targets = if views.is_empty() { vec![self.method_target(m as MethodId, Confidence::Exact)] } else { views };
        Some(Resolution { span: def.loc.name_span, targets, receiver_type: None })
    }

    /// The method that a definition names at an offset: `pay!` in
    /// `def pay!`, with the views of a controller action. `None` when the
    /// offset is on a reference, as `:entity` in `belongs_to :entity` is.
    pub fn method_declaration_at(&self, file: FileId, offset: u32) -> Option<(Symbol, Vec<Target>)> {
        if self.route_links.get(&file).is_some_and(|l| l.iter().any(|l| l.span.contains(offset))) {
            return None;
        }
        if self.ref_at(file, offset).is_some() {
            return None;
        }
        let def = self.methods.iter().find(|m| m.loc.file == file && m.loc.name_span.contains(offset))?;
        let views = if def.singleton { Vec::new() } else { self.action_views(&def.owner, &def.name) };
        let sym = Symbol::Method { owner: def.owner.clone(), name: def.name.clone(), singleton: def.singleton };
        Some((sym, views))
    }

    fn ctx(&self, file: FileId, r: &RawRef) -> Ctx {
        Ctx { file, scope: r.scope, self_ctx: r.self_ctx, offset: r.span.start }
    }

    pub fn resolve_ref(&self, file: FileId, r: &RawRef) -> (Vec<Target>, Option<(String, Confidence)>) {
        let chain = self.chain(file, r.scope);
        let owner = self.self_owner(file, r.scope);
        let targets = match &r.kind {
            RefKind::Const(path) => match self.lookup_const(path, &chain) {
                Some(name) => self.const_targets(&name, Confidence::Exact),
                None => self.consts_named(path.segments.last().map(String::as_str).unwrap_or("")),
            },
            RefKind::Call { name, receiver } => return self.resolve_call(file, r, name, receiver),
            RefKind::Super { method } => {
                let m = &self.files[file as usize].raw.methods[*method as usize];
                self.super_targets(&self.self_owner(file, m.scope), m.singleton, &m.name)
            }
            RefKind::Local { name, lscope } => {
                let f = &self.files[file as usize];
                let assign = f
                    .locals()
                    .iter()
                    .rfind(|l| l.lscope == *lscope && &l.name == name && l.name_span.start <= r.span.start);
                match assign {
                    Some(a) => vec![Target {
                        loc: Loc { file, span: a.name_span, name_span: a.name_span },
                        label: name.clone(),
                        confidence: Confidence::Exact,
                        detail: Some("local variable".into()),
                    }],
                    None => self.let_targets(file, r.span.start, name),
                }
            }
            RefKind::Ivar { name } => self.ivar_targets(&owner, name),
            RefKind::MethodSymbol { name, singleton } => self.methods_for(&owner, *singleton, name).unwrap_or_default(),
            RefKind::AssocClass { assoc } => {
                let a = &self.files[file as usize].raw.associations[*assoc as usize];
                let assoc_owner = self.self_owner(file, a.scope);
                self.association(&assoc_owner, &a.name)
                    .and_then(|assoc| self.association_class(&assoc_owner, assoc))
                    .map(|c| self.const_targets(&c, Confidence::Exact))
                    .unwrap_or_default()
            }
            RefKind::ClassName { name } => ConstPath::parse(name)
                .and_then(|p| self.lookup_const(&p, &chain))
                .map(|c| self.const_targets(&c, Confidence::Exact))
                .unwrap_or_default(),
            RefKind::I18n { key, method } => self.i18n_targets(file, r.scope, key, method.as_deref()),
            RefKind::Render { path } => self.render_targets(file, &owner, path),
            RefKind::Factory { name } => self.factory_targets(name),
            RefKind::Trait { factory, name } => self.trait_targets(factory, name),
            RefKind::SharedGroup { name } => self
                .shared_groups
                .get(name)
                .map(|locs| locs.iter().map(|l| self.simple_target(*l, name, "shared examples")).collect())
                .unwrap_or_default(),
            RefKind::Stimulus { controller, action } => self.stimulus_targets(controller, action.as_deref()),
        };
        (targets, None)
    }

    fn resolve_call(
        &self,
        file: FileId,
        r: &RawRef,
        name: &str,
        receiver: &Receiver,
    ) -> (Vec<Target>, Option<(String, Confidence)>) {
        let chain = self.chain(file, r.scope);
        let owner = self.self_owner(file, r.scope);
        let singleton = r.self_ctx == SelfCtx::Singleton;
        let mut receiver_type = None;
        let found: Option<Vec<Target>> = match receiver {
            Receiver::Implicit | Receiver::SelfRef => {
                let implicit = matches!(receiver, Receiver::Implicit);
                let spec =
                    (implicit && self.is_spec_file(file)).then(|| self.spec_call_targets(file, r, name)).flatten();
                spec.or_else(|| self.methods_for(&owner, singleton, name))
                    .or_else(|| self.kit_targets(&owner, &chain, name))
                    .or_else(|| self.route_targets(name))
            }
            Receiver::Const(path) => match self.lookup_const(path, &chain) {
                Some(class) => {
                    let class = self.alias_target(&class).unwrap_or(class);
                    receiver_type = Some((format!("{class} (class)"), Confidence::Exact));
                    self.class_method_targets(&class, name, Confidence::Exact)
                        .or_else(|| self.kit_member_targets(&class, name))
                }
                None => None,
            },
            Receiver::Expr(expr) => {
                // `main_app.root_path`, `helpers.x_path`, `url_helpers.x_url`
                let route = route_helper_base(name).and_then(|_| self.route_targets(name));
                match self.type_of(expr, &self.ctx(file, r)) {
                    Some((ty, conf)) => {
                        receiver_type = Some((ty.describe(), conf));
                        self.typed_targets(&ty, name, conf).or(route)
                    }
                    None => route,
                }
            }
        };
        let targets = found.filter(|t| !t.is_empty()).unwrap_or_else(|| {
            let named = self.methods_named(name);
            if named.is_empty() { self.kit_consts_named(name) } else { named }
        });
        (targets, receiver_type)
    }

    /// Class methods of `class`, and `new` → `initialize`. Model classes
    /// also answer the relation methods (`where`, `order`...).
    fn class_method_targets(&self, class: &str, name: &str, conf: Confidence) -> Option<Vec<Target>> {
        if name == "new" {
            return self
                .methods_for_conf(class, false, "initialize", conf)
                .or_else(|| Some(self.const_targets(class, conf)));
        }
        self.methods_for_conf(class, true, name, conf).or_else(|| {
            self.is_model(class).then(|| self.methods_for_conf("ActiveRecord::Relation", false, name, conf)).flatten()
        })
    }

    /// Methods for a call on a value of type `ty`.
    fn typed_targets(&self, ty: &Ty, name: &str, conf: Confidence) -> Option<Vec<Target>> {
        if ty.singleton {
            return self.class_method_targets(&ty.class, name, conf);
        }
        if ty.many {
            if self.is_model(&ty.class) {
                return self
                    .methods_for_conf(&ty.class, true, name, conf)
                    .or_else(|| self.methods_for_conf("ActiveRecord::Associations::CollectionProxy", false, name, conf))
                    .or_else(|| self.methods_for_conf("ActiveRecord::Relation", false, name, conf));
            }
            return self.methods_for_conf("Array", false, name, conf);
        }
        self.methods_for_conf(&ty.class, false, name, conf)
    }

    /// `Button(...)` inside a Phlex kit, in a class that includes the kit,
    /// or in a module (such as a concern) that such a class includes: a
    /// method call named like a constant.
    fn kit_targets(&self, owner: &str, chain: &[String], name: &str) -> Option<Vec<Target>> {
        if !is_const_name(name) {
            return None;
        }
        let in_ancestors =
            |class: &str| self.ancestors(class).iter().map(|a| format!("{a}::{name}")).find(|n| self.const_exists(n));
        let path = ConstPath { absolute: false, segments: vec![name.to_string()] };
        if let Some(found) = self.lookup_const(&path, chain).or_else(|| in_ancestors(owner)) {
            return Some(self.const_targets(&found, Confidence::Exact));
        }
        // A module gets the kit from the classes that include it.
        let mut seen = HashSet::from([owner.to_string()]);
        let mut todo = vec![owner.to_string()];
        while let Some(module) = todo.pop() {
            for includer in self.includers.get(&module).into_iter().flatten() {
                let includes = self
                    .namespaces
                    .get(includer)
                    .is_some_and(|ns| ns.includes.contains(&module) || ns.prepends.contains(&module));
                if !includes || !seen.insert(includer.clone()) {
                    continue;
                }
                if let Some(found) = in_ancestors(includer) {
                    return Some(self.const_targets(&found, Confidence::Inferred));
                }
                todo.push(includer.clone());
            }
        }
        None
    }

    /// `Components::Button(...)`: the kit's method for its constant
    /// `Components::Button`.
    fn kit_member_targets(&self, kit: &str, name: &str) -> Option<Vec<Target>> {
        if !is_const_name(name) {
            return None;
        }
        let path = ConstPath { absolute: true, segments: vec![kit.to_string(), name.to_string()] };
        let found = self.lookup_const(&path, &[])?;
        Some(self.const_targets(&found, Confidence::Exact))
    }

    /// `f.Submit(...)` on a receiver of unknown type: the components of
    /// every Phlex kit with that name.
    fn kit_consts_named(&self, name: &str) -> Vec<Target> {
        if !is_const_name(name) {
            return Vec::new();
        }
        let mut kits: Vec<&String> = self
            .includers
            .get("Phlex::Kit")
            .into_iter()
            .flatten()
            .filter(|k| self.namespaces.get(*k).is_some_and(|ns| ns.extends.iter().any(|e| e == "Phlex::Kit")))
            .collect();
        kits.sort();
        kits.into_iter()
            .map(|k| format!("{k}::{name}"))
            .filter(|n| self.consts.contains_key(n))
            .flat_map(|n| self.const_targets(&n, Confidence::NameOnly))
            .take(NAME_ONLY_LIMIT)
            .collect()
    }

    fn route_targets(&self, name: &str) -> Option<Vec<Target>> {
        let base = route_helper_base(name)?;
        let ids = self.route_helpers.get(base)?;
        Some(
            ids.iter()
                .filter_map(|i| {
                    let route = &self.routes[*i];
                    let loc = self.route_loc(route)?;
                    let to = match (&route.controller, &route.action) {
                        (Some(c), Some(a)) => format!(" → {c}#{a}"),
                        _ => String::new(),
                    };
                    Some(Target {
                        loc,
                        label: format!("{base}_path"),
                        confidence: Confidence::Exact,
                        detail: Some(format!("route {} {}{to}", route.verb, route.path)),
                    })
                })
                .collect(),
        )
    }

    /// Implicit calls in a spec file: `let`, `subject`, `described_class`,
    /// and methods defined in an enclosing `describe` block.
    fn spec_call_targets(&self, file: FileId, r: &RawRef, name: &str) -> Option<Vec<Target>> {
        if name == "described_class" {
            let f = &self.files[file as usize];
            let block = f
                .raw
                .spec_blocks
                .iter()
                .filter(|b| b.described.is_some() && b.span.contains(r.span.start))
                .min_by_key(|b| b.span.len())?;
            let path = block.described.as_ref()?;
            let class = self.lookup_const(path, &self.chain(file, block.scope))?;
            return Some(self.const_targets(&class, Confidence::Exact));
        }
        let lets = self.let_targets(file, r.span.start, name);
        if !lets.is_empty() {
            return Some(lets);
        }
        let ids = self.methods_by_name.get(name)?;
        let f = &self.files[file as usize];
        let defs: Vec<Target> = ids
            .iter()
            .filter(|id| {
                let m = &self.methods[**id as usize];
                if m.owner != BLOCK_OWNER || m.loc.file != file {
                    return false;
                }
                // The innermost group around the def must also hold the call.
                let group = f
                    .raw
                    .spec_blocks
                    .iter()
                    .filter(|b| b.kind != SpecBlockKind::Example && b.span.covers(m.loc.span))
                    .min_by_key(|b| b.span.len());
                group.is_none_or(|g| g.span.contains(r.span.start))
            })
            .map(|id| self.method_target(*id, Confidence::Exact))
            .collect();
        (!defs.is_empty()).then_some(defs)
    }

    fn let_targets(&self, file: FileId, offset: u32, name: &str) -> Vec<Target> {
        let Some(i) = self.let_at(file, offset, name) else { return Vec::new() };
        let l = &self.files[file as usize].raw.lets[i];
        vec![Target {
            loc: Loc { file, span: l.span, name_span: l.name_span },
            label: format!("let(:{name})"),
            confidence: Confidence::Exact,
            detail: Some("let".into()),
        }]
    }

    fn ivar_targets(&self, owner: &str, name: &str) -> Vec<Target> {
        let mut out = Vec::new();
        let mut owners = vec![owner.to_string()];
        owners.extend(self.ancestors(owner).iter().skip(1).cloned());
        for o in owners {
            if let Some(list) = self.ivar_index.get(&(o.clone(), name.to_string())) {
                for (file, i) in list {
                    let iv = &self.files[*file as usize].raw.ivars[*i as usize];
                    out.push(Target {
                        loc: Loc { file: *file, span: iv.name_span, name_span: iv.name_span },
                        label: format!("{o} {name}"),
                        confidence: Confidence::Exact,
                        detail: Some("instance variable".into()),
                    });
                }
            }
            if !out.is_empty() {
                break;
            }
        }
        out
    }

    fn super_targets(&self, owner: &str, singleton: bool, name: &str) -> Vec<Target> {
        let chain: Vec<(String, bool)> = if singleton {
            self.singleton_chain(owner)
        } else {
            self.ancestors(owner).iter().map(|a| (a.clone(), false)).collect()
        };
        let start = chain.iter().position(|(o, s)| o == owner && *s == singleton).map_or(0, |p| p + 1);
        for (o, sing) in &chain[start..] {
            let Some(table) = self.owners.get(o) else { continue };
            let side = if *sing { &table.singleton } else { &table.instance };
            if let Some(ids) = side.get(name) {
                return ids.iter().map(|id| self.method_target(*id, Confidence::Exact)).collect();
            }
        }
        Vec::new()
    }

    fn i18n_targets(&self, file: FileId, scope: u32, key: &str, method: Option<&str>) -> Vec<Target> {
        let to_targets = |full: &str, conf: Confidence| -> Vec<Target> {
            self.locale_keys
                .get(full)
                .map(|defs| {
                    defs.iter()
                        .map(|(locale, loc)| Target {
                            loc: *loc,
                            label: format!("{locale}.{full}"),
                            confidence: conf,
                            detail: Some(format!("translation ({locale})")),
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        if let Some(lazy) = key.strip_prefix('.') {
            let owner = self.self_owner(file, scope);
            let class = (!owner.starts_with('(') && owner != "Object").then_some(owner.as_str());
            let is_phlex = class.is_some_and(|c| self.ancestors(c).iter().any(|a| a == "Phlex::SGML"));
            let rel = self.rel_path(file).unwrap_or_default();
            for prefix in rails::lazy_prefixes(&rel, class, method, is_phlex) {
                let t = to_targets(&format!("{prefix}.{lazy}"), Confidence::Exact);
                if !t.is_empty() {
                    return t;
                }
            }
            // Any key that ends the same way.
            let suffix = format!(".{lazy}");
            let mut keys: Vec<&String> = self.locale_keys.keys().filter(|k| k.ends_with(&suffix)).collect();
            keys.sort();
            return keys.into_iter().take(20).flat_map(|k| to_targets(k, Confidence::NameOnly)).collect();
        }
        let exact = to_targets(key, Confidence::Exact);
        if !exact.is_empty() {
            return exact;
        }
        // A key that is a whole subtree: show its first key.
        let prefix = format!("{key}.");
        let mut keys: Vec<&String> = self.locale_keys.keys().filter(|k| k.starts_with(&prefix)).collect();
        keys.sort();
        keys.into_iter().take(1).flat_map(|k| to_targets(k, Confidence::NameOnly)).collect()
    }

    fn render_targets(&self, file: FileId, owner: &str, path: &str) -> Vec<Target> {
        let (dir, base) = match path.rsplit_once('/') {
            Some((d, b)) => (d.to_string(), b.to_string()),
            None => {
                let rel = self.rel_path(file).unwrap_or_default();
                let dir = if let Some(view) = rel.strip_prefix("app/views/") {
                    view.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default()
                } else if let Some(c) = owner.strip_suffix("Controller") {
                    inflect::underscore(c)
                } else {
                    String::new()
                };
                (dir, path.to_string())
            }
        };
        let prefixes = [format!("app/views/{dir}/_{base}."), format!("app/views/{dir}/{base}.")];
        let mut out = Vec::new();
        for prefix in prefixes {
            for rel in self.all_files.iter().filter(|f| f.starts_with(&prefix)) {
                if let Some(id) = self.file_id(std::path::Path::new(rel)) {
                    let loc = Loc { file: id, span: Span::default(), name_span: Span::default() };
                    out.push(self.simple_target(loc, rel, "template"));
                }
            }
            if !out.is_empty() {
                break;
            }
        }
        out
    }

    fn factory_targets(&self, name: &str) -> Vec<Target> {
        self.factories
            .get(name)
            .map(|list| {
                list.iter().map(|f| self.simple_target(f.loc, &format!("factory :{name}"), "factory")).collect()
            })
            .unwrap_or_default()
    }

    fn trait_targets(&self, factory: &str, name: &str) -> Vec<Target> {
        let mut current = Some(factory.to_string());
        let mut steps = 0;
        while let Some(f) = current {
            if let Some(locs) = self.traits.get(&(Some(f.clone()), name.to_string())) {
                return locs.iter().map(|l| self.simple_target(*l, &format!("trait :{name}"), "trait")).collect();
            }
            current = self.factories.get(&f).and_then(|l| l.first()).and_then(|i| i.parent.clone());
            steps += 1;
            if steps > 10 {
                break;
            }
        }
        self.traits
            .get(&(None, name.to_string()))
            .map(|locs| locs.iter().map(|l| self.simple_target(*l, &format!("trait :{name}"), "trait")).collect())
            .unwrap_or_default()
    }

    fn stimulus_targets(&self, controller: &str, action: Option<&str>) -> Vec<Target> {
        let Some(file) = self.stimulus.get(controller).copied() else { return Vec::new() };
        let mut span = Span::default();
        if let Some(action) = action
            && let Some(text) = self.read_text(file)
        {
            let re = regex::Regex::new(&format!(
                r"(?m)^[ \t]*(?:async[ \t]+)?(?:static[ \t]+)?{}[ \t]*(?:\(|=[ \t]*(?:async[ \t]*)?\()",
                regex::escape(action)
            ))
            .expect("valid");
            if let Some(m) = re.find(&text) {
                let start = m.start() + m.as_str().find(action).unwrap_or(0);
                span = Span::new(start, start + action.len());
            }
        }
        let label = match action {
            Some(a) => format!("{controller}#{a}"),
            None => controller.to_string(),
        };
        vec![self.simple_target(Loc { file, span, name_span: span }, &label, "Stimulus controller")]
    }

    fn simple_target(&self, loc: Loc, label: &str, detail: &str) -> Target {
        Target { loc, label: label.to_string(), confidence: Confidence::Exact, detail: Some(detail.to_string()) }
    }

    fn methods_for(&self, owner: &str, singleton: bool, name: &str) -> Option<Vec<Target>> {
        self.methods_for_conf(owner, singleton, name, Confidence::Exact)
    }

    fn methods_for_conf(&self, owner: &str, singleton: bool, name: &str, conf: Confidence) -> Option<Vec<Target>> {
        let ids = self.find_method(owner, singleton, name)?;
        let mut out: Vec<Target> = ids.iter().map(|id| self.method_target(*id, conf)).collect();
        // `delegate :x, to: :y`: also show the method on `y`.
        for id in ids {
            let m = &self.methods[*id as usize];
            if m.kind == MethodKind::Delegate
                && let (Some(to), Some(orig)) = (&m.delegate_to, &m.alias_of)
            {
                let ctx =
                    Ctx { file: m.loc.file, scope: m.scope, self_ctx: SelfCtx::Instance, offset: m.loc.span.start };
                let target = if to.starts_with('@') {
                    Expr::Ivar { name: to.clone() }
                } else {
                    Expr::Call { recv: None, name: to.clone() }
                };
                if let Some((ty, c)) = self.type_of(&target, &ctx)
                    && let Some(more) = self.typed_targets(&ty, orig, c.max(Confidence::Inferred))
                {
                    out.extend(more);
                }
            }
        }
        Some(out)
    }

    pub(crate) fn method_target(&self, id: MethodId, confidence: Confidence) -> Target {
        let m = &self.methods[id as usize];
        let detail = match m.kind {
            MethodKind::Def => None,
            MethodKind::AttrReader | MethodKind::AttrWriter => Some("attribute"),
            MethodKind::DefineMethod => Some("define_method"),
            MethodKind::Alias => Some("alias"),
            MethodKind::Association => Some("association"),
            MethodKind::Scope => Some("scope"),
            MethodKind::Enum => Some("enum"),
            MethodKind::Delegate => Some("delegate"),
            MethodKind::ClassAttribute => Some("class attribute"),
            MethodKind::Attribute => Some("attribute"),
            MethodKind::Column => Some("database column"),
        };
        Target { loc: m.loc, label: m.label(), confidence, detail: detail.map(str::to_string) }
    }

    /// Every definition of a constant. For a class that is opened in many
    /// files, the file that Zeitwerk would load comes first.
    pub fn const_targets(&self, name: &str, confidence: Confidence) -> Vec<Target> {
        let Some(defs) = self.consts.get(name) else { return self.namespace_parts(name, confidence) };
        let expected = format!("{}.rb", inflect::underscore(name.rsplit("::").next().unwrap_or(name)));
        let mut targets: Vec<(u8, Target)> = defs
            .iter()
            .map(|d| {
                let path = &self.files[d.loc.file as usize].path;
                let main_file = path.file_name().is_some_and(|f| f.to_string_lossy() == expected);
                let rank = match (d.kind, main_file) {
                    (ConstKind::Value, _) => 0,
                    (_, true) => 0,
                    _ => 1,
                } + if self.origin(d.loc.file).is_project() { 0 } else { 2 };
                let detail = match d.kind {
                    ConstKind::Class => "class",
                    ConstKind::Module => "module",
                    ConstKind::Value => "constant",
                };
                (rank, Target { loc: d.loc, label: name.to_string(), confidence, detail: Some(detail.into()) })
            })
            .collect();
        targets.sort_by_key(|(rank, t)| (*rank, t.loc.file, t.loc.span.start));
        targets.into_iter().map(|(_, t)| t).collect()
    }

    /// A namespace that no file defines, such as the module Zeitwerk makes
    /// for a folder: the main place of each constant in it.
    fn namespace_parts(&self, name: &str, confidence: Confidence) -> Vec<Target> {
        if !self.implicit_namespaces.contains(name) {
            return Vec::new();
        }
        let prefix = format!("{name}::");
        let mut names: Vec<&String> = self.consts.keys().filter(|k| k.starts_with(&prefix)).collect();
        names.sort();
        names
            .into_iter()
            .filter_map(|n| self.const_targets(n, confidence).into_iter().next())
            .take(NAME_ONLY_LIMIT)
            .collect()
    }

    /// Constants whose last segment is `seg`, when the lookup failed.
    fn consts_named(&self, seg: &str) -> Vec<Target> {
        let mut names: Vec<&String> = self.consts.keys().filter(|k| k.rsplit("::").next() == Some(seg)).collect();
        names.sort();
        names.into_iter().flat_map(|n| self.const_targets(n, Confidence::NameOnly)).take(NAME_ONLY_LIMIT).collect()
    }

    /// Methods with this name anywhere, project methods first.
    fn methods_named(&self, name: &str) -> Vec<Target> {
        let Some(ids) = self.methods_by_name.get(name) else { return Vec::new() };
        let mut ids: Vec<MethodId> = ids.clone();
        ids.sort_by_key(|id| {
            let m = &self.methods[*id as usize];
            (!self.origin(m.loc.file).is_project(), m.owner == VIEW_OWNER, m.loc.file, m.loc.span.start)
        });
        ids.into_iter().take(NAME_ONLY_LIMIT).map(|id| self.method_target(id, Confidence::NameOnly)).collect()
    }
}

impl Index {
    /// The views of a controller action or a mailer method:
    /// `InvoicesController#edit` → `app/views/invoices/edit.html.erb`,
    /// `app/views/invoices/edit.rb`, a variant folder such as
    /// `app/views/invoices/native/edit.rb`, and `Views::Invoices::Edit`.
    pub fn action_views(&self, owner: &str, action: &str) -> Vec<Target> {
        let dir = match owner.strip_suffix("Controller") {
            Some(base) => inflect::underscore(base),
            None if owner.ends_with("Mailer") => inflect::underscore(owner),
            None => return Vec::new(),
        };
        let prefix = format!("app/views/{dir}/");
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for rel in self.all_files.iter().filter(|f| f.starts_with(&prefix)) {
            let rest = &rel[prefix.len()..];
            let (sub, name) = match rest.split_once('/') {
                None => (None, rest),
                Some((sub, name)) if !name.contains('/') => (Some(sub), name),
                _ => continue,
            };
            if name.split('.').next() != Some(action) {
                continue;
            }
            // `invoices/payments/edit.rb` belongs to `Invoices::PaymentsController`.
            if let Some(sub) = sub
                && self.const_exists(&format!("{}Controller", inflect::camelize(&format!("{dir}/{sub}"))))
            {
                continue;
            }
            let Some(id) = self.file_id(std::path::Path::new(rel)) else { continue };
            seen.insert(id);
            out.push(self.view_target(id, rel));
        }
        // Phlex views in other folders.
        let class = format!("Views::{}::{}", inflect::camelize(&dir), inflect::camelize(action));
        for t in self.const_targets(&class, Confidence::Inferred) {
            if seen.insert(t.loc.file) {
                out.push(t);
            }
        }
        out
    }

    /// A view file as a target: the class it defines, or the file start.
    fn view_target(&self, file: FileId, rel: &str) -> Target {
        let raw = &self.files[file as usize].raw;
        let class = raw.scopes.iter().filter(|s| s.kind == ScopeKind::Class).max_by_key(|s| s.span.start);
        let loc = match class {
            Some(s) => Loc { file, span: s.span, name_span: s.name_span },
            None => Loc { file, span: Span::default(), name_span: Span::default() },
        };
        self.simple_target(loc, rel, "view")
    }

    /// The controller actions (or mailer methods) of a view file. For an
    /// action with no `def`, the controller class.
    pub fn view_actions(&self, file: FileId) -> Vec<Target> {
        let Some(rel) = self.rel_path(file) else { return Vec::new() };
        let Some((dir, name)) = rel.strip_prefix("app/views/").and_then(|v| v.rsplit_once('/')) else {
            return Vec::new();
        };
        let action = name.split('.').next().unwrap_or(name);
        if action.starts_with('_') {
            return Vec::new();
        }
        // The folder itself, then its parent for a variant folder.
        let mut dirs = vec![dir];
        if let Some((parent, _)) = dir.rsplit_once('/') {
            dirs.push(parent);
        }
        for d in dirs {
            let camel = inflect::camelize(d);
            for class in [format!("{camel}Controller"), camel] {
                if !class.ends_with("Controller") && !class.ends_with("Mailer") {
                    continue;
                }
                if let Some(targets) = self.methods_for(&class, false, action) {
                    return targets;
                }
                if self.const_exists(&class) {
                    return self.const_targets(&class, Confidence::Inferred);
                }
            }
        }
        Vec::new()
    }

    /// The other half of a controller action and its view, for the place
    /// at an offset: the views of the action around it, or the action of
    /// the view file. The text is the menu label.
    pub fn related_at(&self, file: FileId, offset: u32) -> Option<(&'static str, Vec<Target>)> {
        let raw = &self.files[file as usize].raw;
        let method =
            raw.methods.iter().filter(|m| m.span.contains(offset) && !m.singleton).min_by_key(|m| m.span.len());
        if let Some(m) = method {
            let views = self.action_views(&self.self_owner(file, m.scope), &m.name);
            if !views.is_empty() {
                return Some(("Go to view", views));
            }
        }
        let actions = self.view_actions(file);
        let is_class = actions.first().is_some_and(|t| t.detail.as_deref() != Some("view") && !t.label.contains('#'));
        let label = if is_class { "Go to controller" } else { "Go to controller action" };
        (!actions.is_empty()).then_some((label, actions))
    }
}
