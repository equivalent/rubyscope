//! Data that the indexer extracts from one file.
//!
//! Every position is a byte offset into the file. Names inside a file are
//! still "as written" (for example `Foo::Bar` inside `module A`); the
//! indexer turns them into full names later, when it knows every file.

use serde::{Deserialize, Serialize};

/// A byte range in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start: start as u32, end: end as u32 }
    }

    pub fn contains(&self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    /// True when `other` lies inside this span.
    pub fn covers(&self, other: Span) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// A constant path as written in the code: `Foo`, `Foo::Bar` or `::Foo`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConstPath {
    /// True for `::Foo`, which starts the lookup at the top level.
    pub absolute: bool,
    pub segments: Vec<String>,
}

impl ConstPath {
    pub fn joined(&self) -> String {
        self.segments.join("::")
    }

    /// Parses `Foo::Bar` or `::Foo` as written in a string.
    pub fn parse(s: &str) -> Option<ConstPath> {
        let absolute = s.starts_with("::");
        let segments: Vec<String> = s.trim_start_matches("::").split("::").map(str::to_string).collect();
        let valid = segments.iter().all(|seg| seg.chars().next().is_some_and(|c| c.is_ascii_uppercase()));
        valid.then_some(ConstPath { absolute, segments })
    }
}

