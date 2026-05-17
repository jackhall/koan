use super::*;
use crate::builtins::default_scope;
use crate::machine::model::KObject;
use crate::machine::{CallArena, KError, KErrorKind, ResolveOutcome, Scope};
use crate::parse::parse;
use crate::machine::model::Parseable;

/// Test-only `(scope, expr) → KFuture` driver for one-shot bind without spinning a
/// `Scheduler`. Not production API — the scheduler drives all real dispatches.
fn dispatch_for_test<'a>(
    scope: &'a Scope<'a>,
    expr: KExpression<'a>,
) -> Result<KFuture<'a>, KError> {
    match scope.resolve_dispatch(&expr) {
        ResolveOutcome::Resolved(r) => r.function.bind(expr),
        ResolveOutcome::Ambiguous(n) => Err(KError::new(KErrorKind::AmbiguousDispatch {
            expr: expr.summarize(),
            candidates: n,
        })),
        ResolveOutcome::Deferred | ResolveOutcome::Unmatched => {
            Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(),
                reason: "no matching function".to_string(),
            }))
        }
    }
}

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

// ---- per-arm coverage for `lift_kobject` slow path ----

/// Stamp a sentinel KFunction into `dying.arena()` so `functions_is_empty()` is false
/// and `lift_kobject` enters the slow path. Side-effect only — the alloc'd ref is
/// discarded; the function lives until `dying`'s arena drops.
fn defeat_fast_path(dying: &Rc<CallArena>) {
    use crate::machine::model::{ExpressionSignature, KType, ReturnType, SignatureElement};
    use crate::machine::{Body, BodyResult, KFunction};
    let kf = KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Null),
            elements: vec![SignatureElement::Keyword("__SLOW__".into())],
        },
        Body::Builtin(|s, _, _| BodyResult::Value(s.arena.alloc_object(KObject::Null))),
        dying.scope(),
    );
    let _ = dying.arena().alloc_function(kf);
}

/// A KFunction whose `captured_scope` lives in the dying arena. Caller is responsible
/// for not allocating a separate bait — this KFunction itself defeats `functions_is_empty`.
fn alloc_local_kf<'a>(dying: &'a Rc<CallArena>) -> &'a crate::machine::KFunction<'a> {
    use crate::machine::model::{ExpressionSignature, KType, ReturnType, SignatureElement};
    use crate::machine::{Body, BodyResult, KFunction};
    let kf = KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Null),
            elements: vec![SignatureElement::Keyword("__INNER__".into())],
        },
        Body::Builtin(|s, _, _| BodyResult::Value(s.arena.alloc_object(KObject::Null))),
        dying.scope(),
    );
    dying.arena().alloc_function(kf)
}

// ---- nested-composite recursion ----

