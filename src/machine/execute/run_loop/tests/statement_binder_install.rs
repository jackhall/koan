//! Statement-position binder install, pinned end-to-end through the `TestRun`
//! harness. Every test submits a real program as one top-level block via
//! `enter_block` (index-gated, source order), so a declaration and the later
//! sibling that references its binder are dispatched together: the sibling
//! resolves only because the declaration installed its binder (a name
//! placeholder or a pending-overload bucket) at statement submission, letting
//! the sibling park until the declaration completes.
//!
//! These assertions read only through the language's public name resolution
//! (`scope.lookup`, `scope.resolve_type`) and observable program results — never
//! the binder-discovery internals a later refactor rewrites.

use std::rc::Rc;

use crate::builtins::test_support::{binds_module, TestRun};
use crate::machine::core::{run_root_storage, FrameStorage};
use crate::machine::model::{KObject, KType};
use crate::parse::parse;

/// Run `source` as one top-level block (source-order index gating) and hand back
/// the whole bundle, so callers read the post-run scope and the run's registry.
fn run_block<'run>(region: &'run Rc<FrameStorage>, source: &str) -> TestRun<'run> {
    let mut test_run = TestRun::silent(region);
    let scope = test_run.scope;
    let exprs = parse(source).expect("parse should succeed");
    test_run.runtime.enter_block(scope.id, exprs, scope);
    test_run
        .runtime
        .execute()
        .expect("execute should not surface per-slot errors");
    test_run
}

/// The `Number` a name binds, or a panic naming what it actually bound.
fn number(test_run: &TestRun<'_>, name: &str) -> f64 {
    match test_run.scope.lookup(name) {
        Some(KObject::Number(n)) => *n,
        other => panic!(
            "expected `{name}` to bind a Number, got {:?}",
            other.map(|o| o.ktype().name(test_run.types())),
        ),
    }
}

// ---------------------------------------------------------------------------
// Statement-position install, one test per spec bucket.
// ---------------------------------------------------------------------------

/// `LET _ = _`, value overload: a later sibling reads the value-channel binding.
#[test]
fn let_value_install_lets_sibling_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "LET base = 10\n\
         LET derived = (base + 5)",
    );
    assert_eq!(number(&test_run, "derived"), 15.0);
}

/// `LET _ = _`, type-alias overload (uppercase LHS, type RHS): a later sibling
/// aliases the alias, and both resolve to the same underlying type.
#[test]
fn let_type_alias_install_lets_sibling_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "LET MyNum = Number\n\
         LET Echo = MyNum",
    );
    let scope = test_run.scope;
    assert_eq!(scope.resolve_type("MyNum"), Some(KType::NUMBER));
    assert_eq!(
        scope.resolve_type("Echo"),
        Some(KType::NUMBER),
        "the type-alias placeholder must let `Echo = MyNum` resolve",
    );
}

/// `TYPE _`, bare form. `TYPE` is a SIG-body-only declaration (a bare `TYPE Elt`
/// at top level binds nothing), so its statement position is the SIG body: the
/// abstract member `Carrier` is declared, and a later sibling `VAL zero :Carrier`
/// parks on its placeholder and resolves. Observable: the SIG builds and a module
/// providing `Carrier` satisfies it.
#[test]
fn type_bare_install_in_sig_body_lets_sibling_val_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "SIG WithCarrier = ((TYPE Carrier) (VAL zero :Carrier))\n\
         MODULE carrier_impl = ((LET Carrier = Number) (LET zero = 0))\n\
         LET view = (carrier_impl :| WithCarrier)",
    );
    let scope = test_run.scope;
    assert!(
        scope.resolve_type("WithCarrier").is_some(),
        "the SIG must build, which requires `VAL zero :Carrier` to resolve `Carrier`",
    );
    assert!(
        binds_module(scope, "view"),
        "a module providing `Carrier` and `zero` must satisfy `WithCarrier`",
    );
}

