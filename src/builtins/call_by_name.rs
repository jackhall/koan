use crate::dispatch::{
    Argument, ArgumentBundle, BodyResult, ExpressionSignature, KError, KErrorKind, KObject, KType,
    Parseable, Scope, SchedulerHandle, SignatureElement,
};
use crate::dispatch::values::dispatch_constructor;

use crate::dispatch::kfunction::argument_bundle::extract_kexpression;
use super::{err, register_builtin};

/// `<verb:Identifier> <args:KExpression>` — surface syntax `f (a: 1, b: 2)`. When `verb`
/// resolves to a `TaggedUnionType` or `StructType` instead of a function, delegates to
/// `dispatch_constructor` (mirrors `type_call` but reached via a LET-bound lowercase
/// identifier). Synthesis logic lives on [`KFunction::apply`].
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // Identifier slots resolve to a KString carrying the identifier text.
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
    // `extract_kexpression` collapses missing and wrong-shape into `None`; split them so the
    // surface error wording differs.
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
    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::dispatch::{KErrorKind, KObject, Parseable, RuntimeArena};

    #[test]
    fn fn_callable_via_call_by_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET f = (FN (DOUBLE x: Number) -> Number = (x))");
        let result = run_one(scope, parse_one("f (x: 7)"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// Keyword in a non-leading signature position must be reinserted between reordered args.
    #[test]
    fn call_by_name_weaves_internal_keyword() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET f = (FN (a: Number PICK b: Number) -> Number = (a))");
        let result = run_one(scope, parse_one("f (a: 1, b: 2)"));
        assert!(matches!(result, KObject::Number(n) if *n == 1.0));
    }

    #[test]
    fn call_by_name_named_args_order_independent() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "LET f = (FN (a: Number PICK b: Number) -> Number = (a))");
        let result = run_one(scope, parse_one("f (b: 2, a: 1)"));
        assert!(matches!(result, KObject::Number(n) if *n == 1.0));
    }

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

    /// Missing-name errors fire before unknown-name errors, so the test must provide every
    /// required name plus an extra to actually reach the unknown branch.
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

    /// Verb resolution fires before pair parsing, so a non-function verb errors on the verb
    /// itself rather than on the (potentially malformed) pair shape.
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

    /// `STRUCT` both registers the type token (`Pt`) and returns the `StructType` value;
    /// LET captures the latter under a lowercase alias that routes through `call_by_name`.
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

    /// A closure returned out of its defining call must remain invocable: the lifted
    /// `KObject::KFunction` carries an `Rc<CallArena>` keeping the per-call arena (where the
    /// inner function's storage and captured scope live) alive past frame drop.
    #[test]
    fn closure_escapes_outer_call_and_remains_invocable() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "FN (MAKE) -> Function<() -> Str> = (FN (INNER) -> Str = (\"hi\"))\n\
             LET f = (MAKE)",
        );
        let result = run_one(scope, parse_one("f ()"));
        assert!(
            matches!(result, KObject::KString(s) if s == "hi"),
            "expected KString(\"hi\"), got {}", result.summarize(),
        );
    }

    /// Variant exercising the captured scope's substitute-and-dispatch path after escape.
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

    /// `lift_kobject` must recurse through the `List` variant to attach the dying frame's
    /// `Rc<CallArena>` to embedded `KFunction(_, None)` elements; otherwise the inner
    /// function's `&KFunction` reference would dangle into the freed per-call arena.
    /// Asserting the lifted closure's frame field is `Some` verifies the recursion fired.
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
