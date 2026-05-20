use crate::builtins::default_scope;
use crate::machine::{KError, RuntimeArena};
use super::scheduler::Scheduler;
use crate::parse::{parse, parse_with_path};

/// Parse Koan source and run it on a fresh `RuntimeArena`; all values allocated by the
/// program die when this returns.
pub fn interpret(source: &str) -> Result<(), KError> {
    interpret_with_writer(source, Box::new(std::io::stdout()))
}

/// `interpret` with a caller-supplied writer for `PRINT` output. Source is
/// registered under the synthetic path `<input>`; use [`interpret_with_writer_path`]
/// to surface a real filename in error frames.
pub fn interpret_with_writer(
    source: &str,
    out: Box<dyn std::io::Write>,
) -> Result<(), KError> {
    interpret_with_writer_path(source, None, out)
}

/// `interpret` with both a caller-supplied writer and an optional filename for
/// the source registry. `None` falls back to `<input>`.
pub fn interpret_with_writer_path(
    source: &str,
    path: Option<&str>,
    out: Box<dyn std::io::Write>,
) -> Result<(), KError> {
    let exprs = match path {
        Some(p) => parse_with_path(source, p)?,
        None => parse(source)?,
    };
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
