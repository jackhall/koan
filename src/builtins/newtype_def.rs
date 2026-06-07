//! `NEWTYPE <name> = <repr>` — declare a fresh nominal identity over a transparent
//! representation. The declaration writes only `bindings.types` (no value-side
//! schema carrier). Construction produces a [`KObject::Wrapped`] tagging the inner
//! value with the NEWTYPE identity; the `Wrapped.inner` is invariantly non-`Wrapped`
//! (newtype-over-newtype collapses to a single layer).

use std::rc::Rc;

use crate::machine::core::kfunction::argument_bundle::{
    extract_bare_type_name, extract_ktype, extract_type_name_ref,
};
use crate::machine::core::ApplyOutcome;
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{NominalKind, NominalMember, NominalSchema, RecursiveSet};
use crate::machine::model::values::KObject;
use crate::machine::model::KType;
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, KError, KErrorKind, Resolution,
    SchedulerHandle, Scope,
};

use super::{arg, err, kw, register_builtin_with_binder, sig};

/// Body of `NEWTYPE <name> = <repr>`. Seals a singleton [`RecursiveSet`] over one
/// [`NominalKind::Newtype`] member (`repr` as its [`NominalSchema::Newtype`]), writes the
/// `SetRef` identity into `bindings.types`, and returns it as a `KObject::KTypeValue` so the
/// surface form evaluates to a Type value.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match extract_bare_type_name(&bundle, "name", "NEWTYPE") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    // Gated to the NEWTYPE's lexical position: a repr naming a later type is a position
    // error, like any other forward type reference.
    let chain = sched.current_lexical_chain();
    let bind_index = chain
        .as_ref()
        .map(|c| BindingIndex::value(c.index))
        .unwrap_or(BindingIndex::BUILTIN);
    // `TypeExprRef` carriers split two ways: `KTypeValue` for resolved leaves /
    // structural shapes, `TypeNameRef` for bare-leaf names. Peek before extracting
    // so we route to the right helper — both consume the slot.
    match bundle.get("repr") {
        Some(KObject::KTypeValue(_)) => {
            let repr = match extract_ktype(&mut bundle, "repr") {
                Some(t) => t,
                None => unreachable!("get(KTypeValue) then extract_ktype must succeed"),
            };
            finalize_newtype(scope, name, repr, bind_index)
        }
        Some(KObject::TypeNameRef(_)) => {
            // Bare-leaf carrier (`NEWTYPE Bar = Foo` where `Foo` is user-declared):
            // walk the scope chain for the resolved identity.
            let te = match extract_type_name_ref(&mut bundle, "repr") {
                Some(te) => te,
                None => unreachable!("get(TypeNameRef) then extract_type_name_ref must succeed"),
            };
            if let Some(kt) = scope.resolve_type_with_chain(te.as_str(), chain.as_deref()) {
                return finalize_newtype(scope, name, kt.clone(), bind_index);
            }
            // The repr names a type that is still finalizing in this scheduler — e.g. a
            // record-repr dependency whose `:{…}` defers its own finalize behind a
            // sub-dispatch, so this dependent's body can run first. Park on the
            // producer and re-resolve at Combine-finish, once its identity is in
            // `types`. A name with no in-flight producer is a genuine forward/unknown
            // reference — a position error.
            if let Resolution::Placeholder(producer) =
                scope.resolve_with_chain(te.as_str(), chain.as_deref())
            {
                let finish: CombineFinish<'a> = Box::new(move |scope, _sched, _results| {
                    match scope.resolve_type_with_chain(te.as_str(), chain.as_deref()) {
                        Some(kt) => finalize_newtype(scope, name, kt.clone(), bind_index),
                        None => err(KError::new(KErrorKind::ShapeError(format!(
                            "NEWTYPE repr slot = unknown type name `{}`",
                            te.as_str(),
                        )))),
                    }
                });
                let combine_id = sched.add_combine(Vec::new(), vec![producer], scope, finish);
                return BodyResult::DeferTo(combine_id);
            }
            err(KError::new(KErrorKind::ShapeError(format!(
                "NEWTYPE repr slot = unknown type name `{}`",
                te.as_str(),
            ))))
        }
        _ => err(KError::new(KErrorKind::ShapeError(
            "NEWTYPE repr slot must be a type expression (e.g. `Number`, `Foo`)".to_string(),
        ))),
    }
}

