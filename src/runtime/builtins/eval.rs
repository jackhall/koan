use std::rc::Rc;

use crate::runtime::model::{KObject, KType};
use crate::runtime::machine::{ArgumentBundle, BodyResult, CallArena, KError, KErrorKind, Scope, SchedulerHandle};

use super::{arg, err, kw, register_builtin, sig};

/// `EVAL <expr:Any>` — surface form `$(expr)`. Resolves `expr` to a value and, if that value
/// is a `KObject::KExpression`, dispatches the captured AST in a fresh per-call frame
/// (mirroring MATCH's `CallArena` shape, not the caller's scope). Strict on non-`KExpression`
/// values — anything else is a `TypeMismatch`.
///
/// The fresh-frame choice mirrors MATCH's per-call frame: the EVAL'd expression resolves
/// free names against the surrounding lexical scope (the call site is the new frame's
/// `outer`), but bindings introduced by the EVAL'd body don't leak. The same call-site Rc
/// chain that MATCH uses keeps the parent's per-call arena alive during the tail dispatch.
///
/// The parser desugars `$(expr)` to `(EVAL expr)` — see `expression_tree::build_tree`. The
/// `EVAL` head-keyword is not part of the documented surface; user code should always go
/// through the `$` sigil.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let rc = match bundle.args.get("expr") {
        Some(rc) => Rc::clone(rc),
        None => return err(KError::new(KErrorKind::MissingArg("expr".to_string()))),
    };
    let inner = match &*rc {
        KObject::KExpression(e) => e.clone(),
        other => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "expr".to_string(),
                expected: "KExpression".to_string(),
                got: other.ktype().name(),
            }));
        }
    };
    // Per-EVAL frame, modeled on MATCH: the child scope's `outer` is the call site, so free
    // names in the captured body resolve against the surrounding scope. Chain the call-
    // site's frame Rc onto the new frame so the parent's per-call arena stays alive while
    // the new frame's outer-scope pointer is in use. EVAL doesn't substitute parameters
    // into the captured body (no formal `it`-style binding the way MATCH has), so there's
    // no per-call arena re-borrow to set up.
    let frame: Rc<CallArena> = CallArena::new(scope, sched.current_frame());
    BodyResult::Tail { expr: inner, frame: Some(frame), function: None }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "EVAL",
        sig(KType::Any, vec![kw("EVAL"), arg("expr", KType::Any)]),
        body,
    );
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{parse_one, run, run_one_err, run_root_silent, run_root_with_buf};
    use crate::runtime::machine::{KErrorKind, RuntimeArena};

    fn run_program(source: &str) -> Vec<u8> {
        let arena = RuntimeArena::new();
        let (scope, captured) = run_root_with_buf(&arena);
        run(scope, source);
        let bytes = captured.borrow().clone();
        bytes
    }

    #[test]
    fn eval_of_quoted_literal() {
        let bytes = run_program("LET q = #(1)\nPRINT $(q)");
        assert_eq!(bytes, b"1\n");
    }

    #[test]
    fn eval_of_function_returning_kexpression() {
        // The user-fn returns a captured AST; EVAL pulls it out and runs it.
        let bytes = run_program(
            "FN (MAKE_AST) -> KExpression = (#(1))\n\
             PRINT $(MAKE_AST)",
        );
        assert_eq!(bytes, b"1\n");
    }

    #[test]
    fn eval_of_non_kexpression_errors_with_type_mismatch() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET x = 3");
        let err = run_one_err(scope, parse_one("$(x)"));
        assert!(
            matches!(&err.kind, KErrorKind::TypeMismatch { arg, expected, .. }
                if arg == "expr" && expected == "KExpression"),
            "expected TypeMismatch on EVAL of non-KExpression, got {err}",
        );
    }

    #[test]
    fn eval_runs_side_effects_in_quoted_body() {
        // `#(PRINT 1)` captures the PRINT AST; EVAL'ing it runs PRINT.
        let bytes = run_program("LET q = #(PRINT 1)\n$(q)");
        assert_eq!(bytes, b"1\n");
    }

    #[test]
    fn multiline_sigil_collapse_roundtrip() {
        // Indented sigil-led continuation lines (handled in `collapse_whitespace`) must
        // round-trip through the parser without tripping the sigil-adjacency rule.
        let bytes = run_program(
            "LET q =\n  #3\nPRINT $(q)",
        );
        assert_eq!(bytes, b"3\n");
    }

    #[test]
    fn eval_returns_inner_expression_value() {
        // EVAL's contract: the dispatch result is whatever the inner AST evaluates to. Here
        // PRINT itself returns the rendered string, so EVAL of a PRINT-quote returns that
        // same string — `print` writes `"1\n"` and the outer PRINT prints `"1"` again.
        let bytes = run_program("LET q = #(PRINT 1)\nPRINT $(q)");
        assert_eq!(bytes, b"1\n1\n");
    }

    #[test]
    fn recursive_eval_no_uaf() {
        // Mirrors `match_case::recursive_tagged_match_no_uaf` — the EVAL frame's child
        // scope's `outer` points into the enclosing user-fn's per-call arena. Without
        // chaining the call-site frame's Rc onto the new frame, dropping the enclosing
        // frame on TCO replace would free memory the EVAL frame still references.
        let bytes = run_program(
            "UNION Bit = (one: Null zero: Null)\n\
             FN (HOP b: Tagged) -> Any = (MATCH (b) WITH (\
                 one -> $(#(HOP (Bit (zero null))))\
                 zero -> (PRINT \"done\")\
             ))\n\
             HOP (Bit (one null))",
        );
        assert_eq!(bytes, b"done\n");
    }
}
