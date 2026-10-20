//! Walks the Prism syntax tree of one file and collects its scopes,
//! methods, constant writes, references and the facts that type inference
//! and the Rails, RSpec and Stimulus rules need.

use std::collections::HashMap;

use ruby_prism::{
    AliasMethodNode, AssocNode, BlockNode, CallNode, ClassNode, ConstantPathNode, ConstantPathWriteNode,
    ConstantReadNode, ConstantWriteNode, DefNode, ForwardingSuperNode, InstanceVariableOrWriteNode,
    InstanceVariableReadNode, InstanceVariableWriteNode, LocalVariableOrWriteNode, LocalVariableReadNode,
    LocalVariableWriteNode, Location, ModuleNode, Node, SingletonClassNode, SuperNode, Visit,
};

use crate::model::*;

/// Extracts a Ruby file.
pub fn extract(source: &[u8]) -> RawFile {
    let result = ruby_prism::parse(source);
    let comments = own_line_comments(source, &result);
    let mut ex = Extractor {
        source,
        file: RawFile {
            line_starts: line_starts(source),
            parse_errors: result.errors().count() as u32,
            ..Default::default()
        },
        frames: Vec::new(),
        comments,
        pending_block: None,
        spec_stack: Vec::new(),
        factory_stack: Vec::new(),
        in_factory_define: 0,
    };
    ex.file.scopes.push(RawScope {
        parent: None,
        kind: ScopeKind::Root,
        path: ConstPath { absolute: false, segments: Vec::new() },
        span: Span::new(0, source.len()),
        name_span: Span::new(0, 0),
        superclass: None,
        mixins: Vec::new(),
        is_concern: false,
        doc: None,
    });
    ex.file.local_scopes.push(LocalScope { span: Span::new(0, source.len()) });
    ex.frames.push(Frame::body(0, SelfCtx::Instance, 0));
    ex.visit(&result.node());
    ex.file.refs.sort_by_key(|r| (r.span.start, std::cmp::Reverse(r.span.end)));
    ex.file
}

/// Extracts an ERB template: the Ruby inside the tags is parsed in place,
/// and `data-controller` / `data-action` attributes become Stimulus links.
pub fn extract_erb(source: &[u8]) -> RawFile {
    let ruby = erb_to_ruby(source);
    let mut file = extract(&ruby);
    file.kind = FileKind::Erb;
    file.parse_errors = 0;
    for (span, kind) in html_stimulus_refs(source, &ruby) {
        file.refs.push(RawRef { span, scope: 0, self_ctx: SelfCtx::Instance, kind });
    }
    file.refs.sort_by_key(|r| (r.span.start, std::cmp::Reverse(r.span.end)));
    file
}

/// Replaces everything outside ERB tags with spaces (newlines stay), so
/// the Ruby code keeps its byte offsets. Each closing `%>` becomes `;`.
pub fn erb_to_ruby(source: &[u8]) -> Vec<u8> {
    let mut out = vec![b' '; source.len()];
    let mut i = 0;
    let mut in_code = false;
    let mut comment = false;
    while i < source.len() {
        let b = source[i];
        if b == b'\n' {
            out[i] = b'\n';
            i += 1;
            continue;
        }
        if !in_code {
            if source[i..].starts_with(b"<%%") {
                i += 3;
                continue;
            }
            if source[i..].starts_with(b"<%") {
                in_code = true;
                i += 2;
                comment = source.get(i) == Some(&b'#');
                // `<%=`, `<%-` and `<%#` openers are blanked too.
                if matches!(source.get(i), Some(b'=' | b'-' | b'#')) {
                    i += 1;
                }
                continue;
            }
            i += 1;
            continue;
        }
        if source[i..].starts_with(b"-%>") || source[i..].starts_with(b"%>") {
            if !comment {
                out[i] = b';';
            }
            i += if source[i] == b'-' { 3 } else { 2 };
            in_code = false;
            comment = false;
            continue;
        }
        if !comment {
            out[i] = b;
        }
        i += 1;
    }
    out
}

/// Finds `data-controller="a b"` and `data-action="click->a#go"` in the
/// HTML part of a template (where `ruby` has spaces).
fn html_stimulus_refs(source: &[u8], ruby: &[u8]) -> Vec<(Span, RefKind)> {
    let mut out = Vec::new();
    for (attr, is_action) in [(&b"data-controller=\""[..], false), (&b"data-action=\""[..], true)] {
        let mut from = 0;
        while let Some(pos) = find(source, attr, from) {
            from = pos + attr.len();
            if ruby[pos] != b' ' {
                continue;
            }
            let start = pos + attr.len();
            let Some(len) = source[start..].iter().position(|b| *b == b'"') else { continue };
            let value = &source[start..start + len];
            out.extend(stimulus_tokens(value, start, is_action));
        }
    }
    out
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// Splits a Stimulus attribute value into links. `base` is the offset of
/// the value in the file.
fn stimulus_tokens(value: &[u8], base: usize, is_action: bool) -> Vec<(Span, RefKind)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < value.len() {
        while i < value.len() && value[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < value.len() && !value[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            continue;
        }
        let token = &value[start..i];
        if !is_action {
            if is_stimulus_name(token) {
                let controller = String::from_utf8_lossy(token).into_owned();
                out.push((Span::new(base + start, base + i), RefKind::Stimulus { controller, action: None }));
            }
            continue;
        }
        // `click->tabs#select`, `tabs#select`, `click->tabs#select:prevent`
        let body_start = token.windows(2).position(|w| w == b"->").map(|p| p + 2).unwrap_or(0);
        let body = &token[body_start..];
        let Some(hash) = body.iter().position(|b| *b == b'#') else { continue };
        let controller = &body[..hash];
        let method_end = body.iter().position(|b| *b == b':').unwrap_or(body.len());
        let method = &body[hash + 1..method_end.max(hash + 1)];
        if !is_stimulus_name(controller) {
            continue;
        }
        let c_start = base + start + body_start;
        let controller_s = String::from_utf8_lossy(controller).into_owned();
        out.push((
            Span::new(c_start, c_start + controller.len()),
            RefKind::Stimulus { controller: controller_s.clone(), action: None },
        ));
        if !method.is_empty() {
            let m_start = c_start + hash + 1;
            out.push((
                Span::new(m_start, m_start + method.len()),
                RefKind::Stimulus {
                    controller: controller_s,
                    action: Some(String::from_utf8_lossy(method).into_owned()),
                },
            ));
        }
    }
    out
}

fn is_stimulus_name(token: &[u8]) -> bool {
    !token.is_empty()
        && token[0].is_ascii_lowercase()
        && token.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
}

/// Comments that stand alone on their line, by 0-based line number. They
/// become the docs of the definition below them.
fn own_line_comments(source: &[u8], result: &ruby_prism::ParseResult<'_>) -> HashMap<u32, String> {
    let starts = line_starts(source);
    let mut out = HashMap::new();
    for c in result.comments() {
        let loc = c.location();
        let start = loc.start_offset() as u32;
        let line = match starts.binary_search(&start) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let before = &source[starts[line] as usize..start as usize];
        if !before.iter().all(|b| *b == b' ' || *b == b'\t') {
            continue;
        }
        let text = String::from_utf8_lossy(c.text()).into_owned();
        let body = text.strip_prefix('#').unwrap_or(&text);
        let body = body.strip_prefix(' ').unwrap_or(body).trim_end().to_string();
        out.insert(line as u32, body);
    }
    out
}

/// What the code at the current point of the walk is inside of.
#[derive(Clone, Copy)]
struct Frame {
    scope: u32,
    self_ctx: SelfCtx,
    /// The local scope (method body, class body or file).
    lscope: u32,
    /// The method the code is in, as an index into `RawFile::methods`.
    method: Option<u32>,
    /// True inside `class << self`.
    in_singleton_class: bool,
    /// True inside a method body. DSL calls only count in a class body.
    in_method: bool,
    /// The visibility set by a bare `private` / `protected` / `public`.
    visibility: Visibility,
    /// True after a bare `module_function`.
    module_function: bool,
    /// True inside a block that is not a class body.
    in_block: bool,
    /// Inside a concern's `included do` block: the scope that takes its
    /// class-level methods (`<Module>::ClassMethods`).
    included_cm: Option<u32>,
}

impl Frame {
    fn body(scope: u32, self_ctx: SelfCtx, lscope: u32) -> Self {
        Self {
            scope,
            self_ctx,
            lscope,
            method: None,
            in_singleton_class: false,
            in_method: false,
            visibility: Visibility::Public,
            module_function: false,
            in_block: false,
            included_cm: None,
        }
    }
}

struct Extractor<'src> {
    source: &'src [u8],
    file: RawFile,
    frames: Vec<Frame>,
    comments: HashMap<u32, String>,
    /// Set just before the block of a call is visited: the call's receiver
    /// and name, so block parameters get a type.
    pending_block: Option<(Expr, String)>,
    /// Open RSpec groups and examples, as indexes into `spec_blocks`.
    spec_stack: Vec<u32>,
    /// Open `factory` blocks.
    factory_stack: Vec<String>,
    /// Depth of `FactoryBot.define do` blocks.
    in_factory_define: u32,
}

