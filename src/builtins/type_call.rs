use crate::machine::{
    ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle,
};
use crate::machine::model::types::UserTypeKind;
use crate::machine::model::KType;

use super::dispatch_constructor;
use super::newtype_def::newtype_construct;
use crate::machine::core::kfunction::argument_bundle::{
    extract_bare_type_name, extract_kexpression,
};
use super::{arg, err, register_builtin, sig};

/// `<verb:TypeExprRef> <args:KExpression>` — the type-token construction path.
///
/// Stage 4 retired the stage-3-era `scope.lookup`-first path. The verb is type-classed,
/// so we resolve it through [`Scope::resolve_type`] (which walks `bindings.types`) and
/// branch on the resolved `KType::UserType { kind, .. }`:
///
/// - `Struct` / `Tagged`: stage 3's finalize installs a schema carrier in
///   `bindings.data` alongside the type identity. Fetch it and route through
///   [`dispatch_constructor`] (the existing `tagged_union::apply` /
///   `struct_value::apply` paths).
/// - `Newtype` (stage 4): no value-side carrier — NEWTYPE writes only `types`. Route
///   through [`newtype_construct`] with the resolved `&'a KType` so the construction
///   path can synthesize a tail with the identity riding through.
/// - `Module`: MODULE-as-constructor is reserved for functor application
///   (module-system stage 2). Surfaces as `TypeMismatch` until then.
/// - Anything else: `TypeMismatch` with `expected: "constructible Type"`.
///
/// Pre-rewrite this body looked up `verb` on `scope.lookup` (value-side) and dispatched
/// on the carrier variant. That worked for STRUCT / UNION because their finalize installs
/// a value-side carrier, but couldn't see NEWTYPE (which has no value-side carrier).
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // The verb slot is `TypeExprRef`, so its resolved value is `KObject::KTypeValue(t)`
    // for builtin leaves / structural shapes or `KObject::TypeNameRef(t, _)` for bare
    // user-bound names (`Point`, `Maybe`, `Distance`). The shared helper reads the
    // surface name out of either carrier and rejects parameterized forms
    // (`List<Number>` as a constructor verb makes no sense here).
    let verb = match extract_bare_type_name(&bundle, "verb", "type-call") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let args_expr = match extract_kexpression(&mut bundle, "args") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "type-call args slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    // Type-classed verb: resolve type-side. STRUCT/UNION finalize installs a schema
    // carrier in `data` alongside the type identity; NEWTYPE installs only the type
    // identity. Branch on the resolved `kind` so the dispatch contract drives off
    // the type identity, not by reaching through `data` first.
    let identity = match scope.resolve_type(&verb) {
        Some(kt) => kt,
        None => return err(KError::new(KErrorKind::UnboundName(verb))),
    };
    match identity {
        KType::UserType { kind: UserTypeKind::Struct, .. }
        | KType::UserType { kind: UserTypeKind::Tagged, .. } => {
            // Schema lives in `data`; STRUCT/UNION finalize installs it. Walk the
            // outer chain via `Scope::lookup` (not `bindings().data().get(...)`
            // directly) — STRUCT/UNION installs both entries in the declaring scope,
            // so a child-scope type-call must reach upward for the carrier. The
            // `data` borrow is released inside `lookup` before `dispatch_constructor`
            // re-enters the dispatch loop. A type identity without its paired carrier
            // would be a finalize bug — debug-assert in development, surface as
            // `UnboundName` in release so the consumer still sees something
            // structured.
            let schema_obj = match scope.lookup(&verb) {
                Some(obj) => obj,
                None => {
                    debug_assert!(
                        false,
                        "STRUCT/UNION `{verb}` registered its type identity but no \
                         matching value-side schema carrier",
                    );
                    return err(KError::new(KErrorKind::UnboundName(verb)));
                }
            };
            match dispatch_constructor(schema_obj, args_expr.parts) {
                Some(result) => result,
                None => err(KError::new(KErrorKind::TypeMismatch {
                    arg: "verb".to_string(),
                    expected: "Type".to_string(),
                    got: schema_obj.ktype().name(),
                })),
            }
        }
        KType::UserType { kind: UserTypeKind::Newtype { .. }, .. } => {
            newtype_construct(scope, sched, identity, args_expr.parts)
        }
        KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. } => {
            // A builtin parameterized type registered at prelude (`Result`) installs
            // a schema carrier in `data` alongside the type identity, like STRUCT/UNION
            // — route through it. An *opaque* TypeConstructor minted per-call for
            // SIG/functor ascription has no carrier; for those `lookup` misses and we
            // surface the not-constructible error rather than debug-asserting.
            match scope.lookup(&verb).and_then(|c| dispatch_constructor(c, args_expr.parts)) {
                Some(result) => result,
                None => err(KError::new(KErrorKind::TypeMismatch {
                    arg: "verb".to_string(),
                    expected: "constructible Type".to_string(),
                    got: identity.name(),
                })),
            }
        }
        // MODULE-as-constructor (functor application) lands with the functor-binder
        // roadmap item. Today the verb resolves to a module identity but there's no
        // construction semantics to drive — surface a `TypeMismatch` until then.
        // Post-collapse the carrier is `KType::Module { .. }` directly; the old
        // `UserType { kind: Module, .. }` indirection is gone.
        KType::Module { .. } => err(KError::new(KErrorKind::TypeMismatch {
            arg: "verb".to_string(),
            expected: "constructible Type".to_string(),
            got: identity.name(),
        })),
        other => err(KError::new(KErrorKind::TypeMismatch {
            arg: "verb".to_string(),
            expected: "constructible Type".to_string(),
            got: other.name(),
        })),
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "type_call",
        sig(KType::Any, vec![
            arg("verb", KType::TypeExprRef),
            arg("args", KType::KExpression),
        ]),
        body,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::machine::model::KObject;
    use crate::machine::{KErrorKind, RuntimeArena};

    #[test]
    fn type_token_calls_construct_tagged_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "UNION Maybe = (some :Number none :Null)");
        let result = run_one(scope, parse_one("Maybe (some 42)"));
        match result {
            KObject::Tagged { tag, value, .. } => {
                assert_eq!(tag, "some");
                assert!(matches!(&**value, KObject::Number(n) if *n == 42.0));
            }
            other => panic!("expected Tagged, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn type_call_unbound_type_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("Bogus (some 42)"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "Bogus"),
            "expected UnboundName(Bogus), got {err}",
        );
    }

    #[test]
    fn type_call_propagates_tag_validation_error() {
        // The synthesized TAG call surfaces the schema's tag check.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "UNION Maybe = (some :Number none :Null)");
        let err = run_one_err(scope, parse_one("Maybe (other 42)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`other`")),
            "expected ShapeError mentioning `other`, got {err}",
        );
    }

    #[test]
    fn type_call_with_sub_expression_value() {
        // `(x)` parens-wrapping forces the value-side identifier to resolve via value_lookup
        // before TAG's typed-slot bind sees it.
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "UNION Maybe = (some :Number none :Null)\nLET x = 7");
        let result = run_one(scope, parse_one("Maybe (some (x))"));
        match result {
            KObject::Tagged { tag, value, .. } => {
                assert_eq!(tag, "some");
                assert!(matches!(&**value, KObject::Number(n) if *n == 7.0));
            }
            other => panic!("expected Tagged, got {:?}", other.ktype()),
        }
    }

    /// Stage 4 regression: STRUCT construction works under the rewritten type-side
    /// resolution path. Before the rewrite, `type_call` consulted `scope.lookup`
    /// (value-side); now it consults `scope.resolve_type` first and fetches the
    /// schema carrier from `bindings.data` only after confirming the identity is
    /// `Struct` / `Tagged`. Pins that STRUCT's schema carrier still routes through
    /// `dispatch_constructor`.
    #[test]
    fn struct_construct_via_type_token() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "STRUCT Point = (x :Number, y :Number)");
        let result = run_one(scope, parse_one("Point (x = 1, y = 2)"));
        match result {
            KObject::Struct { name, fields, .. } => {
                assert_eq!(name, "Point");
                assert_eq!(fields.len(), 2);
                assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 1.0));
                assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 2.0));
            }
            other => panic!("expected Struct, got {:?}", other.ktype()),
        }
    }

    /// Stage 4 regression sibling: UNION construction works under the rewritten
    /// type-side resolution path. The `Tagged` arm of the new `kind` branch routes
    /// through `dispatch_constructor` identically to the pre-rewrite shape.
    #[test]
    fn union_construct_via_type_token() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "UNION Maybe = (some :Number none :Null)");
        let result = run_one(scope, parse_one("Maybe (some 42)"));
        match result {
            KObject::Tagged { tag, value, .. } => {
                assert_eq!(tag, "some");
                assert!(matches!(&**value, KObject::Number(n) if *n == 42.0));
            }
            other => panic!("expected Tagged, got {:?}", other.ktype()),
        }
    }
}
