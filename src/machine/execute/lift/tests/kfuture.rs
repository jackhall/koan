//! `kfuture` arms of `lift_kobject`.

use super::*;
use crate::builtins::default_scope;
use crate::machine::model::KObject;
use crate::machine::CallArena;
use crate::parse::parse;

use super::{alloc_local_kf, defeat_fast_path, dispatch_for_test};

/// A KFuture with no descendant borrow into the dying arena must lift to
/// `frame: None` — anchoring would over-keep the arena. The dummy KFunction
/// below defeats `functions_is_empty()`'s fast path so the slow path runs.
#[test]
fn unanchored_kfuture_no_arena_borrow_does_not_anchor() {
    use crate::machine::model::{ExpressionSignature, KType, SignatureElement, ReturnType};
    use crate::machine::{Body, KFunction};

    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf = KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Null),
            elements: vec![SignatureElement::Keyword("__SLOW__".into())],
        },
        Body::Builtin(|s, _, _| crate::machine::BodyResult::Value(
            s.arena.alloc_object(KObject::Null)
        )),
        dying.scope(),
    );
    let _ = dying.arena().alloc_function(kf);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let kf_obj = KObject::KFuture(future, None);

    let strong_before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&kf_obj, &dying);

    match lifted {
        KObject::KFuture(_, frame) => assert!(
            frame.is_none(),
            "KFuture without descendant borrows into dying arena must lift to frame=None",
        ),
        other => panic!("expected lifted KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(
        Rc::strong_count(&dying),
        strong_before,
        "lifting a non-borrowing KFuture must not bump the dying frame's Rc",
    );
}

/// Symmetric case: a KFuture whose parsed parts contain a `Future(&KObject)`
/// allocated in the dying arena must lift with `frame: Some(rc)`.
#[test]
fn unanchored_kfuture_with_arena_borrow_does_anchor() {
    use crate::machine::model::{ExpressionSignature, KType, ReturnType, SignatureElement};
    use crate::machine::{Body, KFunction};

    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);

    // Defeat `functions_is_empty()` fast path so the slow path runs. Captured
    // scope lives in `dying.arena()` to satisfy `alloc_function`'s invariant.
    let kf = KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Null),
            elements: vec![SignatureElement::Keyword("__SLOW__".into())],
        },
        Body::Builtin(|s, _, _| crate::machine::BodyResult::Value(
            s.arena.alloc_object(KObject::Null)
        )),
        dying.scope(),
    );
    let _ = dying.arena().alloc_function(kf);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let inside: &KObject = dying.arena().alloc_object(KObject::Number(7.0));
    future.parsed.parts.push(ExpressionPart::Future(inside));
    let kf_obj = KObject::KFuture(future, None);

    let strong_before = Rc::strong_count(&dying);
    let lifted = lift_kobject(&kf_obj, &dying);
    match &lifted {
        KObject::KFuture(_, frame) => assert!(
            frame.is_some(),
            "KFuture borrowing into dying arena must lift with frame=Some(rc)",
        ),
        other => panic!("expected lifted KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(
        Rc::strong_count(&dying),
        strong_before + 1,
        "lifting a borrowing KFuture must clone the dying frame's Rc once",
    );
    // Drop borrowers before `dying` so arena teardown order is well-defined.
    drop(lifted);
    drop(kf_obj);
}

