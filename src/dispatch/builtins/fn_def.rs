use std::rc::Rc;

use crate::dispatch::kerror::{KError, KErrorKind};
use crate::dispatch::kfunction::{
    Argument, ArgumentBundle, Body, BodyResult, ExpressionSignature, KFunction, KType,
    SchedulerHandle, SignatureElement,
};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::{err, register_builtin};

/// `FN <signature:KExpression> = <body:KExpression>` — the user-defined function constructor.
/// Both slots are `KType::KExpression`, so the parser's parenthesized sub-expressions match
/// and ride through as data. The captured signature `KExpression` is structurally inspected
/// here — never dispatched — to derive the registered function's `ExpressionSignature`. The
/// body `KExpression` is captured raw; `KFunction::invoke` substitutes parameter values into
/// it and re-dispatches at call time.
///
/// Signature shape: each `Keyword` part becomes a `SignatureElement::Keyword` (a fixed token
/// in the call site); each `Identifier` part becomes an `Argument` of type `Any` named after
/// the identifier (a slot the caller supplies). At least one `Keyword` is required so the
/// signature has a fixed token to dispatch on — a signature of all-Identifier slots would
/// shadow `value_lookup`/`value_pass`. Other shapes (literals, nested expressions in the
/// signature itself) are rejected with `null()`. Type annotations on argument slots are a
/// future extension that hangs off this same parsing pass.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let signature_expr = match extract_kexpression(&mut bundle, "signature") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN signature slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    let body_expr = match extract_kexpression(&mut bundle, "body") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN body slot must be a parenthesized expression".to_string(),
            )));
        }
    };

    let elements = match parse_signature_elements(&signature_expr) {
        Some(es) => es,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN signature must contain only Keyword and Identifier parts".to_string(),
            )));
        }
    };
    // Pick the first Keyword as the data-table key. `scope.functions` does the load-bearing
    // dispatch lookup by signature; `scope.data` is mostly for discoverability and
    // shadow-by-name semantics, neither of which has a single right answer for a multi-token
    // signature like `(a ADD b)`. First Keyword is a defensible default.
    let name = elements.iter().find_map(|e| match e {
        SignatureElement::Keyword(s) => Some(s.clone()),
        _ => None,
    });
    let name = match name {
        Some(n) => n,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN signature must contain at least one Keyword (a fixed token to dispatch on)"
                    .to_string(),
            )));
        }
    };

    let user_sig = ExpressionSignature {
        return_type: KType::Any,
        elements,
    };

    let arena = scope.arena;
    let f: &'a KFunction<'a> = arena.alloc_function(KFunction::new(
        user_sig,
        Body::UserDefined(body_expr),
        scope,
    ));
    // `frame: None` here — the lift-on-return logic in the scheduler will populate the Rc
    // when this KFunction value escapes out of a per-call body. For top-level FNs, there's
    // no per-call frame to clone, so None stays.
    let obj: &'a KObject<'a> = arena.alloc_object(KObject::KFunction(f, None));
    scope.add(name, obj);
    // Returning the function reference (rather than null) lets callers do
    // `LET f = (FN ...)` to capture a callable handle, which the dispatch fallback for
    // identifier-bound KFunctions can then invoke.
    BodyResult::Value(obj)
}

