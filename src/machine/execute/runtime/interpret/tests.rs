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
use crate::builtins::test_support::TestRun;
use crate::machine::core::{run_root_storage, FrameStorage};

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
    region: &'run Rc<FrameStorage>,
    captured: Rc<RefCell<Vec<u8>>>,
) -> TestRun<'run> {
    let mut test_run = TestRun::new(region, Box::new(SharedBuf(captured)));
    let root = test_run.scope;
    // The test parses into the run's own storage and crosses each statement into the scheduler,
    // exactly as `run_program` does with program storage.
    let exprs = parse(crate::builtins::test_support::program_brand(), source)
        .expect("parse should succeed");
    for expr in exprs {
        test_run.runtime.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(root.brand(), expr),
            root,
        );
    }
    test_run.runtime.execute().expect("program should run");
    test_run
}
