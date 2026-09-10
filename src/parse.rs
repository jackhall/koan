//! The parser and what it produces: the label vocabulary every symbol is minted in, the syntax AST,
//! and the form table every node is classified against at construction.
//!
//! Source text becomes a sequence of [`KExpression`]s in two passes: the [`sexlex`] crate reads the
//! text into a layout tree of atoms, strings, commas and groups, and [`lower`] gives that tree
//! koan's meaning. The three entry points below are the entire text-to-AST surface; `atom`,
//! `brace`, `lower` and `operators` are private.
//!
//! [`labels`], [`ast`] and [`forms`] are the vocabulary the products are written in. A node fills
//! its structural cache at construction by probing [`forms::FORMS`], so every later reader — the
//! dispatch driver, the scheduler's laziness decision, the close-inference walk, the miss
//! diagnosis — reads a cached fact rather than re-walking the run.
//!
//! Outside `#[cfg(test)]` and doc comments this module reaches only [`source`], [`crate::memory`]
//! and [`KError`]. The runtime operations on the types here — lowering a literal, resolving a part
//! to a cell, installing a binder — are inherent impls under
//! [`machine::model`](crate::machine::model), which imports them by name.
//!
//! See [design/expressions-and-parsing.md](../design/expressions-and-parsing.md) and
//! [design/label-interning.md](../design/label-interning.md).

pub mod ast;
pub mod forms;
pub mod labels;

mod atom;
mod brace;
mod lower;
mod operators;

use std::rc::Rc;

use crate::machine::core::KError;
use crate::memory::ProgramBrand;
use crate::source::{self, CurrentFileGuard, FileId, SourceFile};

pub use ast::{
    DispatchShape, ExpressionPart, KExpression, KLiteral, KeyElement, NodeCache, PartClass,
    ProgramExpression, ProgramNode, UntypedKey,
};
pub use labels::{
    BindKind, BinderSymbol, ClassifiedSymbol, IdentityBuildHasher, IdentityHasher, KeywordSymbol,
    LabelInterner, StaticName, Symbol, TypeSymbol, ValueSymbol, WILDCARD, is_keyword_token,
    is_type_name, snake_case_identifier, wrong_binder_class,
};

#[cfg(test)]
mod tests;

/// Returns one `KExpression` per top-level line, built into `program`'s region and registering the
/// input under the synthetic path `<input>`. Use [`parse_with_path`] to supply a real path.
pub fn parse<'a>(
    program: ProgramBrand<'a>,
    labels: &LabelInterner,
    input: &str,
) -> Result<Vec<KExpression<'a>>, KError> {
    parse_with_path(program, labels, input, "<input>")
}

/// [`parse`] variant that registers the source under a caller-supplied `path` so error frames
/// render real filenames.
pub fn parse_with_path<'a>(
    program: ProgramBrand<'a>,
    labels: &LabelInterner,
    input: &str,
    path: impl Into<Rc<str>>,
) -> Result<Vec<KExpression<'a>>, KError> {
    let id = source::register(SourceFile::new(path, input.to_string()));
    parse_with_source(program, labels, id)
}

/// Parse against a pre-registered `SourceFile`. Installs `id` as the active `CURRENT_FILE` via
/// [`CurrentFileGuard`] so `KError::parse` sees the right file. Every name and every parts run the
/// products hold is bumped into `program`'s region, so the caller owns the storage the whole AST
/// lives in.
pub fn parse_with_source<'a>(
    program: ProgramBrand<'a>,
    labels: &LabelInterner,
    id: FileId,
) -> Result<Vec<KExpression<'a>>, KError> {
    let _guard = CurrentFileGuard::push(id);
    source::with(id, |f| {
        lower::lower_source(program, labels, &f.text, Some(id))
    })
}