/// Seal a resolved `repr` into the NEWTYPE's identity and register it. A NEWTYPE is
/// non-recursive (its `repr` is already resolved), so it seals into a singleton set of one
/// member. The wildcard `AnyUserType { kind: Newtype }` admits any such member, since
/// identity never descends `repr`.
fn finalize_newtype<'a>(
    scope: &'a Scope<'a>,
    name: String,
    repr: KType<'a>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    let scope_id = scope.id;
    let member = NominalMember::pending(name.clone(), scope_id, NominalKind::Newtype);
    member.fill(NominalSchema::Newtype(Box::new(repr)));
    let set = Rc::new(RecursiveSet::new(vec![member]));
    let identity = KType::SetRef { set, index: 0 };
    let kt_ref: &'a KType = scope.arena.alloc_ktype(identity);
    match scope
        .bindings()
        .try_register_type(&name, kt_ref, bind_index)
    {
        Ok(ApplyOutcome::Applied) => {
            BodyResult::Value(scope.arena.alloc_object(KObject::KTypeValue(kt_ref.clone())))
        }
        // Finalize sites run post-Combine outside the re-entrant hot path, so borrow
        // contention here is a programming error. Surface as a structured error rather
        // than panicking — a future re-entrant caller still gets a recoverable diag.
        Ok(ApplyOutcome::Conflict) => err(KError::new(KErrorKind::ShapeError(format!(
            "NEWTYPE `{name}` registration deferred = bindings borrow contention",
        )))),
        Err(e) => err(e),
    }
}

