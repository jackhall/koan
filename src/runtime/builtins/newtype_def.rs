//! `NEWTYPE <name: TypeExprRef> = <repr: TypeExprRef>` — declare a fresh nominal
//! identity over a transparent representation, plus the construction entry
//! invoked from [`type_call`](super::type_call) when the verb resolves to a
//! `KType::UserType { kind: Newtype, .. }`.
//!
//! Stage 4 of the type-identity arc. The declaration mints a per-declaration
//! [`KType::UserType`] with `kind: UserTypeKind::Newtype { repr }` and writes only
//! `bindings.types` — there is no value-side schema carrier (unlike STRUCT / UNION,
//! which dual-write a schema carrier into `bindings.data`). The construction path
//! produces a [`KObject::Wrapped`] tagging the inner value with the NEWTYPE
//! identity; that carrier is the only way `KType::UserType { kind: Newtype, .. }`
//! values reach user code today.
//!
//! Construction is driven from `type_call::body`'s `Newtype` arm via
//! [`newtype_construct`], which schedules the value sub-expression through
//! `add_dispatch` and waits on it via a Combine. The Combine's finish closure
//! validates the resolved inner against the newtype's `repr`, applies the collapse
//! rule (`Wrapped.inner` is invariantly non-`Wrapped`), and produces the
//! `KObject::Wrapped`. Same pattern as `module_def::body`, `sig_def::body`, and
//! `struct_def::defer_struct_via_combine`. No second registered builtin → no
//! bucket-collision infinite loop with `type_call`.

use crate::runtime::machine::model::ast::{ExpressionPart, KExpression, TypeParams};
use crate::runtime::machine::core::ApplyOutcome;
use crate::runtime::machine::core::kfunction::argument_bundle::{
    extract_bare_type_name, extract_ktype, extract_type_name_ref,
};
use crate::runtime::machine::{
    ArgumentBundle, BodyResult, CombineFinish, KError, KErrorKind, Scope, SchedulerHandle,
};
use crate::runtime::machine::model::types::UserTypeKind;
use crate::runtime::machine::model::values::KObject;
use crate::runtime::machine::model::KType;

use super::{arg, err, kw, register_builtin_with_pre_run, sig};

