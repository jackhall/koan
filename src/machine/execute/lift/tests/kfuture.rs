//! `kfuture` arms of `lift_kobject`.

use super::*;
use crate::builtins::default_scope;
use crate::machine::core::FrameStorage;
use crate::machine::model::types::Record;
use crate::machine::model::values::ArgValue;
use crate::machine::model::Carried;
use crate::machine::model::KObject;
use crate::machine::CallFrame;
use crate::parse::parse;
use crate::source::Spanned;

use super::{alloc_local_kf, defeat_fast_path, dispatch_for_test};

/// A KFuture with no descendant borrow into the dying region must lift to
/// `frame: None` — anchoring would over-keep the region.
#[test]
fn unanchored_kfuture_no_region_borrow_does_not_anchor() {
    use crate::machine::model::{ExpressionSignature, KType, ReturnType, SignatureElement};
    use crate::machine::{Body, KFunction};

    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    // Defeat the `functions_is_empty()` fast path so the slow path runs.
    let kf = KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Null),
            elements: vec![SignatureElement::Keyword("__SLOW__".into())],
        },
        Body::Builtin(|ctx| {
            crate::machine::core::kfunction::action::Action::Done(Ok(
                crate::machine::model::Carried::Object(
                    ctx.scope.region.alloc_object(KObject::Null),
                ),
            ))
        }),
        dying.scope(),
    );
    let _ = dying.region().alloc_function(kf);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let kf_obj = KObject::KFuture(future, None);

    let strong_before = Rc::strong_count(&dying.storage_rc());

    let lifted = lift_kobject(&kf_obj, &dying.storage_rc());

    match lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_none()),
        other => panic!("expected lifted KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(Rc::strong_count(&dying.storage_rc()), strong_before);
}

/// A KFuture whose parsed parts contain a `Spliced(Carried::Object(_))` allocated in
/// the dying region must lift with `frame: Some(rc)`.
#[test]
fn unanchored_kfuture_with_region_borrow_does_anchor() {
    use crate::machine::model::{ExpressionSignature, KType, ReturnType, SignatureElement};
    use crate::machine::{Body, KFunction};

    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);

    let kf = KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Null),
            elements: vec![SignatureElement::Keyword("__SLOW__".into())],
        },
        Body::Builtin(|ctx| {
            crate::machine::core::kfunction::action::Action::Done(Ok(
                crate::machine::model::Carried::Object(
                    ctx.scope.region.alloc_object(KObject::Null),
                ),
            ))
        }),
        dying.scope(),
    );
    let _ = dying.region().alloc_function(kf);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let inside: &KObject = dying.region().alloc_object(KObject::Number(7.0));
    future
        .parsed
        .parts
        .push(Spanned::bare(ExpressionPart::Spliced(Carried::Object(
            inside,
        ))));
    let kf_obj = KObject::KFuture(future, None);

    let strong_before = Rc::strong_count(&dying.storage_rc());
    let lifted = lift_kobject(&kf_obj, &dying.storage_rc());
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected lifted KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(Rc::strong_count(&dying.storage_rc()), strong_before + 1);
    // Drop borrowers before `dying` so region teardown order is well-defined.
    drop(lifted);
    drop(kf_obj);
}

/// A KFuture parked inside another KFuture's `bundle.args` exercises the
/// recursive borrow walk; the inner future borrows via its captured function.
#[test]
fn kfuture_bundle_arg_with_nested_kfuture_anchors() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let inner_future = KFuture {
        parsed: KExpression::new(vec![]),
        function: kf_ref,
        args: Record::new(),
    };

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut outer = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    outer.args.insert(
        "f".into(),
        ArgValue::Object(Rc::new(KObject::KFuture(inner_future, None))),
    );
    let obj = KObject::KFuture(outer, None);
    let before = Rc::strong_count(&dying.storage_rc());

    let lifted = lift_kobject(&obj, &dying.storage_rc());
    let count_after = Rc::strong_count(&dying.storage_rc());
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// A KFuture whose `bundle.args` carries a record-repr newtype with a borrowing field
/// exercises recursion through the `Wrapped`'s inner record.
#[test]
fn kfuture_bundle_arg_with_wrapped_field_anchors() {
    use crate::machine::ScopeId;
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    use crate::machine::model::types::{KType, NominalSchema, RecursiveSet};
    use crate::machine::model::values::NonWrappedRef;
    let record = KObject::record(Record::from_pairs(vec![(
        "f".to_string(),
        KObject::KFunction(kf_ref, None),
    )]));
    let type_id: &KType = region.region().alloc_ktype(KType::SetRef {
        set: RecursiveSet::singleton(
            "S".into(),
            ScopeId::next(),
            NominalSchema::NewType(Box::new(KType::Record(Box::new(Record::new())))),
        ),
        index: 0,
    });
    let s = KObject::Wrapped {
        inner: NonWrappedRef::peel(&record),
        type_id,
    };

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    future.args.insert("s".into(), ArgValue::Object(Rc::new(s)));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying.storage_rc());

    let lifted = lift_kobject(&obj, &dying.storage_rc());
    let count_after = Rc::strong_count(&dying.storage_rc());
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// A `parsed.parts` `Expression(Box<KExpression>)` whose inner parts borrow
/// into the dying region must drive anchor.
#[test]
fn kfuture_parsed_expression_part_with_region_borrow_anchors() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    defeat_fast_path(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let inside: &KObject = dying.region().alloc_object(KObject::Number(17.0));
    let inner = KExpression::new(vec![Spanned::bare(ExpressionPart::Spliced(
        Carried::Object(inside),
    ))]);
    future
        .parsed
        .parts
        .push(Spanned::bare(ExpressionPart::Expression(Box::new(inner))));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying.storage_rc());

    let lifted = lift_kobject(&obj, &dying.storage_rc());
    let count_after = Rc::strong_count(&dying.storage_rc());
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// A `KExpression` parked in `bundle.args` whose inner parts borrow into
/// the dying region must drive anchor.
#[test]
fn kfuture_bundle_arg_with_kexpression_borrow_anchors() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    defeat_fast_path(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let inside: &KObject = dying.region().alloc_object(KObject::Number(19.0));
    let inner = KExpression::new(vec![Spanned::bare(ExpressionPart::Spliced(
        Carried::Object(inside),
    ))]);
    future.args.insert(
        "e".into(),
        ArgValue::Object(Rc::new(KObject::KExpression(inner))),
    );
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying.storage_rc());

    let lifted = lift_kobject(&obj, &dying.storage_rc());
    let count_after = Rc::strong_count(&dying.storage_rc());
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// Pre-anchored KFuture preserves its anchor through lift.
#[test]
fn kfuture_with_existing_anchor_preserves_it() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    defeat_fast_path(&dying);
    let other = CallFrame::new_test(scope, None);
    let other_storage = other.storage_rc();

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let obj = KObject::KFuture(future, Some(Rc::clone(&other_storage)));
    let other_before = Rc::strong_count(&other_storage);

    let lifted = lift_kobject(&obj, &dying.storage_rc());
    let other_after = Rc::strong_count(&other_storage);
    match lifted {
        KObject::KFuture(_, frame) => {
            let f = frame.expect("pre-anchored frame must persist");
            assert!(Rc::ptr_eq(&f, &other_storage));
        }
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(other_after, other_before + 1);
}

