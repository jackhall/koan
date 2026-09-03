use crate::builtins::test_support::lookup_type;
use crate::builtins::test_support::{TestRun, binds_module, type_name, value_name};
use crate::machine::model::Symbol;
use crate::machine::model::{KType, SigSchema, TypeNode, TypeRegistry};
use crate::machine::{KErrorKind, ScopeId};
use crate::machine::{program_storage, run_root_storage};

/// The stored schema of the signature `name` binds in `scope`.
fn sig_schema(scope: &crate::machine::Scope<'_>, types: &TypeRegistry, name: &str) -> SigSchema {
    let handle = lookup_type(scope, name).unwrap_or_else(|| panic!("{name} must bind a type"));
    match types.node(handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => panic!("{name} must bind a Signature KType"),
    }
}

/// Smoke: the VAL slot lives in the signature's stored schema (`value_slots`) under its
/// value-class name so `ascribe::check_satisfies` will require it of an ascribed module.
#[test]
fn val_inside_sig_binds_typeexpr_carrier() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Ordered = ((VAL zero :Number))");
    let zero = sig_schema(scope, test_run.types(), "Ordered")
        .value_slots
        .get(&value_name("zero", test_run.registries()))
        .copied()
        .expect("zero must live in Ordered's stored schema value_slots");
    assert_eq!(zero, KType::NUMBER);
}

/// Pins the parking path: sibling statement order isn't guaranteed, so the `ty` slot's bare name
/// parks on TYPE's placeholder at dispatch and resolves on the wake, picking the SIG-local shadow
/// over the meta-type builtin — the scope walk reaches the shadow first. The shadow binds an
/// `AbstractType` sourced at the canonical binder (so the slot records that it *names* the abstract
/// member `Carrier`).
#[test]
fn val_resolves_sig_local_type_shadow() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG WithZero = ((TYPE Carrier) (VAL zero :Carrier))");
    let zero = sig_schema(scope, test_run.types(), "WithZero")
        .value_slots
        .get(&value_name("zero", test_run.registries()))
        .copied()
        .expect("zero must live in WithZero's stored schema value_slots");
    match test_run.types().node(zero) {
        TypeNode::AbstractType { source, name, .. } => {
            assert_eq!(
                name,
                type_name("Carrier", test_run.registries()),
                "VAL slot must record that it names the SIG-local abstract `Carrier`",
            );
            assert_eq!(source, ScopeId::SENTINEL);
        }
        _ => panic!("expected AbstractType(Carrier), got {zero:?}"),
    }
}

/// Duplicate `VAL x` within one SIG body errors `Rebind` naming `x` at the slot insert (VAL
/// installs no dispatch-time placeholder, so the collision surfaces one step later than a
/// placeholder-backed binder would) and the enclosing SIG binds nothing.
#[test]
fn duplicate_val_slot_name_is_rebind() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let err =
        test_run.run_one_err(test_run.parse_one("SIG SigDup = ((VAL x :Number) (VAL x :Str))"));
    assert!(
        matches!(&err.kind, KErrorKind::Rebind { name } if name == "x"),
        "expected Rebind naming `x`, got {err}",
    );
    assert!(
        lookup_type(scope, "SigDup").is_none(),
        "the colliding signature binds nothing",
    );
}

/// Gate fires on the immediate-enclosing labeled scope.
#[test]
fn val_outside_sig_errors() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(test_run.parse_one("VAL x :Number"));
    match &err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(
                msg.contains("VAL is only valid inside a SIG body"),
                "expected SIG-only diagnostic, got: {msg}",
            );
        }
        _ => panic!("expected ShapeError, got something else"),
    }
}

/// Companion to `val_outside_sig_errors`: MODULE's enclosing labeled scope is
/// `"MODULE ..."`, not `"SIG ..."`, so the same diagnostic must fire.
#[test]
fn val_inside_module_errors() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(test_run.parse_one("MODULE foo = ((VAL x :Number))"));
    match &err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(
                msg.contains("VAL is only valid inside a SIG body"),
                "expected SIG-only diagnostic, got: {msg}",
            );
        }
        _ => panic!("expected ShapeError, got something else"),
    }
}