/// `kobject_borrows_arena`'s KFuture predicate arm (221) — a KFuture
/// parked inside another KFuture's `bundle.args` exercises the recursive
/// borrow walk. The inner future borrows via its own captured function.
#[test]
fn kfuture_bundle_arg_with_nested_kfuture_anchors() {
    use crate::machine::core::kfunction::ArgumentBundle;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let inner_future = KFuture {
        parsed: KExpression { parts: vec![] },
        function: kf_ref,
        bundle: ArgumentBundle { args: HashMap::new() },
    };

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut outer = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    outer.bundle.args.insert(
        "f".into(),
        Rc::new(KObject::KFuture(inner_future, None)),
    );
    let obj = KObject::KFuture(outer, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// `any_descendant`'s Struct recursion arm (138–140) is reachable only via
/// `kobject_borrows_arena`'s `None` predicate return on Struct. A KFuture
/// whose `bundle.args` carries a Struct with a borrowing field exercises
/// the recursion through the fields map.
#[test]
fn kfuture_bundle_arg_with_struct_field_anchors() {
    use crate::machine::ScopeId;
    use indexmap::IndexMap;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let mut fields: IndexMap<String, KObject> = IndexMap::new();
    fields.insert("f".into(), KObject::KFunction(kf_ref, None));
    let s = KObject::Struct {
        name: "S".into(),
        scope_id: ScopeId::next(),
        fields: Rc::new(fields),
    };

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    future.bundle.args.insert("s".into(), Rc::new(s));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// `expression_borrows_arena`'s `Expression` part recursion arm (205) — a
/// `parsed.parts` `Expression(Box<KExpression>)` whose inner parts borrow
/// into the dying arena must drive anchor.
#[test]
fn kfuture_parsed_expression_part_with_arena_borrow_anchors() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let inside: &KObject = dying.arena().alloc_object(KObject::Number(17.0));
    let inner = KExpression { parts: vec![ExpressionPart::Future(inside)] };
    future
        .parsed
        .parts
        .push(ExpressionPart::Expression(Box::new(inner)));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// `kobject_borrows_arena`'s `KExpression` predicate arm (220–221) — a
/// `KExpression` parked in `bundle.args` whose inner parts borrow into the
/// dying arena must drive anchor.
#[test]
fn kfuture_bundle_arg_with_kexpression_borrow_anchors() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let inside: &KObject = dying.arena().alloc_object(KObject::Number(19.0));
    let inner = KExpression { parts: vec![ExpressionPart::Future(inside)] };
    future
        .bundle
        .args
        .insert("e".into(), Rc::new(KObject::KExpression(inner)));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// Pre-anchored KFuture preserves its anchor through lift (mirror of the
/// KFunction case — both arms must share the "respect `existing`" rule).
#[test]
fn kfuture_with_existing_anchor_preserves_it() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);
    let other = CallArena::new(scope, None);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let obj = KObject::KFuture(future, Some(Rc::clone(&other)));
    let other_before = Rc::strong_count(&other);

    let lifted = lift_kobject(&obj, &dying);
    let other_after = Rc::strong_count(&other);
    match lifted {
        KObject::KFuture(_, frame) => {
            let f = frame.expect("pre-anchored frame must persist");
            assert!(Rc::ptr_eq(&f, &other));
        }
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(other_after, other_before + 1);
}

/// `kfuture_borrows_dying_arena` walks `bundle.args` for borrowing payloads.
/// A KFunction whose captured scope lives in the dying arena, parked in a
/// bundle slot, must drive lift to anchor — exercises `kobject_borrows_arena`'s
/// KFunction predicate arm (220–225).
#[test]
fn kfuture_bundle_arg_with_local_kfunction_anchors() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    future
        .bundle
        .args
        .insert("borrower".into(), Rc::new(KObject::KFunction(kf_ref, None)));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::KFuture(_, frame) => assert!(
            frame.is_some(),
            "bundle-arg KFunction borrowing into dying arena must drive anchor",
        ),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// `kfuture_borrows_dying_arena`'s function-captured-scope short-circuit (186–187).
/// A KFuture whose own function was captured in the dying arena anchors without
/// needing any borrowing payload in parts or bundle.
#[test]
fn kfuture_with_local_function_anchors() {
    use crate::machine::core::kfunction::ArgumentBundle;
    use std::collections::HashMap;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let future = KFuture {
        parsed: KExpression { parts: vec![] },
        function: kf_ref,
        bundle: ArgumentBundle { args: HashMap::new() },
    };
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::KFuture(_, frame) => assert!(
            frame.is_some(),
            "KFuture whose function captured the dying scope must anchor",
        ),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// `kobject_borrows_arena`'s composite-recursion arms (230–233) only fire when
/// a bundle arg is a List/Dict/Tagged with a borrowing descendant. A `List`
/// containing a dying-captured KFunction exercises the recursion.
#[test]
fn kfuture_bundle_arg_with_list_of_kfunction_anchors() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let nested = KObject::List(Rc::new(vec![KObject::KFunction(kf_ref, None)]));
    future.bundle.args.insert("nested".into(), Rc::new(nested));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// `kobject_borrows_arena`'s KModule arm (226–229) — module child scope in
/// dying arena, parked in a bundle slot.
#[test]
fn kfuture_bundle_arg_with_local_kmodule_anchors() {
    use crate::machine::model::values::Module;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let module = Module::new("BundleMod".into(), dying.scope());
    let m_ref: &Module = dying.arena().alloc_module(module);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    future
        .bundle
        .args
        .insert("m".into(), Rc::new(KObject::KModule(m_ref, None)));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// `expression_borrows_arena`'s `ListLiteral` arm (206) — a `parsed.parts`
/// `ListLiteral` whose inner `Future` part points into the dying arena.
#[test]
fn kfuture_parsed_listliteral_with_arena_borrow_anchors() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let inside: &KObject = dying.arena().alloc_object(KObject::Number(11.0));
    future
        .parsed
        .parts
        .push(ExpressionPart::ListLiteral(vec![ExpressionPart::Future(inside)]));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// `expression_borrows_arena`'s `DictLiteral` arm (207–209) — value side
/// of a `(key, value)` pair carries the borrowing `Future` part.
#[test]
fn kfuture_parsed_dictliteral_with_arena_borrow_anchors() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let inside: &KObject = dying.arena().alloc_object(KObject::Number(13.0));
    future.parsed.parts.push(ExpressionPart::DictLiteral(vec![(
        ExpressionPart::Keyword("k".into()),
        ExpressionPart::Future(inside),
    )]));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}
