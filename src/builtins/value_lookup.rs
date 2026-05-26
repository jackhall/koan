use crate::machine::core::LexicalFrame;
use crate::machine::model::ast::{TypeExpr, TypeParams};
use crate::machine::model::{KObject, KType, Parseable};
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};

use super::{arg, err, register_builtin, sig};

/// Resolve a bare leaf `TypeExpr` against the scope's type-side bindings and return the
/// canonical value-side `KObject` carrier. Pre-`apply_auto_wrap` this was open-coded inside
/// `body_type_expr`; lifted out so the dispatch phase (Phase 3 of `run_dispatch`) can call
/// it directly without routing through a sub-Dispatch.
///
/// Coercions performed:
/// - Reject parameterized shapes (`List<...>`, `Function<...>` etc.) with `ShapeError`.
/// - On a `resolve_type` hit reporting a nominal identity (`UserType`,
///   `SatisfiesSignature`, `Module`, or `Signature`), recover the paired value-side carrier
///   via `scope.lookup` so downstream operators receive the expected `KSignature` /
///   `KModule` / `StructType` / `TaggedUnionType` part rather than a synthesized
///   `KTypeValue`. Dual-write atomicity makes the paired-carrier lookup infallible under
///   normal flow; the synthesis below covers the defensive case.
/// - Otherwise (builtin leaves, `LET <Type-class> = <KTypeValue>` aliases) synthesize a
///   `KObject::KTypeValue(kt.clone())` carrier so the value sits in the same dispatch
///   transport every other body consumes.
/// - On `resolve_type` miss surface `UnboundName(name)`.
pub fn coerce_type_token_value<'a>(
    scope: &'a Scope<'a>,
    t: &TypeExpr,
    chain: Option<&LexicalFrame>,
) -> Result<&'a KObject<'a>, KError> {
    if !matches!(t.params, TypeParams::None) {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "value_lookup = parameterized type expression `{}` is not a value-lookup target",
            t.render()
        ))));
    }
    let name = t.name.as_str();
    match scope.resolve_type_with_chain(name, chain) {
        Some(kt) => {
            // Dual-write invariant: nominal identity types (`UserType`,
            // `SatisfiesSignature`, `Module`, `Signature`) are paired with a value-side
            // carrier at the same scope. Recover the carrier so downstream operators
            // (`:|`, `:!`, ATTR-Module/Struct, `struct_construct`, `MODULE_TYPE_OF`)
            // receive the expected `KSignature` / `KModule` / `StructType` /
            // `TaggedUnionType` part rather than a synthesized `KTypeValue`.
            if matches!(
                kt,
                KType::UserType { .. }
                    | KType::SatisfiesSignature { .. }
                    | KType::Module { .. }
                    | KType::Signature(_)
            ) {
                if let Some(obj) = scope.lookup_with_chain(name, chain) {
                    return Ok(obj);
                }
                // Unreachable under dual-write atomicity; fall through to the
                // KTypeValue synthesis below as a defensive recovery.
            }
            Ok(scope.arena.alloc(KObject::KTypeValue(kt.clone())))
        }
        None => Err(KError::new(KErrorKind::UnboundName(name.to_string()))),
    }
}

/// `<v:Identifier>` — single-part expression containing one identifier-classed name token.
/// Looks `v` up via `Scope::lookup` (walking `data → placeholders → outer`) and returns
/// the bound `KObject`, or `KError::UnboundName` if unbound at every level. Lets a
/// parens-wrapped name (`(some_var)`) — or an auto-wrap of the same — dispatch and
/// resolve to its current value.
pub fn body_identifier<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let v = match bundle.get("v") {
        Some(KObject::KString(s)) => s.clone(),
        other => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "v".to_string(),
                expected: "KString".to_string(),
                got: match other {
                    Some(o) => o.summarize(),
                    None => "(missing)".to_string(),
                },
            }));
        }
    };
    let chain = sched.current_lexical_chain();
    match scope.lookup_with_chain(&v, chain.as_deref()) {
        Some(obj) => BodyResult::Value(obj),
        None => err(KError::new(KErrorKind::UnboundName(v))),
    }
}