/// Convert the captured signature `KExpression` into a list of `SignatureElement`s.
/// `Keyword(s)` → fixed `Keyword(s)` token. `Identifier(s)` → `Argument { name: s, ktype: Any }`.
/// Any other variant (`Literal`, `Expression`, `ListLiteral`, `Future`) means the user wrote
/// something that isn't a valid signature shape — return `None` and let the caller bail.
fn parse_signature_elements<'a>(signature: &KExpression<'a>) -> Option<Vec<SignatureElement>> {
    signature.parts.iter().map(|part| match part {
        ExpressionPart::Keyword(s) => Some(SignatureElement::Keyword(s.clone())),
        ExpressionPart::Identifier(s) => Some(SignatureElement::Argument(Argument {
            name: s.clone(),
            ktype: KType::Any,
        })),
        _ => None,
    }).collect()
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

pub fn register<'a>(scope: &'a Scope<'a>) {
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

    use crate::dispatch::arena::RuntimeArena;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::kfunction::SignatureElement;
    use crate::dispatch::kobject::KObject;
    use crate::dispatch::scope::Scope;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;
    use crate::parse::kexpression::KExpression;

    fn parse_one(src: &str) -> KExpression<'static> {
        let mut exprs = parse(src).expect("parse should succeed");
        assert_eq!(exprs.len(), 1, "test helper expects a single expression");
        exprs.remove(0)
    }

    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    /// Build a default scope in `arena` with PRINT routed into `captured`. Returns the
    /// run-root scope. Replaces the previous `capture_program_output` rig — tests now own
    /// the arena explicitly so they can inspect post-run state.
    fn build_scope<'a>(arena: &'a RuntimeArena, captured: Rc<RefCell<Vec<u8>>>) -> &'a Scope<'a> {
        default_scope(arena, Box::new(SharedBuf(captured)))
    }

    /// Run `source` to completion against `scope`; for one-shot dispatch tests.
    fn run<'a>(scope: &'a Scope<'a>, source: &str) {
        let exprs = parse(source).expect("parse should succeed");
        let mut sched = Scheduler::new();
        for expr in exprs {
            sched.add_dispatch(expr, scope);
        }
        sched.execute().expect("scheduler should succeed");
    }

    /// Run a single parsed expression and return its result reference.
    fn run_one<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should succeed");
        sched.read(id)
    }

    fn capture_program_output(source: &str) -> Vec<u8> {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured.clone());
        run(scope, source);
        let bytes = captured.borrow().clone();
        bytes
    }

    #[test]
    fn fn_registers_user_function_under_keyword_signature() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "FN (GREET) = (PRINT \"hi\")");

        let data = scope.data.borrow();
        let entry = data.get("GREET").expect("GREET should be bound");
        let f = match entry {
            KObject::KFunction(f, _) => *f,
            _ => panic!("expected GREET to bind a KFunction"),
        };
        match f.signature.elements.as_slice() {
            [SignatureElement::Keyword(s)] => assert_eq!(s, "GREET"),
            _ => panic!("expected single-Keyword signature [Keyword(\"GREET\")]"),
        }
    }

    #[test]
    fn fn_call_dispatches_body_at_call_time() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "LET x = 42\nFN (GETX) = (x)");

        let result = run_one(scope, parse_one("GETX"));
        assert!(matches!(result, KObject::Number(n) if *n == 42.0),
            "GETX should return the value bound to x at call time");
    }

    #[test]
    fn fn_rejects_non_keyword_name() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "FN (greet) = (PRINT \"hi\")");
        let data = scope.data.borrow();
        assert!(data.get("greet").is_none());
        assert!(data.get("GREET").is_none());
    }

    #[test]
    fn fn_call_runs_body_each_time() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "LET x = 7\nFN (GETX) = (x)");

        for _ in 0..2 {
            let result = run_one(scope, parse_one("GETX"));
            assert!(matches!(result, KObject::Number(n) if *n == 7.0));
        }
    }

    #[test]
    fn fn_body_with_nested_expression_evaluates() {
        let bytes = capture_program_output(
            "LET msg = \"from outer scope\"\n\
             FN (SAY) = (PRINT (msg))\n\
             SAY",
        );
        assert_eq!(bytes, b"from outer scope\n");
    }

    #[test]
    fn user_fn_calls_user_fn_transitively() {
        let bytes = capture_program_output(
            "FN (BAR) = (PRINT \"ok\")\n\
             FN (FOO) = (BAR)\n\
             FOO",
        );
        assert_eq!(bytes, b"ok\n");
    }

    #[test]
    fn chained_user_fn_tail_calls_reuse_one_slot() {
        let arena = RuntimeArena::new();
        let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured.clone());

        run(
            scope,
            "FN (B) = (PRINT \"ok\")\n\
             FN (A) = (B)",
        );

        let mut sched = Scheduler::new();
        sched.add_dispatch(parse_one("A"), scope);
        sched.execute().expect("A should run");

        assert_eq!(captured.borrow().as_slice(), b"ok\n");
        assert_eq!(
            sched.len(),
            1,
            "tail-call slot reuse: A -> B -> PRINT should collapse into one slot, got {}",
            sched.len(),
        );
    }

    #[test]
    fn calling_user_fn_repeatedly_runs_body_each_time() {
        let bytes = capture_program_output(
            "FN (GREET) = (PRINT \"hello world\")\n\
             GREET\n\
             GREET",
        );
        assert_eq!(bytes, b"hello world\nhello world\n");
    }

    #[test]
    fn fn_with_single_param_substitutes_at_call_site() {
        let bytes = capture_program_output(
            "FN (SAY x) = (PRINT x)\n\
             SAY \"hello\"",
        );
        assert_eq!(bytes, b"hello\n");
    }

    #[test]
    fn fn_with_two_params_binds_each_by_name() {
        let bytes = capture_program_output(
            "FN (FIRST x y) = (PRINT x)\n\
             FIRST \"one\" \"two\"",
        );
        assert_eq!(bytes, b"one\n");
    }

    #[test]
    fn fn_with_infix_shape_dispatches_on_keyword_position() {
        let bytes = capture_program_output(
            "FN (a SAID) = (PRINT a)\n\
             \"hi\" SAID",
        );
        assert_eq!(bytes, b"hi\n");
    }

    #[test]
    fn fn_param_shadows_outer_binding_at_call_site() {
        let bytes = capture_program_output(
            "LET msg = \"outer\"\n\
             FN (SAY msg) = (PRINT msg)\n\
             SAY \"param wins\"",
        );
        assert_eq!(bytes, b"param wins\n");
    }

    #[test]
    fn fn_param_substitutes_inside_nested_subexpression() {
        let bytes = capture_program_output(
            "FN (WRAP x) = (PRINT (x))\n\
             WRAP \"wrapped\"",
        );
        assert_eq!(bytes, b"wrapped\n");
    }

    #[test]
    fn fn_returns_param_value_directly() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "FN (ECHO v) = (v)");

        let result = run_one(scope, parse_one("ECHO 7"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    #[test]
    fn fn_signature_with_no_keyword_is_rejected() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "FN (x) = (PRINT \"oops\")");
        let data = scope.data.borrow();
        assert!(data.get("x").is_none());
    }

    /// The leak-fix regression test: a parameterized user-fn called many times must not
    /// grow the run-root arena per call. Pre-fix, every call leaked a child Scope, a param
    /// clone, the substituted body's identifier->Future rewrites, and value_pass's clone —
    /// 5+ allocations per call into run-root. Post-fix, those land in the per-call arena
    /// and free at call return; only the lift-on-return value persists in run-root (one
    /// `KObject::Number` per call). The bound used here (~3 allocations/call) tolerates
    /// the lift while rejecting the old linear leak.
    #[test]
    fn repeated_user_fn_calls_do_not_grow_run_root_per_call() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(scope, "FN (ECHO v) = (v)");
        let baseline = arena.alloc_count();
        for _ in 0..50 {
            let _ = run_one(scope, parse_one("ECHO 7"));
        }
        let after = arena.alloc_count();
        let growth = after - baseline;
        // Measured at exactly 50 (one `KObject::Number(7)` lifted per call). Old behavior
        // would have been 250-350+: child Scope, param clone, substituted-Future, value_pass
        // clone, and the value_pass dispatch's Bind value, all per call. The < 150 bound
        // tolerates the lift while rejecting the old linear leak.
        assert!(
            growth < 50 * 3,
            "per-call leak regression: {growth} new run-root allocations across 50 \
             ECHO calls (expected < 150)",
        );
    }

    /// `FN` returns the `KObject::KFunction` it just registered, so callers can capture a
    /// callable handle via `LET f = (FN ...)`. Pre-change, `FN` returned `null()`. Calling
    /// the captured handle is tested in [`call_by_name`](super::super::call_by_name).
    #[test]
    fn fn_def_returns_the_registered_kfunction() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        let result = run_one(scope, parse_one("FN (DOUBLE x) = (x)"));
        assert!(
            matches!(result, KObject::KFunction(_, _)),
            "FN should return its registered KFunction",
        );
    }
}
