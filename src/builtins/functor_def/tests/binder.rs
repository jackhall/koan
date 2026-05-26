//! Happy-path tests: a FUNCTOR declaration produces a registered `KFunction`
//! with `is_functor: true`, and the carrier's `ktype()` projects to
//! `KType::KFunctor` per the Stage-2 / Stage-3 contract.

use crate::builtins::test_support::{lookup_fn, run, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::RuntimeArena;

/// Smallest possible FUNCTOR that passes the return-type admissibility check:
/// a 1-ary functor over a signature-typed parameter that returns a module
/// value. The constructed `KFunction` carries `is_functor: true`; reading
/// `ktype()` off the carrier produces a `KType::KFunctor` (Stage-2 projection
/// branch) rather than `KType::KFunction`.
#[test]
fn functor_binder_sets_is_functor_flag() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FUNCTOR (MAKESET Er :OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
    );
    let f = lookup_fn(scope, "MAKESET");
    assert!(f.is_functor, "FUNCTOR-bound KFunction must carry is_functor: true");
}

/// `function_value_ktype` projects `is_functor: true` to `KType::KFunctor` —
/// the registered FUNCTOR's `KFunction::ktype` returns a `KFunctor` (not a
/// `KFunction`). The carrier minted at FUNCTOR-binder finalize time goes
/// through `register_function`; we reach the carrier via the dispatch
/// table's value-side mirror by synthesizing a `KObject::KFunction`.
#[test]
fn functor_carrier_ktype_projects_to_kfunctor() {
    use crate::machine::model::values::KObject;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FUNCTOR (MAKESET Er :OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
    );
    let f = lookup_fn(scope, "MAKESET");
    let obj = KObject::KFunction(f, None);
    match obj.ktype() {
        KType::KFunctor { .. } => {}
        other => panic!("expected KFunctor projection, got {}", other.name()),
    }
}

/// `LET F = (FUNCTOR …)` happy path: with the Stage-5 allowlist flip the LET
/// gate must admit a functor-flagged KFunction as a Type-class RHS. Pre-flip
/// this case silently landed under `bindings.data` only; post-flip the
/// FUNCTOR-flagged carrier passes the allowlist and the binding lands.
#[test]
fn let_type_class_admits_functor_rhs() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         LET MyF = (FUNCTOR (MAKESET Er :OrderedSig) -> Module = (MODULE Result = (LET inner = 1)))",
    );
    // The Type-class binder name must end up reachable as a value, since
    // FUNCTOR-flagged functions are not type-language carriers — the
    // allowlist routes them through `bind_value` (no `nominal_identity` /
    // `register_type` arm fires for `is_functor` KFunctions).
    let obj = scope.lookup("MyF").expect("MyF should be value-bound");
    assert!(
        matches!(obj, KObject::KFunction(f, _) if f.is_functor),
        "MyF should resolve to a FUNCTOR-flagged KFunction, got {:?}",
        obj.ktype(),
    );
}
