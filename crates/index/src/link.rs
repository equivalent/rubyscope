//! The second half of linking: Rails models, routes, locales, RSpec,
//! FactoryBot, Stimulus, and the tables that "find usages" and the type
//! hierarchy need.

use std::collections::HashMap;
use std::path::Path;

use rayon::prelude::*;

use crate::index::{Association, FactoryInfo, FileId, Index, Loc, MethodDef, Namespace, lexical_chain};
use crate::inflect;
use crate::model::*;
use crate::rails;

/// The namespace for code in ERB templates: a view with every helper.
pub const VIEW_OWNER: &str = "(view)";
/// The namespace for code in RSpec files.
pub const SPEC_OWNER: &str = "(spec)";

/// rspec-rails example group modules by spec folder.
const SPEC_TYPES: &[(&str, &str)] = &[
    ("requests", "RSpec::Rails::RequestExampleGroup"),
    ("controllers", "RSpec::Rails::ControllerExampleGroup"),
    ("models", "RSpec::Rails::ModelExampleGroup"),
    ("system", "RSpec::Rails::SystemExampleGroup"),
    ("features", "RSpec::Rails::FeatureExampleGroup"),
    ("helpers", "RSpec::Rails::HelperExampleGroup"),
    ("mailers", "RSpec::Rails::MailerExampleGroup"),
    ("jobs", "RSpec::Rails::JobExampleGroup"),
    ("views", "RSpec::Rails::ViewExampleGroup"),
    ("routing", "RSpec::Rails::RoutingExampleGroup"),
    ("channels", "RSpec::Rails::ChannelExampleGroup"),
];

impl Index {
    pub(crate) fn link_extra(&mut self) {
        self.link_abstract_classes();
        self.link_synthetic_namespaces();
        self.clear_ancestors();
        self.link_associations();
        self.link_schema();
        self.link_routes();
        self.link_locales();
        self.link_stimulus();
        self.link_specs();
        self.link_ivars();
        self.link_hierarchy();
    }

    /// The class or module that `self` is in a scope of a file.
    pub fn self_owner(&self, file: FileId, scope: u32) -> String {
        let f = &self.files[file as usize];
        let n = &f.scope_names[scope as usize];
        if !n.is_empty() {
            return n.clone();
        }
        if f.raw.kind == FileKind::Erb {
            return VIEW_OWNER.to_string();
        }
        if let Some(owner) = self.rel_path(file).as_deref().and_then(spec_owner) {
            return owner;
        }
        "Object".to_string()
    }

    /// True when the file is a spec file.
    pub fn is_spec_file(&self, file: FileId) -> bool {
        self.rel_path(file).is_some_and(|r| r.starts_with("spec/") && r.ends_with("_spec.rb"))
            || !self.files[file as usize].raw.spec_blocks.is_empty()
    }

    fn link_abstract_classes(&mut self) {
        for f in &self.files {
            for scope in &f.raw.abstract_classes {
                self.abstract_classes.insert(f.scope_names[*scope as usize].clone());
            }
        }
    }

    /// `(view)` for ERB templates and `(spec)` for RSpec files.
    fn link_synthetic_namespaces(&mut self) {
        let mut helpers = Vec::new();
        let mut rspec_includes = Vec::new();
        for (file_id, f) in self.files.iter().enumerate() {
            let rel = self.rel_path(file_id as FileId).unwrap_or_default();
            if rel.starts_with("app/helpers/") {
                for (i, s) in f.raw.scopes.iter().enumerate() {
                    if s.kind == ScopeKind::Module && s.parent == Some(0) {
                        helpers.push(f.scope_names[i].clone());
                    }
                }
            }
            for path in &f.raw.rspec_includes {
                let name = self.lookup_const(path, &[]).unwrap_or_else(|| path.joined());
                if !rspec_includes.contains(&name) {
                    rspec_includes.push(name);
                }
            }
        }
        helpers.sort();
        helpers.dedup();
        self.namespaces.insert(
            VIEW_OWNER.to_string(),
            Namespace {
                is_class: true,
                superclass: Some("ActionView::Base".into()),
                includes: helpers,
                ..Default::default()
            },
        );
        let mut includes = vec!["RSpec::Matchers".to_string(), "RSpec::Mocks::ExampleMethods".to_string()];
        includes.extend(rspec_includes);
        self.namespaces.insert(
            SPEC_OWNER.to_string(),
            Namespace {
                is_class: true,
                superclass: Some("RSpec::Core::ExampleGroup".into()),
                includes,
                ..Default::default()
            },
        );
        for (dir, module) in SPEC_TYPES {
            self.namespaces.insert(
                format!("(spec:{dir})"),
                Namespace {
                    is_class: true,
                    superclass: Some(SPEC_OWNER.into()),
                    includes: vec![module.to_string()],
                    ..Default::default()
                },
            );
        }
    }

