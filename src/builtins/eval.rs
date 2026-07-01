use std::rc::Rc;

use crate::machine::model::KType;
use crate::machine::{CallFrame, Scope};

use super::{arg, kw, sig};

/// `EVAL <expr:Any>` — surface form `$(expr)`. Reads the evaluated `expr` (must be a
/// `KExpression`) and tail-replaces into it in a fresh call-site frame (`FreshChild` — the UAF
/// guard) so bindings introduced by the body don't leak; the call site is the new frame's
/// `outer`, so free names resolve against the surrounding scope. Non-`KExpression` values raise
/// `TypeMismatch`.
///
/// The `EVAL` head-keyword is not part of the surface; user code goes through the `$` sigil.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::block_tail::{block_tail, BlockBody, BlockScope};
    use crate::machine::core::kfunction::action::{arg_object, Action, FramePlacement};
    use crate::machine::model::KObject;
    use crate::machine::{KError, KErrorKind};
    let inner = match arg_object(ctx.args, "expr") {
        Some(KObject::KExpression(e)) => e.clone(),
        Some(other) => {
            return Action::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "expr".to_string(),
                expected: "KExpression".to_string(),
                got: other.ktype().name(),
            })))
        }
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("expr".to_string())))),
    };
    // Chain the call-site frame Rc onto the new frame (keeps the parent region alive past the
    // new frame's `outer` pointer) — matching a normal call frame. The tail is the whole quoted
    // expression run in the fresh frame's own scope (`BlockScope::None`): no block push, no seed,
    // and — unlike an arm — no split, so a parenthesized group evaluates as one expression.
    let frame: Rc<CallFrame> = CallFrame::new(ctx.scope, ctx.frame.map(|f| f.storage_rc()));
    block_tail(
        FramePlacement::FreshChild { frame },
        BlockScope::None,
        None,
        BlockBody::Single(inner),
        None,
    )
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(KType::Any, vec![kw("EVAL"), arg("expr", KType::Any)]);
    crate::builtins::register_builtin(scope, "EVAL", signature, body);
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{
        parse_one, run, run_one_err, run_root_silent, run_root_with_buf,
    };
    use crate::machine::core::FrameStorage;
    use crate::machine::KErrorKind;

    fn run_program(source: &str) -> Vec<u8> {
        let region = FrameStorage::run_root();
        let (scope, captured) = run_root_with_buf(&region);
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
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
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
