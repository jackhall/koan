use std::rc::Rc;

use crate::dispatch::kerror::{KError, KErrorKind};
use crate::dispatch::kfunction::{
    Argument, ArgumentBundle, BodyResult, ExpressionSignature, KType, SchedulerHandle,
    SignatureElement,
};
use crate::dispatch::kobject::KObject;
use crate::dispatch::ktraits::Parseable;
use crate::dispatch::scope::Scope;

use super::{err, register_builtin};

/// `<verb:Identifier> <args:KExpression>` — invokes a function bound to `verb` in scope by
/// applying it to the positional parts of `args`. Surface syntax: `f (a b c)` where `f` is
/// an Identifier whose binding is a `KObject::KFunction`. `verb` is resolved via
/// `Scope::lookup_kfunction`; `KFunction::apply` weaves the function's signature keywords
/// back in and returns a `BodyResult::Tail` that the scheduler re-dispatches against the
/// keyword-bucketed signature. Errored cases (verb unbound, bound to a non-function, args
/// slot misshapen) return structured `KError` variants the CLI reports verbatim.
///
/// Body intentionally thin: the synthesis logic lives on [`KFunction::apply`] alongside the
/// rest of "how to call a function," keeping this builtin a clean dispatch consumer rather
/// than a peer that pokes at signature internals.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // The Identifier slot resolves through `ExpressionPart::resolve` to a KString carrying
    // the identifier text — same shape as PRINT et al. observe for Identifier-typed slots.
    let verb = match bundle.get("verb") {
        Some(KObject::KString(s)) => s.clone(),
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "KString".to_string(),
                got: other.summarize(),
            }));
        }
        None => {
            return err(KError::new(KErrorKind::MissingArg("verb".to_string())));
        }
    };
    let args_expr = match bundle.args.remove("args") {
        Some(rc) => match Rc::try_unwrap(rc) {
            Ok(KObject::KExpression(e)) => e,
            Ok(_) => {
                return err(KError::new(KErrorKind::ShapeError(
                    "call_by_name args slot resolved to a non-KExpression".to_string(),
                )));
            }
            Err(rc) => match &*rc {
                KObject::KExpression(e) => e.clone(),
                _ => {
                    return err(KError::new(KErrorKind::ShapeError(
                        "call_by_name args slot resolved to a non-KExpression (shared)"
                            .to_string(),
                    )));
                }
            },
        },
        None => {
            return err(KError::new(KErrorKind::MissingArg("args".to_string())));
        }
    };
    match scope.lookup_kfunction(&verb) {
        Some(f) => f.apply(args_expr.parts),
        None => {
            // Distinguish "unbound" (no name in scope) from "bound to a non-function." The
            // first is UnboundName; the second is TypeMismatch on the verb's resolved value.
            match scope.lookup(&verb) {
                None => err(KError::new(KErrorKind::UnboundName(verb))),
                Some(obj) => err(KError::new(KErrorKind::TypeMismatch {
                    arg: "verb".to_string(),
                    expected: "KFunction".to_string(),
                    got: obj.summarize(),
                })),
            }
        }
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "call_by_name",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument { name: "verb".into(), ktype: KType::Identifier }),
                SignatureElement::Argument(Argument { name: "args".into(), ktype: KType::KExpression }),
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

    use crate::dispatch::arena::RuntimeArena;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::kerror::KErrorKind;
    use crate::dispatch::kobject::KObject;
    use crate::dispatch::ktraits::Parseable;
    use crate::dispatch::scope::Scope;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;
    use crate::parse::kexpression::KExpression;

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

    fn parse_one(src: &str) -> KExpression<'static> {
        let mut exprs = parse(src).expect("parse should succeed");
        assert_eq!(exprs.len(), 1, "test helper expects a single expression");
        exprs.remove(0)
    }

    fn run_one<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should succeed");
        sched.read(id)
    }

    /// Like `run_one` but returns the error if the dispatch errored. Tests asserting on
    /// `KError` variants use `expect_err_kind(this, |k| ...)` to inspect.
    fn run_one_err<'a>(
        scope: &'a Scope<'a>,
        expr: KExpression<'a>,
    ) -> crate::dispatch::kerror::KError {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should not surface errors directly");
        match sched.read_result(id) {
            Ok(v) => panic!("expected dispatch to error, got value {}", v.summarize()),
            Err(e) => e.clone(),
        }
    }

    /// `LET f = (FN ...)` captures the FN's returned KFunction. Calling it via
    /// `f (arg)` dispatches through `call_by_name`, which weaves the function's keyword
    /// (DOUBLE) back in and re-dispatches as `DOUBLE arg`.
    #[test]
    fn fn_callable_via_call_by_name() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "LET f = (FN (DOUBLE x) -> Number = (x))");
        let result = run_one(scope, parse_one("f (7)"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// A function whose signature has a keyword in a non-leading position — the synthesized
    /// expression must reinsert the keyword between the positional args.
    #[test]
    fn call_by_name_weaves_internal_keyword() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "LET f = (FN (a PICK b) -> Number = (a))");
        let result = run_one(scope, parse_one("f (1 2)"));
        assert!(matches!(result, KObject::Number(n) if *n == 1.0));
    }

    /// Arity mismatch: f takes 1 arg, called with 3.
    #[test]
    fn call_by_name_arity_mismatch_returns_error() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "LET f = (FN (DOUBLE x) -> Number = (x))");
        let err = run_one_err(scope, parse_one("f (1 2 3)"));
        assert!(
            matches!(err.kind, KErrorKind::ArityMismatch { expected: 1, got: 3 }),
            "expected ArityMismatch{{1, 3}}, got {err}",
        );
    }

    /// Non-function binding: `x` is a Number; calling `x (7)` errors with TypeMismatch on
    /// the verb, since lookup found a binding but it isn't a function.
    #[test]
    fn call_by_name_on_non_function_returns_error() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "LET x = 42");
        let err = run_one_err(scope, parse_one("x (7)"));
        assert!(
            matches!(&err.kind, KErrorKind::TypeMismatch { arg, .. } if arg == "verb"),
            "expected TypeMismatch on verb, got {err}",
        );
    }

    /// Unbound name: `f` was never bound; lookup returns None, builtin returns
    /// `KError::UnboundName`.
    #[test]
    fn call_by_name_unbound_returns_error() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let err = run_one_err(scope, parse_one("undefined (7)"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "undefined"),
            "expected UnboundName(\"undefined\"), got {err}",
        );
    }

    /// Closure-escape: a function defined inside another function's body, returned out of
    /// that body, can still be invoked after the outer call completes. The lifted
    /// `KObject::KFunction` carries an `Rc<CallArena>` clone keeping the per-call arena
    /// (where the inner function's storage and captured scope live) alive past the outer
    /// call's frame drop. Pre-Stage-3 this would UAF when the inner function's reference
    /// dangled into the freed arena.
    #[test]
    fn closure_escapes_outer_call_and_remains_invocable() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        // MAKE returns a fresh inner function. The inner FN registers under its keyword
        // (INNER) in MAKE's per-call scope, then is returned (FN's value).
        run(
            scope,
            "FN (MAKE) -> KFunction = (FN (INNER) -> Str = (\"hi\"))\n\
             LET f = (MAKE)",
        );
        // After MAKE's call frame drops, only the lifted KObject::KFunction (carrying the
        // Rc) is keeping MAKE's per-call arena alive. Invoking the inner FN must still
        // succeed without UAF.
        let err = run_one_err(scope, parse_one("f (1)"));
        // `f (1)` invokes via `call_by_name` → INNER. INNER's signature is
        // `[Keyword(INNER)]` (no args), so the synthesized call has arity 1 vs expected 0
        // and `KFunction::apply` returns ArityMismatch. The point of this test is that we
        // get a structured error rather than a UAF crash — that proves the reference and
        // arena are alive.
        assert!(
            matches!(err.kind, KErrorKind::ArityMismatch { expected: 0, got: 1 }),
            "expected ArityMismatch{{0, 1}}, got {err}",
        );
    }

    /// Variant of the closure-escape test where the inner FN takes a parameter, so the
    /// invocation actually returns the body's value rather than arity-mismatching to Null.
    /// Confirms the captured scope's substitute-and-dispatch path works after escape.
    #[test]
    fn escaped_closure_with_param_returns_body_value() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(
            scope,
            "FN (MAKE) -> KFunction = (FN (ECHO x) -> Number = (x))\n\
             LET f = (MAKE)",
        );
        let result = run_one(scope, parse_one("f (42)"));
        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
    }

    /// List-of-closures escape: the body returns a list literal whose only element is a
    /// closure defined inside the call. `lift_kobject` must recurse through the `List`
    /// variant to find the embedded `KFunction(_, None)` and attach the dying frame's
    /// `Rc<CallArena>` to it, otherwise the inner function's `&KFunction` reference would
    /// dangle into the freed per-call arena once the slot's frame drops. Asserting the
    /// lifted closure's frame field is `Some` directly verifies the recursion fired.
    #[test]
    fn list_of_closures_escapes_outer_call_with_rc_attached() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "FN (MAKE) -> List = ([(FN (ECHO x) -> Number = (x))])");
        let result = run_one(scope, parse_one("(MAKE)"));
        let items = match result {
            KObject::List(items) => items,
            other => panic!("expected MAKE to return a List, got {}", other.summarize()),
        };
        assert_eq!(items.len(), 1, "list should hold the single inner closure");
        match &items[0] {
            KObject::KFunction(_, frame) => assert!(
                frame.is_some(),
                "list-borne escaping closure must have an Rc<CallArena> attached by \
                 lift_kobject's recursion through the List variant",
            ),
            other => panic!("list element should be a KFunction, got {}", other.summarize()),
        }
    }
}
