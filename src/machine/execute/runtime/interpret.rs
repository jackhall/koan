//! The program entry points. [`interpret`] and its writer-carrying siblings parse Koan source,
//! stand up a fresh [`KoanRegion`] and root [`Scope`], then drive the whole program through
//! [`KoanRuntime::run_program`] — the harness method that enters every top-level statement, runs
//! the scheduler to quiescence, and rejects a bare top-level expression that resolved to an
//! unstamped empty container. All values allocated by the program die when these return.

use super::KoanRuntime;
use crate::builtins::default_scope;
use crate::machine::core::FrameStorage;
use crate::machine::execute::lift::reached_frame;
use crate::machine::model::ast::KExpression;
use crate::machine::{FrameSet, KError, KErrorKind, Scope};
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
                // A closure / module read out of the scheduler as a top-level result (e.g. a returned
                // module the caller inspects) borrows back into its per-call region. The rehomed
                // terminal's witness pins that region only while its slot lives, so retain it on the
                // persistent run-root frame too — the read then survives the scheduler's teardown.
                if let (Some(home), Ok(value)) =
                    (root.region_owner().upgrade(), self.read_result(id))
                {
                    if let Some(reached) = reached_frame(value) {
                        home.retain(reached);
                    }
                }
                // Relocate into the surviving run region via the merge-form transfer: the spine is
                // copied there and the result re-sealed under the root's own reached sources (the run
                // region's `dest_witness` is empty — it outlives the run, so needs no held pin),
                // dropping the per-call frame the producer kept the terminal in.
                if let Ok(witnessed) = self.relocate_terminal(id, root.region, FrameSet::empty()) {
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