fn span(loc: &Location<'_>) -> Span {
    Span::new(loc.start_offset(), loc.end_offset())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Turns a constant expression into a path. Returns `None` for dynamic
/// parents such as `foo::Bar`.
pub(crate) fn const_path(node: &Node<'_>) -> Option<ConstPath> {
    if let Some(read) = node.as_constant_read_node() {
        return Some(ConstPath { absolute: false, segments: vec![text(read.name().as_slice())] });
    }
    if let Some(path) = node.as_constant_path_node() {
        let name = text(path.name()?.as_slice());
        return match path.parent() {
            None => Some(ConstPath { absolute: true, segments: vec![name] }),
            Some(parent) => {
                let mut p = const_path(&parent)?;
                p.segments.push(name);
                Some(p)
            }
        };
    }
    None
}

fn symbol_or_string(node: &Node<'_>) -> Option<(String, Span)> {
    if let Some(sym) = node.as_symbol_node() {
        let loc = sym.value_loc().unwrap_or_else(|| sym.location());
        return Some((text(sym.unescaped()), span(&loc)));
    }
    if let Some(s) = node.as_string_node() {
        return Some((text(s.unescaped()), span(&s.content_loc())));
    }
    None
}

fn symbol(node: &Node<'_>) -> Option<(String, Span)> {
    node.as_symbol_node().and_then(|_| symbol_or_string(node))
}

fn string(node: &Node<'_>) -> Option<(String, Span)> {
    node.as_string_node().and_then(|_| symbol_or_string(node))
}

/// The positional arguments and the keyword arguments of a call.
struct Args<'pr> {
    positional: Vec<Node<'pr>>,
    keywords: Vec<(String, Span, Node<'pr>)>,
}

impl<'pr> Args<'pr> {
    fn of(node: &CallNode<'pr>) -> Args<'pr> {
        let mut positional = Vec::new();
        let mut keywords = Vec::new();
        if let Some(args) = node.arguments() {
            for arg in args.arguments().iter() {
                if let Some(hash) = arg.as_keyword_hash_node() {
                    for el in hash.elements().iter() {
                        if let Some(pair) = el.as_assoc_node()
                            && let Some((key, key_span)) = symbol_or_string(&pair.key())
                        {
                            keywords.push((key, key_span, pair.value()));
                        }
                    }
                } else {
                    positional.push(arg);
                }
            }
        }
        Args { positional, keywords }
    }

    fn keyword(&self, key: &str) -> Option<&Node<'pr>> {
        self.keywords.iter().find(|(k, _, _)| k == key).map(|(_, _, v)| v)
    }

    fn keyword_text(&self, key: &str) -> Option<String> {
        let v = self.keyword(key)?;
        if let Some(path) = const_path(v) {
            return Some(path.joined());
        }
        symbol_or_string(v).map(|(s, _)| s)
    }

    fn keyword_is_false(&self, key: &str) -> bool {
        self.keyword(key).is_some_and(|v| v.as_false_node().is_some())
    }

    fn symbols(&self) -> Vec<(String, Span)> {
        self.positional.iter().filter_map(symbol_or_string).collect()
    }
}

/// Ruby's `Hash#keys` of a hash literal (`{ a: 1, "b" => 2 }`), or the
/// elements of an array literal (`%i[a b]`).
fn literal_keys(node: &Node<'_>) -> Vec<(String, Span)> {
    let mut out = Vec::new();
    if let Some(hash) = node.as_hash_node() {
        for el in hash.elements().iter() {
            if let Some(pair) = el.as_assoc_node()
                && let Some(k) = symbol_or_string(&pair.key())
            {
                out.push(k);
            }
        }
    } else if let Some(hash) = node.as_keyword_hash_node() {
        for el in hash.elements().iter() {
            if let Some(pair) = el.as_assoc_node()
                && let Some(k) = symbol_or_string(&pair.key())
            {
                out.push(k);
            }
        }
    } else if let Some(array) = node.as_array_node() {
        out.extend(array.elements().iter().filter_map(|e| symbol_or_string(&e)));
    }
    out
}

/// Callback-like class body calls whose symbol arguments name methods of
/// the class: `before_action :load_invoice`, `validate :check`.
fn names_method_symbols(call: &str) -> bool {
    ["before_", "after_", "around_", "skip_", "prepend_before", "prepend_after", "append_before", "append_after"]
        .iter()
        .any(|p| call.starts_with(p))
        || matches!(
            call,
            "validate"
                | "validates_each"
                | "helper_method"
                | "private"
                | "protected"
                | "public"
                | "module_function"
                | "private_class_method"
                | "public_class_method"
                | "memoize"
                | "set_callback"
        )
}

/// The last statement of a body, as a node.
fn last_statement<'pr>(node: Option<Node<'pr>>) -> Option<Node<'pr>> {
    let node = node?;
    if let Some(stmts) = node.as_statements_node() {
        return stmts.body().iter().last();
    }
    if let Some(begin) = node.as_begin_node() {
        return begin.statements().and_then(|s| s.body().iter().last());
    }
    Some(node)
}

const SPEC_GROUPS: &[&str] =
    &["describe", "context", "feature", "xdescribe", "fdescribe", "xcontext", "fcontext", "example_group"];
const SPEC_SHARED: &[&str] = &["shared_examples", "shared_examples_for", "shared_context"];
const SPEC_EXAMPLES: &[&str] = &["it", "specify", "scenario", "example", "xit", "fit", "its"];
const SPEC_SHARED_REFS: &[&str] = &["it_behaves_like", "it_should_behave_like", "include_examples", "include_context"];
const FACTORY_CALLS: &[&str] = &[
    "create",
    "build",
    "build_stubbed",
    "attributes_for",
    "create_list",
    "build_list",
    "build_stubbed_list",
    "attributes_for_list",
    "create_pair",
    "build_pair",
];

impl<'src> Extractor<'src> {
    fn frame(&self) -> Frame {
        *self.frames.last().expect("frame stack is never empty")
    }