/// `any_descendant`'s Dict recursion arm (136) and List None-recursion arm
/// (177) only fire when a Dict / List sits inside another composite at lift
/// time. `List<Dict<KFunction>>` triggers both: the outer list rebuild walks
/// each item through `needs_lift` → `any_descendant`, which recurses into
/// Dict, which recurses into the KFunction leaf.
#[test]
fn list_of_dict_with_kfunction_anchors_via_recursion() {
    use crate::machine::model::types::Serializable;
    use crate::machine::model::values::KKey;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let mut inner_map: HashMap<Box<dyn Serializable>, KObject> = HashMap::new();
    inner_map.insert(
        Box::new(KKey::String("f".into())),
        KObject::KFunction(kf_ref, None),
    );
    let outer = KObject::List(Rc::new(vec![KObject::Dict(Rc::new(inner_map))]));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&outer, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(items) => match &items[0] {
            KObject::Dict(entries) => match entries.values().next().unwrap() {
                KObject::KFunction(_, frame) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {:?}", other.ktype()),
            },
            other => panic!("expected nested Dict, got {:?}", other.ktype()),
        },
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// `any_descendant`'s Tagged recursion arm (137). `List<Tagged<KFunction>>`
/// walks the outer list, recurses into Tagged's `value`, finds the KFunction.
#[test]
fn list_of_tagged_with_kfunction_anchors_via_recursion() {
    use crate::machine::ScopeId;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let tagged = KObject::Tagged {
        tag: "T".into(),
        value: Rc::new(KObject::KFunction(kf_ref, None)),
        scope_id: ScopeId::next(),
        name: "Carrier".into(),
    };
    let outer = KObject::List(Rc::new(vec![tagged]));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&outer, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(items) => match &items[0] {
            KObject::Tagged { value, .. } => match &**value {
                KObject::KFunction(_, frame) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {:?}", other.ktype()),
            },
            other => panic!("expected nested Tagged, got {:?}", other.ktype()),
        },
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// `needs_lift`'s pre-anchored short-circuit arms (164, 169, 171) — when a
/// List descendant already carries its own `Some(rc)` anchor, the predicate
/// must return `Some(false)` and the list must NOT mark them as needing lift.
#[test]
fn list_with_pre_anchored_variants_skips_them() {
    use crate::machine::core::kfunction::ArgumentBundle;
    use crate::machine::model::values::Module;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);
    let other = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);
    let module = Module::new("M".into(), dying.scope());
    let m_ref: &Module = dying.arena().alloc_module(module);

    let future = KFuture {
        parsed: KExpression { parts: vec![] },
        function: kf_ref,
        bundle: ArgumentBundle { args: HashMap::new() },
    };
    let items = Rc::new(vec![
        KObject::KFunction(kf_ref, Some(Rc::clone(&other))),
        KObject::KFuture(future, Some(Rc::clone(&other))),
        KObject::KModule(m_ref, Some(Rc::clone(&other))),
    ]);
    let list = KObject::List(Rc::clone(&items));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let dying_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(out) => assert!(
            Rc::ptr_eq(out, &items),
            "all pre-anchored ⇒ no needs_lift descendant ⇒ Rc reuse",
        ),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(dying_after, before, "pre-anchored variants must not bump dying Rc");
}

/// `needs_lift`'s KFuture None arm (170) — unanchored KFuture inside a list
/// whose function captured the dying scope drives the rebuild.
#[test]
fn list_with_unanchored_kfuture_anchors() {
    use crate::machine::core::kfunction::ArgumentBundle;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let future = KFuture {
        parsed: KExpression { parts: vec![] },
        function: kf_ref,
        bundle: ArgumentBundle { args: HashMap::new() },
    };
    let list = KObject::List(Rc::new(vec![KObject::KFuture(future, None)]));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(out) => assert!(matches!(&out[0], KObject::KFuture(_, Some(_)))),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// `needs_lift`'s KModule None arm (172–174) — unanchored KModule whose
/// child scope is the dying arena, inside a list.
#[test]
fn list_with_unanchored_kmodule_anchors() {
    use crate::machine::model::values::Module;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);
    let module = Module::new("LocalM".into(), dying.scope());
    let m_ref: &Module = dying.arena().alloc_module(module);

    let list = KObject::List(Rc::new(vec![KObject::KModule(m_ref, None)]));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(out) => assert!(matches!(&out[0], KObject::KModule(_, Some(_)))),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
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

/// `needs_lift`'s `Struct | KExpression => Some(false)` arm (176) — Struct
/// and KExpression descendants inside a List are leaves to needs_lift, so
/// the list must reuse its Rc (no rebuild) when those are its only contents.
#[test]
fn list_with_struct_and_kexpression_descendants_clones_rc() {
    use crate::machine::ScopeId;
    use indexmap::IndexMap;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let fields: IndexMap<String, KObject> = IndexMap::new();
    let s = KObject::Struct {
        name: "S".into(),
        scope_id: ScopeId::next(),
        fields: Rc::new(fields),
    };
    let e = KObject::KExpression(KExpression { parts: vec![] });
    let items = Rc::new(vec![s, e]);
    let list = KObject::List(Rc::clone(&items));
    let before = Rc::strong_count(&items);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&items);
    match &lifted {
        KObject::List(out) => assert!(Rc::ptr_eq(out, &items)),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// List of non-borrowing leaves must lift via `Rc::clone` — the rebuild branch
/// would over-allocate and break the fast-path/needs_lift invariant.
#[test]
fn list_no_descendants_clones_rc() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let items = Rc::new(vec![KObject::Number(1.0), KObject::Number(2.0)]);
    let list = KObject::List(Rc::clone(&items));
    let before = Rc::strong_count(&items);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&items);
    match lifted {
        KObject::List(out) => assert!(
            Rc::ptr_eq(&out, &items),
            "non-borrowing list must reuse the inner Rc"
        ),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1, "Rc::clone bumps count by 1");
}

