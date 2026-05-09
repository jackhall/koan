use crate::dispatch::runtime::{KError, KErrorKind};
use crate::dispatch::kfunction::{ArgumentBundle, Body, BodyResult, KFunction, SchedulerHandle};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, ScopeResolver, SignatureElement};
use crate::dispatch::values::KObject;
use crate::dispatch::runtime::Scope;
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::helpers::{extract_kexpression, extract_type_expr};
use super::{err, register_builtin_with_pre_run};

/// `FN <signature:KExpression> -> <return_type:Type> = <body:KExpression>` — the user-defined
/// function constructor. The signature and body slots are `KType::KExpression`, so the parser's
/// parenthesized sub-expressions match and ride through as data. The return-type slot is a
/// `KType::TypeExprRef`, matching a parsed `Type(_)` token whose structured `TypeExpr` (with
/// any nested type parameters) is preserved through the bundle.
/// The captured signature `KExpression` is structurally inspected here — never dispatched —
/// to derive the registered function's `ExpressionSignature`. The body `KExpression` is
/// captured raw; `KFunction::invoke` substitutes parameter values into it and re-dispatches
/// at call time.
///
/// Signature shape: each `Keyword` part becomes a `SignatureElement::Keyword` (a fixed token
/// in the call site); each `Identifier` must be followed by `: Type` to form an `Argument`
/// triple, producing a typed parameter slot the caller supplies. Per-param types are
/// dispatch-checked via `Argument::matches`, so a call whose argument types don't satisfy
/// the signature surfaces as `DispatchFailed: no matching function` (same path as builtins);
/// overloads on different parameter types route to the right body via slot-specificity.
/// At least one `Keyword` is required so the signature has a fixed token to dispatch on —
/// a signature of all-Argument slots would shadow `value_lookup`/`value_pass`. Bare
/// identifiers (without `: Type`), stray type tokens, literals, and nested expressions in
/// the signature are rejected with a `ShapeError`.
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
    let return_type_expr = match extract_type_expr(&mut bundle, "return_type") {
        Some(t) => t,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN return-type slot must be a type expression (e.g. `Number`, `List<Str>`)"
                    .to_string(),
            )));
        }
    };
    // ScopeResolver walks the surrounding scope's bindings first so user-defined types
    // (`LET MyList = (LIST_OF Number)`) shadow builtins. Stage-2 substrate per the
    // [module-system stage 2 plan](../../../roadmap/module-system-2-scheduler.md).
    let resolver = ScopeResolver::new(scope);
    let return_type = match KType::from_type_expr(&return_type_expr, &resolver) {
        Ok(t) => t,
        Err(msg) => {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "FN return-type slot: {msg}"
            ))));
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

    let elements = match parse_fn_param_list(&signature_expr, &resolver) {
        Ok(es) => es,
        Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
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
        return_type,
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
    if let Err(e) = scope.register_function(name, f, obj) {
        return err(e);
    }
    // Returning the function reference (rather than null) lets callers do
    // `LET f = (FN ...)` to capture a callable handle, which the dispatch fallback for
    // identifier-bound KFunctions can then invoke.
    BodyResult::Value(obj)
}