/// Body of `NEWTYPE <name> = <repr>`. Extracts the bare type name, resolves `repr`
/// to a concrete [`KType`] (rejecting unresolved bare-leaf carriers), mints a
/// [`KType::UserType`] with `kind: UserTypeKind::Newtype { repr }`, and writes the
/// identity into `bindings.types` via [`crate::runtime::machine::core::Bindings::try_register_type`].
///
/// Unlike STRUCT / named-UNION's [`crate::runtime::machine::core::Bindings::try_register_nominal`]
/// dual-write, NEWTYPE writes *only* `types`. The declaration has no payload value
/// to bind — there is no schema carrier paired with the identity. The construction
/// path keys on the identity alone (via [`Scope::resolve_type`]) and routes through
/// [`newtype_construct`] in [`super::type_call`]'s `Newtype` arm.
///
/// Returns the minted identity as a `KObject::KTypeValue(KType)` so the surface
/// form `NEWTYPE Distance = Number` evaluates to a Type value, mirroring STRUCT /
/// UNION declaration returns.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // Same shared helper STRUCT / UNION use — rejects parameterized binder forms
    // (`NEWTYPE Foo<X> = ...`) which functors don't ship today.
    let name = match extract_bare_type_name(&bundle, "name", "NEWTYPE") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    // The repr slot is `TypeExprRef`, so the carrier is either `KTypeValue(KType)`
    // (builtin leaves / structural shapes resolved at `resolve_for` time) or
    // `TypeNameRef(TypeExpr, _)` (bare-leaf names not in `KType::from_name`'s table).
    // Peek before extracting so we route to the right helper — both consume the slot.
    let repr: KType = match bundle.get("repr") {
        Some(KObject::KTypeValue(_)) => match extract_ktype(&mut bundle, "repr") {
            Some(t) => t,
            None => unreachable!("get(KTypeValue) then extract_ktype must succeed"),
        },
        Some(KObject::TypeNameRef(_, _)) => {
            // Carrier path: a bare leaf the parser couldn't lower (`NEWTYPE Bar = Foo`
            // where `Foo` is itself user-declared). Walk the scope chain for the
            // resolved identity; reject if unresolved (the NEWTYPE declaration is the
            // identity-minting site, not a producer of pending placeholders).
            let te = match extract_type_name_ref(&mut bundle, "repr") {
                Some(te) => te,
                None => unreachable!("get(TypeNameRef) then extract_type_name_ref must succeed"),
            };
            if !matches!(te.params, TypeParams::None) {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "NEWTYPE repr must be a bare type name, got `{}`",
                    te.render(),
                ))));
            }
            match scope.resolve_type(&te.name) {
                Some(kt) => kt.clone(),
                None => {
                    return err(KError::new(KErrorKind::ShapeError(format!(
                        "NEWTYPE repr slot = unknown type name `{}`",
                        te.name,
                    ))));
                }
            }
        }
        _ => {
            return err(KError::new(KErrorKind::ShapeError(
                "NEWTYPE repr slot must be a type expression (e.g. `Number`, `Foo`)".to_string(),
            )));
        }
    };
    // Per-declaration identity: `scope_id` is the declaring scope's address, same scheme
    // STRUCT / UNION / MODULE use. The repr lives variant-internally on the `Newtype`
    // arm; identity equality (per the manual `UserTypeKind::PartialEq`) ignores `repr`
    // so the wildcard `AnyUserType { kind: Newtype { repr: <sentinel> } }` admits any
    // concrete identity.
    let scope_id = scope.id;
    let identity = KType::UserType {
        kind: UserTypeKind::Newtype { repr: Box::new(repr) },
        scope_id,
        name: name.clone(),
    };
    let arena = scope.arena;
    let kt_ref: &'a KType = arena.alloc_ktype(identity);
    match scope.bindings().try_register_type(&name, kt_ref) {
        Ok(ApplyOutcome::Applied) => {
            // Mirror STRUCT / UNION's declaration return: the value is a `KTypeValue`
            // carrying a clone of the minted identity. Tests inspect `bindings.types`
            // for the persisted entry; surface-level code receives the Type-value for
            // potential chaining (`LET D = NEWTYPE Distance = Number` style).
            let v: &'a KObject<'a> = arena.alloc_object(KObject::KTypeValue(kt_ref.clone()));
            BodyResult::Value(v)
        }
        // Borrow contention at the declaration site is a programming error — finalize
        // sites run post-Combine outside the re-entrant hot path. Surface as a
        // structured error rather than panicking so a future re-entrant caller still
        // gets a recoverable diagnostic.
        Ok(ApplyOutcome::Conflict) => err(KError::new(KErrorKind::ShapeError(format!(
            "NEWTYPE `{name}` registration deferred = bindings borrow contention",
        )))),
        Err(e) => err(e),
    }
}

/// Dispatch-time placeholder extractor. Same shape STRUCT / UNION use — the binder
/// name lives at `parts[1]` (after the `NEWTYPE` keyword).
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