/// List containing a KFunction whose captured scope is the dying arena must rebuild
/// the list and anchor the inner KFunction on the dying frame's Rc.
#[test]
fn list_with_local_kfunction_rebuilds_and_anchors() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let list = KObject::List(Rc::new(vec![KObject::KFunction(kf_ref, None)]));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::List(out) => match &out[0] {
            KObject::KFunction(_, frame) => assert!(
                frame.is_some(),
                "nested KFunction must anchor on dying frame's Rc",
            ),
            other => panic!("expected nested KFunction, got {:?}", other.ktype()),
        },
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1, "one anchored descendant ⇒ +1 Rc");
}

/// Dict counterpart of `list_no_descendants_clones_rc`.
#[test]
fn dict_no_descendants_clones_rc() {
    use crate::machine::model::types::Serializable;
    use crate::machine::model::values::KKey;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let mut map: HashMap<Box<dyn Serializable>, KObject> = HashMap::new();
    map.insert(Box::new(KKey::String("a".into())), KObject::Number(1.0));
    let entries = Rc::new(map);
    let dict = KObject::Dict(Rc::clone(&entries));
    let before = Rc::strong_count(&entries);

    let lifted = lift_kobject(&dict, &dying);
    let count_after = Rc::strong_count(&entries);
    match lifted {
        KObject::Dict(out) => assert!(
            Rc::ptr_eq(&out, &entries),
            "non-borrowing dict must reuse the inner Rc",
        ),
        other => panic!("expected Dict, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// Dict counterpart of `list_with_local_kfunction_rebuilds_and_anchors`.
#[test]
fn dict_with_local_kfunction_rebuilds_and_anchors() {
    use crate::machine::model::types::Serializable;
    use crate::machine::model::values::KKey;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let mut map: HashMap<Box<dyn Serializable>, KObject> = HashMap::new();
    map.insert(
        Box::new(KKey::String("f".into())),
        KObject::KFunction(kf_ref, None),
    );
    let dict = KObject::Dict(Rc::new(map));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&dict, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::Dict(out) => {
            let v = out.values().next().expect("one entry");
            match v {
                KObject::KFunction(_, frame) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {:?}", other.ktype()),
            }
        }
        other => panic!("expected Dict, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// Tagged wrapping a non-borrowing value must reuse the inner `Rc` *and* preserve
/// `(scope_id, name)` identity through the no-rebuild branch.
#[test]
fn tagged_no_borrow_clones_inner_rc() {
    use crate::machine::ScopeId;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let inner = Rc::new(KObject::Number(42.0));
    let sid = ScopeId::next();
    let tagged = KObject::Tagged {
        tag: "Just".into(),
        value: Rc::clone(&inner),
        scope_id: sid,
        name: "Maybe".into(),
    };
    let before = Rc::strong_count(&inner);

    let lifted = lift_kobject(&tagged, &dying);
    let count_after = Rc::strong_count(&inner);
    match lifted {
        KObject::Tagged { tag, value, scope_id, name } => {
            assert!(Rc::ptr_eq(&value, &inner), "no-borrow Tagged must reuse inner Rc");
            assert_eq!(tag, "Just");
            assert_eq!(name, "Maybe");
            assert_eq!(scope_id, sid);
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// Tagged wrapping a borrowing KFunction must rebuild and propagate
/// `(scope_id, name)` unchanged through the rebuild branch.
#[test]
fn tagged_with_local_kfunction_rebuilds_and_anchors() {
    use crate::machine::ScopeId;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let sid = ScopeId::next();
    let tagged = KObject::Tagged {
        tag: "Wrap".into(),
        value: Rc::new(KObject::KFunction(kf_ref, None)),
        scope_id: sid,
        name: "Carrier".into(),
    };
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&tagged, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::Tagged { tag, value, scope_id, name } => {
            assert_eq!(tag, "Wrap");
            assert_eq!(name, "Carrier");
            assert_eq!(scope_id, sid);
            match &*value {
                KObject::KFunction(_, frame) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {:?}", other.ktype()),
            }
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// A pre-anchored KFunction must keep its existing `Rc` instead of re-deriving
/// from `dying` — even if it could have anchored fresh, double-anchoring would
/// extend two arenas' lives on one descendant.
#[test]
fn kfunction_with_existing_anchor_preserves_it() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let other = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let pre_anchored = KObject::KFunction(kf_ref, Some(Rc::clone(&other)));
    let other_before = Rc::strong_count(&other);
    let dying_before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&pre_anchored, &dying);
    let other_after = Rc::strong_count(&other);
    let dying_after = Rc::strong_count(&dying);
    match lifted {
        KObject::KFunction(_, frame) => {
            let f = frame.expect("pre-anchored frame must persist");
            assert!(Rc::ptr_eq(&f, &other), "must reuse existing anchor, not re-derive");
        }
        other => panic!("expected KFunction, got {:?}", other.ktype()),
    }
    assert_eq!(
        other_after,
        other_before + 1,
        "preserved anchor clones the existing Rc once",
    );
    assert_eq!(
        dying_after,
        dying_before,
        "preserved anchor must not also touch the dying frame's Rc",
    );
}

/// A KFunction whose captured scope lives in a different runtime arena must
/// lift to `frame: None` — anchoring on `dying` would not protect the foreign
/// captured scope (which `dying`'s arena doesn't own).
#[test]
fn kfunction_with_foreign_runtime_does_not_anchor() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    use crate::machine::model::{ExpressionSignature, KType, ReturnType, SignatureElement};
    use crate::machine::{Body, BodyResult, KFunction};
    let foreign = KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Null),
            elements: vec![SignatureElement::Keyword("__FOREIGN__".into())],
        },
        Body::Builtin(|s, _, _| BodyResult::Value(s.arena.alloc_object(KObject::Null))),
        scope,
    );
    let foreign_ref: &KFunction = arena.alloc_function(foreign);
    let obj = KObject::KFunction(foreign_ref, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::KFunction(_, frame) => assert!(
            frame.is_none(),
            "foreign-runtime KFunction must not anchor on dying frame",
        ),
        other => panic!("expected KFunction, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before, "non-anchor lift must not bump Rc");
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

/// KModule whose child scope was allocated in the dying frame's arena must
/// anchor on the dying frame's Rc — same lifecycle rule as the KFunction arm.
#[test]
fn kmodule_with_local_child_scope_anchors() {
    use crate::machine::model::values::Module;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let module = Module::new("LocalMod".into(), dying.scope());
    let m_ref: &Module = dying.arena().alloc_module(module);
    let obj = KObject::KModule(m_ref, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::KModule(_, frame) => assert!(
            frame.is_some(),
            "KModule with child scope in dying arena must anchor",
        ),
        other => panic!("expected KModule, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// Symmetric: KModule whose child scope lives in a foreign runtime must lift
/// with `frame: None`.
#[test]
fn kmodule_with_foreign_child_scope_does_not_anchor() {
    use crate::machine::model::values::Module;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let module = Module::new("ForeignMod".into(), scope);
    let m_ref: &Module = arena.alloc_module(module);
    let obj = KObject::KModule(m_ref, None);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::KModule(_, frame) => assert!(frame.is_none()),
        other => panic!("expected KModule, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before);
}

/// Pre-anchored KModule preserves its existing Rc — same shape as the
/// KFunction / KFuture preservation cases.
#[test]
fn kmodule_with_existing_anchor_preserves_it() {
    use crate::machine::model::values::Module;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);
    let other = CallArena::new(scope, None);

    let module = Module::new("Pre".into(), dying.scope());
    let m_ref: &Module = dying.arena().alloc_module(module);
    let obj = KObject::KModule(m_ref, Some(Rc::clone(&other)));
    let other_before = Rc::strong_count(&other);

    let lifted = lift_kobject(&obj, &dying);
    let other_after = Rc::strong_count(&other);
    match lifted {
        KObject::KModule(_, frame) => {
            let f = frame.expect("pre-anchored frame persists");
            assert!(Rc::ptr_eq(&f, &other));
        }
        other => panic!("expected KModule, got {:?}", other.ktype()),
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

/// Non-composite, non-function variants fall through to `deep_clone` on the
/// slow path — the `other` catch-all arm. Defeats the fast path so the match
/// is actually reached.
#[test]
fn primitive_lifts_via_deep_clone_on_slow_path() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let obj = KObject::Number(2.5);
    let lifted = lift_kobject(&obj, &dying);
    match lifted {
        KObject::Number(n) => assert_eq!(n, 2.5),
        other => panic!("expected Number, got {:?}", other.ktype()),
    }
}