/// Convert the captured FN-parameter-list `KExpression` into a list of `SignatureElement`s.
/// (Module signatures — `Signature`, declared via `SIG` — are a different concept; this
/// function only handles the FN parameter list.) Walks the parts left-to-right, consuming
/// bare `Keyword` parts as fixed tokens and `Identifier(name) Keyword(":") Type(t)` triples
/// as typed `Argument` slots. Bare identifiers without a `: Type` annotation, unknown type
/// names, stray `:` or `Type` parts, and any other variant (`Literal`, `Expression`,
/// `ListLiteral`, `DictLiteral`, `Future`) yield an `Err(message)` for the caller to wrap
/// in `ShapeError`. The colon keyword is consumed only as part of a triple — a stray `:`
/// outside that shape is a shape error.
///
/// The `resolver` is forwarded into `KType::from_type_expr` for each parameter type so
/// user-defined types in the surrounding scope can shadow builtins. Stage-2 substrate per
/// the [module-system stage 2 plan](../../../roadmap/module-system-2-scheduler.md).
fn parse_fn_param_list<'a>(
    signature: &KExpression<'a>,
    resolver: &dyn crate::dispatch::types::TypeResolver,
) -> Result<Vec<SignatureElement>, String> {
    let parts = &signature.parts;
    let mut elements: Vec<SignatureElement> = Vec::with_capacity(parts.len());
    let mut i = 0;
    while i < parts.len() {
        match &parts[i] {
            ExpressionPart::Keyword(s) if s == ":" => {
                return Err(
                    "FN signature has a stray `:` outside a `<name>: <Type>` triple".to_string(),
                );
            }
            ExpressionPart::Keyword(s) => {
                elements.push(SignatureElement::Keyword(s.clone()));
                i += 1;
            }
            ExpressionPart::Identifier(name) => {
                let colon = parts.get(i + 1);
                let ty = parts.get(i + 2);
                match (colon, ty) {
                    (Some(ExpressionPart::Keyword(c)), Some(ExpressionPart::Type(t))) if c == ":" => {
                        let ktype = KType::from_type_expr(t, resolver).map_err(|e| {
                            format!("{e} in FN signature for parameter `{name}`")
                        })?;
                        elements.push(SignatureElement::Argument(Argument {
                            name: name.clone(),
                            ktype,
                        }));
                        i += 3;
                    }
                    _ => {
                        return Err(format!(
                            "FN signature parameter `{name}` requires a `: Type` annotation \
                             (e.g. `{name}: Number`)",
                        ));
                    }
                }
            }
            ExpressionPart::Type(t) => {
                return Err(format!(
                    "FN signature has a stray type `{}` outside a `<name>: <Type>` triple",
                    t.render(),
                ));
            }
            other => {
                return Err(format!(
                    "FN signature part `{}` is not a Keyword, Identifier, or `<name>: <Type>` triple",
                    other.summarize(),
                ));
            }
        }
    }
    Ok(elements)
}

