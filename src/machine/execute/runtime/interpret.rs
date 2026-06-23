//! The program entry points. [`interpret`] and its writer-carrying siblings parse Koan source,
//! stand up a fresh [`KoanRegion`] and root [`Scope`], then drive the whole program through
//! [`KoanRuntime::run_program`] — the harness method that enters every top-level statement, runs
//! the scheduler to quiescence, and rejects a bare top-level expression that resolved to an
//! unstamped empty container. All values allocated by the program die when these return.

use super::KoanRuntime;
use crate::builtins::default_scope;
use crate::machine::execute::lift::NodeLift;
use crate::machine::model::ast::KExpression;
use crate::machine::{KError, KErrorKind, KoanRegion, Scope};
use crate::parse::{parse, parse_with_path};

/// Parse Koan source and run it on a fresh `KoanRegion`; all values allocated by the
/// program die when this returns.
pub fn interpret(source: &str) -> Result<(), KError> {
    interpret_with_writer(source, Box::new(std::io::stdout()))
}

/// `interpret` with a caller-supplied writer for `PRINT` output. Source is
/// registered under the synthetic path `<input>`; use [`interpret_with_writer_path`]
/// to surface a real filename in error frames.
pub fn interpret_with_writer(source: &str, out: Box<dyn std::io::Write>) -> Result<(), KError> {
    interpret_with_writer_path(source, None, out)
}

/// `None` for `path` falls back to `<input>`.
pub fn interpret_with_writer_path(
    source: &str,
    path: Option<&str>,
    out: Box<dyn std::io::Write>,
) -> Result<(), KError> {
    let exprs = match path {
        Some(p) => parse_with_path(source, p)?,
        None => parse(source)?,
    };
    let region = KoanRegion::new();
    let root = default_scope(&region, out);
    let mut runtime = KoanRuntime::new();
    runtime.run_program(root, exprs)
}

impl<'run> KoanRuntime<'run> {
    /// Drive a parsed program to completion: route each top-level statement through
    /// [`enter_block`](Self::enter_block) so it gets a root `LexicalFrame { scope_id: root.id,
    /// index: i, parent: None }` (every other dispatched node inherits from there via the cactus
    /// chain), run the scheduler to quiescence, then surface the first error or the
    /// untyped-resolution rejection below.
    pub(in crate::machine::execute) fn run_program(
        &mut self,
        root: &'run Scope<'run>,
        exprs: Vec<KExpression<'run>>,
    ) -> Result<(), KError> {
        let top_level = self.enter_block(root.id, exprs, root);
        self.execute()?;
        // Drain boundary: each top-level statement is a consumer-less root, so its terminal stays
        // pinned in the producer's per-call frame (a producer keeps its terminal in-frame and does
        // not lift at Done; each consumer pull-lifts instead). Lift each into the run region here so
        // the root lives run-long and its per-call frame is released. A frameless / run-region or
        // errored terminal needs no lift.
        for &id in &top_level {
            if let Ok((value, Some(frame))) = self.sched.read_result_with_frame(id) {
                // The scheduler hands back the value re-anchored to this `&self` borrow. A
                // consumer-less root has no pull-lift to node-scale it, so this `'run` re-home copies
                // it into the run-global root region via `lift` (which owns the read's re-anchor). The
                // lifted root is handed back live — the scheduler re-erases it for storage.
                let lifted = self.lift(value, &frame, root.region);
                self.sched.rehome_terminal(id, Ok(lifted));
            }
        }
        // A bare top-level expression is an untyped resolution boundary: an unstamped
        // empty `[]` / `{}` reaching it has no element type to infer, so reject rather
        // than silently resolve to `List<Any>` / `Dict<Any, Any>`.
        for id in top_level {
            match self.read_result(id) {
                Err(e) => return Err(e.clone()),
                Ok(value)
                    if value
                        .as_object()
                        .is_some_and(|o| o.is_unstamped_empty_container()) =>
                {
                    return Err(KError::new(KErrorKind::ShapeError(
                        "bare empty container has no element type to infer; annotate its \
                         type (e.g. via a typed FN return) or use a non-empty literal"
                            .to_string(),
                    )));
                }
                Ok(_) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