    fn frame_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("frame stack is never empty")
    }

    fn push_scope(&mut self, scope: RawScope) -> u32 {
        self.file.scopes.push(scope);
        (self.file.scopes.len() - 1) as u32
    }

    fn push_lscope(&mut self, span: Span) -> u32 {
        self.file.local_scopes.push(LocalScope { span });
        (self.file.local_scopes.len() - 1) as u32
    }

    fn new_scope(&mut self, kind: ScopeKind, path: ConstPath, span: Span, name_span: Span) -> u32 {
        let parent = self.frame().scope;
        let doc = self.doc_above(span.start);
        self.push_scope(RawScope {
            parent: Some(parent),
            kind,
            path,
            span,
            name_span,
            superclass: None,
            mixins: Vec::new(),
            is_concern: false,
            doc,
        })
    }

    fn add_ref(&mut self, span: Span, kind: RefKind) {
        let f = self.frame();
        self.file.refs.push(RawRef { span, scope: f.scope, self_ctx: f.self_ctx, kind });
    }

    /// The comment block right above the line of `offset`.
    fn doc_above(&self, offset: u32) -> Option<String> {
        let line = self.file.line_of(offset);
        let mut lines = Vec::new();
        let mut l = line;
        while l > 0 {
            l -= 1;
            match self.comments.get(&l) {
                Some(c) => lines.push(c.clone()),
                None => break,
            }
        }
        if lines.is_empty() {
            return None;
        }
        lines.reverse();
        // Skip magic comments and tool markers at the top of a file.
        lines.retain(|l| !l.starts_with("frozen_string_literal") && !l.starts_with("rubocop:"));
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    fn params_text(&self, node: &DefNode<'_>) -> String {
        match (node.lparen_loc(), node.rparen_loc(), node.parameters()) {
            (Some(l), Some(r), _) => text(&self.source[l.start_offset()..r.end_offset()]),
            (_, _, Some(p)) => format!("({})", text(p.location().as_slice())),
            _ => String::new(),
        }
    }

    /// Records a method. `doc_from` is where to look for a doc comment above.
    #[allow(clippy::too_many_arguments)]
    fn add_method(
        &mut self,
        name: String,
        kind: MethodKind,
        singleton: bool,
        span: Span,
        name_span: Span,
        params: String,
        returns: Option<Expr>,
    ) -> u32 {
        let f = self.frame();
        let visibility = if singleton && !f.in_singleton_class { Visibility::Public } else { f.visibility };
        let doc = self.doc_above(span.start);
        // In `included do`, class-level methods belong to the includer:
        // record them on `<Module>::ClassMethods`, which includers search.
        let (scope, singleton) = match f.included_cm {
            Some(cm) if singleton => (cm, false),
            _ => (f.scope, singleton),
        };
        let base = RawMethod {
            scope,
            singleton,
            name,
            kind,
            visibility,
            span,
            name_span,
            params,
            alias_of: None,
            in_block: f.in_block,
            doc,
            returns,
            delegate_to: None,
        };
        if f.module_function && !singleton {
            let mut copy = base.clone();
            copy.singleton = true;
            copy.visibility = Visibility::Public;
            self.file.methods.push(copy);
        }
        self.file.methods.push(base);
        (self.file.methods.len() - 1) as u32
    }

    /// Records a generated method on the instance or the singleton side,
    /// pointing at the DSL call.
    fn add_generated(&mut self, name: String, kind: MethodKind, singleton: bool, call: Span, at: Span) -> u32 {
        let singleton = singleton || self.frame().in_singleton_class;
        self.add_method(name, kind, singleton, call, at, String::new(), None)
    }

    fn add_alias(&mut self, new_name: String, old_name: String, span: Span, name_span: Span) {
        let singleton = self.frame().in_singleton_class;
        let n0 = self.file.methods.len();
        self.add_method(new_name, MethodKind::Alias, singleton, span, name_span, String::new(), None);
        for m in &mut self.file.methods[n0..] {
            m.alias_of = Some(old_name.clone());
        }
    }

    /// Pushes a scope that reopens the constant `path` without opening a
    /// lexical scope, as `Foo.class_eval` and `class << Foo` do.
    fn push_eval_scope(&mut self, path: ConstPath, span: Span, name_span: Span) -> u32 {
        let parent = self.frame().scope;
        self.push_scope(RawScope {
            parent: Some(parent),
            kind: ScopeKind::EvalBlock,
            path,
            span,
            name_span,
            superclass: None,
            mixins: Vec::new(),
            is_concern: false,
            doc: None,
        })
    }

    /// Visits the body of a class, module or singleton class in a new frame.
    fn visit_body(&mut self, body: Option<Node<'_>>, frame: Frame) {
        self.frames.push(frame);
        if let Some(body) = body {
            self.visit(&body);
        }
        self.frames.pop();
    }

    /// A class body frame with its own local scope.
    fn class_frame(&mut self, scope: u32, span: Span) -> Frame {
        let lscope = self.push_lscope(span);
        Frame::body(scope, SelfCtx::Singleton, lscope)
    }

    // ---- expressions -----------------------------------------------------

    fn expr_of(&self, node: &Node<'_>) -> Expr {
        self.expr_depth(node, 0)
    }

    fn expr_depth(&self, node: &Node<'_>, depth: u32) -> Expr {
        if depth > 8 {
            return Expr::Unknown;
        }
        let d = depth + 1;
        if node.as_self_node().is_some() {
            return Expr::SelfRef;
        }
        if let Some(path) = const_path(node) {
            return Expr::Const(path);
        }
        if node.as_string_node().is_some()
            || node.as_interpolated_string_node().is_some()
            || node.as_x_string_node().is_some()
        {
            return Expr::Lit(Lit::String);
        }
        let lit = if node.as_symbol_node().is_some() || node.as_interpolated_symbol_node().is_some() {
            Some(Lit::Symbol)
        } else if node.as_integer_node().is_some() {
            Some(Lit::Integer)
        } else if node.as_float_node().is_some() {
            Some(Lit::Float)
        } else if node.as_array_node().is_some() {
            Some(Lit::Array)
        } else if node.as_hash_node().is_some() {
            Some(Lit::Hash)
        } else if node.as_nil_node().is_some() {
            Some(Lit::Nil)
        } else if node.as_true_node().is_some() {
            Some(Lit::True)
        } else if node.as_false_node().is_some() {
            Some(Lit::False)
        } else if node.as_range_node().is_some() {
            Some(Lit::Range)
        } else if node.as_regular_expression_node().is_some()
            || node.as_interpolated_regular_expression_node().is_some()
        {
            Some(Lit::Regexp)
        } else if node.as_lambda_node().is_some() {
            Some(Lit::Proc)
        } else {
            None
        };
        if let Some(lit) = lit {
            return Expr::Lit(lit);
        }
        if let Some(read) = node.as_local_variable_read_node() {
            return Expr::Local {
                name: text(read.name().as_slice()),
                lscope: self.frame().lscope,
                offset: read.location().start_offset() as u32,
            };
        }
        if let Some(read) = node.as_instance_variable_read_node() {
            return Expr::Ivar { name: text(read.name().as_slice()) };
        }
        if let Some(call) = node.as_call_node() {
            let name = text(call.name().as_slice());
            let factory_recv = match call.receiver() {
                None => true,
                Some(r) => const_path(&r).is_some_and(|p| p.joined() == "FactoryBot"),
            };
            if factory_recv
                && FACTORY_CALLS.contains(&name.as_str())
                && let Some((factory, _)) = Args::of(&call).positional.first().and_then(symbol)
            {
                let many = name.ends_with("_list") || name.ends_with("_pair");
                return Expr::Factory { name: factory, many };
            }
            let recv = call.receiver().map(|r| Box::new(self.expr_depth(&r, d)));
            return Expr::Call { recv, name };
        }
        if let Some(p) = node.as_parentheses_node() {
            return last_statement(p.body()).map(|n| self.expr_depth(&n, d)).unwrap_or(Expr::Unknown);
        }
        if let Some(s) = node.as_statements_node() {
            return s.body().iter().last().map(|n| self.expr_depth(&n, d)).unwrap_or(Expr::Unknown);
        }
        if let Some(b) = node.as_begin_node() {
            return last_statement(Some(b.as_node())).map(|n| self.expr_depth(&n, d)).unwrap_or(Expr::Unknown);
        }
        if let Some(or) = node.as_or_node() {
            return union(vec![self.expr_depth(&or.left(), d), self.expr_depth(&or.right(), d)]);
        }
        if let Some(and) = node.as_and_node() {
            return self.expr_depth(&and.right(), d);
        }
        if let Some(w) = node.as_local_variable_write_node() {
            return self.expr_depth(&w.value(), d);
        }
        if let Some(w) = node.as_instance_variable_write_node() {
            return self.expr_depth(&w.value(), d);
        }
        if let Some(w) = node.as_instance_variable_or_write_node() {
            return union(vec![Expr::Ivar { name: text(w.name().as_slice()) }, self.expr_depth(&w.value(), d)]);
        }
        if let Some(w) = node.as_local_variable_or_write_node() {
            return self.expr_depth(&w.value(), d);
        }
        if let Some(i) = node.as_if_node() {
            let mut parts = Vec::new();
            if let Some(s) = i.statements() {
                parts.push(self.expr_depth(&s.as_node(), d));
            }
            if let Some(sub) = i.subsequent() {
                parts.push(self.expr_depth(&sub, d));
            }
            return union(parts);
        }
        if let Some(u) = node.as_unless_node() {
            let mut parts = Vec::new();
            if let Some(s) = u.statements() {
                parts.push(self.expr_depth(&s.as_node(), d));
            }
            if let Some(e) = u.else_clause() {
                parts.push(self.expr_depth(&e.as_node(), d));
            }
            return union(parts);
        }
        if let Some(e) = node.as_else_node() {
            return e.statements().map(|s| self.expr_depth(&s.as_node(), d)).unwrap_or(Expr::Unknown);
        }
        Expr::Unknown
    }

    fn add_local(&mut self, name: String, name_span: Span, value: Expr) {
        let lscope = self.frame().lscope;
        self.file.locals.push(LocalAssign { lscope, name, name_span, value });
    }

    fn add_ivar(&mut self, name: String, name_span: Span, value: Expr) {
        let f = self.frame();
        self.file.ivars.push(IvarAssign {
            scope: f.scope,
            singleton: f.self_ctx == SelfCtx::Singleton,
            name,
            name_span,
            value,
        });
    }

    /// Records the parameters of a method as locals of its local scope.
    fn add_params(&mut self, params: &ruby_prism::ParametersNode<'_>) {
        for p in params.requireds().iter() {
            if let Some(r) = p.as_required_parameter_node() {
                let name = text(r.name().as_slice());
                self.add_local(name.clone(), span(&r.location()), Expr::Param { name });
            }
        }
        for p in params.optionals().iter() {
            if let Some(o) = p.as_optional_parameter_node() {
                let name = text(o.name().as_slice());
                let value = union(vec![self.expr_of(&o.value()), Expr::Param { name: name.clone() }]);
                self.add_local(name, span(&o.name_loc()), value);
            }
        }
        for p in params.keywords().iter() {
            if let Some(k) = p.as_required_keyword_parameter_node() {
                let name = text(k.name().as_slice());
                self.add_local(name.clone(), span(&k.name_loc()), Expr::Param { name });
            } else if let Some(k) = p.as_optional_keyword_parameter_node() {
                let name = text(k.name().as_slice());
                let value = union(vec![self.expr_of(&k.value()), Expr::Param { name: name.clone() }]);
                self.add_local(name, span(&k.name_loc()), value);
            }
        }
    }

    // ---- class body DSL calls --------------------------------------------

    /// Handles calls with no receiver in a class or module body, such as
    /// `include`, `attr_reader`, `has_many` and `private`. Returns true when
    /// the call's arguments and block were already visited.
    fn class_body_call(&mut self, node: &CallNode<'_>, name: &str) -> bool {
        let args = Args::of(node);
        let scope = self.frame().scope as usize;
        let call_span = span(&node.location());
        // Symbols that name methods of this class.
        if names_method_symbols(name) {
            let singleton = name.ends_with("_class_method");
            for (sym, sym_span) in args.symbols() {
                self.add_ref(sym_span, RefKind::MethodSymbol { name: sym, singleton });
            }
        }
        for (key, _, value) in &args.keywords {
            if matches!(key.as_str(), "if" | "unless" | "with" | "on_error") {
                let mut syms: Vec<(String, Span)> = Vec::new();
                match value.as_array_node() {
                    Some(a) => syms.extend(a.elements().iter().filter_map(|e| symbol(&e))),
                    None => syms.extend(symbol(value)),
                }
                for (sym, sym_span) in syms {
                    self.add_ref(sym_span, RefKind::MethodSymbol { name: sym, singleton: false });
                }
            }
        }
        match name {
            "include" | "extend" | "prepend" => {
                let kind = match name {
                    "include" => MixinKind::Include,
                    "extend" => MixinKind::Extend,
                    _ => MixinKind::Prepend,
                };
                for arg in &args.positional {
                    let target = if arg.as_self_node().is_some() {
                        MixinTarget::SelfRef
                    } else if let Some(path) = const_path(arg) {
                        if kind == MixinKind::Extend && path.joined() == "ActiveSupport::Concern" {
                            self.file.scopes[scope].is_concern = true;
                        }
                        MixinTarget::Const(path)
                    } else {
                        continue;
                    };
                    self.file.scopes[scope].mixins.push(Mixin { kind, target });
                }
            }
            "attr_reader" | "attr_writer" | "attr_accessor" => {
                for (attr, attr_span) in args.symbols() {
                    if name != "attr_writer" {
                        let id = self.add_generated(attr.clone(), MethodKind::AttrReader, false, call_span, attr_span);
                        self.file.methods[id as usize].returns = Some(Expr::Ivar { name: format!("@{attr}") });
                    }
                    if name != "attr_reader" {
                        self.add_generated(format!("{attr}="), MethodKind::AttrWriter, false, call_span, attr_span);
                    }
                }
            }
            "private" | "protected" | "public" | "module_function" => {
                let vis = match name {
                    "private" | "module_function" => Visibility::Private,
                    "protected" => Visibility::Protected,
                    _ => Visibility::Public,
                };
                if args.positional.is_empty() {
                    let f = self.frame_mut();
                    f.visibility = vis;
                    f.module_function = name == "module_function";
                    return false;
                }
                // `private def x` and `private :x, :y`: change methods that
                // are already recorded in this scope.
                let mut targets = Vec::new();
                for arg in &args.positional {
                    if let Some(def) = arg.as_def_node() {
                        // Visit the def first, so it is recorded.
                        self.visit(arg);
                        targets.push(text(def.name().as_slice()));
                    } else if let Some((n, _)) = symbol_or_string(arg) {
                        targets.push(n);
                    } else {
                        self.visit(arg);
                    }
                }
                let scope = scope as u32;
                let mut copies = Vec::new();
                for m in self.file.methods.iter_mut().rev() {
                    if m.scope == scope && !m.singleton && targets.contains(&m.name) {
                        m.visibility = vis;
                        if name == "module_function" {
                            let mut copy = m.clone();
                            copy.singleton = true;
                            copy.visibility = Visibility::Public;
                            copies.push(copy);
                        }
                    }
                }
                self.file.methods.extend(copies);
                return true;
            }
            "define_method" => {
                if let Some((meth, meth_span)) = args.positional.first().and_then(symbol_or_string) {
                    self.add_generated(meth, MethodKind::DefineMethod, false, call_span, meth_span);
                }
            }
            "alias_method" => {
                if let (Some((new_name, new_span)), Some((old_name, old_span))) = (
                    args.positional.first().and_then(symbol_or_string),
                    args.positional.get(1).and_then(symbol_or_string),
                ) {
                    self.add_ref(old_span, RefKind::MethodSymbol { name: old_name.clone(), singleton: false });
                    self.add_alias(new_name, old_name, call_span, new_span);
                }
            }
            "alias_attribute" => {
                if let (Some((new_name, new_span)), Some((old_name, old_span))) = (
                    args.positional.first().and_then(symbol_or_string),
                    args.positional.get(1).and_then(symbol_or_string),
                ) {
                    self.add_ref(old_span, RefKind::MethodSymbol { name: old_name.clone(), singleton: false });
                    for suffix in ["", "=", "?"] {
                        self.add_alias(
                            format!("{new_name}{suffix}"),
                            format!("{old_name}{suffix}"),
                            call_span,
                            new_span,
                        );
                    }
                }
            }
            "class_methods" => {
                let Some(block) = node.block().and_then(|b| b.as_block_node()) else { return false };
                let parent = self.frame().scope;
                let lscope = self.frame().lscope;
                let id = self.push_scope(RawScope {
                    parent: Some(parent),
                    kind: ScopeKind::ClassMethodsBlock,
                    path: ConstPath { absolute: false, segments: vec!["ClassMethods".into()] },
                    span: call_span,
                    name_span: span(&node.message_loc().unwrap_or_else(|| node.location())),
                    superclass: None,
                    mixins: Vec::new(),
                    is_concern: false,
                    doc: None,
                });
                for arg in &args.positional {
                    self.visit(arg);
                }
                self.visit_body(block.body(), Frame::body(id, SelfCtx::Instance, lscope));
                return true;
            }
            "belongs_to" | "has_one" | "has_many" | "has_and_belongs_to_many" => {
                let Some((assoc, assoc_span)) = args.positional.first().and_then(symbol_or_string) else {
                    return false;
                };
                let kind = match name {
                    "belongs_to" => AssocKind::BelongsTo,
                    "has_one" => AssocKind::HasOne,
                    "has_many" => AssocKind::HasMany,
                    _ => AssocKind::HasAndBelongsToMany,
                };
                let class_name = args.keyword_text("class_name");
                if let Some(v) = args.keyword("class_name")
                    && let Some((cn, cn_span)) = string(v)
                {
                    self.add_ref(cn_span, RefKind::ClassName { name: cn });
                }
                if let Some(v) = args.keyword("through")
                    && let Some((t, t_span)) = symbol(v)
                {
                    self.add_ref(t_span, RefKind::MethodSymbol { name: t, singleton: false });
                }
                let polymorphic = args.keyword("polymorphic").is_some_and(|v| v.as_true_node().is_some());
                let idx = self.file.associations.len() as u32;
                self.file.associations.push(RawAssociation {
                    scope: scope as u32,
                    kind,
                    name: assoc.clone(),
                    class_name,
                    through: args.keyword_text("through"),
                    polymorphic,
                    span: call_span,
                    name_span: assoc_span,
                });
                self.add_ref(assoc_span, RefKind::AssocClass { assoc: idx });
                let singular = crate::inflect::singularize(&assoc);
                let mut names = vec![assoc.clone(), format!("{assoc}=")];
                if kind.is_collection() {
                    names.push(format!("{singular}_ids"));
                    names.push(format!("{singular}_ids="));
                } else {
                    for prefix in ["build_", "create_", "reload_", "reset_"] {
                        names.push(format!("{prefix}{assoc}"));
                    }
                    names.push(format!("create_{assoc}!"));
                }
                for n in names {
                    self.add_generated(n, MethodKind::Association, false, call_span, assoc_span);
                }
            }
            "scope" => {
                if let Some((s, s_span)) = args.positional.first().and_then(symbol_or_string) {
                    self.add_generated(s, MethodKind::Scope, true, call_span, s_span);
                }
            }
            "enum" => self.enum_call(&args, call_span),
            "delegate" => {
                let Some(to) = args.keyword_text("to") else { return false };
                if let Some(v) = args.keyword("to")
                    && let Some((t, t_span)) = symbol(v)
                {
                    self.add_ref(t_span, RefKind::MethodSymbol { name: t, singleton: false });
                }
                let prefix = match args.keyword("prefix") {
                    Some(v) if v.as_true_node().is_some() => Some(to.trim_start_matches('@').to_string()),
                    Some(v) => symbol_or_string(v).map(|(p, _)| p),
                    None => None,
                };
                for (m, m_span) in args.symbols() {
                    let generated = match &prefix {
                        Some(p) => format!("{p}_{m}"),
                        None => m.clone(),
                    };
                    let id = self.add_generated(generated, MethodKind::Delegate, false, call_span, m_span);
                    let raw = &mut self.file.methods[id as usize];
                    raw.delegate_to = Some(to.clone());
                    raw.alias_of = Some(m);
                }
            }
            "class_attribute"
            | "mattr_accessor"
            | "cattr_accessor"
            | "mattr_reader"
            | "cattr_reader"
            | "mattr_writer"
            | "cattr_writer"
            | "thread_mattr_accessor"
            | "thread_cattr_accessor" => {
                let reader = !name.ends_with("_writer");
                let writer = !name.ends_with("_reader");
                let predicate = name == "class_attribute" && !args.keyword_is_false("instance_predicate");
                let instance_reader =
                    !args.keyword_is_false("instance_reader") && !args.keyword_is_false("instance_accessor");
                let instance_writer =
                    !args.keyword_is_false("instance_writer") && !args.keyword_is_false("instance_accessor");
                for (attr, attr_span) in args.symbols() {
                    for singleton in [true, false] {
                        let k = MethodKind::ClassAttribute;
                        if reader && (singleton || instance_reader) {
                            self.add_generated(attr.clone(), k, singleton, call_span, attr_span);
                        }
                        if writer && (singleton || instance_writer) {
                            self.add_generated(format!("{attr}="), k, singleton, call_span, attr_span);
                        }
                        if predicate && (singleton || instance_reader) {
                            self.add_generated(format!("{attr}?"), k, singleton, call_span, attr_span);
                        }
                    }
                }
            }
            "attribute" => {
                if let Some((attr, attr_span)) = args.positional.first().and_then(symbol_or_string) {
                    self.add_generated(attr.clone(), MethodKind::Attribute, false, call_span, attr_span);
                    self.add_generated(format!("{attr}="), MethodKind::Attribute, false, call_span, attr_span);
                }
            }
            "store_accessor" | "store" => {
                for (attr, attr_span) in args.symbols().into_iter().skip(1) {
                    self.add_generated(attr.clone(), MethodKind::Attribute, false, call_span, attr_span);
                    self.add_generated(format!("{attr}="), MethodKind::Attribute, false, call_span, attr_span);
                }
                if let Some(v) = args.keyword("accessors") {
                    for (attr, attr_span) in literal_keys(v) {
                        self.add_generated(attr.clone(), MethodKind::Attribute, false, call_span, attr_span);
                        self.add_generated(format!("{attr}="), MethodKind::Attribute, false, call_span, attr_span);
                    }
                }
            }
            "has_one_attached" | "has_many_attached" | "has_rich_text" => {
                if let Some((attr, attr_span)) = args.positional.first().and_then(symbol_or_string) {
                    let names: Vec<String> = match name {
                        "has_one_attached" => {
                            vec![attr.clone(), format!("{attr}="), format!("{attr}_attachment"), format!("{attr}_blob")]
                        }
                        "has_many_attached" => vec![
                            attr.clone(),
                            format!("{attr}="),
                            format!("{attr}_attachments"),
                            format!("{attr}_blobs"),
                        ],
                        _ => vec![attr.clone(), format!("{attr}="), format!("rich_text_{attr}")],
                    };
                    for n in names {
                        self.add_generated(n, MethodKind::Attribute, false, call_span, attr_span);
                    }
                }
            }
            "has_secure_password" => {
                let attr =
                    args.positional.first().and_then(symbol_or_string).map(|(a, _)| a).unwrap_or("password".into());
                let at = span(&node.message_loc().unwrap_or_else(|| node.location()));
                for n in
                    [attr.clone(), format!("{attr}="), format!("{attr}_confirmation="), format!("authenticate_{attr}")]
                {
                    self.add_generated(n, MethodKind::Attribute, false, call_span, at);
                }
                if attr == "password" {
                    self.add_generated("authenticate".into(), MethodKind::Attribute, false, call_span, at);
                }
            }
            "primary_abstract_class" => self.file.abstract_classes.push(scope as u32),
            _ => {}
        }
        false
    }

    /// `enum :status, { draft: 0, paid: 1 }, prefix: true` and the older
    /// `enum status: { draft: 0 }`.
    fn enum_call(&mut self, args: &Args<'_>, call_span: Span) {
        // (enum name, where the name is, its values)
        type EnumDef = (String, Span, Vec<(String, Span)>);
        let mut enums: Vec<EnumDef> = Vec::new();
        if let Some((name, name_span)) = args.positional.first().and_then(symbol_or_string) {
            let values = args.positional.get(1).map(literal_keys).unwrap_or_default();
            let values = if values.is_empty() {
                // `enum :status, draft: 0, paid: 1`
                args.keywords
                    .iter()
                    .filter(|(k, _, _)| {
                        !matches!(
                            k.as_str(),
                            "prefix" | "suffix" | "default" | "scopes" | "validate" | "instance_methods"
                        )
                    })
                    .map(|(k, s, _)| (k.clone(), *s))
                    .collect()
            } else {
                values
            };
            enums.push((name, name_span, values));
        } else {
            for (k, s, v) in &args.keywords {
                if !k.starts_with('_') && !matches!(k.as_str(), "prefix" | "suffix" | "default" | "scopes" | "validate")
                {
                    enums.push((k.clone(), *s, literal_keys(v)));
                }
            }
        }
        let affix = |key: &str, enum_name: &str| -> Option<String> {
            let v = args.keyword(key).or_else(|| args.keyword(&format!("_{key}")))?;
            if v.as_true_node().is_some() {
                return Some(enum_name.to_string());
            }
            symbol_or_string(v).map(|(s, _)| s)
        };
        for (enum_name, enum_span, values) in enums {
            let prefix = affix("prefix", &enum_name);
            let suffix = affix("suffix", &enum_name);
            let plural = crate::inflect::pluralize(&enum_name);
            self.add_generated(plural, MethodKind::Enum, true, call_span, enum_span);
            for (value, value_span) in values {
                let mut base = value.clone();
                if let Some(p) = &prefix {
                    base = format!("{p}_{base}");
                }
                if let Some(s) = &suffix {
                    base = format!("{base}_{s}");
                }
                self.add_generated(format!("{base}?"), MethodKind::Enum, false, call_span, value_span);
                self.add_generated(format!("{base}!"), MethodKind::Enum, false, call_span, value_span);
                self.add_generated(base.clone(), MethodKind::Enum, true, call_span, value_span);
                self.add_generated(format!("not_{base}"), MethodKind::Enum, true, call_span, value_span);
            }
        }
    }

    /// `Foo = Class.new(Bar) do ... end`, `Struct.new`, `Data.define` and
    /// `Module.new` make a new class or module. Returns true when handled.
    fn const_write_of_class(&mut self, path: ConstPath, write_span: Span, name_span: Span, value: &Node<'_>) -> bool {
        let Some(call) = value.as_call_node() else { return false };
        let Some(recv) = call.receiver().as_ref().and_then(const_path) else { return false };
        let method = text(call.name().as_slice());
        let kind = match (recv.joined().as_str(), method.as_str()) {
            ("Class", "new") | ("Struct", "new") | ("Data", "define") => ScopeKind::Class,
            ("Module", "new") => ScopeKind::Module,
            _ => return false,
        };
        let args = Args::of(&call);
        let superclass = match recv.joined().as_str() {
            "Class" => args.positional.first().and_then(const_path),
            "Struct" => Some(ConstPath { absolute: false, segments: vec!["Struct".into()] }),
            "Data" => Some(ConstPath { absolute: false, segments: vec!["Data".into()] }),
            _ => None,
        };
        // References in the receiver and the arguments belong to the outer scope.
        if let Some(r) = call.receiver() {
            self.visit(&r);
        }
        if let Some(a) = call.arguments() {
            self.visit(&a.as_node());
        }
        let id = self.new_scope(kind, path, write_span, name_span);
        self.file.scopes[id as usize].superclass = superclass;
        let frame = self.class_frame(id, write_span);
        // `Struct.new(:a, :b)` makes readers and writers for each member.
        if recv.joined() == "Struct" || recv.joined() == "Data" {
            self.frames.push(frame);
            let call_span = span(&call.location());
            for (member, member_span) in args.symbols() {
                self.add_generated(member.clone(), MethodKind::AttrReader, false, call_span, member_span);
                if recv.joined() == "Struct" {
                    self.add_generated(format!("{member}="), MethodKind::AttrWriter, false, call_span, member_span);
                }
            }
            self.frames.pop();
        }
        if let Some(block) = call.block().and_then(|b| b.as_block_node()) {
            self.visit_body(block.body(), frame);
        }
        true
    }

    // ---- RSpec, FactoryBot, i18n, render ---------------------------------

    /// Records an RSpec group or example and returns its index.
    fn spec_block(&mut self, node: &CallNode<'_>, name: &str) -> Option<u32> {
        let block = node.block()?.as_block_node()?;
        let kind = if SPEC_GROUPS.contains(&name) {
            SpecBlockKind::Group
        } else if SPEC_SHARED.contains(&name) {
            SpecBlockKind::Shared
        } else if SPEC_EXAMPLES.contains(&name) {
            SpecBlockKind::Example
        } else {
            return None;
        };
        let receiver_ok = match node.receiver() {
            None => true,
            Some(r) => const_path(&r).is_some_and(|p| p.joined() == "RSpec"),
        };
        if !receiver_ok {
            return None;
        }
        let args = Args::of(node);
        let described = if kind == SpecBlockKind::Group { args.positional.first().and_then(const_path) } else { None };
        if kind == SpecBlockKind::Shared
            && let Some((shared, shared_span)) = args.positional.first().and_then(symbol_or_string)
        {
            self.file.shared_groups.push(NamedDef {
                name: shared,
                span: span(&node.location()),
                name_span: shared_span,
            });
        }
        let idx = self.file.spec_blocks.len() as u32;
        self.file.spec_blocks.push(SpecBlock {
            parent: self.spec_stack.last().copied(),
            kind,
            span: span(&block.location()),
            scope: self.frame().scope,
            described,
        });
        Some(idx)
    }

    /// `let`, `subject`, shared example refs, factories, traits, `t(...)`,
    /// `render "x"`, `send(:x)` and Stimulus helper calls.
    fn special_call(&mut self, node: &CallNode<'_>, name: &str, receiver: &Option<Node<'_>>) {
        let implicit = receiver.is_none();
        let recv_const = receiver.as_ref().and_then(const_path).map(|p| p.joined());
        let args = Args::of(node);
        match name {
            "let"
            | "let!"
            | "subject"
            | "subject!"
            | "let_it_be"
            | "let_it_be_with_reload"
            | "let_it_be_with_refind"
                if implicit =>
            {
                let (let_name, name_span) = match args.positional.first().and_then(symbol_or_string) {
                    Some(n) => n,
                    None if name.starts_with("subject") => {
                        ("subject".to_string(), span(&node.message_loc().unwrap_or_else(|| node.location())))
                    }
                    None => return,
                };
                let value = node
                    .block()
                    .and_then(|b| b.as_block_node())
                    .and_then(|b| last_statement(b.body()))
                    .map(|n| self.expr_of(&n))
                    .unwrap_or(Expr::Unknown);
                self.file.lets.push(LetDef {
                    block: self.spec_stack.last().copied(),
                    name: let_name,
                    span: span(&node.location()),
                    name_span,
                    value,
                });
            }
            _ if SPEC_SHARED_REFS.contains(&name) && implicit => {
                if let Some((shared, shared_span)) = args.positional.first().and_then(symbol_or_string) {
                    self.add_ref(shared_span, RefKind::SharedGroup { name: shared });
                }
            }
            "factory" if self.in_factory_define > 0 && implicit => {
                if let Some((factory, factory_span)) = args.positional.first().and_then(symbol_or_string) {
                    if let Some(v) = args.keyword("class")
                        && let Some((cn, cn_span)) = string(v)
                    {
                        self.add_ref(cn_span, RefKind::ClassName { name: cn });
                    }
                    self.file.factories.push(FactoryDef {
                        name: factory,
                        class_name: args.keyword_text("class"),
                        parent: args.keyword_text("parent").or_else(|| self.factory_stack.last().cloned()),
                        span: span(&node.location()),
                        name_span: factory_span,
                    });
                }
            }
            "trait" if self.in_factory_define > 0 && implicit => {
                if let Some((t, t_span)) = args.positional.first().and_then(symbol_or_string) {
                    self.file.traits.push(TraitDef {
                        factory: self.factory_stack.last().cloned(),
                        name: t,
                        span: span(&node.location()),
                        name_span: t_span,
                    });
                }
            }
            _ if FACTORY_CALLS.contains(&name)
                && (implicit || recv_const.as_deref() == Some("FactoryBot"))
                && args.positional.first().is_some_and(|a| a.as_symbol_node().is_some()) =>
            {
                let (factory, factory_span) = symbol(&args.positional[0]).expect("checked above");
                self.add_ref(factory_span, RefKind::Factory { name: factory.clone() });
                for arg in &args.positional[1..] {
                    if let Some((t, t_span)) = symbol(arg) {
                        self.add_ref(t_span, RefKind::Trait { factory: factory.clone(), name: t });
                    }
                }
            }
            "t" | "translate" | "t!" | "translate!" if implicit || recv_const.as_deref() == Some("I18n") => {
                if let Some((key, key_span)) = args.positional.first().and_then(string) {
                    let method = self.frame().method.map(|m| self.file.methods[m as usize].name.clone());
                    self.add_ref(key_span, RefKind::I18n { key, method });
                }
            }
            "render" if implicit => {
                if let Some((path, path_span)) = args.positional.first().and_then(string) {
                    self.add_ref(path_span, RefKind::Render { path });
                }
                for key in ["partial", "template", "layout"] {
                    if let Some(v) = args.keyword(key)
                        && let Some((path, path_span)) = string(v)
                    {
                        self.add_ref(path_span, RefKind::Render { path });
                    }
                }
            }
            "send"
            | "public_send"
            | "__send__"
            | "try"
            | "try!"
            | "respond_to?"
            | "method"
            | "define_singleton_method"
                if implicit || receiver.as_ref().is_some_and(|r| r.as_self_node().is_some()) =>
            {
                if let Some((m, m_span)) = args.positional.first().and_then(symbol) {
                    let singleton = self.frame().self_ctx == SelfCtx::Singleton;
                    self.add_ref(m_span, RefKind::MethodSymbol { name: m, singleton });
                }
            }
            "build_controller" | "stimulus_controller" if implicit => {
                if let Some(arg) = args.positional.first()
                    && let Some(s) = arg.as_string_node()
                {
                    let content = s.content_loc();
                    let refs = stimulus_tokens(content.as_slice(), content.start_offset(), false);
                    for (sp, kind) in refs {
                        self.add_ref(sp, kind);
                    }
                }
            }
            _ => {}
        }
        if let Some(v) = args.keyword("factory")
            && let Some((f, f_span)) = symbol(v)
        {
            self.add_ref(f_span, RefKind::Factory { name: f });
        }
        // `config.include Helpers` in `RSpec.configure do |config|`.
        if name == "include"
            && receiver
                .as_ref()
                .is_some_and(|r| r.as_local_variable_read_node().is_some_and(|l| l.name().as_slice() == b"config"))
            && let Some(path) = args.positional.first().and_then(const_path)
        {
            self.file.rspec_includes.push(path);
        }
    }
}