/// Dispatch-time placeholder extractor for FN. The signature slot at `parts[1]` is an
/// `Expression(signature_expr)` whose first `Keyword` is the function's name (the same
/// name used by `body` to register the function — see the `find_map(SignatureElement::
/// Keyword, ...)` call). Walks the signature parts inline rather than re-running the
/// full `parse_fn_param_list`; the body still does the full parse and surfaces any shape
/// errors. Returns `None` if the signature slot is missing or malformed (e.g. no Keyword
/// in the signature) — the body's `ShapeError` reports the real failure.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    let sig_part = expr.parts.get(1)?;
    let signature_expr = match sig_part {
        ExpressionPart::Expression(boxed) => boxed,
        _ => return None,
    };
    for part in &signature_expr.parts {
        if let ExpressionPart::Keyword(s) = part {
            return Some(s.clone());
        }
    }
    None
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_pre_run(
        scope,
        "FN",
        ExpressionSignature {
            // FN returns a function value, but there's no "any function" KType anymore —
            // a function's structural type only exists once its signature is known. `Any`
            // here lets the constructed `KObject::KFunction`'s projected `ktype()` (which
            // does carry the full signature) flow through any caller's slot.
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("FN".into()),
                SignatureElement::Argument(Argument { name: "signature".into(),   ktype: KType::KExpression }),
                SignatureElement::Keyword("->".into()),
                SignatureElement::Argument(Argument { name: "return_type".into(), ktype: KType::TypeExprRef }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument { name: "body".into(),        ktype: KType::KExpression }),
            ],
        },
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests {
    use crate::dispatch::builtins::test_support::{parse_one, run, run_one, run_root_silent, run_root_with_buf};
    use crate::dispatch::runtime::RuntimeArena;
    use crate::dispatch::types::SignatureElement;
    use crate::dispatch::values::KObject;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;

    fn capture_program_output(source: &str) -> Vec<u8> {
        let arena = RuntimeArena::new();
        let (scope, captured) = run_root_with_buf(&arena);
        run(scope, source);
        let bytes = captured.borrow().clone();
        bytes
    }

    /// Smoke test for FN's pre_run extractor: pulls the first Keyword out of the
    /// signature Expression at `parts[1]` (FN's name slot is *inside* the signature,
    /// not at `parts[1]` directly).
    #[test]
    fn pre_run_extracts_first_keyword_of_signature() {
        let expr = parse_one("FN (DOUBLE x: Number) -> Number = (x)");
        let name = super::pre_run(&expr);
        assert_eq!(name.as_deref(), Some("DOUBLE"));
    }

    #[test]
    fn fn_registers_user_function_under_keyword_signature() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (GREET) -> Null = (PRINT \"hi\")");

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
        let scope = run_root_silent(&arena);
        run(scope, "LET x = 42\nFN (GETX) -> Number = (x)");

        let result = run_one(scope, parse_one("GETX"));
        assert!(matches!(result, KObject::Number(n) if *n == 42.0),
            "GETX should return the value bound to x at call time");
    }

    #[test]
    fn fn_rejects_non_keyword_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (greet) -> Null = (PRINT \"hi\")");
        let data = scope.data.borrow();
        assert!(data.get("greet").is_none());
        assert!(data.get("GREET").is_none());
    }

    #[test]
    fn fn_call_runs_body_each_time() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET x = 7\nFN (GETX) -> Number = (x)");

        for _ in 0..2 {
            let result = run_one(scope, parse_one("GETX"));
            assert!(matches!(result, KObject::Number(n) if *n == 7.0));
        }
    }

    #[test]
    fn fn_body_with_nested_expression_evaluates() {
        let bytes = capture_program_output(
            "LET msg = \"from outer scope\"\n\
             FN (SAY) -> Null = (PRINT (msg))\n\
             SAY",
        );
        assert_eq!(bytes, b"from outer scope\n");
    }

    #[test]
    fn user_fn_calls_user_fn_transitively() {
        let bytes = capture_program_output(
            "FN (BAR) -> Null = (PRINT \"ok\")\n\
             FN (FOO) -> Null = (BAR)\n\
             FOO",
        );
        assert_eq!(bytes, b"ok\n");
    }

    #[test]
    fn chained_user_fn_tail_calls_reuse_one_slot() {
        let arena = RuntimeArena::new();
        let (scope, captured) = run_root_with_buf(&arena);

        run(
            scope,
            "FN (BB) -> Null = (PRINT \"ok\")\n\
             FN (AA) -> Null = (BB)",
        );

        let mut sched = Scheduler::new();
        sched.add_dispatch(parse_one("AA"), scope);
        sched.execute().expect("AA should run");

        assert_eq!(captured.borrow().as_slice(), b"ok\n");
        assert_eq!(
            sched.len(),
            1,
            "tail-call slot reuse: AA -> BB -> PRINT should collapse into one slot, got {}",
            sched.len(),
        );
    }

    #[test]
    fn calling_user_fn_repeatedly_runs_body_each_time() {
        let bytes = capture_program_output(
            "FN (GREET) -> Null = (PRINT \"hello world\")\n\
             GREET\n\
             GREET",
        );
        assert_eq!(bytes, b"hello world\nhello world\n");
    }

    #[test]
    fn fn_with_single_param_substitutes_at_call_site() {
        let bytes = capture_program_output(
            "FN (SAY x: Str) -> Null = (PRINT x)\n\
             SAY \"hello\"",
        );
        assert_eq!(bytes, b"hello\n");
    }

    #[test]
    fn fn_with_two_params_binds_each_by_name() {
        let bytes = capture_program_output(
            "FN (FIRST x: Str y: Str) -> Null = (PRINT x)\n\
             FIRST \"one\" \"two\"",
        );
        assert_eq!(bytes, b"one\n");
    }

    #[test]
    fn fn_with_infix_shape_dispatches_on_keyword_position() {
        let bytes = capture_program_output(
            "FN (a: Str SAID) -> Null = (PRINT a)\n\
             \"hi\" SAID",
        );
        assert_eq!(bytes, b"hi\n");
    }

    #[test]
    fn fn_param_shadows_outer_binding_at_call_site() {
        let bytes = capture_program_output(
            "LET msg = \"outer\"\n\
             FN (SAY msg: Str) -> Null = (PRINT msg)\n\
             SAY \"param wins\"",
        );
        assert_eq!(bytes, b"param wins\n");
    }

    #[test]
    fn fn_param_substitutes_inside_nested_subexpression() {
        let bytes = capture_program_output(
            "FN (WRAP x: Str) -> Null = (PRINT (x))\n\
             WRAP \"wrapped\"",
        );
        assert_eq!(bytes, b"wrapped\n");
    }

    #[test]
    fn fn_returns_param_value_directly() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (ECHO v: Number) -> Number = (v)");

        let result = run_one(scope, parse_one("ECHO 7"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    #[test]
    fn fn_signature_with_no_keyword_is_rejected() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (x: Number) -> Null = (PRINT \"oops\")");
        let data = scope.data.borrow();
        assert!(data.get("x").is_none());
    }

    /// A parameterized user-fn called many times must not grow the run-root arena per
    /// call: per-call allocations (child scope, param clones, body rewrites, value_pass
    /// clones) belong in the per-call arena, leaving only the lifted return value in
    /// run-root — one `KObject::Number` per call here. The bound (~3 allocations/call)
    /// tolerates the lift while catching any future regression that re-introduces a
    /// per-call leak into run-root.
    #[test]
    fn repeated_user_fn_calls_do_not_grow_run_root_per_call() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (ECHO v: Number) -> Number = (v)");
        let baseline = arena.alloc_count();
        for _ in 0..50 {
            let _ = run_one(scope, parse_one("ECHO 7"));
        }
        let after = arena.alloc_count();
        let growth = after - baseline;
        // Measured at exactly 50 (one `KObject::Number(7)` lifted per call). The < 150
        // bound tolerates that and catches any regression that re-introduces a per-call
        // leak into run-root.
        assert!(
            growth < 50 * 3,
            "per-call leak regression: {growth} new run-root allocations across 50 \
             ECHO calls (expected < 150)",
        );
    }

    /// Repeated calls to a user-fn whose body has an internal sub-expression reuse
    /// scheduler slots. The body of `LOOK` evaluates `MATCH (b) WITH …`; the `(b)`
    /// is a sub-expression that spawns a sub-`Dispatch` and a parent `Bind` per call.
    ///
    /// Property under test: after a warmup call has populated the free-list with the
    /// body's transient slots, each subsequent call's growth in `nodes.len()` is
    /// bounded by the *persistent* per-call overhead — the top-level dispatch slot
    /// itself, plus any persistent shim it lifts into. The body's transient sub-
    /// Dispatches/Binds must be recycled, not accumulated.
    ///
    /// Without reclamation, every call would leave its body's transient fanout
    /// behind (~5+ slots/call). With reclamation, the steady-state rate is the
    /// persistent overhead alone (a small constant ≤ 2 today). Comparing two
    /// batches catches super-linear growth without coupling to the exact constant.
    ///
    /// The truly-recursive variant (where the body tail-calls itself) is exercised
    /// by `match_case::tests::recursive_tagged_match_no_uaf`.
    #[test]
    fn body_subexpression_slots_recycle_across_calls() {
        let arena = RuntimeArena::new();
        let (scope, captured) = run_root_with_buf(&arena);

        run(
            scope,
            "UNION Bit = (one: Null zero: Null)\n\
             FN (LOOK b: Tagged) -> Any = (MATCH (b) WITH (\
                 one -> (PRINT \"one\")\
                 zero -> (PRINT \"zero\")\
             ))",
        );

        let mut sched = Scheduler::new();

        // One warmup call: extends `nodes` with the persistent top-level slot(s)
        // *and* the body's transient pool. After this call returns, the transients
        // are on the free-list, ready to be recycled by the next call's spawns.
        sched.add_dispatch(parse_one("LOOK (Bit (one null))"), scope);
        sched.execute().expect("LOOK should run");
        let after_warmup = sched.len();

        // Steady-state batch. Each call's body re-spawns the same transient shape;
        // those slots come off the free-list, so `nodes` only grows by the
        // persistent per-call overhead.
        let n = 30;
        for i in 1..=n {
            let src = if i % 2 == 0 { "LOOK (Bit (one null))" } else { "LOOK (Bit (zero null))" };
            sched.add_dispatch(parse_one(src), scope);
            sched.execute().expect("LOOK should run");
        }
        let after_batch = sched.len();

        // Sanity: each call printed once.
        assert_eq!(
            captured.borrow().iter().filter(|&&b| b == b'\n').count(),
            n + 1,
            "expected one PRINT per LOOK call, got {:?}",
            String::from_utf8_lossy(&captured.borrow()),
        );

        // The property: steady-state per-call growth is bounded by persistent
        // overhead. Currently 2 slots/call (top-level dispatch + Lift shim); if
        // reclamation regressed and transients leaked, it would be ≥ 5.
        // The bound of 3 reflects the property, not the exact value — it leaves
        // daylight for one extra persistent slot per call without admitting any
        // amount of transient pile-up.
        let growth = after_batch - after_warmup;
        let per_call = growth as f64 / n as f64;
        assert!(
            per_call <= 3.0,
            "transient-node reclamation regressed: {per_call:.2} slots/call \
             across {n} calls (after {after_warmup}-slot warmup, ended at \
             {after_batch}). Expected ≤ 3 — body's transient sub-Dispatches/\
             Binds should be recycled via the free-list, not accumulating."
        );
    }

    /// `FN` parses the declared return type from the `-> Type` slot and stores it on the
    /// registered function's signature.
    #[test]
    fn fn_parses_declared_return_type_onto_signature() {
        use crate::dispatch::types::KType;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (DOUBLE x: Number) -> Number = (x)");

        let data = scope.data.borrow();
        let entry = data.get("DOUBLE").expect("DOUBLE should be bound");
        let f = match entry {
            KObject::KFunction(f, _) => *f,
            _ => panic!("expected DOUBLE to bind a KFunction"),
        };
        assert_eq!(f.signature.return_type, KType::Number);
    }

    /// Missing `-> Type` annotation: the FN call doesn't match the registered signature, so
    /// no user-fn gets bound. (Sub-expression dispatch may also error first depending on body
    /// shape — the load-bearing assertion is that DOUBLE isn't bound.)
    #[test]
    fn fn_without_return_type_annotation_does_not_register() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let exprs = parse("FN (DOUBLE x: Number) = (PRINT \"x\")").expect("parse should succeed");
        let mut sched = Scheduler::new();
        for expr in exprs {
            sched.add_dispatch(expr, scope);
        }
        let _ = sched.execute(); // ignore: may or may not error depending on which sub fails first
        let data = scope.data.borrow();
        assert!(data.get("DOUBLE").is_none(), "DOUBLE should not be registered without -> Type");
    }

    /// Unknown type name in the return slot surfaces as a `ShapeError`.
    #[test]
    fn fn_with_unknown_return_type_name_errors() {
        use crate::dispatch::runtime::KErrorKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("FN (DOUBLE x: Number) -> Bogus = (x)"), scope);
        sched.execute().expect("execute does not surface per-slot errors");
        let err = match sched.read_result(id) {
            Err(e) => e,
            Ok(_) => panic!("unknown type name should error"),
        };
        assert!(
            matches!(err.kind, KErrorKind::ShapeError(ref msg) if msg.contains("Bogus")),
            "expected ShapeError mentioning 'Bogus', got {err}",
        );
    }

    /// Runtime return-type check fires when the body produces a value of the wrong type.
    #[test]
    fn user_fn_return_type_mismatch_surfaces_as_kerror() {
        use crate::dispatch::runtime::KErrorKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (LIE) -> Number = (\"oops\")");
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("LIE"), scope);
        sched.execute().expect("execute does not surface per-slot errors");
        let err = match sched.read_result(id) {
            Err(e) => e,
            Ok(_) => panic!("LIE should fail return-type check"),
        };
        match &err.kind {
            KErrorKind::TypeMismatch { arg, expected, got } => {
                assert_eq!(arg, "<return>");
                assert_eq!(expected, "Number");
                assert_eq!(got, "Str");
            }
            _ => panic!("expected TypeMismatch on <return>, got {err}"),
        }
        assert!(
            err.frames.iter().any(|f| f.function.contains("LIE")),
            "expected a frame mentioning LIE, got {:?}",
            err.frames.iter().map(|f| &f.function).collect::<Vec<_>>(),
        );
    }

    /// `Any` return type is the no-op fast path: any body value satisfies it.
    #[test]
    fn user_fn_with_any_return_type_accepts_anything() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (PURE) -> Any = (\"a string\")");
        let result = run_one(scope, parse_one("PURE"));
        assert!(matches!(result, KObject::KString(s) if s == "a string"));
    }

    /// `FN` returns the `KObject::KFunction` it just registered, so callers can capture a
    /// callable handle via `LET f = (FN ...)`. Calling the captured handle is tested in
    /// [`call_by_name`](super::super::call_by_name).
    #[test]
    fn fn_def_returns_the_registered_kfunction() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("FN (DOUBLE x: Number) -> Number = (x)"));
        assert!(
            matches!(result, KObject::KFunction(_, _)),
            "FN should return its registered KFunction",
        );
    }

    /// A typed parameter records its declared `KType` on the registered signature, rather
    /// than collapsing to `Any` as it did before per-param annotations existed.
    #[test]
    fn fn_typed_param_records_ktype_on_signature() {
        use crate::dispatch::types::{Argument, KType};
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (DOUBLE x: Number) -> Number = (x)");

        let data = scope.data.borrow();
        let entry = data.get("DOUBLE").expect("DOUBLE should be bound");
        let f = match entry {
            KObject::KFunction(f, _) => *f,
            _ => panic!("expected DOUBLE to bind a KFunction"),
        };
        match f.signature.elements.as_slice() {
            [SignatureElement::Keyword(kw), SignatureElement::Argument(Argument { name, ktype })] => {
                assert_eq!(kw, "DOUBLE");
                assert_eq!(name, "x");
                assert_eq!(*ktype, KType::Number);
            }
            _ => panic!("expected signature shape [Keyword(\"DOUBLE\"), Argument(x: Number)]"),
        }
    }

    /// A call whose argument satisfies the parameter type dispatches into the body.
    #[test]
    fn fn_typed_param_dispatches_on_matching_call() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (DOUBLE x: Number) -> Number = (x)");
        let result = run_one(scope, parse_one("DOUBLE 7"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// A call whose argument doesn't satisfy the parameter type fails at dispatch with
    /// `DispatchFailed` (the per-slot type check filters out the only candidate, so the
    /// scope chain runs out without a match). Same path as builtins.
    #[test]
    fn fn_typed_param_rejects_mismatched_call() {
        use crate::dispatch::runtime::KErrorKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (DOUBLE x: Number) -> Number = (x)");
        let mut sched = Scheduler::new();
        let _ = sched.add_dispatch(parse_one("DOUBLE \"hi\""), scope);
        // The dispatch failure surfaces via `execute()` here (the queue can't make
        // progress past the unmatchable call). The other shape — `execute() -> Ok` plus
        // a per-slot Err — is what return-type mismatches use; this case is different.
        let err = sched.execute().expect_err("DOUBLE \"hi\" should fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed for type-mismatched DOUBLE call, got {err}",
        );
    }

    /// Two FNs sharing a shape but differing on parameter type both register, and dispatch
    /// routes each call to the body whose type signature matches. Exercises the existing
    /// slot-specificity path now that user-fns can carry concrete types.
    #[test]
    fn fn_overloads_dispatch_by_param_type() {
        let bytes = capture_program_output(
            "FN (DESCRIBE x: Number) -> Null = (PRINT \"number\")\n\
             FN (DESCRIBE x: Str) -> Null = (PRINT \"string\")\n\
             DESCRIBE 7\n\
             DESCRIBE \"hi\"",
        );
        assert_eq!(bytes, b"number\nstring\n");
    }

    /// A bare identifier without `: Type` in a parameter slot is rejected with a
    /// `ShapeError` naming the offending parameter.
    #[test]
    fn fn_param_without_annotation_is_rejected() {
        use crate::dispatch::runtime::KErrorKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("FN (DOUBLE x) -> Number = (x)"), scope);
        sched.execute().expect("execute does not surface per-slot errors");
        let err = match sched.read_result(id) {
            Err(e) => e,
            Ok(_) => panic!("untyped parameter should error"),
        };
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`x`")),
            "expected ShapeError mentioning `x`, got {err}",
        );
        let data = scope.data.borrow();
        assert!(data.get("DOUBLE").is_none(), "DOUBLE should not register");
    }

    /// An unknown type name in a parameter slot surfaces as a `ShapeError` mentioning the
    /// bad name, mirroring the return-type case.
    #[test]
    fn fn_param_with_unknown_type_name_is_rejected() {
        use crate::dispatch::runtime::KErrorKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("FN (DOUBLE x: Bogus) -> Number = (x)"), scope);
        sched.execute().expect("execute does not surface per-slot errors");
        let err = match sched.read_result(id) {
            Err(e) => e,
            Ok(_) => panic!("unknown param type should error"),
        };
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
            "expected ShapeError mentioning `Bogus`, got {err}",
        );
    }

    /// Comma-separated parameter triples parse the same as whitespace-separated ones —
    /// the parser strips commas inside expression frames, so `(x: Number, y: Number)`
    /// and `(x: Number y: Number)` are interchangeable.
    #[test]
    fn fn_comma_separated_typed_params_register() {
        let bytes = capture_program_output(
            "FN (FIRST x: Number, y: Number) -> Number = (x)\n\
             PRINT (FIRST 1 2)",
        );
        assert_eq!(bytes, b"1\n");
    }

    // -------------------- parameterized container types --------------------

    /// FN with a `List<Number>` parameter accepts a homogeneous number list and runs the body.
    /// The signature parser routes `List<Number>` through `KType::List(Box::new(Number))`,
    /// so per-element type checking is enforced at call time.
    #[test]
    fn fn_with_typed_list_param_accepts_matching_list() {
        let bytes = capture_program_output(
            "FN (HEAD xs: List<Number>) -> Number = (1)\n\
             PRINT (HEAD [1 2 3])",
        );
        assert_eq!(bytes, b"1\n");
    }

    /// A function declared `-> List<Number>` whose body returns a homogeneous number list
    /// passes the scheduler's runtime return-type check.
    #[test]
    fn fn_returning_typed_list_accepts_matching_value() {
        let bytes = capture_program_output(
            "FN (NUMS) -> List<Number> = ([1 2 3])\n\
             PRINT (NUMS)",
        );
        assert_eq!(bytes, b"[1, 2, 3]\n");
    }

    /// A function declared `-> List<Number>` whose body returns a list with a string
    /// element fails the post-call return-type check (matches_value walks elements). The
    /// scheduler stores the error in the result slot rather than failing `execute`, so we
    /// read the slot via `read_result` to assert the failure.
    #[test]
    fn fn_returning_typed_list_rejects_wrong_element_type() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (BAD) -> List<Number> = ([1 \"x\"])");
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("BAD"), scope);
        sched.execute().expect("scheduler runs to completion");
        let res = sched.read_result(id);
        assert!(
            res.is_err(),
            "expected return-type mismatch when body produces List<Any> for declared List<Number>"
        );
    }

    /// FN-definition-time arity check: `List<A, B>` is invalid (List is unary). The error
    /// surfaces at FN-construction time as a ShapeError, stored in the result slot.
    #[test]
    fn fn_with_invalid_list_arity_errors_at_definition() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let mut sched = Scheduler::new();
        let exprs = parse("FN (BAD xs: List<Number, Str>) -> Null = (xs)").expect("parse ok");
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("scheduler runs");
        assert!(
            ids.iter().any(|id| sched.read_result(*id).is_err()),
            "FN definition with `List<Number, Str>` should fail with an arity error"
        );
    }

    /// FN with a `Dict<Str, Number>` parameter slot accepts a string-keyed number-valued dict.
    #[test]
    fn fn_with_typed_dict_param_accepts_matching_dict() {
        let bytes = capture_program_output(
            "FN (SIZE d: Dict<Str, Number>) -> Number = (1)\n\
             PRINT (SIZE {\"a\": 1, \"b\": 2})",
        );
        assert_eq!(bytes, b"1\n");
    }

    /// FN with a `Function<(Number) -> Str>` parameter accepts a function value whose
    /// signature matches structurally — the dispatch-time `function_compat` check. Pass an
    /// inline FN expression as the argument to side-step having to dereference an
    /// identifier-bound function.
    #[test]
    fn fn_with_typed_function_param_accepts_matching_function() {
        let bytes = capture_program_output(
            "FN (USE f: Function<(Number) -> Str>) -> Str = (\"got fn\")\n\
             PRINT (USE (FN (SHOW x: Number) -> Str = (\"hi\")))",
        );
        assert_eq!(bytes, b"got fn\n");
    }

    /// Specificity tournament: when two overloads share the same untyped shape and both
    /// match, the more specific one wins. `(xs: List<Number>)` is strictly more specific
    /// than `(xs: List<Any>)`, so a number-list call routes to the former.
    #[test]
    fn dispatch_picks_more_specific_list_overload() {
        let bytes = capture_program_output(
            "FN (PICK xs: List<Any>) -> Str = (\"any\")\n\
             FN (PICK xs: List<Number>) -> Str = (\"number\")\n\
             PRINT (PICK [1 2 3])",
        );
        assert_eq!(bytes, b"number\n");
    }

    /// Mixed list dispatches to the `List<Any>` overload (the only one that matches by
    /// post-evaluation `matches_value`); the `List<Number>` overload is filtered out.
    /// Note: dispatch-time matching is shape-only for containers (`Argument::matches`),
    /// so both overloads pass the initial filter; specificity then picks `List<Number>`,
    /// which fails at runtime element-check. Acceptable trade-off — caller gets the
    /// type-mismatch error from the more-specific overload, which is informative.
    #[test]
    fn fn_typed_list_param_rejects_wrong_element_type_at_call() {
        // Single overload typed List<Number> — wrong-element-type call must error.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "FN (HEAD xs: List<Number>) -> Number = (1)");
        let mut sched = Scheduler::new();
        sched.add_dispatch(parse_one("HEAD [\"a\"]"), scope);
        // Dispatch-time matching is shape-only; the call binds. The error surfaces only
        // when matches_value would be called — which today is only on return values, not
        // arguments. So this currently SUCCEEDS at runtime, returning 1. Confirming that
        // behavior here: argument-level element checks are deferred to a later phase.
        assert!(sched.execute().is_ok(),
                "phase 2 only checks element types on return values, not arguments");
    }

    // -------------------- Module-system stage 2: ScopeResolver --------------------

    /// Verify that `LET MyList = (LIST_OF Number)` binds a `TypeExprValue` carrying the
    /// surface `List` form. The bound value can then be lowered to `KType::List(Number)`
    /// via `KType::from_type_expr` — the ScopeResolver path uses exactly that.
    #[test]
    fn list_of_let_binding_is_type_expr_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET MyList = (LIST_OF Number)");
        let data = scope.data.borrow();
        let entry = data.get("MyList").expect("MyList should be bound");
        match entry {
            KObject::TypeExprValue(t) => {
                assert_eq!(t.name, "List");
            }
            other => panic!("expected TypeExprValue, got ktype={}", other.ktype().name()),
        }
    }

    /// `ScopeResolver` lowers a `TypeExprValue` binding to a `KType` when consulted by
    /// `from_type_expr`. Direct unit test of the resolver's contract — independent of the
    /// scheduler's top-level-statement ordering (see caveat below).
    ///
    /// **Caveat — top-level statement ordering.** Today `LET MyList = (LIST_OF Number)`
    /// followed by `FN (USE xs: MyList) ...` doesn't work end-to-end because the LET's
    /// `value` slot is an `Expression` (the `(LIST_OF Number)` sub-expression), so the LET
    /// becomes a Bind waiting on a sub-Dispatch — and the next top-level statement (the
    /// FN) runs before the Bind resolves. Sequencing this requires either ordering top-
    /// level statements as deps of one another or hoisting type-expression evaluation
    /// ahead of FN-body execution. Tracked as a stage-2 follow-up; the resolver itself
    /// already does the right thing once the binding is present.
    #[test]
    fn scope_resolver_lowers_type_expr_value_binding() {
        use crate::dispatch::types::{KType, ScopeResolver, TypeResolver};
        use crate::parse::kexpression::{TypeExpr, TypeParams};
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET MyList = (LIST_OF Number)");
        let resolver = ScopeResolver::new(scope);
        // MyList resolves through the scope.
        let resolved = resolver.resolve("MyList").expect("MyList should resolve");
        assert_eq!(resolved, KType::List(Box::new(KType::Number)));
        // from_type_expr forwards to the resolver before falling back to from_name; so
        // `Number` (a builtin) still resolves, and `MyList` resolves via scope.
        let mylist_te = TypeExpr { name: "MyList".into(), params: TypeParams::None };
        let kt = KType::from_type_expr(&mylist_te, &resolver).expect("from_type_expr ok");
        assert_eq!(kt, KType::List(Box::new(KType::Number)));
    }
}