/// Dispatch-time placeholder extractor.
pub(crate) fn binder_name(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Only the declaration form is registered; construction lives in the `TypeCall` fast lane
    // via `constructors::dispatch_construct_newtype`.
    register_builtin_with_binder(
        scope,
        "NEWTYPE",
        sig(
            KType::Type,
            vec![
                kw("NEWTYPE"),
                arg("name", KType::TypeExprRef),
                kw("="),
                arg("repr", KType::TypeExprRef),
            ],
        ),
        body,
        Some(binder_name),
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::machine::execute::Scheduler;
    use crate::machine::model::types::{NominalKind, ProjectedSchema, RecursiveSet};
    use crate::machine::model::{KObject, KType};
    use crate::machine::{KErrorKind, RuntimeArena};

    /// NEWTYPE writes the `SetRef` identity into `bindings.types` and nothing into
    /// `bindings.data` — the declaration has no payload value to bind.
    #[test]
    fn declare_mints_newtype_identity() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run_one(scope, parse_one("NEWTYPE Distance = Number"));
        let types = scope.bindings().types();
        let (kt, _) = types
            .get("Distance")
            .expect("Distance should be in bindings.types");
        match **kt {
            KType::SetRef { ref set, index } => {
                assert_eq!(set.member(index).name, "Distance");
                assert_eq!(set.member(index).kind, NominalKind::Newtype);
                match RecursiveSet::projected_schema(set, index) {
                    ProjectedSchema::Newtype(repr) => assert_eq!(repr, KType::Number),
                    _ => panic!("expected a Newtype schema"),
                }
            }
            ref other => panic!("expected Newtype SetRef identity, got {other:?}"),
        }
        drop(types);
        let data = scope.bindings().data();
        assert!(
            data.get("Distance").is_none(),
            "NEWTYPE must not write a value-side carrier",
        );
    }

    /// `Distance(3.0)` returns a `Wrapped` whose `ktype()` is `Distance` and whose
    /// `inner` is the bare `Number`.
    #[test]
    fn construct_wraps_repr_matching_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Distance = Number");
        let result = run_one(scope, parse_one("Distance (3.0)"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::SetRef { ref set, index } => {
                        assert_eq!(set.member(index).name, "Distance");
                        assert_eq!(set.member(index).kind, NominalKind::Newtype);
                    }
                    ref other => panic!("expected Newtype SetRef type_id, got {other:?}"),
                }
                assert!(matches!(inner.get(), KObject::Number(n) if *n == 3.0));
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// `Distance("hi")` (Number repr, Str value) surfaces as `TypeMismatch`.
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

    /// A record-repr NEWTYPE and a NEWTYPE depending on it, declared in the *same*
    /// scheduler, then constructed. The dependency's `:{…}` defers its finalize behind a
    /// sub-dispatch, so the dependent's body would run first; it must park on the
    /// dependency's producer rather than error on an unresolved repr (which previously
    /// leaked a stale value-side placeholder that panicked the next construction).
    #[test]
    fn dependent_newtype_parks_on_record_repr_dependency() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "NEWTYPE Point = :{x :Number, y :Number}\nNEWTYPE Boxed = Point",
        );
        // No placeholder may survive the declaration run: a leaked one corrupts the next
        // scheduler on this REPL-persistent scope.
        assert!(
            scope.bindings().placeholders().is_empty(),
            "NEWTYPE declarations must leave no value-side placeholder, got {:?}",
            *scope.bindings().placeholders(),
        );
        let result = run_one(scope, parse_one("(Boxed (Point {x = 1, y = 2}))"));
        assert!(
            matches!(result, KObject::Wrapped { .. }),
            "expected Wrapped, got {:?}",
            result.ktype(),
        );
    }

    /// A NEWTYPE whose repr names a genuinely unknown type errors — and clears the
    /// value-side placeholder its dispatch installed, so a later construction of the same
    /// name fails cleanly (unbound) rather than tripping over a leaked producer `NodeId`.
    #[test]
    fn unknown_repr_errors_without_leaking_placeholder() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Boxed = Nope");
        assert!(
            scope.bindings().placeholders().is_empty(),
            "a failed NEWTYPE must not leak its placeholder, got {:?}",
            *scope.bindings().placeholders(),
        );
        let err = run_one_err(scope, parse_one("(Boxed (3.0))"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(n) if n == "Boxed"),
            "expected UnboundName(Boxed) after failed declaration, got {err}",
        );
    }

    /// `Bar(Foo(3.0))` produces a single-layer `Wrapped { type_id: Bar,
    /// inner: Number(3.0) }` — pins the collapse invariant.
    #[test]
    fn newtype_over_newtype_collapses() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Foo = Number\nNEWTYPE Bar = Foo");
        let result = run_one(scope, parse_one("Bar (Foo (3.0))"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::SetRef { ref set, index } => assert_eq!(set.member(index).name, "Bar"),
                    ref other => panic!("expected Bar identity, got {other:?}"),
                }
                // Critical: `inner` must be the bare Number, NOT another Wrapped.
                assert!(
                    matches!(inner.get(), KObject::Number(n) if *n == 3.0),
                    "expected bare Number inner, got {:?}",
                    inner.get().ktype(),
                );
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// `Distance` and `Number` are observably distinct at dispatch.
    ///
    /// Rejection lands as `DispatchFailed` out of `Scheduler::execute` (the per-slot
    /// type check filters the only candidate, scope chain runs out without a match)
    /// — drive the scheduler directly rather than `run_one_err`, which expects a
    /// per-slot Err result.
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
        let r1 = run_one(scope, parse_one("TAKES_DIST (Distance (3.0))"));
        match r1 {
            KObject::KString(s) => assert_eq!(s, "dist"),
            other => panic!("expected \"dist\", got {:?}", other.ktype()),
        }
        let r2 = run_one(scope, parse_one("TAKES_NUM (3.0)"));
        match r2 {
            KObject::KString(s) => assert_eq!(s, "num"),
            other => panic!("expected \"num\", got {:?}", other.ktype()),
        }
        let mut sched1 = Scheduler::new();
        sched1.add_dispatch(parse_one("TAKES_NUM (Distance (3.0))"), scope);
        let err = sched1
            .execute()
            .expect_err("TAKES_NUM on Distance should fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on Number-slot Distance, got {err}",
        );
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

    /// `Distance(x)` resolves the inner identifier inside the Combine's dispatched
    /// dep before the finish closure runs — pins the non-trivial-dispatch path.
    #[test]
    fn construct_with_identifier_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Distance = Number\nLET x = 3.0");
        let result = run_one(scope, parse_one("Distance (x)"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::SetRef { ref set, index } => {
                        assert_eq!(set.member(index).name, "Distance")
                    }
                    ref other => panic!("expected Distance identity, got {other:?}"),
                }
                assert!(matches!(inner.get(), KObject::Number(n) if *n == 3.0));
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// Pins the pre-dispatch arity guard: `Distance ()` rejects with `ArityMismatch`.
    #[test]
    fn construct_arity_zero_rejects() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Distance = Number");
        let err = run_one_err(scope, parse_one("Distance ()"));
        assert!(
            matches!(
                &err.kind,
                KErrorKind::ArityMismatch {
                    expected: 1,
                    got: 0
                }
            ),
            "expected ArityMismatch(1, 0) on Distance(), got {err}",
        );
    }

    /// Pins the "any sub-expression in the value position" path. Koan has no
    /// arithmetic operators today (per TUTORIAL.md § "No arithmetic, comparison, or
    /// logical operators"), so a user-fn call stands in for non-trivial dispatch.
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
                    KType::SetRef { ref set, index } => {
                        assert_eq!(set.member(index).name, "Distance")
                    }
                    ref other => panic!("expected Distance identity, got {other:?}"),
                }
                assert!(matches!(inner.get(), KObject::Number(n) if *n == 3.0));
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }
}