/// `TYPE _`, higher-kinded form `TYPE (<Param> AS <Name>)`. Also SIG-body-only;
/// declaring the abstract constructor member in a SIG body is the statement
/// position for this bucket.
#[test]
fn type_higher_kinded_install_in_sig_body_declares() {
    let region = run_root_storage();
    let test_run = run_block(&region, "SIG Mappable = ((TYPE (Elem AS Wrap)))");
    assert!(
        test_run.scope.resolve_type("Mappable").is_some(),
        "a SIG with a higher-kinded `TYPE (Elem AS Wrap)` member must build",
    );
}

/// `MODULE _ = _` (identifier overload): a later sibling reaches a member through
/// the module value's dot-projection.
#[test]
fn module_install_lets_sibling_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "MODULE mo = (LET x = 42)\n\
         LET got = mo.x",
    );
    assert_eq!(number(&test_run, "got"), 42.0);
}

/// `GROUP _ FOLD LEFT = _` (identifier overload): a later sibling reaches a
/// member of the group module through dot-projection.
#[test]
fn group_install_lets_sibling_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "GROUP nums FOLD LEFT = ((OP #(+) OVER Number = (left)) (LET seed = 7))\n\
         LET got = nums.seed",
    );
    assert_eq!(number(&test_run, "got"), 7.0);
}

/// `SIG _ = _`: a later sibling aliases the signature type.
#[test]
fn sig_install_lets_sibling_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "SIG Ordered = (VAL compare :Number)\n\
         LET OrdAlias = Ordered",
    );
    let scope = test_run.scope;
    let ordered = scope.resolve_type("Ordered");
    assert!(ordered.is_some(), "SIG must bind `Ordered`");
    assert_eq!(
        scope.resolve_type("OrdAlias"),
        ordered,
        "the SIG placeholder must let `OrdAlias = Ordered` resolve to the same type",
    );
}

/// `UNION _ = _`: a later sibling constructs a value at the union type.
#[test]
fn union_install_lets_sibling_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "UNION Color = (Red :Null Green :Null)\n\
         LET picked = (Color (Red null))",
    );
    assert!(
        test_run.scope.lookup("picked").is_some(),
        "constructing `Color (Red null)` in a later sibling must resolve `Color`",
    );
}

/// `NEWTYPE _ = _`: a later sibling aliases the newtype identity.
#[test]
fn newtype_equals_install_lets_sibling_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "NEWTYPE Distance = Number\n\
         LET DistAlias = Distance",
    );
    let scope = test_run.scope;
    let distance = scope.resolve_type("Distance");
    assert!(distance.is_some(), "NEWTYPE must bind `Distance`");
    assert_eq!(
        scope.resolve_type("DistAlias"),
        distance,
        "the newtype placeholder must let `DistAlias = Distance` resolve",
    );
}

/// `NEWTYPE _` (constructor-family bucket, no `=`): a later sibling applies the
/// family through `AS`.
#[test]
fn newtype_constructor_family_install_lets_sibling_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "NEWTYPE (Elem AS Boxed)\n\
         LET NumberBox = :(Number AS Boxed)",
    );
    assert!(
        test_run.scope.resolve_type("NumberBox").is_some(),
        "applying `:(Number AS Boxed)` in a later sibling must resolve the family `Boxed`",
    );
}

/// `RECURSIVE TYPES _ = _`: a later sibling aliases a co-declared member.
#[test]
fn recursive_types_install_lets_sibling_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "RECURSIVE TYPES Pair = (\n\
        \x20 NEWTYPE Aa = :{b :Bb}\n\
        \x20 NEWTYPE Bb = :{a :Aa}\n\
         )\n\
         LET AaAlias = Aa",
    );
    let scope = test_run.scope;
    let aa = scope.resolve_type("Aa");
    assert!(aa.is_some(), "RECURSIVE TYPES must bind member `Aa`");
    assert_eq!(
        scope.resolve_type("AaAlias"),
        aa,
        "the member placeholder must let `AaAlias = Aa` resolve",
    );
}