/// Joins alternatives, dropping unknown ones and flattening nested unions.
fn union(parts: Vec<Expr>) -> Expr {
    let mut out = Vec::new();
    for p in parts {
        match p {
            Expr::Unknown => {}
            Expr::Union(inner) => out.extend(inner),
            other => out.push(other),
        }
    }
    match out.len() {
        0 => Expr::Unknown,
        1 => out.pop().expect("one item"),
        _ => Expr::Union(out),
    }
}

/// The class that runs an `ActiveSupport.on_load(hook)` block.
fn load_hook_class(hook: &str) -> Option<&'static str> {
    Some(match hook {
        "active_record" => "ActiveRecord::Base",
        "active_record_postgresqladapter" => "ActiveRecord::ConnectionAdapters::PostgreSQLAdapter",
        "active_record_mysql2adapter" => "ActiveRecord::ConnectionAdapters::Mysql2Adapter",
        "active_record_sqlite3adapter" => "ActiveRecord::ConnectionAdapters::SQLite3Adapter",
        "action_controller" | "action_controller_base" => "ActionController::Base",
        "action_controller_api" => "ActionController::API",
        "action_view" => "ActionView::Base",
        "action_mailer" => "ActionMailer::Base",
        "active_job" => "ActiveJob::Base",
        "action_cable" => "ActionCable::Server::Base",
        "action_cable_channel" => "ActionCable::Channel::Base",
        "action_cable_connection" => "ActionCable::Connection::Base",
        "active_storage_blob" => "ActiveStorage::Blob",
        "active_storage_attachment" => "ActiveStorage::Attachment",
        "action_text_rich_text" => "ActionText::RichText",
        "action_mailbox" => "ActionMailbox::Base",
        "active_support_test_case" => "ActiveSupport::TestCase",
        "action_dispatch_integration_test" => "ActionDispatch::IntegrationTest",
        "action_dispatch_request" => "ActionDispatch::Request",
        "action_dispatch_response" => "ActionDispatch::Response",
        "active_model_translation" => "ActiveModel::Translation",
        _ => return None,
    })
}