/// A KFunction whose captured scope lives in the dying region, parked in a
/// bundle slot, must drive lift to anchor.
#[test]
fn kfuture_bundle_arg_with_local_kfunction_anchors() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    future.args.insert(
        "borrower".into(),
        ArgValue::Object(Rc::new(KObject::KFunction(kf_ref, None))),
    );
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying.storage_rc());

    let lifted = lift_kobject(&obj, &dying.storage_rc());
    let count_after = Rc::strong_count(&dying.storage_rc());
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// A KFuture whose own function was captured in the dying region anchors
/// without needing any borrowing payload in parts or bundle.
#[test]
fn kfuture_with_local_function_anchors() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let future = KFuture {
        parsed: KExpression::new(vec![]),
        function: kf_ref,
        args: Record::new(),
    };
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying.storage_rc());

    let lifted = lift_kobject(&obj, &dying.storage_rc());
    let count_after = Rc::strong_count(&dying.storage_rc());
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// A bundle-arg `List` containing a dying-captured KFunction exercises the
/// composite-recursion arms (List/Dict/Tagged).
#[test]
fn kfuture_bundle_arg_with_list_of_kfunction_anchors() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let nested = KObject::list(vec![KObject::KFunction(kf_ref, None)]);
    future
        .args
        .insert("nested".into(), ArgValue::Object(Rc::new(nested)));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying.storage_rc());

    let lifted = lift_kobject(&obj, &dying.storage_rc());
    let count_after = Rc::strong_count(&dying.storage_rc());
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// A KModule whose child scope lives in the dying region, parked in a
/// bundle slot, must drive anchor.
#[test]
fn kfuture_bundle_arg_with_local_kmodule_anchors() {
    use crate::machine::model::values::Module;
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    defeat_fast_path(&dying);

    let module = Module::new("BundleMod".into(), dying.scope());
    let m_ref: &Module = dying.region().alloc_module(module);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    future.args.insert(
        "m".into(),
        ArgValue::Type(KType::Module {
            module: m_ref,
            frame: None,
        }),
    );
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying.storage_rc());

    let lifted = lift_kobject(&obj, &dying.storage_rc());
    let count_after = Rc::strong_count(&dying.storage_rc());
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// A `parsed.parts` `ListLiteral` whose inner `Spliced` part points into
/// the dying region must drive anchor.
#[test]
fn kfuture_parsed_listliteral_with_region_borrow_anchors() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    defeat_fast_path(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let inside: &KObject = dying.region().alloc_object(KObject::Number(11.0));
    future
        .parsed
        .parts
        .push(Spanned::bare(ExpressionPart::ListLiteral(vec![
            ExpressionPart::Spliced(Carried::Object(inside)),
        ])));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying.storage_rc());

    let lifted = lift_kobject(&obj, &dying.storage_rc());
    let count_after = Rc::strong_count(&dying.storage_rc());
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}

/// A `parsed.parts` `DictLiteral` whose value side carries a borrowing
/// `Spliced` part must drive anchor.
#[test]
fn kfuture_parsed_dictliteral_with_region_borrow_anchors() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let dying = CallFrame::new_test(scope, None);
    defeat_fast_path(&dying);

    let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
    let parsed = exprs.remove(0);
    let mut future = dispatch_for_test(scope, parsed).expect("dispatch should succeed");
    let inside: &KObject = dying.region().alloc_object(KObject::Number(13.0));
    future
        .parsed
        .parts
        .push(Spanned::bare(ExpressionPart::DictLiteral(vec![(
            ExpressionPart::Keyword("k".into()),
            ExpressionPart::Spliced(Carried::Object(inside)),
        )])));
    let obj = KObject::KFuture(future, None);
    let before = Rc::strong_count(&dying.storage_rc());

    let lifted = lift_kobject(&obj, &dying.storage_rc());
    let count_after = Rc::strong_count(&dying.storage_rc());
    match &lifted {
        KObject::KFuture(_, frame) => assert!(frame.is_some()),
        other => panic!("expected KFuture, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
    drop(lifted);
    drop(obj);
}
