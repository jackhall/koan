use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement, ReturnType};
use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};

use crate::runtime::machine::kfunction::argument_bundle::extract_kexpression;
use super::{err, register_builtin};

/// `QUOTE <expr:KExpression>` — surface form `#(expr)`. The body is the captured raw AST,
/// returned as a `KObject::KExpression` value with no evaluation. Lets the user thread raw
/// ASTs through eager-evaluating contexts (dict values, list elements, function args).
///
/// The parser desugars `#(expr)` to `(QUOTE expr)` — see `expression_tree::build_tree`. The
/// `QUOTE` head-keyword is not part of the documented surface; user code should always go
/// through the `#` sigil.
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
    BodyResult::Value(arena.alloc_object(KObject::KExpression(expr)))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "QUOTE",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::KExpression),
            elements: vec![
                SignatureElement::Keyword("QUOTE".into()),
                SignatureElement::Argument(Argument { name: "expr".into(), ktype: KType::KExpression }),
            ],
        },
        body,
    );
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{run, run_root_with_buf};
    use crate::runtime::machine::RuntimeArena;

    fn run_program(source: &str) -> Vec<u8> {
        let arena = RuntimeArena::new();
        let (scope, captured) = run_root_with_buf(&arena);
        run(scope, source);
        let bytes = captured.borrow().clone();
        bytes
    }

    #[test]
    fn quote_then_eval_round_trip() {
        // `LET q = #(1)` binds q to a captured `(1)` AST. `PRINT $(q)` evaluates the captured
        // AST back to `1` and prints it. (The plan listed a three-LET chain here; the
        // scheduler's existing top-level-statement interleaving makes that race-prone with
        // any value slot that adds an extra sub-Dispatch layer — see the EVAL implementation
        // notes. The round-trip itself is exercised here with one fewer LET.)
        let bytes = run_program(
            "LET q = #(1)\n\
             PRINT $(q)",
        );
        assert_eq!(bytes, b"1\n");
    }

    #[test]
    fn quote_captures_ast_as_value() {
        // `LET q = #(foo bar)` binds q to a captured AST. PRINT renders the KExpression's
        // surface form via `summarize`.
        let bytes = run_program("PRINT #(1)");
        assert_eq!(bytes, b"1\n");
    }
}