    fn link_associations(&mut self) {
        let mut found: Vec<(String, Association)> = Vec::new();
        for (file_id, f) in self.files.iter().enumerate() {
            for a in &f.raw.associations {
                let owner = self.self_owner(file_id as FileId, a.scope);
                let chain = lexical_chain(&f.raw, &f.scope_names, a.scope);
                let class = if a.polymorphic {
                    None
                } else {
                    let candidate = match &a.class_name {
                        Some(c) => c.clone(),
                        None if a.kind.is_collection() => inflect::camelize(&inflect::singularize(&a.name)),
                        None => inflect::camelize(&a.name),
                    };
                    ConstPath::parse(&candidate).and_then(|p| self.lookup_const(&p, &chain))
                };
                let loc = Loc { file: file_id as FileId, span: a.span, name_span: a.name_span };
                let assoc = Association {
                    kind: a.kind,
                    name: a.name.clone(),
                    class,
                    polymorphic: a.polymorphic,
                    through: a.through.clone(),
                    loc,
                };
                found.push((owner, assoc));
            }
        }
        for (owner, assoc) in found {
            self.associations.entry(owner).or_default().push(assoc);
        }
    }

    /// An association of a class or of one of its ancestors.
    pub fn association(&self, owner: &str, name: &str) -> Option<&Association> {
        for anc in self.ancestors(owner).iter() {
            if let Some(a) = self.associations.get(anc).and_then(|list| list.iter().find(|a| a.name == name)) {
                return Some(a);
            }
        }
        None
    }

    /// The class of an association, following `through:` when the class
    /// is not given.
    pub fn association_class(&self, owner: &str, assoc: &Association) -> Option<String> {
        if let Some(c) = &assoc.class {
            return Some(c.clone());
        }
        let through = assoc.through.as_ref()?;
        let via = self.association(owner, through)?;
        let via_class = self.association_class(owner, via)?;
        let target = self
            .association(&via_class, &assoc.name)
            .or_else(|| self.association(&via_class, &inflect::singularize(&assoc.name)))?;
        target.class.clone()
    }

    /// True for classes that descend from `ActiveRecord::Base`.
    pub fn is_model(&self, class: &str) -> bool {
        self.ancestors(class).iter().any(|a| a == "ActiveRecord::Base")
    }

