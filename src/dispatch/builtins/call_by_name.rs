use crate::dispatch::runtime::{KError, KErrorKind};
use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, Parseable, SignatureElement};
use crate::dispatch::values::{KObject, dispatch_constructor};
use crate::dispatch::runtime::Scope;

use super::helpers::extract_kexpression;
use super::{err, register_builtin};

/// `<verb:Identifier> <args:KExpression>` — invokes a function bound to `verb` in scope by
/// applying it to the **named-argument** parts of `args`. Surface syntax: `f (a: 1, b: 2)`
/// where `f` is an Identifier whose binding is a `KObject::KFunction`. `verb` is resolved
/// via `Scope::lookup_kfunction`; `KFunction::apply` parses the inner expression as
/// `<name>: <value>` triples, reorders by signature parameter names, weaves the function's
/// signature keywords back in, and returns a `BodyResult::Tail` that the scheduler
/// re-dispatches against the keyword-bucketed signature. Errored cases (verb unbound, bound
/// to a non-function, args slot misshapen, missing/unknown/duplicate name) return
/// structured `KError` variants the CLI reports verbatim.
///
/// **Type-construction shortcut.** When `verb` resolves to a `TaggedUnionType` or
/// `StructType` rather than a function, the body delegates to the corresponding
/// construction path — mirroring `type_call`, but reached through a LET-bound lowercase
/// identifier rather than a `Type` token.
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
    // Distinguish missing-vs-wrong-shape before extracting: `extract_kexpression` returns
    // `None` for either case, but the surface error wording differs. The signature already
    // constrains the slot to `KType::KExpression`, so the non-KExpression branch is mostly
    // defensive — kept for parity with the pre-helper code.
    let was_present = bundle.get("args").is_some();
    let args_expr = match extract_kexpression(&mut bundle, "args") {
        Some(e) => e,
        None if was_present => {
            return err(KError::new(KErrorKind::ShapeError(
                "call_by_name args slot resolved to a non-KExpression".to_string(),
            )));
        }
        None => return err(KError::new(KErrorKind::MissingArg("args".to_string()))),
    };
    match scope.lookup_kfunction(&verb) {
        Some(f) => f.apply(args_expr.parts),
        None => {
            // Verb isn't a KFunction: route any constructible Type through the shared
            // `dispatch_constructor` helper, surface UnboundName for missing bindings, and
            // emit a "KFunction or Type" TypeMismatch otherwise (the wider expected-set
            // is what differentiates this site from `type_call`'s "Type"-only error).
            match scope.lookup(&verb) {
                None => err(KError::new(KErrorKind::UnboundName(verb))),
                Some(obj) => match dispatch_constructor(obj, args_expr.parts) {
                    Some(result) => result,
                    None => err(KError::new(KErrorKind::TypeMismatch {
                        arg: "verb".to_string(),
                        expected: "KFunction or Type".to_string(),
                        got: obj.summarize(),
                    })),
                },
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
    use crate::dispatch::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::dispatch::runtime::{KErrorKind, RuntimeArena};
    use crate::dispatch::types::Parseable;
    use crate::dispatch::values::KObject;

    /// `LET f = (FN ...)` captures the FN's returned KFunction. Calling it via
    /// `f (x: 7)` dispatches through `call_by_name`, which parses named pairs, reorders by
    /// signature parameter names, weaves the function's keyword (DOUBLE) back in, and
    /// re-dispatches as `DOUBLE 7`.
    #[test]
    fn fn_callable_via_call_by_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET f = (FN (DOUBLE x: Number) -> Number = (x))");
        let result = run_one(scope, parse_one("f (x: 7)"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// A function whose signature has a keyword in a non-leading position — the synthesized
    /// expression must reinsert the keyword between the named-and-reordered args.
    #[test]
    fn call_by_name_weaves_internal_keyword() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET f = (FN (a: Number PICK b: Number) -> Number = (a))");
        let result = run_one(scope, parse_one("f (a: 1, b: 2)"));
        assert!(matches!(result, KObject::Number(n) if *n == 1.0));
    }

    /// Named args are order-independent: caller writes them in any order, `apply` reorders
    /// to signature order. Reverse the caller's order from the previous test and the keyword
    /// PICK still sits between `a` and `b` in the synthesized tail.
    #[test]
    fn call_by_name_named_args_order_independent() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET f = (FN (a: Number PICK b: Number) -> Number = (a))");
        let result = run_one(scope, parse_one("f (b: 2, a: 1)"));
        assert!(matches!(result, KObject::Number(n) if *n == 1.0));
    }

    /// Missing named arg: f takes both `a` and `b`, called with only `a`.
    #[test]
    fn call_by_name_missing_named_arg() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET f = (FN (a: Number PICK b: Number) -> Number = (a))");
        let err = run_one_err(scope, parse_one("f (a: 1)"));
        assert!(
            matches!(&err.kind, KErrorKind::MissingArg(name) if name == "b"),
            "expected MissingArg(\"b\"), got {err}",
        );
    }

    /// Unknown named arg: f's signature names `a` and `b`, but caller passes `c`. Missing-
    /// first error precedence means `b` is reported before `c`, so to test the unknown
    /// branch we provide both required names plus an extra.
    #[test]
    fn call_by_name_unknown_named_arg() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET f = (FN (a: Number PICK b: Number) -> Number = (a))");
        let err = run_one_err(scope, parse_one("f (a: 1, b: 2, c: 3)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("unknown name") && msg.contains("`c`")),
            "expected ShapeError on unknown name c, got {err}",
        );
    }

    /// Missing colon: caller writes `f (a 1)` instead of `f (a: 1)`. The named-pair parser
    /// rejects the malformed shape with a ShapeError.
    #[test]
    fn call_by_name_missing_colon() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET f = (FN (DOUBLE x: Number) -> Number = (x))");
        let err = run_one_err(scope, parse_one("f (a 1)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`:`") || msg.contains("separator") || msg.contains("triples")),
            "expected ShapeError on missing colon, got {err}",
        );
    }

    /// Duplicate name in the named-arg list: `f (x: 1, x: 2)`.
    #[test]
    fn call_by_name_duplicate_named_arg() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET f = (FN (DOUBLE x: Number) -> Number = (x))");
        let err = run_one_err(scope, parse_one("f (x: 1, x: 2)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`x`")),
            "expected ShapeError on duplicate name, got {err}",
        );
    }

    /// Non-function binding: `x` is a Number; calling `x (foo: 7)` errors with TypeMismatch
    /// on the verb. Verb resolution fires before pair parsing, so the error is about the
    /// verb's binding rather than the pair shape.
    #[test]
    fn call_by_name_on_non_function_returns_error() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET x = 42");
        let err = run_one_err(scope, parse_one("x (foo: 7)"));
        assert!(
            matches!(
                &err.kind,
                KErrorKind::TypeMismatch { arg, expected, .. }
                    if arg == "verb" && expected == "KFunction or Type"
            ),
            "expected TypeMismatch on verb, got {err}",
        );
    }

    /// LET-bound TaggedUnionType: a lowercase identifier whose value is a tagged-union type
    /// can be used as a constructor — `call_by_name` detects the `TaggedUnionType` and takes
    /// the same construction path as the type-token form.
    #[test]
    fn call_by_name_on_tagged_union_constructs() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET maybe = (UNION (some: Number none: Null))");
        let result = run_one(scope, parse_one("maybe (some 42)"));
        match result {
            KObject::Tagged { tag, value } => {
                assert_eq!(tag, "some");
                assert!(matches!(&**value, KObject::Number(n) if *n == 42.0));
            }
            other => panic!("expected Tagged, got {:?}", other.ktype()),
        }
    }

    /// LET-bound StructType: same as the tagged-union case but for the struct path. The
    /// outer `STRUCT` form registers the type token (`Pt`) in scope as a side effect AND
    /// returns the `StructType` value, which LET captures under the lowercase alias. The
    /// alias then routes through `call_by_name` and uses named-arg construction.
    #[test]
    fn call_by_name_on_struct_type_constructs() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET pt = (STRUCT Pt = (x: Number, y: Number))");
        let result = run_one(scope, parse_one("pt (x: 3, y: 4)"));
        match result {
            KObject::Struct { type_name, fields } => {
                assert_eq!(type_name, "Pt");
                assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 3.0));
                assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 4.0));
            }
            other => panic!("expected Struct, got {:?}", other.ktype()),
        }
    }

    /// Unbound name: `f` was never bound; lookup returns None, builtin returns
    /// `KError::UnboundName`. Verb resolution fires before pair parsing.
    #[test]
    fn call_by_name_unbound_returns_error() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("undefined (foo: 7)"));
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
        let scope = run_root_silent(&arena);
        // MAKE returns a fresh inner function. The inner FN registers under its keyword
        // (INNER) in MAKE's per-call scope, then is returned (FN's value).
        run(
            scope,
            "FN (MAKE) -> Function<() -> Str> = (FN (INNER) -> Str = (\"hi\"))\n\
             LET f = (MAKE)",
        );
        // After MAKE's frame drops, the Rc on the lifted KFunction is the only thing
        // keeping its arena alive. A `KString("hi")` return proves no UAF.
        let result = run_one(scope, parse_one("f ()"));
        assert!(
            matches!(result, KObject::KString(s) if s == "hi"),
            "expected KString(\"hi\"), got {}", result.summarize(),
        );
    }

    /// Variant of the closure-escape test where the inner FN takes a parameter, so the
    /// invocation actually returns the body's value via the named-arg path. Confirms the
    /// captured scope's substitute-and-dispatch path works after escape.
    #[test]
    fn escaped_closure_with_param_returns_body_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "FN (MAKE) -> Function<(Number) -> Number> = (FN (ECHO x: Number) -> Number = (x))\n\
             LET f = (MAKE)",
        );
        let result = run_one(scope, parse_one("f (x: 42)"));
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
        let scope = run_root_silent(&arena);
        run(scope, "FN (MAKE) -> List = ([(FN (ECHO x: Number) -> Number = (x))])");
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
