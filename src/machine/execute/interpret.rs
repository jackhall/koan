//! The program entry points. [`interpret`] and its writer-carrying siblings parse Koan source,
//! stand up program storage and a fresh run region with its root [`Scope`], establish the run
//! frame, seed the builtins against that frame's type registry, then drive the whole program
//! through [`KoanRuntime::run_program`]. All values allocated by the program die when these return.

use super::KoanRuntime;
use crate::builtins::{seed_builtins, unseeded_scopes};
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::model::{LabelInterner, RunRegistries};
use crate::machine::{KError, Scope, WriteGate};
use crate::parse::{parse, parse_with_path};

/// The run-root seeding door. The run-global root is unreachable by any node until the program
/// starts, so builtin registration is a construction-time write. `crate::builtins` cannot mint a
/// [`WriteGate`] itself, so the seed takes one as a parameter rather than helping itself to a
/// write verb, and this is where that gate is minted.
pub(crate) fn seed_run_root<'a>(root: &'a Scope<'a>, registries: &RunRegistries) {
    seed_builtins(root, registries, &mut WriteGate::for_unpublished_scope());
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
    // Declared before the run region so it is created first and released last — the whole run
    // reads AST nodes out of it.
    let program = program_storage();
    // The run's interner, created before parse so the parser is the site that populates it, and
    // handed to the runtime below to become the run frame's own.
    let labels = LabelInterner::new();
    let exprs = match path {
        Some(p) => parse_with_path(program.brand(), &labels, source, p)?,
        None => parse(program.brand(), &labels, source)?,
    };
    // The run region lives inside an `Rc<FrameStorage>` so the run-root scope has an owning handle:
    // both root-edge wiring and a top-level-defined FN's captured region upgrade
    // `Scope::region_owner` and expect it to be live.
    let run_storage = run_root_storage();
    let (root, top) = unseeded_scopes(&run_storage);
    let mut runtime = KoanRuntime::with_labels(program.brand(), out, labels);
    // The run frame adopts `top`, the same scope `run_program` dispatches top-level statements
    // against. Establishing it before seeding puts the builtins in the run's own type registry.
    runtime.ensure_run_frame(top);
    let registries = runtime
        .registries()
        .expect("run frame was just established");
    seed_run_root(root, registries);
    runtime.run_program(top, exprs)
}

#[cfg(test)]
mod tests;
