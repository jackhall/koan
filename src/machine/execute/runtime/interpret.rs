//! The program entry points. [`interpret`] and its writer-carrying siblings parse Koan source,
//! stand up a fresh [`KoanRegion`] and root [`Scope`], establish the run frame, seed the builtins
//! against that frame's type registry, then drive the whole program through
//! [`KoanRuntime::run_program`] — the harness method that enters every top-level statement, runs
//! the scheduler to quiescence, and rejects a bare top-level expression that resolved to an
//! unstamped empty container. All values allocated by the program die when these return.

use super::{DestHandleFamily, KoanRuntime};
use crate::builtins::{seed_builtins, unseeded_scopes};
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::model::TypeRegistry;
use crate::machine::model::{KExpression, WorkingExpression};
use crate::machine::{KError, KErrorKind, Scope, WriteGate};
use crate::parse::{parse, parse_with_path};
use crate::scheduler::EdgeId;

/// The run-root seeding door. The run-global root is unreachable by any node until the program
/// starts, so the builtin registration is a construction-time write: this mints the
/// [`WriteGate`] for it and threads that one gate through every `register_*` call the seed makes.
/// `crate::builtins` cannot mint one itself — which is exactly why the seeding entry point takes
/// the gate as a parameter rather than helping itself to a write verb.
pub(crate) fn seed_run_root<'a>(root: &'a Scope<'a>, types: &TypeRegistry) {
    seed_builtins(root, types, &mut WriteGate::for_unpublished_scope());
}

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
    // Program storage: where the source's text and its raw AST live. Declared before the run region
    // so it is created first and released last — the whole run reads nodes out of it. It sits at the
    // eternal tier, so a value pointing into it pins nothing, and `KExpression` is covariant, so a
    // node parsed here flows into `'run` code by ordinary subtyping.
    let program = program_storage();
    let exprs = match path {
        Some(p) => parse_with_path(program.brand(), source, p)?,
        None => parse(program.brand(), source)?,
    };
    // The run region lives inside an `Rc<FrameStorage>` so the run-root scope's region has an owning
    // handle: top-level-defined FNs resolve their captured-region owner to it (`Scope::region_owner`),
    // and an escaping value bound at top level retains its per-call region on this run-root frame (the
    // drain below).
    let run_storage = run_root_storage();
    let (root, top) = unseeded_scopes(&run_storage);
    let mut runtime = KoanRuntime::new(program.brand(), out);
    // The run frame adopts `top`: `run_program` dispatches top-level statements against it, and
    // `resolve_node_scope` requires pointer-equality between the run frame's scope and the
    // dispatch target. Establishing it before seeding means the builtins are registered against
    // the run's own registry — the only registry in the tree.
    runtime.ensure_run_frame(top);
    let types = runtime
        .type_registry()
        .expect("run frame was just established");
    seed_run_root(root, &types);
    runtime.run_program(top, exprs)
}

impl<'run> KoanRuntime<'run> {
    /// Drive a parsed program to completion: enter each top-level statement as a root via
    /// [`enter_block`](Self::enter_block), wire one root edge apiece, run the scheduler to
    /// quiescence through [`drive_roots`](Self::drive_roots), then release those edges — on the
    /// error path too, so no name outlives the run frame it was destined at.
    pub(in crate::machine::execute) fn run_program(
        &mut self,
        root: &'run Scope<'run>,
        exprs: Vec<KExpression<'run>>,
    ) -> Result<(), KError> {
        // Each top-level statement crosses into the scheduler here — one slice copy of its parts run
        // into the run region, the door every AST node enters dispatch through.
        let statements: Vec<WorkingExpression<'run>> = exprs
            .into_iter()
            .map(|expr| WorkingExpression::from_ast(root.brand(), expr))
            .collect();
        // The run's roots leave submission as edges. Each names the run frame's region as its
        // destination — where the drain below re-homes a root that reaches a per-call region — and
        // holding that owner across the install is the wiring-time proof the region is pinned. The
        // submit-time `NodeId`s are transient currency and go out of scope right here.
        let run_owner = root
            .region_owner()
            .upgrade()
            .expect("the run root's region owner is held for the whole run");
        let roots: Vec<EdgeId> = self
            .enter_block(root.id, statements, root)
            .into_iter()
            .map(|id| self.sched.install_edge(id, &run_owner).edge_id())
            .collect();
        let outcome = self.drive_roots(root, &roots);
        // Koan is the roots' owner, so koan releases them — before the harness (and with it the run
        // frame these edges name) tears down.
        for &edge in &roots {
            self.sched.release_edge(edge);
        }
        outcome
    }

    /// Run to quiescence, drain the roots into the run region, and rule on their resolution — the
    /// fallible middle of [`run_program`](Self::run_program), split out so every exit from it passes
    /// through the root-edge release.
    fn drive_roots(&mut self, root: &'run Scope<'run>, roots: &[EdgeId]) -> Result<(), KError> {
        self.execute()?;
        // Each top-level statement is a consumer-less root: its terminal stays pinned in the
        // producer's per-call frame, since no consumer ever pull-lifts it. Relocate every root that
        // reaches a per-call region into the run region so it lives run-long and its per-call frame
        // releases; a root whose whole reach is eternal storage — the run region itself — and an
        // errored terminal need no re-home, because nothing they name dies with a per-call frame.
        for &edge in roots {
            let reaches_per_call = self
                .sched
                .edge_delivered(edge)
                .is_ok_and(|delivered| delivered.open_at().pins_beyond_eternal());
            if reaches_per_call {
                // The dest rides an empty-set `resident`: the run region outlives everything and is
                // externally pinned, and yoking the run-root frame here would re-form a reference
                // cycle into the drained value's witness.
                // The relocation's own composition mints the rehomed terminal's reach into the run
                // root region, which is the act that retains it there — so those regions stay alive
                // past scheduler teardown with nothing folded here. The product envelope's coverage
                // is the transit copy, dropped by `rehome_terminal`: what the run region now holds
                // is the mint, not these pins.
                if let Ok(delivered) = self.relocate_terminal(
                    edge,
                    root.deliver_resident::<DestHandleFamily>(root.brand().handle()),
                ) {
                    self.sched.rehome_terminal_via_edge(edge, Ok(delivered));
                }
            }
        }
        // Seal the run root's reach-set; it is run-global and never reopens.
        root.close();
        // A bare top-level expression is an untyped resolution boundary: an unstamped
        // empty `[]` / `{}` reaching it has no element type to infer, so reject rather
        // than silently resolve to `List<Any>` / `Dict<Any, Any>`.
        for &edge in roots {
            // Copy out the empty-container verdict from inside the open — the carrier never escapes.
            let is_unannotated_empty = match self.sched.read_edge_result_with(edge, |value| {
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
