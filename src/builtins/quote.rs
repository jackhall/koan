use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, SchedulerHandle, Scope};

use super::{arg, err, kw, register_builtin, sig};
use crate::machine::core::kfunction::argument_bundle::extract_kexpression;

/// `QUOTE <expr:KExpression>` — surface form `#(expr)`, desugared by the parser
/// (see `expression_tree::build_tree`). Returns the captured AST as a
/// `KObject::KExpression` with no evaluation, so raw ASTs can thread through
/// eager-evaluating contexts. The `QUOTE` head-keyword is not part of the
/// documented surface; user code goes through the `#` sigil.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let expr = match extract_kexpression(&mut bundle, "expr") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "QUOTE expects a parenthesized expression body".to_string(),
            )));
        }
    };
    let arena = scope.arena;
    BodyResult::Value(arena.alloc(KObject::KExpression(expr)))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "QUOTE",
        sig(
            KType::KExpression,
            vec![kw("QUOTE"), arg("expr", KType::KExpression)],
        ),
        body,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{run, run_root_with_buf};
    use crate::machine::RuntimeArena;

    fn run_program(source: &str) -> Vec<u8> {
        let arena = RuntimeArena::new();
        let (scope, captured) = run_root_with_buf(&arena);
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
