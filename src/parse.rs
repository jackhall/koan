//! Turns Koan source text into a sequence of `KExpression`s in two passes: the [`sexlex`] crate
//! reads the text into a layout tree of atoms, strings, commas and groups, and [`lower`] gives
//! that tree koan's meaning. The three entry points below are the entire public surface;
//! submodules are private.
//!
//! See [design/expressions-and-parsing.md](../design/expressions-and-parsing.md).

mod atom;
mod brace;
mod lower;
mod operators;

use std::rc::Rc;

use crate::machine::KError;
use crate::machine::model::ast::KExpression;
use crate::machine::model::labels::LabelInterner;
use crate::memory::ProgramBrand;
use crate::source::{self, CurrentFileGuard, FileId, SourceFile};

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
