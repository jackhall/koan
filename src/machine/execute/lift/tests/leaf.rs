//! `leaf` arms of `lift_kobject`.

use super::*;
use crate::builtins::default_scope;
use crate::machine::model::KObject;
use crate::machine::CallArena;

use super::{alloc_local_kf, defeat_fast_path};

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
            assert!(
                Rc::ptr_eq(&f, &other),
                "must reuse existing anchor, not re-derive"
            );
        }
        other => panic!("expected KFunction, got {:?}", other.ktype()),
    }
    assert_eq!(
        other_after,
        other_before + 1,
        "preserved anchor clones the existing Rc once",
    );
    assert_eq!(
        dying_after, dying_before,
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
        Body::Builtin(|s, _, _| BodyResult::value(s.arena.alloc_object(KObject::Null))),
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
    let obj = KType::Module {
        module: m_ref,
        frame: None,
    };
    let before = Rc::strong_count(&dying);

    let lifted = lift_ktype(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KType::Module { module: _, frame } => assert!(
            frame.is_some(),
            "KModule with child scope in dying arena must anchor",
        ),
        other => panic!("expected KModule, got {}", other.name()),
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
    let obj = KType::Module {
        module: m_ref,
        frame: None,
    };
    let before = Rc::strong_count(&dying);

    let lifted = lift_ktype(&obj, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KType::Module { module: _, frame } => assert!(frame.is_none()),
        other => panic!("expected KModule, got {}", other.name()),
    }
    assert_eq!(count_after, before);
}

/// Pre-anchored KModule preserves its existing Rc.
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
    let obj = KType::Module {
        module: m_ref,
        frame: Some(Rc::clone(&other)),
    };
    let other_before = Rc::strong_count(&other);

    let lifted = lift_ktype(&obj, &dying);
    let other_after = Rc::strong_count(&other);
    match lifted {
        KType::Module { module: _, frame } => {
            let f = frame.expect("pre-anchored frame persists");
            assert!(Rc::ptr_eq(&f, &other));
        }
        other => panic!("expected KModule, got {}", other.name()),
    }
    assert_eq!(other_after, other_before + 1);
}

/// Non-composite, non-function variants fall through to `deep_clone` on the
/// slow path. Defeats the fast path so the match is actually reached.
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
