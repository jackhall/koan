use std::rc::Rc;

use crate::dispatch::kfunction::{
    Argument, ArgumentBundle, Body, BodyResult, ExpressionSignature, KFunction, KType,
    SchedulerHandle, SignatureElement,
};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::{null, register_builtin};

/// `FN <signature:KExpression> = <body:KExpression>` — the user-defined function constructor.
/// Both slots are `KType::KExpression`, so the parser's parenthesized sub-expressions match
/// and ride through as data (the existing `lazy_candidate` path covers two-`KExpression`
/// signatures). The captured signature `KExpression` is structurally inspected here — never
/// dispatched — to derive the registered function's `ExpressionSignature`. The body
/// `KExpression` is captured raw and re-dispatched at call time inside `KFunction::invoke`.
///
/// First cut: zero-arg shape only. The signature `KExpression` must be exactly one part of
/// variant `ExpressionPart::Keyword(name)` (so a call site like `GREET` is invocable like a
/// builtin, not via parens). Anything else is rejected with `null()`. Once the error story
/// roadmap item lands, those rejections become `KError`s that say *what* the user got wrong.
pub fn body<'a>(
    scope: &mut Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let signature_expr = match extract_kexpression(&mut bundle, "signature") {
        Some(e) => e,
        None => return null(),
    };
    let body_expr = match extract_kexpression(&mut bundle, "body") {
        Some(e) => e,
        None => return null(),
    };

    let name = match signature_expr.parts.as_slice() {
        [ExpressionPart::Keyword(n)] => n.clone(),
        _ => return null(),
    };

    let user_sig = ExpressionSignature {
        return_type: KType::Any,
        elements: vec![SignatureElement::Keyword(name.clone())],
    };

    let f: &'a KFunction<'a> = Box::leak(Box::new(KFunction::new(
        None,
        user_sig,
        Body::UserDefined(body_expr),
    )));
    let obj: &'a KObject<'a> = Box::leak(Box::new(KObject::KFunction(f)));
    scope.add(name, obj);
    null()
}

/// Pull a `KType::KExpression`-typed argument out of the bundle and return the inner
/// `KExpression`. Mirrors the `Rc::try_unwrap` shape `if_then::body` uses to avoid cloning
/// when the bundle holds the only reference.
fn extract_kexpression<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<KExpression<'a>> {
    let rc = bundle.args.remove(name)?;
    match Rc::try_unwrap(rc) {
        Ok(KObject::KExpression(e)) => Some(e),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::KExpression(e) => Some(e.clone()),
            _ => None,
        },
    }
}

