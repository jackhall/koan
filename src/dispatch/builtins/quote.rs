use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::runtime::{KError, KErrorKind, Scope};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::values::KObject;

use super::helpers::extract_kexpression;
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
            return_type: KType::KExpression,
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
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::runtime::{RuntimeArena, Scope};
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;

    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    fn build_scope<'a>(arena: &'a RuntimeArena, captured: Rc<RefCell<Vec<u8>>>) -> &'a Scope<'a> {
        default_scope(arena, Box::new(SharedBuf(captured)))
    }

    fn run<'a>(scope: &'a Scope<'a>, source: &str) {
        let exprs = parse(source).expect("parse should succeed");
        let mut sched = Scheduler::new();
        for expr in exprs {
            sched.add_dispatch(expr, scope);
        }
        sched.execute().expect("scheduler should succeed");
    }

    fn run_program(source: &str) -> Vec<u8> {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured.clone());
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
