//! The principal signature (self-sig) a module or ascription view carries, derived at
//! creation. A plain module records its manifest members and raw value-slot types; an opaque
//! view records per-call abstract identities and re-expresses SIG-declared value slots against
//! them; a transparent view records the source's concrete types.

use crate::builtins::test_support::{
    parse_one, register_arity1_constructor, run, run_one_err, run_root_silent,
};
use crate::machine::core::run_root_storage;
use crate::machine::model::values::Module;
use crate::machine::model::KType;
use crate::machine::{KErrorKind, Scope};

fn module_named<'a>(scope: &'a Scope<'a>, name: &str) -> &'a Module<'a> {
    match scope.resolve_type(name) {
        Some(KType::Module { module }) => module,
        other => panic!("`{name}` should resolve to a module identity, got {other:?}"),
    }
}

#[test]
fn plain_module_self_sig_is_manifest_and_raw_value_slots() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE IntOrd = ((LET Tag = Number) (LET compare = 5))",
    );
    let m = module_named(scope, "IntOrd");
    let sig = m.self_sig();
    assert!(
        sig.abstract_members.is_empty(),
        "a plain module has no abstract members"
    );
    assert_eq!(sig.manifest_members.get("Tag"), Some(&KType::Number));
    // `compare = 5` reads its raw value type — a plain module records no declared type.
    assert_eq!(sig.value_slots.get("compare"), Some(&KType::Number));
}

#[test]
fn opaque_view_self_sig_carries_abstract_identity_in_slots() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE IntOrd = ((LET Elem = Number) (LET zero = 0) \
         (LET compare = (FN :{x :Number} -> Number = (x))))\n\
         SIG OrderedSig = ((TYPE Elem) (VAL zero :Elem) \
         (VAL compare :(FN (x :Elem) -> Number)))\n\
         LET View = (IntOrd :| OrderedSig)",
    );
    let view = module_named(scope, "View");
    let sig = view.self_sig();

    // The view's manifest `Elem` is the per-call abstract identity it minted.
    let elem_abstract = view
        .type_members
        .borrow()
        .get("Elem")
        .cloned()
        .expect("opaque view mints an abstract `Elem`");
    assert!(matches!(elem_abstract, KType::AbstractType { .. }));
    assert_eq!(sig.manifest_members.get("Elem"), Some(&elem_abstract));

    // The `zero` slot, declared `:Elem`, reads that same abstract identity (not `Number`).
    assert_eq!(sig.value_slots.get("zero"), Some(&elem_abstract));

    // The `compare` slot's `x` param reads the abstract identity — the substitution reaches
    // inside the function type, the case a raw value read would get wrong.
    match sig.value_slots.get("compare") {
        Some(KType::KFunction { params, ret, .. }) => {
            assert_eq!(params.get("x"), Some(&elem_abstract));
            assert_eq!(**ret, KType::Number);
        }
        other => panic!("compare slot should be a function type, got {other:?}"),
    }
}

#[test]
fn transparent_view_self_sig_reads_source_concrete_types() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE IntOrd = ((LET Elem = Number) (LET zero = 0) \
         (LET compare = (FN :{x :Number} -> Number = (x))))\n\
         SIG OrderedSig = ((TYPE Elem) (VAL zero :Elem) \
         (VAL compare :(FN (x :Elem) -> Number)))\n\
         LET View = (IntOrd :! OrderedSig)",
    );
    let view = module_named(scope, "View");
    let sig = view.self_sig();

    // A transparent view reads the source's concrete `Elem = Number`.
    assert_eq!(sig.manifest_members.get("Elem"), Some(&KType::Number));
    // Declared slots substitute to the concrete source type.
    assert_eq!(sig.value_slots.get("zero"), Some(&KType::Number));
    match sig.value_slots.get("compare") {
        Some(KType::KFunction { params, ret, .. }) => {
            assert_eq!(params.get("x"), Some(&KType::Number));
            assert_eq!(**ret, KType::Number);
        }
        other => panic!("compare slot should be a function type, got {other:?}"),
    }
}

#[test]
fn two_opaque_views_carry_distinct_abstract_identities() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE IntOrd = ((LET Elem = Number) (LET zero = 0))\n\
         SIG ElemSig = ((TYPE Elem) (VAL zero :Elem))\n\
         LET First = (IntOrd :| ElemSig)\n\
         LET Second = (IntOrd :| ElemSig)",
    );
    let first = module_named(scope, "First");
    let second = module_named(scope, "Second");
    // Generativity: each ascription mints its own abstract `Elem`, so the self-sigs differ.
    assert_ne!(
        first.self_sig().manifest_members.get("Elem"),
        second.self_sig().manifest_members.get("Elem"),
    );
}

// --- satisfaction through the relation (Phase 3) --------------------------------------

#[test]
fn value_slot_type_mismatch_is_rejected() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG NumSig = ((VAL v :Number))\n\
         MODULE StrMod = ((LET v = (\"hi\")))",
    );
    let err = run_one_err(scope, parse_one("StrMod :| NumSig"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("NumSig") && msg.contains("`v`") && msg.contains("has type")),
        "expected a value-slot type error naming `v`, got {err}",
    );
}

#[test]
fn higher_kinded_slot_rejects_proper_type_with_kind_message() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    // A proper type cannot fill a `TYPE (Type AS Wrap)` arity-1 slot.
    run(
        scope,
        "SIG MonadSig = ((TYPE (Type AS Wrap)))\n\
         MODULE IntList = ((LET Wrap = Number))",
    );
    let err = run_one_err(scope, parse_one("IntList :| MonadSig"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("`Wrap`") && msg.contains("type constructor") && msg.contains("1 parameter")),
        "expected a kind/arity error naming `Wrap`, got {err}",
    );
}

#[test]
fn satisfying_module_ascribes_and_repeat_hits_memo() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    register_arity1_constructor(scope, "Wrapper");
    // A module satisfying every rule ascribes; a second ascription of the same module+sig
    // succeeds too — the satisfaction memo caches the first result.
    run(
        scope,
        "SIG FullSig = ((TYPE (Type AS Wrap)) (LET Tag = Number) (VAL zero :Number))\n\
         MODULE Impl = ((LET Wrap = Wrapper) (LET Tag = Number) (LET zero = 0))\n\
         LET FirstView = (Impl :| FullSig)\n\
         LET SecondView = (Impl :| FullSig)",
    );
    assert!(matches!(
        scope.resolve_type("FirstView"),
        Some(KType::Module { .. })
    ));
    assert!(matches!(
        scope.resolve_type("SecondView"),
        Some(KType::Module { .. })
    ));
    // The memo recorded the (satisfying) result for the sig.
    let impl_mod = module_named(scope, "Impl");
    assert!(!impl_mod.satisfaction_memo.borrow().is_empty());
}

#[test]
fn manifest_member_mismatch_names_the_member() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG TagSig = ((LET Tag = Number) (VAL item :Number))\n\
         MODULE Bad = ((LET Tag = Str) (LET item = 5))",
    );
    let err = run_one_err(scope, parse_one("Bad :| TagSig"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("`Tag`") && msg.contains("fixes it to")),
        "expected a manifest-mismatch error naming `Tag`, got {err}",
    );
}