/// Construction entry point reached from [`super::type_call`]'s `Newtype` arm. The
/// verb resolved type-side to `identity` (`KType::UserType { kind: Newtype, .. }`);
/// `parts` is the unevaluated argument list from the type-call site
/// (`Distance(3.0)` → `[Literal(3.0)]`, `Distance(2.0 + 1.0)` → operator-form parts,
/// `Bar(Foo(3.0))` → `[Type(Foo), Expression([Literal(3.0)])]`).
///
/// Schedules the value sub-expression through `add_dispatch` on the outer scheduler,
/// then registers a `Combine` whose finish closure receives the resolved inner value,
/// validates it against the newtype's `repr`, applies the collapse rule (extracting
/// `*inner` if the value is itself `Wrapped`), and produces the final
/// `KObject::Wrapped`. Returns `BodyResult::DeferTo(combine_id)` so the type-call
/// slot's terminal lifts off the Combine's terminal — same pattern as `module_def`,
/// `struct_def::defer_struct_via_combine`, and `sig_def`.
///
/// **No upper arity check** on `parts.len()`: surface forms like `Bar(Foo(3.0))`
/// legitimately produce multi-part `parts` (`[Type(Foo), Expression([Literal(3.0)])]`)
/// that the scheduler dispatches as one expression yielding one inner value. The
/// scheduler surfaces any structural mismatch as `DispatchFailed` through the dep's
/// terminal — short-circuits the Combine and we never run the finish closure.
pub fn newtype_construct<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    identity: &'a KType,
    parts: Vec<ExpressionPart<'a>>,
) -> BodyResult<'a> {
    if parts.is_empty() {
        return err(KError::new(KErrorKind::ArityMismatch { expected: 1, got: 0 }));
    }
    // Dispatch the value sub-expression on the outer scheduler. Any binding lookup,
    // operator resolution, or nested type-call inside the parts resolves through the
    // standard dispatch loop — the Combine's finish closure sees only the terminalized
    // inner value, already arena-resident.
    let value_expr = KExpression { parts };
    let value_id = sched.add_dispatch(value_expr, scope);
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        debug_assert_eq!(results.len(), 1, "newtype_construct registered exactly one dep");
        let value: &'a KObject<'a> = results[0];
        // The identity is `&'a KType`, arena-resident — moved into the closure as a
        // ref, no clone needed. Recover the `repr` for the type-check; the
        // `unreachable!` is structurally guarded by `type_call::body`'s match arm
        // that routes only `UserTypeKind::Newtype` here.
        let repr: &KType = match identity {
            KType::UserType { kind: UserTypeKind::Newtype { repr }, .. } => repr.as_ref(),
            _ => unreachable!("type_call routed non-Newtype identity into newtype_construct"),
        };
        if !repr.matches_value(value) {
            return BodyResult::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "value".to_string(),
                expected: repr.name(),
                got: value.ktype().name(),
            }));
        }
        // Collapse invariant: a `Wrapped` inner is invariantly non-`Wrapped` by
        // induction (every prior construction collapsed through this same closure).
        // `Bar(some_foo)` where `some_foo: Foo` takes `some_foo.inner` directly so
        // the produced `Wrapped` is exactly one layer over the bottom representation
        // value. Avoids unbounded nesting and lets stage-4.C ATTR fall-through recurse
        // only one level.
        let inner_ref: &'a KObject<'a> = match value {
            KObject::Wrapped { inner, .. } => inner,
            // `add_dispatch` writes its result into an arena slot, so `value` is
            // already arena-resident — reuse it directly.
            _ => value,
        };
        let wrapped = KObject::Wrapped {
            inner: inner_ref,
            type_id: identity,
        };
        BodyResult::Value(scope.arena.alloc_object(wrapped))
    });
    let combine_id = sched.add_combine(vec![value_id], scope, finish);
    BodyResult::DeferTo(combine_id)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Surface declaration form: `NEWTYPE <name> = <repr>`. Construction is driven
    // from `type_call::body`'s `Newtype` arm via `newtype_construct`; no second
    // registered builtin (a separate value-side primitive would share `type_call`'s
    // signature bucket and re-dispatch infinitely).
    register_builtin_with_pre_run(
        scope,
        "NEWTYPE",
        sig(KType::Type, vec![
            kw("NEWTYPE"),
            arg("name", KType::TypeExprRef),
            kw("="),
            arg("repr", KType::TypeExprRef),
        ]),
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{
        parse_one, run, run_one, run_one_err, run_root_silent,
    };
    use crate::runtime::machine::{KErrorKind, RuntimeArena, Scheduler};
    use crate::runtime::machine::model::types::UserTypeKind;
    use crate::runtime::machine::model::{KObject, KType};

    /// NEWTYPE declaration writes the per-declaration identity into `bindings.types`
    /// (with `kind: Newtype { repr: <resolved> }`) and writes *nothing* into
    /// `bindings.data` — the declaration has no payload value to bind. Stage-3-style
    /// dual-write does not apply to NEWTYPE.
    #[test]
    fn declare_mints_newtype_identity() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run_one(scope, parse_one("NEWTYPE Distance = Number"));
        let types = scope.bindings().types();
        let kt = types
            .get("Distance")
            .expect("Distance should be in bindings.types");
        match **kt {
            KType::UserType {
                kind: UserTypeKind::Newtype { ref repr },
                ref name,
                ..
            } => {
                assert_eq!(name, "Distance");
                assert_eq!(**repr, KType::Number);
            }
            ref other => panic!("expected Newtype identity, got {other:?}"),
        }
        drop(types);
        let data = scope.bindings().data();
        assert!(
            data.get("Distance").is_none(),
            "NEWTYPE must not write a value-side carrier",
        );
    }

    /// `Distance(3.0)` returns a `Wrapped` whose `ktype()` reports the `Distance`
    /// identity and whose `inner` is the bare `Number`. The surface NEWTYPE call goes
    /// through `type_call`'s `Newtype` arm into `newtype_construct`'s Combine.
    #[test]
    fn construct_wraps_repr_matching_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Distance = Number");
        let result = run_one(scope, parse_one("Distance (3.0)"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::UserType {
                        kind: UserTypeKind::Newtype { .. },
                        ref name,
                        ..
                    } => assert_eq!(name, "Distance"),
                    ref other => panic!("expected Newtype type_id, got {other:?}"),
                }
                assert!(matches!(inner, KObject::Number(n) if *n == 3.0));
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// `Distance("hi")` (Number repr, Str value) surfaces as `TypeMismatch` — the
    /// Combine's finish closure rejects when `value.ktype()` doesn't match `repr`.
    #[test]
    fn construct_rejects_non_matching_repr() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Distance = Number");
        let err = run_one_err(scope, parse_one("Distance (\"hi\")"));
        assert!(
            matches!(&err.kind, KErrorKind::TypeMismatch { expected, got, .. }
                if expected == "Number" && got == "Str"),
            "expected TypeMismatch(Number, Str), got {err}",
        );
    }

    /// Newtype-over-newtype collapse: `NEWTYPE Foo = Number; NEWTYPE Bar = Foo`;
    /// constructing `Bar(Foo(3.0))` produces a single-layer `Wrapped { type_id: Bar,
    /// inner: Number(3.0) }`. Pins the construction-time collapse invariant.
    #[test]
    fn newtype_over_newtype_collapses() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Foo = Number\nNEWTYPE Bar = Foo");
        let result = run_one(scope, parse_one("Bar (Foo (3.0))"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::UserType { ref name, .. } => assert_eq!(name, "Bar"),
                    ref other => panic!("expected Bar identity, got {other:?}"),
                }
                // Critical: `inner` must be the bare Number, NOT another Wrapped.
                assert!(
                    matches!(inner, KObject::Number(n) if *n == 3.0),
                    "expected bare Number inner, got {:?}",
                    inner.ktype(),
                );
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// Per-declaration dispatch: a FN with a `Number` slot rejects a `Distance`
    /// value; a FN with a `Distance` slot rejects a raw `Number`. NEWTYPE produces
    /// fresh nominal identity — `Distance` and `Number` are observably distinct at
    /// dispatch.
    ///
    /// The rejection lands as `DispatchFailed` out of `Scheduler::execute` rather
    /// than a per-slot `Err` terminal — the per-slot type check filters out the only
    /// candidate, so the scope chain runs out without a match. Same shape as
    /// `fn_def::tests::param_type::fn_typed_param_rejects_mismatched_call`; use the
    /// scheduler directly (not `run_one_err`, which expects a per-slot Err result).
    #[test]
    fn dispatch_distinguishes_distance_from_number() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "NEWTYPE Distance = Number\n\
             FN (TAKES_NUM x :Number) -> Str = (\"num\")\n\
             FN (TAKES_DIST x :Distance) -> Str = (\"dist\")",
        );
        // Distance-typed slot accepts a Distance value.
        let r1 = run_one(scope, parse_one("TAKES_DIST (Distance (3.0))"));
        match r1 {
            KObject::KString(s) => assert_eq!(s, "dist"),
            other => panic!("expected \"dist\", got {:?}", other.ktype()),
        }
        // Number-typed slot accepts a raw Number.
        let r2 = run_one(scope, parse_one("TAKES_NUM (3.0)"));
        match r2 {
            KObject::KString(s) => assert_eq!(s, "num"),
            other => panic!("expected \"num\", got {:?}", other.ktype()),
        }
        // Number-typed slot rejects a Distance — surfaces as a dispatch failure
        // (no matching overload).
        let mut sched1 = Scheduler::new();
        sched1.add_dispatch(parse_one("TAKES_NUM (Distance (3.0))"), scope);
        let err = sched1
            .execute()
            .expect_err("TAKES_NUM on Distance should fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on Number-slot Distance, got {err}",
        );
        // Distance-typed slot rejects a raw Number — symmetric.
        let mut sched2 = Scheduler::new();
        sched2.add_dispatch(parse_one("TAKES_DIST (3.0)"), scope);
        let err2 = sched2
            .execute()
            .expect_err("TAKES_DIST on raw Number should fail dispatch");
        assert!(
            matches!(&err2.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on Distance-slot Number, got {err2}",
        );
    }

    /// `LET x = 3.0; Distance(x)` resolves the inner identifier through `value_lookup`
    /// inside the Combine's dispatched dep, then wraps. Pins the non-trivial-dispatch
    /// path — the Combine waits on the dep's terminalization before the finish
    /// closure runs.
    #[test]
    fn construct_with_identifier_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Distance = Number\nLET x = 3.0");
        let result = run_one(scope, parse_one("Distance (x)"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::UserType { ref name, .. } => assert_eq!(name, "Distance"),
                    ref other => panic!("expected Distance identity, got {other:?}"),
                }
                assert!(matches!(inner, KObject::Number(n) if *n == 3.0));
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// `Distance ()` (zero-argument type-call) surfaces as `ArityMismatch { expected:
    /// 1, got: 0 }`. Pins the pre-dispatch arity guard in `newtype_construct`.
    #[test]
    fn construct_arity_zero_rejects() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Distance = Number");
        let err = run_one_err(scope, parse_one("Distance ()"));
        assert!(
            matches!(&err.kind, KErrorKind::ArityMismatch { expected: 1, got: 0 }),
            "expected ArityMismatch(1, 0) on Distance(), got {err}",
        );
    }

    /// `Distance(MAKE_NUM (3.0))` resolves the inner FN call inside the Combine's
    /// dispatched dep, then wraps. Pins the "any sub-expression" claim — the
    /// value-part doesn't have to be a literal. Koan has no arithmetic operators
    /// today (per TUTORIAL.md § "No arithmetic, comparison, or logical operators"),
    /// so a user-fn call stands in for the "non-trivial dispatch in the value
    /// position" shape the plan calls out.
    #[test]
    fn construct_with_operator_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "NEWTYPE Distance = Number\n\
             FN (MAKE_NUM x :Number) -> Number = (x)",
        );
        let result = run_one(scope, parse_one("Distance (MAKE_NUM 3.0)"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::UserType { ref name, .. } => assert_eq!(name, "Distance"),
                    ref other => panic!("expected Distance identity, got {other:?}"),
                }
                assert!(matches!(inner, KObject::Number(n) if *n == 3.0));
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }
}
