//! Primitive ascription behaviors: transparent passthrough, missing-member errors, opaque type-minting.

use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
use crate::machine::execute::Scheduler;
use crate::machine::model::{KObject, KType};
use crate::machine::{KErrorKind, RuntimeArena};
use crate::parse::parse;

#[test]
fn transparent_ascription_returns_module() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "MODULE IntOrd = (LET compare = 0)\n\
         SIG OrderedSig = (VAL compare :Number)\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    // Transparent ascription `:!` of a module is type-only — its module identity lives
    // in `types`.
    assert!(matches!(
        scope.resolve_type("IntOrdView"),
        Some(KType::Module {
            module: _,
            frame: _
        }),
    ));
}

#[test]
fn ascription_missing_member_errors() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "MODULE Empty = (LET unrelated = 0)\n\
         SIG OrderedSig = (VAL compare :Number)",
    );
    let err = run_one_err(scope, parse_one("Empty :| OrderedSig"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("OrderedSig") && msg.contains("`compare`")),
        "expected ShapeError naming OrderedSig and the missing member, got {err}",
    );
}

#[test]
fn opaque_ascription_mints_distinct_module_type_per_application() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let src = "MODULE IntOrd = (LET compare = 0)\n\
         SIG OrderedSig = ((LET Carrier = Number) (VAL compare :Number))\n\
         LET FirstAbstract = (IntOrd :| OrderedSig)\n\
         LET SecondAbstract = (IntOrd :| OrderedSig)";
    let exprs = parse(src).expect("parse should succeed");
    let mut sched = Scheduler::new();
    let mut ids = Vec::new();
    for expr in exprs {
        ids.push(sched.add_dispatch(expr, scope));
    }
    sched.execute().expect("scheduler should succeed");
    for (i, id) in ids.iter().enumerate() {
        if let Err(e) = sched.read_result(*id) {
            panic!("expr {} errored: {}", i, e);
        }
    }
    let a = match scope.resolve_type("FirstAbstract") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("FirstAbstract should be a module identity in types"),
    };
    let b = match scope.resolve_type("SecondAbstract") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("SecondAbstract should be a module identity in types"),
    };
    let a_t = a.type_members.borrow().get("Carrier").cloned();
    let b_t = b.type_members.borrow().get("Carrier").cloned();
    // Post-collapse: opaque-ascription abstract-type members are minted as
    // `KType::AbstractType { source: Module(view), name }`.
    assert!(matches!(
        &a_t,
        Some(KType::AbstractType { name, .. }) if name == "Carrier"
    ));
    assert!(matches!(
        &b_t,
        Some(KType::AbstractType { name, .. }) if name == "Carrier"
    ));
    assert_ne!(
        a_t, b_t,
        "two opaque ascriptions must mint distinct module abstract types"
    );
}

#[test]
fn transparent_ascription_does_not_mint_module_types() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "MODULE IntOrd = (LET compare = 0)\n\
         SIG OrderedSig = (VAL compare :Number)\n\
         LET ViewMod = (IntOrd :! OrderedSig)",
    );
    let v = match scope.resolve_type("ViewMod") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("ViewMod should be a module identity in types"),
    };
    assert!(v.type_members.borrow().is_empty());
}

/// End-to-end example from [design/typing/modules.md](../../../../design/typing/modules.md).
#[test]
fn roadmap_example_int_ord_with_ordered_sig() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         SIG OrderedSig = ((LET Carrier = Number) (VAL compare :Number))\n\
         LET IntOrdAbstract = (IntOrd :| OrderedSig)",
    );

    let abstract_mod = match scope.resolve_type("IntOrdAbstract") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("IntOrdAbstract should be a module identity in types"),
    };
    let minted = abstract_mod
        .type_members
        .borrow()
        .get("Carrier")
        .cloned()
        .expect("opaque ascription should mint a Carrier member");
    match &minted {
        KType::AbstractType { name, .. } => assert_eq!(name, "Carrier"),
        other => panic!("minted abstract type must be AbstractType, got {:?}", other),
    }
    assert_ne!(
        minted,
        KType::Number,
        "opaque IntOrdAbstract.Carrier must not equal Number"
    );
    let compare = abstract_mod
        .child_scope()
        .bindings()
        .data()
        .get("compare")
        .map(|(o, _)| *o);
    assert!(matches!(compare, Some(KObject::Number(n)) if *n == 7.0));
}