/// `FN` named bucket: the FN's pending-overload bucket installs at submission, so
/// a later sibling call parks on it and resolves when the FN registers.
#[test]
fn fn_named_install_lets_sibling_call_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "FN (DOUBLE x :Number) -> Number = (x + x)\n\
         LET out = (DOUBLE 5)",
    );
    assert_eq!(number(&test_run, "out"), 10.0);
}

/// `OP` bucket: the operator's pending-overload bucket installs at submission, so
/// a later sibling use of the operator parks on it and resolves.
#[test]
fn op_install_lets_sibling_use_resolve() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "OP #(⊕) OVER Number = (left + right)\n\
         LET out = (1 ⊕ 2)",
    );
    assert_eq!(number(&test_run, "out"), 3.0);
}

// ---------------------------------------------------------------------------
// Legal binder chains.
// ---------------------------------------------------------------------------

/// The functor idiom: `LET make_set = (FN (MAKESET …) …)` installs the nested
/// FN's pending-overload bucket `[MAKESET, Slot]` at the outer statement's
/// submission, so a later sibling `(MAKESET …)` call parks and resolves when
/// `make_set` completes.
#[test]
fn functor_chain_sibling_call_parks_then_resolves() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)\n\
         LET make_set = \
            (FN (MAKESET er :Ordered) -> Module = (MODULE result = (LET inner = 1)))\n\
         LET the_set = (MAKESET int_ord)",
    );
    assert!(
        matches!(test_run.scope.lookup("the_set"), Some(KObject::Module(_))),
        "the nested FN's bucket must install at the outer submission so the \
         sibling `(MAKESET int_ord)` call resolves to a module; got {:?}",
        test_run
            .scope
            .lookup("the_set")
            .map(|o| o.ktype().name(test_run.types())),
    );
}

/// The LET-chain idiom: `LET z = (LET a = 3)` installs the inner LET's name
/// placeholder `a` with the outer statement's node id, so a later sibling reading
/// `a` parks and resolves.
#[test]
fn let_chain_sibling_reads_inner_binding() {
    let region = run_root_storage();
    let test_run = run_block(
        &region,
        "LET z = (LET a = 3)\n\
         LET use_a = a",
    );
    assert_eq!(number(&test_run, "use_a"), 3.0);
}

// ---------------------------------------------------------------------------
// VAL negative: a VAL inside a SIG installs no binder.
// ---------------------------------------------------------------------------

/// A `VAL` declares a required signature slot, not a binder: the enclosing SIG
/// builds and enforces the slot, but the slot name is never installed as a
/// resolvable binding in scope, and (unlike a placeholder-backed binder, whose
/// duplicate collides at install) a duplicate `VAL` collides one layer later, at
/// slot insert, leaving the enclosing signature bound to nothing.
#[test]
fn val_inside_sig_installs_no_binder() {
    let region = run_root_storage();
    let test_run = run_block(&region, "SIG Ordered = ((VAL zero :Number))");
    let scope = test_run.scope;
    assert!(
        scope.resolve_type("Ordered").is_some(),
        "the SIG carrying the VAL slot must build",
    );
    assert!(
        scope.lookup("zero").is_none(),
        "VAL installs no value-channel binder for its slot name",
    );
    assert!(
        scope.resolve_type("zero").is_none(),
        "VAL installs no type binder for its slot name",
    );
}

/// The VAL slot is nonetheless a real signature requirement: a module missing it
/// fails shape-check. Confirms the negative above is "no binder", not "no slot".
#[test]
fn val_slot_is_a_real_requirement() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(
        "SIG WithCompare = ((VAL compare :Number))\n\
         MODULE empty = (LET unrelated = 0)",
    );
    let err = test_run.run_one_err(crate::builtins::test_support::parse_one(
        "empty :| WithCompare",
    ));
    assert!(
        format!("{err}").contains("compare"),
        "a module missing the VAL slot must fail shape-check naming `compare`, got {err}",
    );
}
