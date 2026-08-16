//! The program entry points. [`interpret`] and its writer-carrying siblings parse Koan source,
//! stand up a fresh [`KoanRegion`] and root [`Scope`], establish the run frame, seed the builtins
//! against that frame's type registry, then drive the whole program through
//! [`KoanRuntime::run_program`] — the harness method that enters every top-level statement, runs
//! the scheduler to quiescence, and rejects a bare top-level expression that resolved to an
//! unstamped empty container. All values allocated by the program die when these return.

use super::KoanRuntime;
use crate::builtins::{seed_builtins, unseeded_scopes};
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::model::TypeRegistry;
use crate::machine::{KError, Scope, WriteGate};
use crate::parse::{parse, parse_with_path};

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

#[cfg(test)]
mod tests;