pub fn register(scope: &mut Scope<'static>) {
    register_builtin(
        scope,
        "FN",
        ExpressionSignature {
            return_type: KType::Null,
            elements: vec![
                SignatureElement::Keyword("FN".into()),
                SignatureElement::Argument(Argument { name: "signature".into(), ktype: KType::KExpression }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument { name: "body".into(),      ktype: KType::KExpression }),
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
    use crate::dispatch::kfunction::SignatureElement;
    use crate::dispatch::kobject::KObject;
    use crate::dispatch::scope::Scope;
    use crate::execute::interpret::interpret;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;
    use crate::parse::kexpression::KExpression;

    fn parse_one(src: &str) -> KExpression<'static> {
        let mut exprs = parse(src).expect("parse should succeed");
        assert_eq!(exprs.len(), 1, "test helper expects a single expression");
        exprs.remove(0)
    }

    fn run_one<'a>(scope: &mut Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr);
        let results = sched.execute(scope).expect("scheduler should succeed");
        results[id.index()]
    }

    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    /// Run `source` against a fresh `default_scope()` with `PRINT` redirected to a buffer;
    /// return the captured bytes. The standard rig for end-to-end FN tests that assert on
    /// observable output rather than dispatch internals.
    fn capture_program_output(source: &str) -> Vec<u8> {
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let mut scope = default_scope();
        scope.out = Box::new(SharedBuf(captured.clone()));
        interpret(source, &mut scope).expect("program should run");
        let bytes = captured.borrow().clone();
        bytes
    }

    #[test]
    fn fn_registers_user_function_under_keyword_signature() {
        let mut scope = default_scope();
        interpret("FN (GREET) = (PRINT \"hi\")", &mut scope).expect("FN should run");

        let entry = scope.data.get("GREET").expect("GREET should be bound");
        let f = match entry {
            KObject::KFunction(f) => *f,
            _ => panic!("expected GREET to bind a KFunction"),
        };
        match f.signature.elements.as_slice() {
            [SignatureElement::Keyword(s)] => assert_eq!(s, "GREET"),
            _ => panic!("expected single-Keyword signature [Keyword(\"GREET\")]"),
        }
    }

    #[test]
    fn fn_call_dispatches_body_at_call_time() {
        let mut scope = default_scope();
        interpret("LET x = 42\nFN (GETX) = (x)", &mut scope).expect("setup should run");

        let result = run_one(&mut scope, parse_one("GETX"));
        assert!(matches!(result, KObject::Number(n) if *n == 42.0),
            "GETX should return the value bound to x at call time");
    }

    #[test]
    fn fn_rejects_non_keyword_name() {
        // Lowercase `greet` parses as Identifier, not Keyword. The signature `KExpression`
        // shape doesn't match the zero-arg pattern, so FN should refuse without registering.
        let mut scope = default_scope();
        interpret("FN (greet) = (PRINT \"hi\")", &mut scope).expect("FN should run");
        assert!(scope.data.get("greet").is_none());
        assert!(scope.data.get("GREET").is_none());
    }

    #[test]
    fn fn_call_runs_body_each_time() {
        // Two separate calls each produce a fresh result. (Per-call allocation is implicit:
        // every dispatch leaks a new KFuture and the body's `value_lookup` leaks a new clone
        // of x's value.)
        let mut scope = default_scope();
        interpret("LET x = 7\nFN (GETX) = (x)", &mut scope).expect("setup should run");

        for _ in 0..2 {
            let result = run_one(&mut scope, parse_one("GETX"));
            assert!(matches!(result, KObject::Number(n) if *n == 7.0));
        }
    }

    #[test]
    fn fn_body_with_nested_expression_evaluates() {
        // Canonical regression for the dispatch-as-node refactor: a user-fn body whose own
        // parts include a nested `(msg)` Expression now works, because the scheduler walks
        // the body's AST when it runs the spawned Dispatch — same machinery as a top-level
        // expression. Previously this silently nulled because `KFunction::invoke` dispatched
        // its body inline against `scope` without scheduler access.
        let bytes = capture_program_output(
            "LET msg = \"from outer scope\"\n\
             FN (SAY) = (PRINT (msg))\n\
             SAY",
        );
        assert_eq!(bytes, b"from outer scope\n");
    }

    #[test]
    fn user_fn_calls_user_fn_transitively() {
        // FOO's body is just `(BAR)` — calling another user fn. The forward chain runs
        // FOO -> spawned Dispatch for `(BAR)` -> spawned Dispatch for BAR's body -> PRINT.
        // Tests that `BodyResult::Defer` composes through multiple layers without losing
        // the final value (the bind that depends on FOO's result correctly waits for
        // BAR's downstream PRINT to complete before reading).
        let bytes = capture_program_output(
            "FN (BAR) = (PRINT \"ok\")\n\
             FN (FOO) = (BAR)\n\
             FOO",
        );
        assert_eq!(bytes, b"ok\n");
    }

    #[test]
    fn calling_user_fn_repeatedly_runs_body_each_time() {
        // Each `GREET` is its own top-level Dispatch and produces its own observable
        // PRINT side effect. Confirms per-call execution via captured stdout (the existing
        // `fn_call_runs_body_each_time` test asserts the return value, this one asserts
        // the side-effect output).
        let bytes = capture_program_output(
            "FN (GREET) = (PRINT \"hello world\")\n\
             GREET\n\
             GREET",
        );
        assert_eq!(bytes, b"hello world\nhello world\n");
    }
}