/// Structural carriers (here `Function<...>`) are lifted directly — no SIG-local
/// shadow to honor, so the body skips the re-dispatch path.
#[test]
fn val_function_typed_slot() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Ordered = ((VAL compare :(FN :{x :Number, y :Number} -> Number)))");
    let compare = sig_schema(scope, test_run.types(), "Ordered")
        .value_slots
        .get(&value_name("compare", test_run.registries()))
        .copied()
        .expect("compare must live in Ordered's stored schema value_slots");
    match test_run.types().node(compare) {
        TypeNode::KFunction { params, ret } => {
            assert_eq!(params.len(), 2);
            assert_eq!(params.get(Symbol::of("x")).copied(), Some(KType::NUMBER));
            assert_eq!(params.get(Symbol::of("y")).copied(), Some(KType::NUMBER));
            assert_eq!(ret, KType::NUMBER);
        }
        _ => panic!("expected KFunction-typed slot, got {compare:?}"),
    }
}

/// `shape_check` walks `bindings.data` and VAL writes there, so a missing slot
/// surfaces as a ShapeError naming the missing member and the signature. A signature carries no
/// declaration label (ruling 12), so it is named structurally (`SIG (compare: Number)`), not by
/// the binder `WithCompare`.
#[test]
fn val_slot_required_by_shape_check() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG WithCompare = ((VAL compare :Number))\n\
         MODULE empty = (LET unrelated = 0)",
    );
    let err = test_run.run_one_err(test_run.parse_one("empty :| WithCompare"));
    match &err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(
                msg.contains("missing member `compare`") && msg.contains("SIG (compare: Number)"),
                "expected diagnostic naming missing `compare` and the structural signature, got: {msg}",
            );
        }
        _ => panic!("expected ShapeError, got something else"),
    }
}

/// Pins the name-presence-only contract: shape_check passes even though the
/// MODULE's `LET compare = 0` value isn't type-checked against the VAL's declared
/// `Number` — that's modular implicits' job, not shape_check's.
#[test]
fn val_slot_satisfied_by_module_let_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG WithCompare = ((VAL compare :Number))\n\
         MODULE int_ord = (LET compare = 0)\n\
         LET ord = (int_ord :| WithCompare)",
    );
    // The `:|`-ascribed view `ord` is a module value, so it binds on the value channel.
    assert!(binds_module(scope, "ord"));
}

/// Pins the canonical SIG form: abstract type via `TYPE Carrier` plus a VAL
/// slot whose declared type references it. `Carrier` lives in the schema's abstract members,
/// `zero` in the signature's stored schema `value_slots`; both carry the same `AbstractType`
/// identity, sourced at the canonical binder (so opacity threads to the per-call module's
/// `slot_type_tags`), not the collapsed underlying `Number`.
#[test]
fn val_with_abstract_type_member_declaration() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG WithZero = ((TYPE Carrier) (VAL zero :Carrier))");
    let schema = sig_schema(scope, test_run.types(), "WithZero");
    let carrier = type_name("Carrier", test_run.registries());
    let type_kt = schema
        .abstract_members
        .get(&carrier)
        .copied()
        .expect("Carrier must live in WithZero's stored schema abstract_members");
    assert!(matches!(
        test_run.types().node(type_kt),
        TypeNode::AbstractType { source, name, .. } if source == ScopeId::SENTINEL && name == carrier
    ));
    let zero = schema
        .value_slots
        .get(&value_name("zero", test_run.registries()))
        .copied()
        .expect("zero must live in WithZero's stored schema value_slots");
    assert!(matches!(
        test_run.types().node(zero),
        TypeNode::AbstractType { source, name, .. } if source == ScopeId::SENTINEL && name == carrier
    ));
    assert_eq!(type_kt, zero, "both name the same abstract identity");
}

/// The `ty` slot is an ordinary kind expectation the dispatch lane resolves into, so an unknown
/// name misses there — and the slot's registered role keeps the miss naming the form.
#[test]
fn unknown_slot_type_names_the_slot_role() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(test_run.parse_one("SIG Ordered = ((VAL zero :Nope))"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("unknown type name `Nope` in VAL slot type")),
        "expected the role-framed unknown-type message, got {err}",
    );
}
