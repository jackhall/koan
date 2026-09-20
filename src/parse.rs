//! The parser and what it produces: the symbol vocabulary every name is minted in, the syntax AST,
//! and the builtin shape table every node is classified against at construction.
//!
//! Source text becomes a sequence of [`KExpression`]s in two passes: the [`sexlex`] crate reads the
//! text into a layout tree of atoms, strings, commas and groups, and [`lower`] gives that tree
//! koan's meaning. The three entry points below are the entire text-to-AST surface; `atom`,
//! `brace`, `lower` and `operators` are private.
//!
//! [`crate::symbols`], [`ast`] and [`builtin_shapes`] are the vocabulary the products are written
//! in. A node
//! fills its structural cache at construction by probing
//! [`builtin_shapes::BUILTIN_SHAPES`], so every later reader — the dispatch driver, the scheduler's
//! laziness decision, the close-inference walk, the miss diagnosis — reads a cached fact rather
//! than re-walking the run.
//!
//! Outside `#[cfg(test)]` and doc comments this module reaches [`source`], [`crate::memory`],
//! [`crate::symbols`] and one name from [`crate::type_lattice`]:
//! [`KType`](crate::type_lattice::KType), whose builtin handles are `const` content digests, so a
//! builtin shape states its slots' types without a registry in hand. The lattice rests on
//! [`crate::symbols`] too, and on nothing here, so that edge runs one way. A failure is this
//! module's own [`ParseError`]. The runtime operations on the types here —
//! lowering a literal, resolving a part to a cell, installing a binder — are inherent impls in the
//! runtime, which imports them by name.
//!
//! The runtime is this module's consumer, and the runtime is `pending_rewrite`: an item marked
//! `cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))` — or an `unused_imports` twin on
//! a crate-visible re-export — has no caller in a default build until the rewrite adopts it, and
//! the marker comes off with the adoption.
//!
//! See [parse/README.md](parse/README.md).

pub mod ast;
pub mod builtin_shapes;

mod atom;
mod brace;
mod error;
mod lower;
mod operators;

use std::rc::Rc;

use crate::memory::ProgramBrand;
use crate::source::{self, CurrentFileGuard, FileId, SourceFile};
use crate::symbols::SymbolInterner;

pub use error::ParseError;

/// The span wrapper a node's parts run carries — the type every construction door here takes, so a
/// caller building a node names it through `parse` rather than reaching past it.
pub use crate::source::Spanned;
pub use ast::{
    DispatchShape, ExpressionKey, ExpressionPart, KExpression, KLiteral, KeyElement, NodeCache,
    PartClass, ProgramExpression, ProgramNode, classify_dispatch_shape,
};
pub use builtin_shapes::binder::{BinderBucketFn, BinderNameFn, BinderSurface, StoredBinderKey};
pub use builtin_shapes::lazy::LazyKinds;

#[cfg_attr(not(feature = "pending_rewrite"), allow(unused_imports))]
pub(crate) use builtin_shapes::binder::{
    OpArity, op_declaration_arity, symbol_from_parts, symbol_from_quote_body,
};
#[cfg_attr(not(feature = "pending_rewrite"), allow(unused_imports))]
pub(crate) use builtin_shapes::layout::SlotLayout;

#[cfg(test)]
mod tests;

/// Returns one `KExpression` per top-level line, built into `program`'s region and registering the
/// input under the synthetic path `<input>`. Use [`parse_with_path`] to supply a real path.
pub fn parse<'a>(
    program: ProgramBrand<'a>,
    symbols: &SymbolInterner,
    input: &str,
) -> Result<Vec<KExpression<'a>>, ParseError> {
    parse_with_path(program, symbols, input, "<input>")
}

/// [`parse`] variant that registers the source under a caller-supplied `path` so error frames
/// render real filenames.
pub fn parse_with_path<'a>(
    program: ProgramBrand<'a>,
    symbols: &SymbolInterner,
    input: &str,
    path: impl Into<Rc<str>>,
) -> Result<Vec<KExpression<'a>>, ParseError> {
    let id = source::register(SourceFile::new(path, input.to_string()));
    parse_with_source(program, symbols, id)
}

/// Parse against a pre-registered `SourceFile`. Installs `id` as the active `CURRENT_FILE` via
/// [`CurrentFileGuard`] so [`ParseError::new`] sees the right file. Every name and every parts run the
/// products hold is bumped into `program`'s region, so the caller owns the storage the whole AST
/// lives in.
pub fn parse_with_source<'a>(
    program: ProgramBrand<'a>,
    symbols: &SymbolInterner,
    id: FileId,
) -> Result<Vec<KExpression<'a>>, ParseError> {
    let _guard = CurrentFileGuard::push(id);
    source::with(id, |f| {
        lower::lower_source(program, symbols, &f.text, Some(id))
    })
}
