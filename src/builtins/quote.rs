use crate::machine::model::{KObject, KType};
use crate::machine::Scope;

use super::{arg, kw, sig};

/// `QUOTE <expr:KExpression>` — surface form `#(expr)`, desugared by the parser
/// (see `expression_tree::build_tree`). Reads the unevaluated `expr` cell from `BodyCtx::args`
/// and returns it as a `KObject::KExpression` value with no evaluation, so raw ASTs can thread
/// through eager-evaluating contexts. The `QUOTE` head-keyword is not part of the documented
/// surface; user code goes through the `#` sigil.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{require_kexpression, Action};
    let expr = crate::try_action!(require_kexpression(ctx.args, "QUOTE", "expr"));
    // A quoted expression is raw, unevaluated AST, region-pure iff splice-free: a `Spliced` part is
    // a resolved value, not raw AST, and its cell carries a producer reach the empty (foreign-reach-
    // only) witnessed set could not name. `KExpression<'a>` is invariant with no `'static` rebuild,
    // so the audited twin runs the splice-free check as an always-on loud gate (rather than a
    // debug-only assert) before the value is stored.
    let carrier = ctx.scope.brand().alloc_object_witnessed_checked(
        KObject::KExpression(expr),
        |_region, o| matches!(o, KObject::KExpression(e) if e.is_splice_free()),
    );
    Action::Done(carrier)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::KExpression,
        vec![kw("QUOTE"), arg("expr", KType::KExpression)],
    );
    crate::builtins::register_builtin(scope, "QUOTE", signature, body);
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{run, run_root_with_buf};
    use crate::machine::core::run_root_storage;

    fn run_program(source: &str) -> Vec<u8> {
        let region = run_root_storage();
        let (scope, captured) = run_root_with_buf(&region);
        run(scope, source);
        let bytes = captured.borrow().clone();
        bytes
    }

    #[test]
    fn quote_then_eval_round_trip() {
        let bytes = run_program(
            "LET q = #(1)\n\
             PRINT $(q)",
        );
        assert_eq!(bytes, b"1\n");
    }

    #[test]
    fn quote_captures_ast_as_value() {
        let bytes = run_program("PRINT #(1)");
        assert_eq!(bytes, b"1\n");
    }

    /// A `Spliced` part is a resolved value, not raw AST — its cell carries a producer reach the
    /// empty (foreign-reach-only) witnessed seal `QUOTE`'s bare move-in uses cannot name.
    /// `is_splice_free`'s audit must reject it, and — since `alloc_object_witnessed_checked` is an
    /// always-on loud gate, not a `debug_assert!` — the rejection surfaces as a structured `KError`,
    /// not an assertion failure or a silently-stored dangle.
    #[test]
    fn quote_with_spliced_part_is_rejected_not_stored() {
        use crate::machine::model::ast::{ExpressionPart, KExpression};
        use crate::machine::model::KObject;
        use crate::witnessed::{Delivered, Sealed};

        let storage = run_root_storage();
        let scope = crate::builtins::test_support::run_root_bare(&storage);

        let witnessed = scope
            .seal_fresh_object(KObject::Number(7.0))
            .expect("a bare Number borrows no region, so its checked seal cannot fail");
        let spliced = ExpressionPart::Spliced {
            cell: Delivered::hosted(Sealed::seal(witnessed), std::rc::Rc::clone(&storage)),
        };
        let expr = KExpression::new(vec![spliced.into()]);
        assert!(
            !expr.is_splice_free(),
            "a Spliced part makes the expression not splice-free"
        );

        let result = scope.brand().alloc_object_witnessed_checked(
            KObject::KExpression(expr),
            |_region, o| matches!(o, KObject::KExpression(e) if e.is_splice_free()),
        );
        assert!(
            result.is_err(),
            "a spliced quoted expression must be rejected, not silently stored"
        );
    }
}