    fn link_schema(&mut self) {
        // `db/schema.rb`, and `db/<name>_schema.rb` for other databases.
        let schema_files: Vec<FileId> = (0..self.files.len() as FileId)
            .filter(|f| {
                self.rel_path(*f)
                    .is_some_and(|r| r.starts_with("db/") && r.ends_with("schema.rb") && r.matches('/').count() == 1)
            })
            .collect();
        let mut tables: std::collections::HashMap<String, (FileId, rails::Table)> = Default::default();
        for file in schema_files {
            let Some(text) = self.read_text(file) else { continue };
            for t in rails::parse_schema(text.as_bytes()) {
                tables.entry(t.name.clone()).or_insert((file, t));
            }
        }
        if tables.is_empty() {
            return;
        }
        // Explicit `self.table_name = "x"` by class.
        let mut explicit: std::collections::HashMap<String, String> = Default::default();
        for f in &self.files {
            for (scope, name) in &f.raw.table_names {
                explicit.insert(f.scope_names[*scope as usize].clone(), name.clone());
            }
        }
        let classes: Vec<String> = self
            .namespaces
            .iter()
            .filter(|(n, ns)| ns.is_class && !n.starts_with('('))
            .map(|(n, _)| n.clone())
            .filter(|n| self.consts.get(n).is_some_and(|d| d.iter().any(|d| self.origin(d.loc.file).is_project())))
            .collect();
        let mut new_methods = Vec::new();
        for class in classes {
            if self.abstract_classes.contains(&class) || !self.is_model(&class) {
                continue;
            }
            // STI subclasses use the table of their base class.
            let sup = self.namespaces.get(&class).and_then(|ns| ns.superclass.clone());
            let is_base = match &sup {
                None => true,
                Some(s) => s == "ActiveRecord::Base" || self.abstract_classes.contains(s),
            };
            if !is_base {
                continue;
            }
            let demodulized = class.rsplit("::").next().unwrap_or(&class);
            let table = explicit.get(&class).cloned().unwrap_or_else(|| inflect::tableize(demodulized));
            let Some((file, t)) = tables.get(&table) else { continue };
            self.model_tables.insert(class.clone(), table.clone());
            for col in &t.columns {
                let loc = Loc { file: *file, span: col.span, name_span: col.name_span };
                let value_class = rails::column_class(&col.sql_type).map(str::to_string);
                for (suffix, vc) in [("", value_class.clone()), ("=", None), ("?", Some("TrueClass".to_string()))] {
                    new_methods.push(MethodDef {
                        owner: class.clone(),
                        name: format!("{}{suffix}", col.name),
                        singleton: false,
                        kind: MethodKind::Column,
                        visibility: Visibility::Public,
                        loc,
                        params: String::new(),
                        alias_of: None,
                        scope: 0,
                        doc: Some(format!("Column `{}` of table `{}` ({})", col.name, table, col.sql_type)),
                        returns: None,
                        delegate_to: None,
                        value_class: vc,
                    });
                }
            }
        }
        for m in new_methods {
            self.push_method(m);
        }
    }

    fn link_routes(&mut self) {
        let memory = &self.memory;
        let read = |p: &Path| -> Option<Vec<u8>> {
            memory.get(p).map(|t| t.as_bytes().to_vec()).or_else(|| std::fs::read(p).ok())
        };
        let parsed = rails::parse_routes(&self.root, &read);
        for (i, r) in parsed.routes.iter().enumerate() {
            if let Some(n) = &r.name {
                self.route_helpers.entry(n.clone()).or_default().push(i);
            }
        }
        for link in parsed.links {
            if let Some(file) = self.file_ids.get(&link.file).copied() {
                self.route_links.entry(file).or_default().push(link);
            }
        }
        self.routes = parsed.routes;
    }

    /// The place of a route in its file.
    pub fn route_loc(&self, route: &rails::Route) -> Option<Loc> {
        let file = *self.file_ids.get(&route.file)?;
        Some(Loc { file, span: route.span, name_span: route.span })
    }

    fn link_locales(&mut self) {
        for file in 0..self.files.len() as FileId {
            let Some(rel) = self.rel_path(file) else { continue };
            if !rel.starts_with("config/locales/") || self.files[file as usize].raw.kind != FileKind::Text {
                continue;
            }
            let Some(text) = self.read_text(file) else { continue };
            for k in rails::parse_locale(&text) {
                let loc = Loc { file, span: k.span, name_span: k.span };
                self.locale_keys.entry(k.key).or_default().push((k.locale, loc));
            }
        }
        // English first, then the other locales by name.
        for list in self.locale_keys.values_mut() {
            list.sort_by(|a, b| (a.0 != "en", &a.0).cmp(&(b.0 != "en", &b.0)));
        }
    }

    fn link_stimulus(&mut self) {
        for file in 0..self.files.len() as FileId {
            let Some(rel) = self.rel_path(file) else { continue };
            if let Some(id) = stimulus_identifier(&rel) {
                self.stimulus.entry(id).or_insert(file);
            }
        }
    }

    fn link_specs(&mut self) {
        for (file_id, f) in self.files.iter().enumerate() {
            let file = file_id as FileId;
            for g in &f.raw.shared_groups {
                let loc = Loc { file, span: g.span, name_span: g.name_span };
                self.shared_groups.entry(g.name.clone()).or_default().push(loc);
            }
            for fd in &f.raw.factories {
                let loc = Loc { file, span: fd.span, name_span: fd.name_span };
                self.factories.entry(fd.name.clone()).or_default().push(FactoryInfo {
                    loc,
                    class_name: fd.class_name.clone(),
                    parent: fd.parent.clone(),
                });
            }
            for t in &f.raw.traits {
                let loc = Loc { file, span: t.span, name_span: t.name_span };
                self.traits.entry((t.factory.clone(), t.name.clone())).or_default().push(loc);
            }
        }
    }

