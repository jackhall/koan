//! The principal signature (self-sig) a module or ascription view carries, derived at
//! creation. A plain module records its manifest members and raw value-slot types; an opaque
//! view records per-call abstract identities and re-expresses SIG-declared value slots against
//! them; a transparent view records the source's concrete types.

use crate::builtins::test_support::{
    binds_module, lookup_module, parse_one, run, run_one_err, run_returning_registry,
    run_root_silent,
};
use crate::machine::model::KObject;
use crate::machine::model::KType;
use crate::machine::model::Module;
use crate::machine::run_root_storage;
use crate::machine::{KErrorKind, Scope};

fn module_named<'a>(scope: &'a Scope<'a>, name: &str) -> &'a Module<'a> {
    lookup_module(scope, name)
}

#[test]
fn plain_module_self_sig_is_manifest_and_raw_value_slots() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE int_ord = ((LET Tag = Number) (LET compare = 5))",
    );
    let m = module_named(scope, "int_ord");
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
        "MODULE int_ord = ((LET Elem = Number) (LET zero = 0) \
         (LET compare = (FN :{x :Number} -> Number = (x))))\n\
         SIG Ordered = ((TYPE Elem) (VAL zero :Elem) \
         (VAL compare :(FN (x :Elem) -> Number)))\n\
         LET view = (int_ord :| Ordered)",
    );
    let view = module_named(scope, "view");
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
        "MODULE int_ord = ((LET Elem = Number) (LET zero = 0) \
         (LET compare = (FN :{x :Number} -> Number = (x))))\n\
         SIG Ordered = ((TYPE Elem) (VAL zero :Elem) \
         (VAL compare :(FN (x :Elem) -> Number)))\n\
         LET view = (int_ord :! Ordered)",
    );
    let view = module_named(scope, "view");
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
        "MODULE int_ord = ((LET Elem = Number) (LET zero = 0))\n\
         SIG Pointed = ((TYPE Elem) (VAL zero :Elem))\n\
         LET first = (int_ord :| Pointed)\n\
         LET second = (int_ord :| Pointed)",
    );
    let first = module_named(scope, "first");
    let second = module_named(scope, "second");
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
        "SIG Numeric = ((VAL v :Number))\n\
         MODULE str_mod = ((LET v = (\"hi\")))",
    );
    let err = run_one_err(scope, parse_one("str_mod :| Numeric"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("Numeric") && msg.contains("`v`") && msg.contains("has type")),
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
        "SIG Monad = ((TYPE (Type AS Wrap)))\n\
         MODULE int_list = ((LET Wrap = Number))",
    );
    let err = run_one_err(scope, parse_one("int_list :| Monad"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("`Wrap`") && msg.contains("type constructor") && msg.contains("1 parameter")),
        "expected a kind/arity error naming `Wrap`, got {err}",
    );
}

#[test]
fn satisfying_module_ascribes_and_repeat_hits_verdict() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    // A module satisfying every rule ascribes; a second ascription of the same module+sig
    // succeeds too — the run's registry records the first verdict and the repeat check hits it.
    let registry = run_returning_registry(
        scope,
        "NEWTYPE (Type AS Wrapper)\n\
         SIG Complete = ((TYPE (Type AS Wrap)) (LET Tag = Number) (VAL zero :Number))\n\
         MODULE implementation = ((LET Wrap = Wrapper) (LET Tag = Number) (LET zero = 0))\n\
         LET first_view = (implementation :| Complete)\n\
         LET second_view = (implementation :| Complete)",
    );
    assert!(binds_module(scope, "first_view"));
    assert!(binds_module(scope, "second_view"));
    assert!(
        registry.hit_count() > 0,
        "the repeat ascription should hit the verdict the first one recorded, got {} hits / {} misses",
        registry.hit_count(),
        registry.miss_count(),
    );
}

#[test]
fn manifest_member_mismatch_names_the_member() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Tagged = ((LET Tag = Number) (VAL item :Number))\n\
         MODULE bad = ((LET Tag = Str) (LET item = 5))",
    );
    let err = run_one_err(scope, parse_one("bad :| Tagged"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("`Tag`") && msg.contains("fixes it to")),
        "expected a manifest-mismatch error naming `Tag`, got {err}",
    );
}

/// Opacity survives content identity: two `:|` ascriptions of the same module against the same
/// SIG mint distinct per-call abstract identities, so their `TYPE OF` types stay distinct even
/// though both self-sigs digest by content. This is the sole generative exception (`AbstractType`
/// stays id-keyed).
#[test]
fn opaque_views_have_distinct_type_of() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = ((TYPE Elt) (VAL zero :Elt))\n\
         MODULE int_ord = ((LET Elt = Number) (LET zero = 7))\n\
         LET v1 = (int_ord :| Ordered)\n\
         LET v2 = (int_ord :| Ordered)",
    );
    let v1 = lookup_module(scope, "v1");
    let v2 = lookup_module(scope, "v2");
    assert_ne!(
        KObject::Module(v1).ktype(),
        KObject::Module(v2).ktype(),
        "each opaque ascription is a fresh generative identity",
    );
}

/// The `SigSatisfies` verdict keys the subject on the module's self-sig content digest, so two
/// modules with an identical interface share one verdict: `TAKE b`'s satisfaction check hits the
/// verdict `TAKE a` recorded, rather than re-walking `sig_subtype`. Verdicts are scoped to the run,
/// so both takes run against the one registry of a single runtime.
#[test]
fn identical_modules_share_satisfaction_verdict() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let registry = run_returning_registry(
        scope,
        "SIG Ord = ((VAL x :Number))\n\
         MODULE a = ((LET x = 1))\n\
         MODULE b = ((LET x = 2))\n\
         FN (TAKE m :Ord) -> Number = (m.x)\n\
         TAKE a\n\
         TAKE b",
    );
    assert!(
        registry.hit_count() > 0,
        "the second take's satisfaction check should hit the first's verdict, got {} hits / {} misses",
        registry.hit_count(),
        registry.miss_count(),
    );
}
