//! Tests for the interpret/execute pipeline, split by surface:
//!
//! - [`basics`] — LET, MATCH, nested expressions, list literals.
//! - [`dict`] — dict literal integration, scalar keys, sub-expression keys/values.
//! - [`errors`] — KError surfacing (unbound name, dispatch failure, frame chain).
//! - [`tagged`] — tagged-union construction via TYPE tokens and LET-bound types.

mod basics;
mod dict;
mod errors;
mod tagged;

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use super::*;
use crate::machine::Scope;

pub(super) struct SharedBuf(Rc<RefCell<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run `source` and return the root scope so callers can inspect post-run bindings;
/// PRINT output lands in `captured`.
pub(super) fn run<'run>(
    source: &str,
    arena: &'run RuntimeArena,
    captured: Rc<RefCell<Vec<u8>>>,
) -> &'run Scope<'run> {
    let exprs = parse(source).expect("parse should succeed");
    let root = default_scope(arena, Box::new(SharedBuf(captured)));
    let mut scheduler = KoanHarness::new();
    for expr in exprs {
        scheduler.add_dispatch(expr, root);
    }
    scheduler.execute().expect("program should run");
    root
}
