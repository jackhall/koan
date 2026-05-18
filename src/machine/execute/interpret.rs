use crate::builtins::default_scope;
use crate::machine::{KError, KErrorKind, RuntimeArena};
use super::scheduler::Scheduler;
use crate::parse::parse;

/// Parse Koan source and run it on a fresh `RuntimeArena`; all values allocated by the
/// program die when this returns.
pub fn interpret(source: &str) -> Result<(), KError> {
    interpret_with_writer(source, Box::new(std::io::stdout()))
}

/// `interpret` with a caller-supplied writer for `PRINT` output.
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
    // Top-level dispatches share the run-root scope and execute independently; surface
    // the first errored result as the program's outcome.
    for id in top_level {
        if let Err(e) = scheduler.read_result(id) {
            return Err(e.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
