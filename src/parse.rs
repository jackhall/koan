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
//! See [parse/README.md](parse/README.md).

mod ast;
mod atom;
mod brace;
mod builtin_shapes;
mod depth;
mod error;
mod lower;
mod operators;

use std::rc::Rc;

use crate::memory::ProgramBrand;
use crate::source::{self, CurrentFileGuard, FileId, SourceFile};
use crate::symbols::SymbolInterner;

pub use depth::MAX_SYNTAX_DEPTH;
pub use error::ParseError;

/// The span wrapper a node's parts run carries — the type every construction door here takes, so a
/// caller building a node names it through `parse` rather than reaching past it.
pub use crate::source::Spanned;
pub use ast::{
    DispatchShape, ExpressionKey, ExpressionPart, KExpression, KLiteral, KeyElement, Mark,
    NodeCache, PartClass, ProgramExpression, ProgramNode, classify_dispatch_shape,
};
pub use builtin_shapes::binder::{
    BinderBucketFn, BinderNameFn, BinderSurface, BucketKeys, StoredBinderKey,
};
pub use builtin_shapes::role::{BodyKind, DefinitionKind, Heads, Opens, Reading, Role};
pub use builtin_shapes::{
    BUILTIN_SHAPES, BuiltinShape, BuiltinShapeId, ShapeElement, builtin_shape_for,
};

pub(crate) use ast::RunIter;
pub(crate) use builtin_shapes::KEYWORDS;
pub(crate) use builtin_shapes::binder::{
    DeclaredElement, OpArity, SlotLabel, bounded, declarator_parameters, declared_element,
    fn_def_binder_bucket, head_run, needed_entry, needed_key, needed_name, needing,
    next_is_type_slot, op_declaration_arity, op_def_binder_bucket, quantifier_entries, quoted_body,
    quoted_part, slot_label, symbol_from_quote_body,
};

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
/// [`CurrentFileGuard`] so [`ParseError::new`] sees the right file. A top-level expression nested
/// deeper than [`MAX_SYNTAX_DEPTH`] is refused. Every name and every parts run the
/// products hold is written into `program`'s store, so the caller owns the storage the whole AST
/// lives in.
pub fn parse_with_source<'a>(
    program: ProgramBrand<'a>,
    symbols: &SymbolInterner,
    id: FileId,
) -> Result<Vec<KExpression<'a>>, ParseError> {
    let _guard = CurrentFileGuard::push(id);
    let expressions = source::with(id, |f| lower::lower_source(program, symbols, &f.text, id))?;
    if let Some(deep) = expressions.iter().find(|e| e.depth() > MAX_SYNTAX_DEPTH) {
        return Err(ParseError::new(
            format!("syntax nests deeper than {MAX_SYNTAX_DEPTH}"),
            Some(deep.source.span),
        ));
    }
    Ok(expressions)
}
