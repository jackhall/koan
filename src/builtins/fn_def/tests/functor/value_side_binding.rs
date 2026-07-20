//! A functor — a module-returning function — is an ordinary FN: it binds value-side under a
//! snake_case name in `bindings.data`, is applied by the ordinary keyworded call convention, and
//! its `ktype()` is `KType::KFunction`. `bindings.types` holds no callable value.

use crate::builtins::test_support::{parse_one, TestRun};
use crate::machine::model::{KObject, KType};
use crate::machine::run_root_storage;

const SETUP: &str = "SIG Ordered = (VAL compare :Number)\n\
                     MODULE int_ord = (LET compare = 7)\n\
                     LET make_set = \
                       (FN (MAKESET er :Ordered) -> Module = (MODULE result = (LET inner = 1)))";

#[test]
fn module_returning_fn_binds_value_side() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(SETUP);

    let bound = scope
        .bindings()
        .data()
        .get("make_set")
        .map(|(object, _, _)| *object);
    assert!(
        matches!(bound, Some(KObject::KFunction(_))),
        "make_set must bind value-side in bindings.data as a KFunction, got {:?}",
        bound.map(|object| object.ktype()),
    );
    assert!(
        scope.lookup("make_set").is_some(),
        "make_set must resolve through the ordinary value channel",
    );
}

#[test]
fn module_returning_fn_ktype_is_kfunction() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(SETUP);

    let bound = scope
        .lookup("make_set")
        .expect("make_set must bind value-side");
    let ktype = bound.ktype();
    assert!(
        matches!(ktype, KType::KFunction { .. }),
        "a module-returning FN's ktype() must be KType::KFunction, got {:?}",
        ktype,
    );
}

#[test]
fn module_returning_fn_applies_by_the_keyworded_call_convention() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(SETUP);

    let result = test_run.run_one(parse_one("MAKESET int_ord"));
    let module = match result {
        KObject::Module(module) => module,
        other => panic!(
            "(MAKESET int_ord) must return a module, got {}",
            other.summarize(&test_run.types),
        ),
    };
    let inner = module
        .child_scope()
        .bindings()
        .data()
        .get("inner")
        .map(|(object, _, _)| *object);
    assert!(
        matches!(inner, Some(KObject::Number(n)) if *n == 1.0),
        "the returned module must carry inner = 1, got {:?}",
        inner.map(|object| object.ktype()),
    );
}
