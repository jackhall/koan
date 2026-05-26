use crate::builtins::default_scope;
use crate::machine::{KError, RuntimeArena, SchedulerHandle};
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
    // Route top-level statements through `enter_block` so each gets a root
    // `LexicalFrame { scope_id: root.id, index: i, parent: None }`. Every other
    // dispatched node inherits from there (cactus chain).
    let top_level = scheduler.enter_block(root.id, exprs, root);
    scheduler.execute()?;
    // Top-level dispatches share the run-root scope and execute independently; surface
    // the first errored result as the program's outcome.
    //
    // Empty-container error rule: a bare top-level expression is an untyped resolution
    // boundary. An unstamped empty `[]` / `{}` reaching it has no element type to infer
    // and was never given one by an annotation — reject it rather than silently resolve to
    // `List<Any>` / `Dict<Any, Any>`.
    for id in top_level {
        match scheduler.read_result(id) {
            Err(e) => return Err(e.clone()),
            Ok(value) if value.is_unstamped_empty_container() => {
                return Err(KError::new(crate::machine::KErrorKind::ShapeError(
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

#[cfg(test)]
mod tests;
