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
    use crate::machine::model::Carried;
    let expr = crate::try_action!(require_kexpression(ctx.args, "QUOTE", "expr"));
    let obj = ctx.scope.region.alloc_object(KObject::KExpression(expr));
    Action::Done(Ok(Carried::Object(obj)))
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
    use crate::machine::KoanRegion;

    fn run_program(source: &str) -> Vec<u8> {
        let region = KoanRegion::new();
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
