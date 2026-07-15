//! The program entry points. [`interpret`] and its writer-carrying siblings parse Koan source,
//! stand up a fresh [`KoanRegion`] and root [`Scope`], then drive the whole program through
//! [`KoanRuntime::run_program`] — the harness method that enters every top-level statement, runs
//! the scheduler to quiescence, and rejects a bare top-level expression that resolved to an
//! unstamped empty container. All values allocated by the program die when these return.

use super::{DestHandleFamily, KoanRuntime};
use crate::builtins::default_scope;
use crate::machine::core::run_root_storage;
use crate::machine::model::KExpression;
use crate::machine::{CarrierWitness, KError, KErrorKind, Scope};
use crate::parse::{parse, parse_with_path};
use crate::witnessed::Witnessed;

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
    // The run region lives inside an `Rc<FrameStorage>` so the run-root scope's region has an owning
    // handle: top-level-defined FNs resolve their captured-region owner to it (`Scope::region_owner`),
    // and an escaping value bound at top level retains its per-call region on this run-root frame (the
    // drain below).
    let run_storage = run_root_storage();
    let root = default_scope(&run_storage, out);
    let mut runtime = KoanRuntime::new();
    runtime.run_program(root, exprs)
}

impl<'run> KoanRuntime<'run> {
    /// Drive a parsed program to completion: enter each top-level statement as a root via
    /// [`enter_block`](Self::enter_block), run the scheduler to quiescence, then surface the
    /// first error or the untyped-resolution rejection below.
    pub(in crate::machine::execute) fn run_program(
        &mut self,
        root: &'run Scope<'run>,
        exprs: Vec<KExpression<'run>>,
    ) -> Result<(), KError> {
        let top_level = self.enter_block(root.id, exprs, root);
        self.execute()?;
        // Each top-level statement is a consumer-less root: its terminal stays pinned in the
        // producer's per-call frame, since no consumer ever pull-lifts it. Relocate every root that
        // reaches a per-call region into the run region so it lives run-long and its per-call frame
        // releases; a fully-surviving root (empty witness) and an errored terminal need no re-home.
        for &id in &top_level {
            let reaches_per_call = self
                .sched
                .dep_carrier(id)
                .is_ok_and(|sealed| !sealed.witness().is_empty());
            if reaches_per_call {
                // The dest rides an empty-set `resident`: the run region outlives everything and is
                // externally pinned, and yoking the run-root frame here would re-form a reference
                // cycle into the drained value's witness.
                if let Ok(witnessed) = self.relocate_terminal(
                    id,
                    Witnessed::<DestHandleFamily, CarrierWitness>::resident(root.brand().handle()),
                ) {
                    // Mint the rehomed terminal's reach into the run root's arena so those regions stay
                    // alive past scheduler teardown.
                    let _ = root.resident_reach_of(&witnessed);
                    self.sched.rehome_terminal(id, Ok(witnessed));
                }
            }
        }
        // Seal the run root's reach-set; it is run-global and never reopens.
        root.close();
        // A bare top-level expression is an untyped resolution boundary: an unstamped
        // empty `[]` / `{}` reaching it has no element type to infer, so reject rather
        // than silently resolve to `List<Any>` / `Dict<Any, Any>`.
        for id in top_level {
            // Copy out the empty-container verdict from inside the open — the carrier never escapes.
            let is_unannotated_empty = match self.read_result_with(id, |value| {
                value
                    .as_object()
                    .is_some_and(|o| o.is_unstamped_empty_container())
            }) {
                Err(e) => return Err(e.clone()),
                Ok(flag) => flag,
            };
            if is_unannotated_empty {
                return Err(KError::new(KErrorKind::ShapeError(
                    "bare empty container has no element type to infer; annotate its \
                     type (e.g. via a typed FN return) or use a non-empty literal"
                        .to_string(),
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
