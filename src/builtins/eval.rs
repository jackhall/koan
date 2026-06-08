use std::rc::Rc;

use crate::machine::model::KType;
use crate::machine::{ArgumentBundle, BodyResult, CallArena, SchedulerHandle, Scope};

use super::{arg, err, kw, register_builtin, sig};

/// `EVAL <expr:Any>` — surface form `$(expr)`. Dispatches the captured AST inside a
/// `KExpression` in a fresh per-call frame so bindings introduced by the body don't leak;
/// the call site is the new frame's `outer`, so free names resolve against the surrounding
/// scope. Non-`KExpression` values raise `TypeMismatch`.
///
/// The `EVAL` head-keyword is not part of the surface; user code goes through the `$` sigil.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a, 'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let inner = match bundle.require_kexpression("expr") {
        Ok(e) => e.clone(),
        Err(e) => return err(e),
    };
    // Chain the call-site's frame Rc onto the new frame so the parent's per-call arena
    // stays alive while the new frame's `outer`-scope pointer is in use.
    let frame: Rc<CallArena> = CallArena::new(scope, sched.current_frame());
    BodyResult::Tail {
        expr: inner,
        frame: Some(frame),
        function: None,
        block_entry: None,
        body_index: 0,
    }
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
    use crate::builtins::test_support::{
        parse_one, run, run_one_err, run_root_silent, run_root_with_buf,
    };
    use crate::machine::{KErrorKind, RuntimeArena};

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
        let bytes = run_program("LET q = #(PRINT 1)\n$(q)");
        assert_eq!(bytes, b"1\n");
    }

    #[test]
    fn multiline_sigil_collapse_roundtrip() {
        let bytes = run_program("LET q =\n  #3\nPRINT $(q)");
        assert_eq!(bytes, b"3\n");
    }

    #[test]
    fn eval_returns_inner_expression_value() {
        // PRINT returns the rendered string, so EVAL of a PRINT-quote prints once
        // (the inner PRINT) and the outer PRINT prints the returned string again.
        let bytes = run_program("LET q = #(PRINT 1)\nPRINT $(q)");
        assert_eq!(bytes, b"1\n1\n");
    }

    #[test]
    fn recursive_eval_no_uaf() {
        // Without chaining the call-site frame's Rc onto the new frame, dropping the
        // enclosing frame on TCO replace would free memory the EVAL frame still references
        // through its `outer` pointer.
        let bytes = run_program(
            "UNION Bit = (One :Null Zero :Null)\n\
             FN (HOP b :Any) -> Any = (MATCH (b) -> :Str WITH (\
                 One -> $(#(HOP (Bit (Zero null))))\
                 Zero -> (PRINT \"done\")\
             ))\n\
             HOP (Bit (One null))",
        );
        assert_eq!(bytes, b"done\n");
    }
}
