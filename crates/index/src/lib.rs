//! Rubyscope's Ruby index: parses Ruby files with Prism, joins them into
//! one table of namespaces, constants and methods, and resolves clicks.

pub mod cache;
pub mod discover;
pub mod extract;
pub mod index;
pub mod infer;
pub mod inflect;
pub mod link;
pub mod model;
pub mod query;
pub mod rails;
pub mod resolve;

pub use index::{BuildOptions, FileId, Index, Loc};
pub use infer::{Confidence, Ty};
pub use query::{Hover, OutlineItem, OutlineKind, SearchHit, Symbol, SymbolEntry, Usage};
pub use resolve::{Resolution, Target};