    fn link_ivars(&mut self) {
        let mut entries = Vec::new();
        for (file_id, f) in self.files.iter().enumerate() {
            for (i, iv) in f.raw.ivars.iter().enumerate() {
                let owner = self.self_owner(file_id as FileId, iv.scope);
                entries.push(((owner, iv.name.clone()), (file_id as FileId, i as u32)));
            }
        }
        for (k, v) in entries {
            self.ivar_index.entry(k).or_default().push(v);
        }
    }

    /// References by name, for "find usages". Built on first use, because
    /// it takes a quarter of the link time and only usages need it.
    pub fn refs_by_name(&self) -> &HashMap<String, Vec<(FileId, u32)>> {
        self.refs_by_name.get_or_init(|| {
            // Decode the saved references of every file first, in parallel.
            self.files.par_iter().for_each(|f| {
                f.refs();
            });
            let mut out: HashMap<String, Vec<(FileId, u32)>> = HashMap::new();
            for (file_id, f) in self.files.iter().enumerate() {
                for (i, r) in f.refs().iter().enumerate() {
                    let name = match &r.kind {
                        RefKind::Call { name, .. } | RefKind::MethodSymbol { name, .. } => name.as_str(),
                        RefKind::Const(path) => path.segments.last().map(String::as_str).unwrap_or_default(),
                        _ => continue,
                    };
                    match out.get_mut(name) {
                        Some(list) => list.push((file_id as FileId, i as u32)),
                        None => {
                            out.insert(name.to_string(), vec![(file_id as FileId, i as u32)]);
                        }
                    }
                }
            }
            out
        })
    }

    fn link_hierarchy(&mut self) {
        for (name, ns) in &self.namespaces {
            if let Some(sup) = &ns.superclass {
                self.subclasses.entry(sup.clone()).or_default().push(name.clone());
            }
            for m in ns.includes.iter().chain(&ns.extends).chain(&ns.prepends) {
                self.includers.entry(m.clone()).or_default().push(name.clone());
            }
        }
        for list in self.subclasses.values_mut().chain(self.includers.values_mut()) {
            list.sort();
            list.dedup();
        }
    }
}

/// The namespace that runs code at the top of a spec file.
pub fn spec_owner(rel: &str) -> Option<String> {
    let rest = rel.strip_prefix("spec/")?;
    let dir = rest.split('/').next().unwrap_or("");
    Some(match SPEC_TYPES.iter().find(|(d, _)| *d == dir) {
        Some((d, _)) => format!("(spec:{d})"),
        None => SPEC_OWNER.to_string(),
    })
}

/// The Stimulus identifier of a controller file:
/// `app/javascript/controllers/admin/sales_charts_controller.js` →
/// `admin--sales-charts`.
pub fn stimulus_identifier(rel: &str) -> Option<String> {
    let stem = [".js", ".ts", ".jsx", ".tsx"].iter().find_map(|e| rel.strip_suffix(&format!("_controller{e}")))?;
    let after = match stem.find("/controllers/") {
        Some(i) => &stem[i + "/controllers/".len()..],
        None => stem.strip_prefix("app/components/").or_else(|| stem.strip_prefix("app/javascript/"))?,
    };
    Some(after.replace('/', "--").replace('_', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stimulus_names() {
        assert_eq!(
            stimulus_identifier("app/javascript/controllers/admin/sales_charts_controller.js").as_deref(),
            Some("admin--sales-charts")
        );
        assert_eq!(
            stimulus_identifier("app/javascript/controllers/date_picker_controller.ts").as_deref(),
            Some("date-picker")
        );
        assert_eq!(stimulus_identifier("app/components/ui/select_controller.js").as_deref(), Some("ui--select"));
        assert_eq!(stimulus_identifier("app/models/x.rb"), None);
    }

    #[test]
    fn spec_owners() {
        assert_eq!(spec_owner("spec/requests/a_spec.rb").as_deref(), Some("(spec:requests)"));
        assert_eq!(spec_owner("spec/services/a_spec.rb").as_deref(), Some("(spec)"));
        assert_eq!(spec_owner("app/models/a.rb"), None);
    }
}