impl<'pr> Visit<'pr> for Extractor<'_> {
    fn visit_class_node(&mut self, node: &ClassNode<'pr>) {
        // The name and the superclass are looked up in the outer scope.
        self.visit(&node.constant_path());
        if let Some(sup) = node.superclass() {
            self.visit(&sup);
        }
        let Some(path) = const_path(&node.constant_path()) else {
            self.visit_body(node.body(), self.frame());
            return;
        };
        let whole = span(&node.location());
        let id = self.new_scope(ScopeKind::Class, path, whole, span(&node.constant_path().location()));
        self.file.scopes[id as usize].superclass = node.superclass().as_ref().and_then(const_path);
        let frame = self.class_frame(id, whole);
        self.visit_body(node.body(), frame);
    }

    fn visit_module_node(&mut self, node: &ModuleNode<'pr>) {
        self.visit(&node.constant_path());
        let Some(path) = const_path(&node.constant_path()) else {
            self.visit_body(node.body(), self.frame());
            return;
        };
        let whole = span(&node.location());
        let id = self.new_scope(ScopeKind::Module, path, whole, span(&node.constant_path().location()));
        let frame = self.class_frame(id, whole);
        self.visit_body(node.body(), frame);
    }

    fn visit_singleton_class_node(&mut self, node: &SingletonClassNode<'pr>) {
        let whole = span(&node.location());
        if node.expression().as_self_node().is_some() {
            let scope = self.frame().scope;
            let mut frame = self.class_frame(scope, whole);
            frame.in_singleton_class = true;
            self.visit_body(node.body(), frame);
        } else if let Some(path) = const_path(&node.expression()) {
            // `class << Time`
            self.visit(&node.expression());
            let name_span = span(&node.expression().location());
            let id = self.push_eval_scope(path, whole, name_span);
            let mut frame = self.class_frame(id, whole);
            frame.in_singleton_class = true;
            self.visit_body(node.body(), frame);
        } else {
            self.visit(&node.expression());
            self.visit_body(node.body(), self.frame());
        }
    }

    fn visit_def_node(&mut self, node: &DefNode<'pr>) {
        let f = self.frame();
        let singleton = match node.receiver() {
            Some(_) => true,
            None => f.in_singleton_class,
        };
        let name = text(node.name().as_slice());
        let params = self.params_text(node);
        let whole = span(&node.location());
        let lscope = self.push_lscope(whole);
        let mut frame = f;
        frame.self_ctx = if singleton { SelfCtx::Singleton } else { SelfCtx::Instance };
        frame.in_method = true;
        frame.lscope = lscope;
        // The return value is read inside the method's own local scope.
        self.frames.push(frame);
        let returns = last_statement(node.body()).map(|n| self.expr_of(&n));
        self.frames.pop();
        let id = self.add_method(name, MethodKind::Def, singleton, whole, span(&node.name_loc()), params, returns);
        frame.method = Some(id);
        self.frames.push(frame);
        if let Some(p) = node.parameters() {
            self.add_params(&p);
        }
        ruby_prism::visit_def_node(self, node);
        self.frames.pop();
    }

    fn visit_call_node(&mut self, node: &CallNode<'pr>) {
        let name = text(node.name().as_slice());
        let receiver = node.receiver();

        if let Some(msg) = node.message_loc() {
            let recv = match &receiver {
                None => Receiver::Implicit,
                Some(r) if r.as_self_node().is_some() => Receiver::SelfRef,
                Some(r) => match const_path(r) {
                    Some(p) => Receiver::Const(p),
                    None => Receiver::Expr(self.expr_of(r)),
                },
            };
            // Operators such as `a + b` or `a[0]` have no name to click.
            let first = msg.as_slice().first().copied().unwrap_or(b' ');
            if first == b'_' || first.is_ascii_alphabetic() || first >= 0x80 {
                self.add_ref(span(&msg), RefKind::Call { name: name.clone(), receiver: recv });
            }
        }

        // `self.table_name = "x"`, `self.abstract_class = true`
        if receiver.as_ref().is_some_and(|r| r.as_self_node().is_some()) && !self.frame().in_method {
            let args = Args::of(node);
            let scope = self.frame().scope;
            match name.as_str() {
                "table_name=" => {
                    if let Some((t, _)) = args.positional.first().and_then(string) {
                        self.file.table_names.push((scope, t));
                    }
                }
                "abstract_class=" if args.positional.first().is_some_and(|a| a.as_true_node().is_some()) => {
                    self.file.abstract_classes.push(scope);
                }
                _ => {}
            }
        }

        self.special_call(node, &name, &receiver);

        if receiver.is_none() && !self.frame().in_method && self.class_body_call(node, &name) {
            return;
        }
        if matches!(name.as_str(), "class_eval" | "class_exec" | "module_eval" | "module_exec")
            && let (Some(path), Some(block)) =
                (receiver.as_ref().and_then(const_path), node.block().and_then(|b| b.as_block_node()))
        {
            if let Some(r) = &receiver {
                self.visit(r);
            }
            let name_span = span(&receiver.as_ref().map(|r| r.location()).unwrap_or_else(|| node.location()));
            let id = self.push_eval_scope(path, span(&node.location()), name_span);
            let lscope = self.frame().lscope;
            self.visit_body(block.body(), Frame::body(id, SelfCtx::Singleton, lscope));
            return;
        }
        // `ActiveSupport.on_load(:active_record) do ... end` runs the block
        // inside the class that the hook names.
        if name == "on_load" {
            let target = node
                .arguments()
                .and_then(|a| a.arguments().iter().next())
                .and_then(|a| symbol_or_string(&a))
                .and_then(|(hook, _)| load_hook_class(&hook));
            if let (Some(target), Some(block)) = (target, node.block().and_then(|b| b.as_block_node())) {
                if let Some(r) = &receiver {
                    self.visit(r);
                }
                let path = ConstPath { absolute: true, segments: target.split("::").map(str::to_string).collect() };
                let name_span = span(&node.message_loc().unwrap_or_else(|| node.location()));
                let id = self.push_eval_scope(path, span(&node.location()), name_span);
                let lscope = self.frame().lscope;
                self.visit_body(block.body(), Frame::body(id, SelfCtx::Singleton, lscope));
                return;
            }
        }
        // `Fiber.attr_accessor :x`, `Foo.include(Bar)`: a class body call
        // sent to a constant.
        if !self.frame().in_method
            && matches!(
                name.as_str(),
                "attr_reader"
                    | "attr_writer"
                    | "attr_accessor"
                    | "include"
                    | "extend"
                    | "prepend"
                    | "define_method"
                    | "alias_method"
            )
            && let Some(path) = receiver.as_ref().and_then(const_path)
        {
            if let Some(r) = &receiver {
                self.visit(r);
            }
            let name_span = span(&receiver.as_ref().map(|r| r.location()).unwrap_or_else(|| node.location()));
            let id = self.push_eval_scope(path, span(&node.location()), name_span);
            let lscope = self.frame().lscope;
            self.frames.push(Frame::body(id, SelfCtx::Singleton, lscope));
            let visited = self.class_body_call(node, &name);
            self.frames.pop();
            if !visited && let Some(args) = node.arguments() {
                self.visit(&args.as_node());
            }
            return;
        }

        // `included do ... end` in a concern.
        if name == "included"
            && receiver.is_none()
            && !self.frame().in_method
            && let Some(block) = node.block().and_then(|b| b.as_block_node())
            && self.file.scopes[self.frame().scope as usize].kind == ScopeKind::Module
        {
            let parent = self.frame().scope;
            let cm = self.push_scope(RawScope {
                parent: Some(parent),
                kind: ScopeKind::ClassMethodsBlock,
                path: ConstPath { absolute: false, segments: vec!["ClassMethods".into()] },
                span: span(&node.location()),
                name_span: span(&node.message_loc().unwrap_or_else(|| node.location())),
                superclass: None,
                mixins: Vec::new(),
                is_concern: false,
                doc: None,
            });
            let mut frame = self.frame();
            frame.in_block = true;
            frame.included_cm = Some(cm);
            self.visit_body(block.body(), frame);
            return;
        }
        // `FactoryBot.define do`, `factory :x do`, RSpec groups.
        let factory_define = name == "define"
            && receiver
                .as_ref()
                .and_then(const_path)
                .is_some_and(|p| matches!(p.joined().as_str(), "FactoryBot" | "FactoryGirl"));
        let factory_block = (name == "factory" && self.in_factory_define > 0)
            .then(|| Args::of(node).symbols().first().map(|(n, _)| n.clone()))
            .flatten();
        let spec = self.spec_block(node, &name);
        if factory_define {
            self.in_factory_define += 1;
        }
        if let Some(f) = &factory_block {
            self.factory_stack.push(f.clone());
        }
        if let Some(s) = spec {
            self.spec_stack.push(s);
        }
        if node.block().is_some_and(|b| b.as_block_node().is_some()) {
            let recv_expr = match &receiver {
                Some(r) => self.expr_of(r),
                None => Expr::SelfRef,
            };
            if let Some(r) = &receiver {
                self.visit(r);
            }
            if let Some(a) = node.arguments() {
                self.visit(&a.as_node());
            }
            self.pending_block = Some((recv_expr, name.clone()));
            if let Some(b) = node.block() {
                self.visit(&b);
            }
            self.pending_block = None;
        } else {
            ruby_prism::visit_call_node(self, node);
        }
        if spec.is_some() {
            self.spec_stack.pop();
        }
        if factory_block.is_some() {
            self.factory_stack.pop();
        }
        if factory_define {
            self.in_factory_define -= 1;
        }
    }

    fn visit_block_node(&mut self, node: &BlockNode<'pr>) {
        let mut frame = self.frame();
        frame.in_block = true;
        self.frames.push(frame);
        let pending = self.pending_block.take();
        if let Some(params) = node.parameters().and_then(|p| p.as_block_parameters_node()).and_then(|p| p.parameters())
        {
            for (i, p) in params.requireds().iter().enumerate() {
                if let Some(r) = p.as_required_parameter_node() {
                    let value = match &pending {
                        Some((recv, method)) => {
                            Expr::BlockParam { recv: Box::new(recv.clone()), method: method.clone(), index: i as u32 }
                        }
                        None => Expr::Unknown,
                    };
                    self.add_local(text(r.name().as_slice()), span(&r.location()), value);
                }
            }
        }
        ruby_prism::visit_block_node(self, node);
        self.frames.pop();
    }

    fn visit_super_node(&mut self, node: &SuperNode<'pr>) {
        if let Some(method) = self.frame().method {
            self.add_ref(span(&node.keyword_loc()), RefKind::Super { method });
        }
        ruby_prism::visit_super_node(self, node);
    }

    fn visit_forwarding_super_node(&mut self, node: &ForwardingSuperNode<'pr>) {
        if let Some(method) = self.frame().method {
            let start = node.location().start_offset();
            self.add_ref(Span::new(start, start + 5), RefKind::Super { method });
        }
        ruby_prism::visit_forwarding_super_node(self, node);
    }

    fn visit_alias_method_node(&mut self, node: &AliasMethodNode<'pr>) {
        let new = node.new_name();
        let old = node.old_name();
        if let (Some((new_name, new_span)), Some((old_name, old_span))) =
            (symbol_or_string(&new), symbol_or_string(&old))
            && !self.frame().in_method
        {
            self.add_ref(old_span, RefKind::MethodSymbol { name: old_name.clone(), singleton: false });
            self.add_alias(new_name, old_name, span(&node.location()), new_span);
        }
    }

    fn visit_constant_read_node(&mut self, node: &ConstantReadNode<'pr>) {
        let path = ConstPath { absolute: false, segments: vec![text(node.name().as_slice())] };
        self.add_ref(span(&node.location()), RefKind::Const(path));
    }

    fn visit_constant_path_node(&mut self, node: &ConstantPathNode<'pr>) {
        if let Some(path) = const_path(&node.as_node()) {
            self.add_ref(span(&node.name_loc()), RefKind::Const(path));
        }
        ruby_prism::visit_constant_path_node(self, node);
    }

    fn visit_constant_write_node(&mut self, node: &ConstantWriteNode<'pr>) {
        let path = ConstPath { absolute: false, segments: vec![text(node.name().as_slice())] };
        let write_span = span(&node.location());
        let name_span = span(&node.name_loc());
        let value = node.value();
        if self.const_write_of_class(path.clone(), write_span, name_span, &value) {
            return;
        }
        let scope = self.frame().scope;
        self.file.const_writes.push(RawConstWrite {
            scope,
            path,
            span: write_span,
            name_span,
            alias_of: const_path(&value),
        });
        self.visit(&value);
    }

    fn visit_constant_path_write_node(&mut self, node: &ConstantPathWriteNode<'pr>) {
        let target = node.target();
        let value = node.value();
        let Some(path) = const_path(&target.as_node()) else {
            ruby_prism::visit_constant_path_write_node(self, node);
            return;
        };
        if let Some(parent) = target.parent() {
            self.visit(&parent);
        }
        let write_span = span(&node.location());
        let name_span = span(&target.name_loc());
        if self.const_write_of_class(path.clone(), write_span, name_span, &value) {
            return;
        }
        let scope = self.frame().scope;
        self.file.const_writes.push(RawConstWrite {
            scope,
            path,
            span: write_span,
            name_span,
            alias_of: const_path(&value),
        });
        self.visit(&value);
    }

    fn visit_local_variable_write_node(&mut self, node: &LocalVariableWriteNode<'pr>) {
        let value = self.expr_of(&node.value());
        self.add_local(text(node.name().as_slice()), span(&node.name_loc()), value);
        ruby_prism::visit_local_variable_write_node(self, node);
    }

    fn visit_local_variable_or_write_node(&mut self, node: &LocalVariableOrWriteNode<'pr>) {
        let value = self.expr_of(&node.value());
        self.add_local(text(node.name().as_slice()), span(&node.name_loc()), value);
        ruby_prism::visit_local_variable_or_write_node(self, node);
    }

    fn visit_local_variable_read_node(&mut self, node: &LocalVariableReadNode<'pr>) {
        let lscope = self.frame().lscope;
        self.add_ref(span(&node.location()), RefKind::Local { name: text(node.name().as_slice()), lscope });
    }

    fn visit_instance_variable_write_node(&mut self, node: &InstanceVariableWriteNode<'pr>) {
        let value = self.expr_of(&node.value());
        self.add_ivar(text(node.name().as_slice()), span(&node.name_loc()), value);
        ruby_prism::visit_instance_variable_write_node(self, node);
    }

    fn visit_instance_variable_or_write_node(&mut self, node: &InstanceVariableOrWriteNode<'pr>) {
        let value = self.expr_of(&node.value());
        self.add_ivar(text(node.name().as_slice()), span(&node.name_loc()), value);
        ruby_prism::visit_instance_variable_or_write_node(self, node);
    }

    fn visit_instance_variable_read_node(&mut self, node: &InstanceVariableReadNode<'pr>) {
        self.add_ref(span(&node.location()), RefKind::Ivar { name: text(node.name().as_slice()) });
    }

    /// Stimulus names in hashes: `data: { controller: "tabs", action:
    /// "click->tabs#open", tabs_target: "panel" }`.
    fn visit_assoc_node(&mut self, node: &AssocNode<'pr>) {
        if let Some((key, key_span)) = symbol_or_string(&node.key()) {
            let value = node.value();
            let key_norm = key.trim_start_matches("data-").trim_start_matches("data_");
            if let Some(s) = value.as_string_node() {
                let content = s.content_loc();
                let is_action = key_norm == "action";
                if key_norm == "controller" || is_action {
                    for (sp, kind) in stimulus_tokens(content.as_slice(), content.start_offset(), is_action) {
                        self.add_ref(sp, kind);
                    }
                }
            }
            // `tabs_target:` / `"tabs-target"` / `tabs_url_value:`: the key names a controller.
            for suffix in ["_target", "-target", "_outlet", "-outlet"] {
                if let Some(ctrl) = key_norm.strip_suffix(suffix)
                    && !ctrl.is_empty()
                {
                    let ctrl_dashed = ctrl.replace('_', "-");
                    let start = key_span.start + (key.len() - key_norm.len()) as u32;
                    let sp = Span { start, end: start + ctrl.len() as u32 };
                    self.add_ref(sp, RefKind::Stimulus { controller: ctrl_dashed, action: None });
                    break;
                }
            }
        }
        ruby_prism::visit_assoc_node(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erb_keeps_offsets() {
        let src = b"<div><%= link_to user.name, x %>\n<% if a %>b<% end %></div>";
        let ruby = erb_to_ruby(src);
        assert_eq!(ruby.len(), src.len());
        let s = String::from_utf8(ruby).unwrap();
        assert!(s.contains("link_to user.name, x ;"));
        assert_eq!(&s[src.iter().position(|b| *b == b'l').unwrap()..][..7], "link_to");
    }

    #[test]
    fn stimulus_action_tokens() {
        let refs = stimulus_tokens(b"click->tabs#select keyup->admin--search#run:prevent", 0, true);
        let kinds: Vec<_> = refs.iter().map(|(_, k)| format!("{k:?}")).collect();
        assert_eq!(refs.len(), 4, "{kinds:?}");
    }
}
