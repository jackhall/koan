//! Happy-path: a FUNCTOR declaration produces a registered `KFunction` with
//! `is_functor: true`, and the carrier's `ktype()` projects to `KFunctor`.

use crate::builtins::test_support::{lookup_fn, run, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::RuntimeArena;

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
    assert!(
        f.is_functor,
        "FUNCTOR-bound KFunction must carry is_functor: true"
    );
}

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

#[test]
fn let_type_class_admits_functor_rhs() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         LET MyF = (FUNCTOR (MAKESET Er :OrderedSig) -> Module = (MODULE Result = (LET inner = 1)))",
    );
    let obj = scope.lookup("MyF").expect("MyF should be value-bound");
    assert!(
        matches!(obj, KObject::KFunction(f, _) if f.is_functor),
        "MyF should resolve to a FUNCTOR-flagged KFunction, got {:?}",
        obj.ktype(),
    );
}