impl std::fmt::Display for ConstPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.absolute {
            f.write_str("::")?;
        }
        f.write_str(&self.joined())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScopeKind {
    /// The top level of the file.
    Root,
    Class,
    Module,
    /// The block of `class_methods do ... end` in an `ActiveSupport::Concern`.
    /// Methods inside it belong to `<Module>::ClassMethods`, but constant
    /// lookup still uses the module around it, because a block does not
    /// open a new lexical scope.
    ClassMethodsBlock,
    /// The block of `Foo.class_eval do ... end` (also `class_exec`,
    /// `module_eval`, `module_exec`, `class << Foo`, `on_load`). Methods
    /// inside it belong to `Foo`. Like `ClassMethodsBlock`, it does not
    /// open a lexical scope.
    EvalBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MixinKind {
    Include,
    Extend,
    Prepend,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MixinTarget {
    Const(ConstPath),
    /// `extend self`
    SelfRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mixin {
    pub kind: MixinKind,
    pub target: MixinTarget,
}

/// A `class`, `module`, `Foo = Class.new` or `class_methods do` block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawScope {
    pub parent: Option<u32>,
    pub kind: ScopeKind,
    /// The name as written. Empty for `Root`.
    pub path: ConstPath,
    /// The whole definition, from `class` to `end`.
    pub span: Span,
    /// The constant path after `class` / `module`.
    pub name_span: Span,
    pub superclass: Option<ConstPath>,
    pub mixins: Vec<Mixin>,
    /// True when the body has `extend ActiveSupport::Concern`.
    pub is_concern: bool,
    /// The comment block right above the definition.
    pub doc: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MethodKind {
    Def,
    AttrReader,
    AttrWriter,
    DefineMethod,
    Alias,
    /// `has_many`, `belongs_to`, ...
    Association,
    /// `scope :name, -> { }`
    Scope,
    /// The methods that `enum` makes.
    Enum,
    /// `delegate :x, to: :y`
    Delegate,
    /// `class_attribute`, `mattr_accessor`, `cattr_accessor`, ...
    ClassAttribute,
    /// `attribute :x`, `store_accessor`, `has_one_attached`, ...
    Attribute,
    /// A database column from `db/schema.rb`.
    Column,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMethod {
    /// Index of the lexical scope that owns the method.
    pub scope: u32,
    /// True for `def self.x`, methods inside `class << self`, and the
    /// singleton copy that `module_function` makes.
    pub singleton: bool,
    pub name: String,
    pub kind: MethodKind,
    pub visibility: Visibility,
    pub span: Span,
    pub name_span: Span,
    /// The parameter list as written, for example `(invoice, force: false)`.
    pub params: String,
    /// For aliases: the name of the original method.
    pub alias_of: Option<String>,
    /// True when the method is defined inside a block, such as an RSpec
    /// `describe do ... end`. At the top level such a method does not
    /// belong to `Object`.
    pub in_block: bool,
    /// The comment block right above the definition.
    pub doc: Option<String>,
    /// What the method returns: its last expression.
    pub returns: Option<Expr>,
    /// For `delegate :x, to: :y`: the target (`y`, `@y` or `class`).
    pub delegate_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConstWrite {
    pub scope: u32,
    pub path: ConstPath,
    pub span: Span,
    pub name_span: Span,
    /// Set for `Foo = Bar`, so a lookup of `Foo::X` can follow to `Bar::X`.
    pub alias_of: Option<ConstPath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelfCtx {
    /// `self` is an instance of the scope's class or module.
    Instance,
    /// `self` is the class or module itself (class body, `def self.x`).
    Singleton,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lit {
    String,
    Symbol,
    Integer,
    Float,
    Array,
    Hash,
    Nil,
    True,
    False,
    Range,
    Regexp,
    Proc,
}

impl Lit {
    pub fn class_name(self) -> &'static str {
        match self {
            Lit::String => "String",
            Lit::Symbol => "Symbol",
            Lit::Integer => "Integer",
            Lit::Float => "Float",
            Lit::Array => "Array",
            Lit::Hash => "Hash",
            Lit::Nil => "NilClass",
            Lit::True => "TrueClass",
            Lit::False => "FalseClass",
            Lit::Range => "Range",
            Lit::Regexp => "Regexp",
            Lit::Proc => "Proc",
        }
    }
}

/// A small description of an expression, kept so that types can be
/// worked out later without parsing the file again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    Unknown,
    SelfRef,
    Lit(Lit),
    Const(ConstPath),
    /// A local variable read. `lscope` is the local scope (method, class
    /// body or file) and `offset` is where it is read.
    Local {
        name: String,
        lscope: u32,
        offset: u32,
    },
    /// A method parameter. Its type can only be guessed from its name.
    Param {
        name: String,
    },
    Ivar {
        name: String,
    },
    Call {
        recv: Option<Box<Expr>>,
        name: String,
    },
    /// A block parameter of `recv.method { |a, b| }`, `index` is its place.
    BlockParam {
        recv: Box<Expr>,
        method: String,
        index: u32,
    },
    /// One of several values: `a || b`, `if` / `else`, ternaries.
    Union(Vec<Expr>),
    /// `create(:invoice)`, `build_list(:invoice, 2)`: a FactoryBot call.
    Factory {
        name: String,
        many: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Receiver {
    /// `foo` with no receiver.
    Implicit,
    /// `self.foo`
    SelfRef,
    /// `Foo.bar` or `Foo::Bar.baz`
    Const(ConstPath),
    /// Any other expression.
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefKind {
    Const(ConstPath),
    Call {
        name: String,
        receiver: Receiver,
    },
    /// `super`, inside the method with this index in `RawFile::methods`.
    Super {
        method: u32,
    },
    /// A local variable read.
    Local {
        name: String,
        lscope: u32,
    },
    /// An instance variable read.
    Ivar {
        name: String,
    },
    /// A symbol or string that names a method of `self`'s class:
    /// `before_action :load`, `validate :check`, `if: :paid?`, `send(:x)`.
    MethodSymbol {
        name: String,
        singleton: bool,
    },
    /// The name of an association (`has_many :invoices`): its class.
    AssocClass {
        assoc: u32,
    },
    /// A class name in a string, such as `class_name: "Invoice"`.
    ClassName {
        name: String,
    },
    /// `t("a.b")`, `t(".title")`. `method` is the method it is in.
    I18n {
        key: String,
        method: Option<String>,
    },
    /// `render "x/y"` or `render partial: "x/y"`.
    Render {
        path: String,
    },
    /// `create(:invoice)`
    Factory {
        name: String,
    },
    /// `create(:invoice, :paid)`
    Trait {
        factory: String,
        name: String,
    },
    /// `it_behaves_like "x"`, `include_context "x"`
    SharedGroup {
        name: String,
    },
    /// `data-controller="tabs"` or `data-action="click->tabs#open"`.
    Stimulus {
        controller: String,
        action: Option<String>,
    },
}

/// A place in the code that a click can resolve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawRef {
    /// The clickable text: the constant name, or the method name.
    pub span: Span,
    /// The lexical scope the reference is in.
    pub scope: u32,
    pub self_ctx: SelfCtx,
    pub kind: RefKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssocKind {
    BelongsTo,
    HasOne,
    HasMany,
    HasAndBelongsToMany,
}

impl AssocKind {
    pub fn is_collection(self) -> bool {
        matches!(self, AssocKind::HasMany | AssocKind::HasAndBelongsToMany)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawAssociation {
    pub scope: u32,
    pub kind: AssocKind,
    pub name: String,
    pub class_name: Option<String>,
    pub through: Option<String>,
    pub polymorphic: bool,
    pub span: Span,
    pub name_span: Span,
}

/// A local scope: a method body, a class body or the file. Blocks share
/// the local scope they are in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalScope {
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAssign {
    pub lscope: u32,
    pub name: String,
    /// Where the name is written.
    pub name_span: Span,
    pub value: Expr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IvarAssign {
    /// The lexical scope that owns the instance variable.
    pub scope: u32,
    pub singleton: bool,
    pub name: String,
    pub name_span: Span,
    pub value: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecBlockKind {
    /// `describe`, `context`, `feature`
    Group,
    /// `shared_examples`, `shared_context`, `shared_examples_for`
    Shared,
    /// `it`, `specify`, `scenario`, `example`
    Example,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecBlock {
    pub parent: Option<u32>,
    pub kind: SpecBlockKind,
    pub span: Span,
    /// The lexical scope the block is in.
    pub scope: u32,
    /// `describe Invoice do`: the described class.
    pub described: Option<ConstPath>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LetDef {
    /// Index into `RawFile::spec_blocks`, or none at the file level.
    pub block: Option<u32>,
    pub name: String,
    pub span: Span,
    pub name_span: Span,
    pub value: Expr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedDef {
    pub name: String,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactoryDef {
    pub name: String,
    pub class_name: Option<String>,
    pub parent: Option<String>,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraitDef {
    /// The factory the trait is in; none for a global trait.
    pub factory: Option<String>,
    pub name: String,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FileKind {
    #[default]
    Ruby,
    /// An ERB template. The Ruby inside the tags is parsed in place.
    Erb,
    /// Any other text file: YAML, JS, CSS... Only lines are known.
    Text,
}

/// Everything extracted from one file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawFile {
    pub kind: FileKind,
    pub line_starts: Vec<u32>,
    pub scopes: Vec<RawScope>,
    pub methods: Vec<RawMethod>,
    pub const_writes: Vec<RawConstWrite>,
    pub refs: Vec<RawRef>,
    pub associations: Vec<RawAssociation>,
    pub local_scopes: Vec<LocalScope>,
    pub locals: Vec<LocalAssign>,
    pub ivars: Vec<IvarAssign>,
    /// `self.table_name = "x"` in a class body.
    pub table_names: Vec<(u32, String)>,
    /// Scopes with `self.abstract_class = true` or `primary_abstract_class`.
    pub abstract_classes: Vec<u32>,
    pub spec_blocks: Vec<SpecBlock>,
    pub lets: Vec<LetDef>,
    pub shared_groups: Vec<NamedDef>,
    pub factories: Vec<FactoryDef>,
    pub traits: Vec<TraitDef>,
    /// `config.include X` in an `RSpec.configure` block.
    pub rspec_includes: Vec<ConstPath>,
    pub parse_errors: u32,
}

impl RawFile {
    /// Returns the 0-based line and the 0-based byte column of an offset.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (line as u32, offset - self.line_starts[line])
    }

    /// The 0-based line of an offset.
    pub fn line_of(&self, offset: u32) -> u32 {
        self.line_col(offset).0
    }

    /// Only the line starts, for a file that is not Ruby.
    pub fn text(source: &[u8]) -> RawFile {
        RawFile { kind: FileKind::Text, line_starts: line_starts(source), ..Default::default() }
    }
}

/// Byte offsets where each line starts. The first line starts at 0.
pub fn line_starts(source: &[u8]) -> Vec<u32> {
    let mut starts = vec![0];
    for (i, b) in source.iter().enumerate() {
        if *b == b'\n' {
            starts.push(i as u32 + 1);
        }
    }
    starts
}
