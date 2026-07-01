//! The program entry points. [`interpret`] and its writer-carrying siblings parse Koan source,
//! stand up a fresh [`KoanRegion`] and root [`Scope`], then drive the whole program through
//! [`KoanRuntime::run_program`] — the harness method that enters every top-level statement, runs
//! the scheduler to quiescence, and rejects a bare top-level expression that resolved to an
//! unstamped empty container. All values allocated by the program die when these return.

use super::{KoanRuntime, RegionRefFamily};
use crate::builtins::default_scope;
use crate::machine::core::FrameStorage;
use crate::machine::model::ast::KExpression;
use crate::machine::{FrameSet, KError, KErrorKind, Scope};
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
    // The run region lives inside an `Rc<FrameStorage>` so the run-root scope's region has an
    // owning handle: top-level-defined FNs resolve their captured-region owner to it (via
    // `Scope::region_owner`), and an escaping value bound at top level retains its per-call region on
    // this run-root frame (the drain below). `run_storage` outlives `root`, which borrows it.
    let run_storage = FrameStorage::run_root();
    let root = default_scope(&run_storage, out);
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
            // A root that reaches a per-call region (a non-empty witness set) is relocated into the
            // run-global root region carrying exactly those sources, so the root lives run-long and its
            // per-call frame is released. A fully-surviving root (empty witness) — already in a region
            // that outlives the run — and an errored terminal need no re-home.
            let pin = self.sched.dep_witness(id);
            if !pin.is_empty() {
                // Relocate into the surviving run region via the merge-form transfer: the spine is
                // copied there and the result re-sealed under the root's own reached sources. The dest
                // rides an empty-set `resident` carrier — the run region outlives everything and is
                // externally pinned, so it needs no held pin. Yoking the run-root frame here would
                // re-form a reference cycle into a drained value's witness, so the empty set is the
                // sound source. Dropping the per-call frame the producer kept the terminal in.
                if let Ok(witnessed) = self.relocate_terminal(
                    id,
                    Witnessed::<RegionRefFamily, FrameSet>::resident(root.brand()),
                ) {
                    // Deposit the rehomed terminal's reach (a returned closure's / module's captured
                    // regions, named on the carrier with the producer frame already dropped by the
                    // relocate) onto the run-root scope's reach-set. The run root lives in the run
                    // region that outlives the scheduler, so its reach-set keeps every such region
                    // alive past teardown — multi-region correct, read straight off the carrier for
                    // both channels (the type-channel module included now that it is witnessed).
                    root.fold_reach(witnessed.witness());
                    self.sched.rehome_terminal(id, Ok(witnessed));
                }
            }
        }
        // The scheduler is quiescent and every top-level statement has bound into the run root — seal
        // its reach-set at run end. The run root is run-global (never reopens), so this is its close.
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