/// `<v:TypeExprRef>` — single-part expression containing one type-classed name token.
/// Resolves the name through `Scope::resolve_type` exclusively (the type-side binding
/// home). Two outcomes:
///
/// - Hit reports a nominal identity (`KType::UserType` / `KType::SatisfiesSignature`) —
///   the binding was dual-written by STRUCT / UNION / MODULE / SIG finalize (or a
///   `LET <Type-class> = <carrier>` alias). Recover the paired value-side carrier via
///   `scope.lookup` so downstream operators (`:|`, `:!`, ATTR-Struct/Module,
///   `struct_construct`, `MODULE_TYPE_OF`) receive the expected `KSignature` /
///   `KModule` / `StructType` / `TaggedUnionType` part rather than a synthesized
///   `KTypeValue`. The `lookup` call here is paired-carrier recovery gated on a
///   nominal `types` hit — structurally not a fall-through.
/// - Any other `types` hit (builtin leaves, `LET <Type-class> = <KTypeValue>` aliases)
///   synthesizes a per-lookup `KObject::KTypeValue(kt.clone())` carrier so the value
///   sits in the same dispatch transport every other body consumes.
///
/// On a `resolve_type` miss the result is `UnboundName` directly — there is no
/// independent `lookup` fall-through anymore. Structural type-syntax (`List<X>`,
/// function types, `Mu` / `RecursiveRef`) is rejected up front with `ShapeError`.
pub fn body_type_expr<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // Normalize incoming carrier into a leaf `TypeExpr` that `coerce_type_token_value`
    // consumes. Reject parameterized shapes here on the `KTypeValue` arm because the
    // KType variant (`List`, `Dict`, `KFunction`, `Mu`, `RecursiveRef`) carries strictly
    // more shape information than a `TypeExpr::leaf` synthesized from its name — the
    // bare `t.name()` is a leaf string and the helper's `TypeParams::None` check would
    // miss the structural rejection.
    let leaf = match bundle.get("v") {
        Some(KObject::KTypeValue(t)) => match t {
            KType::List(_)
            | KType::Dict(_, _)
            | KType::KFunction { .. }
            | KType::Mu { .. }
            | KType::RecursiveRef(_) => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "value_lookup = parameterized type expression `{}` is not a value-lookup target",
                    t.render()
                ))));
            }
            _ => TypeExpr::leaf(t.name()),
        },
        // Stage-2 carrier: a bare-leaf type token whose name isn't in the builtin table
        // (`Foo` after `STRUCT Foo = …`, etc.). Pass the surface `TypeExpr` straight
        // through — the helper handles its own parametric-shape rejection.
        Some(KObject::TypeNameRef(t)) => t.clone(),
        other => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "v".to_string(),
                expected: "KTypeValue".to_string(),
                got: match other {
                    Some(o) => o.summarize(),
                    None => "(missing)".to_string(),
                },
            }));
        }
    };
    let chain = sched.current_lexical_chain();
    match coerce_type_token_value(scope, &leaf, chain.as_deref()) {
        Ok(obj) => BodyResult::Value(obj),
        Err(e) => err(e),
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "value_lookup",
        sig(KType::Any, vec![arg("v", KType::Identifier)]),
        body_identifier,
    );
    register_builtin(
        scope,
        "value_lookup",
        sig(KType::Any, vec![arg("v", KType::TypeExprRef)]),
        body_type_expr,
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::{body_identifier, body_type_expr, coerce_type_token_value};
    use crate::builtins::test_support::run_root_bare;
    use crate::machine::model::ast::TypeExpr;
    use crate::machine::model::{KObject, KType};
    use crate::machine::{
        ArgumentBundle, BindingIndex, BodyResult, KError, KErrorKind, RuntimeArena, Scope,
    };
    use crate::machine::execute::Scheduler;

    fn run_body<'a>(
        scope: &'a Scope<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        body_identifier(scope, &mut sched, bundle).expect_value("value_lookup")
    }

    /// Like `run_body` but returns the `BodyResult` so error-path tests can pattern-match
    /// on the `Err` variant.
    fn run_body_result<'a>(
        scope: &'a Scope<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        let mut sched = Scheduler::new();
        body_identifier(scope, &mut sched, bundle)
    }

    fn run_body_type_expr<'a>(
        scope: &'a Scope<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        let mut sched = Scheduler::new();
        body_type_expr(scope, &mut sched, bundle)
    }

    #[test]
    fn value_lookup_returns_binding() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let bound = arena.alloc(KObject::Number(42.0));
        scope.bind_value("foo".to_string(), bound, BindingIndex::BUILTIN).unwrap();

        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("foo".into())));

        let result = run_body(scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
    }

    #[test]
    fn value_lookup_unbound_returns_error() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("missing".into())));

        let result = run_body_result(scope, ArgumentBundle { args });

        match result {
            BodyResult::Err(KError { kind: KErrorKind::UnboundName(name), .. }) => {
                assert_eq!(name, "missing");
            }
            other => panic!("expected UnboundName error, got {:?}", error_kind_name(&other)),
        }
    }

    fn error_kind_name(r: &BodyResult<'_>) -> &'static str {
        match r {
            BodyResult::Value(_) => "Value",
            BodyResult::Tail { .. } => "Tail",
            BodyResult::DeferTo(_) => "DeferTo",
            BodyResult::Err(_) => "Err",
        }
    }

    #[test]
    fn value_lookup_walks_outer_scope() {
        let arena = RuntimeArena::new();
        let outer = run_root_bare(&arena);
        let bound = arena.alloc(KObject::Number(7.0));
        outer.bind_value("from_outer".to_string(), bound, BindingIndex::BUILTIN).unwrap();

        let inner = arena.alloc_scope(outer.child_for_call());

        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("from_outer".into())));

        let result = run_body(inner, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// `body_type_expr` consults `Scope::resolve_type` (the post-1.5 type-side binding
    /// home) and synthesizes the `KObject::KTypeValue` dispatch transport on hit. The
    /// incoming bundle slot's `KTypeValue` only carries the surface leaf name via
    /// `name()`; the result is the `KType` stored under that name in `bindings.types`.
    #[test]
    fn value_lookup_type_expr_resolves_via_resolve_type() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        // Register the type under the name `Number` — `KType::Number.name()` is "Number",
        // so the body's `resolve_type("Number")` walks the types map and returns the
        // arena-allocated entry.
        scope.register_type("Number".into(), KType::Number, BindingIndex::BUILTIN);
        let mut args = HashMap::new();
        args.insert(
            "v".to_string(),
            Rc::new(KObject::KTypeValue(KType::Number)),
        );
        let result = run_body_type_expr(scope, ArgumentBundle { args });
        match result {
            BodyResult::Value(KObject::KTypeValue(kt)) => {
                assert!(matches!(kt, KType::Number), "expected Number, got {kt:?}");
            }
            other => panic!(
                "expected Value(KTypeValue(Number)), got {:?}",
                error_kind_name(&other)
            ),
        }
    }

    /// Structural type shapes are rejected before any `resolve_type` lookup — the same
    /// `ShapeError` the pre-migration body produced.
    #[test]
    fn value_lookup_type_expr_rejects_parameterized_shapes() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let mut args = HashMap::new();
        args.insert(
            "v".to_string(),
            Rc::new(KObject::KTypeValue(KType::List(Box::new(KType::Number)))),
        );
        let result = run_body_type_expr(scope, ArgumentBundle { args });
        match result {
            BodyResult::Err(KError { kind: KErrorKind::ShapeError(msg), .. }) => {
                assert!(
                    msg.contains("parameterized type expression"),
                    "expected ShapeError about parameterized type, got `{msg}`",
                );
            }
            other => panic!(
                "expected ShapeError on parameterized lookup, got {:?}",
                error_kind_name(&other)
            ),
        }
    }

    /// An unbound type-token name surfaces as `UnboundName`.
    #[test]
    fn value_lookup_type_expr_unbound_returns_error() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        // No `register_type` call — the bare runtime scope has no types map entries.
        let mut args = HashMap::new();
        args.insert(
            "v".to_string(),
            Rc::new(KObject::KTypeValue(KType::Number)),
        );
        let result = run_body_type_expr(scope, ArgumentBundle { args });
        match result {
            BodyResult::Err(KError { kind: KErrorKind::UnboundName(name), .. }) => {
                assert_eq!(name, "Number");
            }
            other => panic!(
                "expected UnboundName on missing type, got {:?}",
                error_kind_name(&other)
            ),
        }
    }

    // Equivalence coverage for `coerce_type_token_value` — pins every coercion the
    // existing `body_type_expr` produces. Mirrors the body_type_expr tests above
    // (resolve-via-resolve_type, parameterized-shape rejection, unbound surface) plus a
    // module-identity recovery case that exercises the dual-write path.

    #[test]
    fn coerce_type_token_value_builtin_synthesizes_ktypevalue() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        scope.register_type("Number".into(), KType::Number, BindingIndex::BUILTIN);
        let leaf = TypeExpr::leaf("Number".to_string());
        let obj = coerce_type_token_value(scope, &leaf, None).expect("expected Number lookup");
        assert!(matches!(obj, KObject::KTypeValue(KType::Number)));
    }

    #[test]
    fn coerce_type_token_value_rejects_parameterized_shapes() {
        use crate::machine::model::ast::TypeParams;
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let parametric = TypeExpr {
            name: "List".to_string(),
            params: TypeParams::List(vec![TypeExpr::leaf("Number".to_string())]),
            builtin_cache: std::cell::OnceCell::new(),
        };
        let result = coerce_type_token_value(scope, &parametric, None);
        match result {
            Err(KError { kind: KErrorKind::ShapeError(msg), .. }) => {
                assert!(
                    msg.contains("parameterized type expression"),
                    "expected ShapeError about parameterized type, got `{msg}`",
                );
            }
            other => panic!("expected ShapeError, got {:?}", other.map(|_| "Ok(_)")),
        }
    }

    #[test]
    fn coerce_type_token_value_unbound_returns_error() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let leaf = TypeExpr::leaf("Missing".to_string());
        match coerce_type_token_value(scope, &leaf, None) {
            Err(KError { kind: KErrorKind::UnboundName(name), .. }) => {
                assert_eq!(name, "Missing");
            }
            other => panic!("expected UnboundName, got {:?}", other.map(|_| "Ok(_)")),
        }
    }

    /// Dual-write recovery: a `KType::UserType { .. }` registered in `bindings.types`
    /// paired with a value-side carrier in `bindings.data` returns the paired value, not
    /// a synthesized `KTypeValue`.
    #[test]
    fn coerce_type_token_value_recovers_paired_value() {
        use crate::machine::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let kind = UserTypeKind::Struct;
        let kt = KType::UserType {
            kind,
            scope_id: scope.id,
            name: "Point".to_string(),
        };
        scope.register_type("Point".into(), kt.clone(), BindingIndex::BUILTIN);
        let paired = arena.alloc(KObject::KTypeValue(kt));
        scope.bind_value("Point".to_string(), paired, BindingIndex::BUILTIN).unwrap();

        let leaf = TypeExpr::leaf("Point".to_string());
        let obj = coerce_type_token_value(scope, &leaf, None).expect("expected Point lookup");
        // Identity-equality: the helper hands back the paired carrier rather than
        // synthesizing a fresh one.
        assert!(std::ptr::eq(obj, paired));
    }

    /// Defensive paired-recovery fall-through: `bindings.types[name]` carries a nominal
    /// identity (`UserType`) but `bindings.data[name]` is empty. Under dual-write
    /// atomicity this is unreachable through normal flow — the test forces it by
    /// `register_type` *without* a paired `bind_value`. The helper must not panic; it
    /// falls through to synthesizing a fresh `KTypeValue(kt)` carrier so the dispatch
    /// transport stays valid.
    #[test]
    fn coerce_type_token_value_falls_through_when_paired_value_absent() {
        use crate::machine::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let kt = KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: scope.id,
            name: "Orphan".to_string(),
        };
        // types-side only — no paired `bind_value`. Exercises the "Unreachable under
        // dual-write atomicity" fall-through.
        scope.register_type("Orphan".into(), kt.clone(), BindingIndex::BUILTIN);

        let leaf = TypeExpr::leaf("Orphan".to_string());
        let obj = coerce_type_token_value(scope, &leaf, None).expect("fall-through must Ok");
        match obj {
            KObject::KTypeValue(KType::UserType { name, .. }) => {
                assert_eq!(name, "Orphan", "fall-through synthesized carrier for the registered identity");
            }
            other => panic!("expected synthesized KTypeValue(UserType(Orphan)), got {:?}", other.ktype()),
        }
    }
}
