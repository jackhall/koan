use crate::machine::WriteGate;
use std::rc::Rc;

use crate::machine::model::KType;
use crate::machine::{CallFrame, Scope};

use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { expr } }

/// `EVAL <expr:Any>` — surface form `$(expr)`. Reads the evaluated `expr` (must be a
/// `KExpression`) and tail-replaces into it in a fresh call-site frame (`FreshChild` — the UAF
/// guard) so bindings introduced by the body don't leak; the call site is the new frame's
/// `outer`, so free names resolve against the surrounding scope. Non-`KExpression` values raise
/// `TypeMismatch`.
///
/// The `EVAL` head-keyword is not part of the surface; user code goes through the `$` sigil.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::model::KObject;
    use crate::machine::{Action, FramePlacement};
    use crate::machine::{BlockBody, BlockScope, NoSeed, block_tail};
    use crate::machine::{KError, KErrorKind};
    let inner = match ctx.args.object(&SLOTS.expr) {
        Some(KObject::KExpression(e)) => e.node(),
        Some(other) => {
            return Action::done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "expr".to_string(),
                expected: "KExpression".to_string(),
                got: other.ktype().name(ctx.registries),
            })));
        }
        None => return Action::done(Err(KError::new(KErrorKind::MissingArg("expr".to_string())))),
    };
    // Chain the call-site frame Rc onto the new frame (keeps the parent region alive past the
    // new frame's `outer` pointer) — matching a normal call frame. The tail is the whole quoted
    // expression run in the fresh frame's own scope (`BlockScope::None`): no block push, no seed,
    // and — unlike an arm — no split, so a parenthesized group evaluates as one expression.
    let frame: Rc<CallFrame> = CallFrame::new(ctx.scope);
    block_tail(
        ctx.scope.brand(),
        FramePlacement::FreshChild { frame },
        BlockScope::None,
        None::<NoSeed>,
        BlockBody::Single(inner),
        None,
        ctx.registries,
    )
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let signature = sig(
        KType::ANY,
        vec![
            kw(registries, "EVAL"),
            arg(registries, &SLOTS.expr, KType::ANY),
        ],
    );
    crate::builtins::register_builtin(scope, "EVAL", signature, body, registries, gate);
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::TestRun;
    use crate::machine::KErrorKind;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;

    fn run_program(source: &str) -> Vec<u8> {
        let program = program_storage();
        let region = run_root_storage();
        let (mut test_run, captured) = TestRun::with_buf(&program, &region);
        test_run.run(source);

        captured.borrow().clone()
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
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET x = 3");
        let err = test_run.run_one_err(test_run.parse_one("$(x)"));
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

    /// A spliced `LET` runs inside `EVAL`'s fresh frame and never reaches the
    /// enclosing scope — statement position or not, nothing installs outside.
    /// [roadmap/metaprogramming/eval-splices-in-place.md] owns the gap to the
    /// designed splice-in-place semantics.
    #[test]
    fn eval_spliced_let_is_frame_local() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("$(#(LET x = 5))");
        assert!(
            test_run.scope.lookup("x").is_none(),
            "a spliced LET must not bind in the scope enclosing the EVAL",
        );
    }

    /// A spliced `LET` in an eager argument position runs frame-local and yields
    /// its value — it does not hit the `NestedBinder` position check, because
    /// `EVAL` evaluates through its own frame, not through sub-dispatch
    /// submission. When [roadmap/metaprogramming/eval-splices-in-place.md] routes
    /// splices through submission, this position must error like hand-written
    /// source; this test pins the pre-splice-in-place behavior.
    #[test]
    fn eval_spliced_let_in_argument_position_runs_frame_local() {
        let bytes = run_program("PRINT $(#(LET x = 5))");
        assert_eq!(bytes, b"5\n");
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
