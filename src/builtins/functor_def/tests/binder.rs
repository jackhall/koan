//! Happy-path: a FUNCTOR declaration produces a registered `KFunction` with
//! `is_functor: true`, and the carrier's `ktype()` projects to `KFunctor`.

use crate::builtins::test_support::{lookup_fn, run, run_root_silent};
use crate::machine::core::FrameStorage;
use crate::machine::model::KType;

#[test]
fn functor_binder_sets_is_functor_flag() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FUNCTOR (MAKESET Er :OrderedSig) -> Module = (MODULE Generated = (LET inner = 1))",
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
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FUNCTOR (MAKESET Er :OrderedSig) -> Module = (MODULE Generated = (LET inner = 1))",
    );
    let f = lookup_fn(scope, "MAKESET");
    let obj = KObject::KFunction(f, None);
    match obj.ktype() {
        KType::KFunctor { .. } => {}
        other => panic!("expected KFunctor projection, got {}", other.name()),
    }
}

/// A `LET <Type-class> = (FUNCTOR …)` name-binding registers the functor *type-side*
/// (`bindings.types` as `KType::KFunctor { body: Some(f) }`), never in `bindings.data`.
#[test]
fn let_type_class_admits_functor_rhs() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         LET MyF = (FUNCTOR (MAKESET Er :OrderedSig) -> Module = (MODULE Generated = (LET inner = 1)))",
    );
    assert!(
        scope.lookup("MyF").is_none(),
        "MyF must NOT be value-bound — a functor name registers type-side",
    );
    let kt = scope
        .resolve_type("MyF")
        .expect("MyF should be type-bound in bindings.types");
    assert!(
        matches!(kt, KType::KFunctor { body: Some(f), .. } if f.is_functor),
        "MyF should resolve type-side to a KFunctor carrying the callable body, got {:?}",
        kt,
    );
}
