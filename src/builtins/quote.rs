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
    // A quoted expression is raw, unevaluated AST — splice-free, so it references no other region.
    // It is therefore region-pure: the `KObject::KExpression` allocs through the witnessed object
    // surface born under the empty (foreign-reach-only) set, the active frame folded in at close. A
    // `Spliced` part is a resolved value, not raw AST, and its cell carries the producer reach the
    // empty set could not name, so the splice-free precondition is asserted.
    debug_assert!(
        expr.is_splice_free(),
        "QUOTE expr must be splice-free raw AST: a Spliced cell is a resolved value, not raw AST, \
         and carries a producer reach the region-pure witnessed alloc would mis-witness as empty"
    );
    let carrier = ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::KExpression(expr));
    Action::Done(Ok(carrier))
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
    use crate::machine::core::FrameStorage;

    fn run_program(source: &str) -> Vec<u8> {
        let region = FrameStorage::run_root();
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
}
