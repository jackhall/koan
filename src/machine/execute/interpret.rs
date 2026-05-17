use crate::builtins::default_scope;
use crate::machine::{KError, KErrorKind, RuntimeArena};
use super::scheduler::Scheduler;
use crate::parse::parse;

/// Parse Koan source and run it. Each call constructs a fresh `RuntimeArena` and a per-run
/// scope tree (the default scope with builtins registered, allocated in that arena); every
/// value the program allocates lives in that arena and is dropped when this function returns.
/// The scheduler walks the AST itself — every top-level expression goes in as a single
/// `Dispatch` node bound to the run-root scope; the scheduler then handles nested
/// sub-expressions, list literals, and lazy slots dynamically as nodes execute.
///
/// Returns `Err(KError)` for parse failures (wrapped as `KError::ParseError`) and runtime
/// errors that bubble up to a top-level dispatch.
pub fn interpret(source: &str) -> Result<(), KError> {
    interpret_with_writer(source, Box::new(std::io::stdout()))
}

/// Same as `interpret` but lets the caller supply a writer for `PRINT` output. Tests use this
/// to capture `PRINT` into a buffer; the CLI uses the default-stdout `interpret`. Constructs
/// a fresh arena local to this call; everything the program allocates dies when this function
/// returns.
pub fn interpret_with_writer(
    source: &str,
    out: Box<dyn std::io::Write>,
) -> Result<(), KError> {
    let exprs = parse(source).map_err(|e| KError::new(KErrorKind::ParseError(e)))?;
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, out);
    let mut scheduler = Scheduler::new();
    let mut top_level: Vec<crate::machine::NodeId> = Vec::with_capacity(exprs.len());
    for expr in exprs {
        top_level.push(scheduler.add_dispatch(expr, root));
    }
    scheduler.execute()?;
    // After execute, scan the top-level dispatches for the first errored result and surface
    // it. Top-level dispatches share the run-root scope, so an error in expression N doesn't
    // prevent expression N+1's dispatch from being scheduled — but per the design, the
    // first error short-circuits the program's reported outcome.
    for id in top_level {
        if let Err(e) = scheduler.read_result(id) {
            return Err(e.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
