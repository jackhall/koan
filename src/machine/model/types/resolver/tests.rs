use crate::machine::BindingIndex;
use super::*;
use crate::machine::model::ast::TypeExpr;
use crate::machine::RuntimeArena;
use crate::builtins::test_support::run_root_silent;

fn leaf(n: &str) -> TypeExpr {
    TypeExpr::leaf(n.into())
}

fn list_typeexpr(name: &str, items: Vec<TypeExpr>) -> TypeExpr {
    TypeExpr {
        name: name.into(),
        params: TypeParams::List(items),
        builtin_cache: std::cell::OnceCell::new(),
    }
}

#[test]
fn wrap_applied_elaborates_to_constructor_apply() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let ctor = KType::UserType {
        kind: UserTypeKind::TypeConstructor { schema: std::rc::Rc::new(std::collections::HashMap::new()), param_names: vec!["Type".into()] },
        scope_id: ScopeId::from_raw(0, 0xC0DE),
        name: "Wrap".into(),
    };
    scope.register_type("Wrap".into(), ctor.clone(), BindingIndex::BUILTIN);
    let te = list_typeexpr("Wrap", vec![leaf("Number")]);
    let mut el = Elaborator::new(scope);
    match elaborate_type_expr(&mut el, &te) {
        ElabResult::Done(kt) => match kt {
            KType::ConstructorApply { ctor: got_ctor, args } => {
                assert_eq!(*got_ctor, ctor);
                assert_eq!(args, vec![KType::Number]);
            }
            other => panic!("expected ConstructorApply, got {:?}", other),
        },
        other => panic!("expected Done, got {:?}", other),
    }
}

/// Per-call generativity: distinct `Wrap` constructors yield structurally
/// distinct `ConstructorApply`s under the same surface form.
#[test]
fn wrap_applied_distinct_per_ascription() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let scope_a = arena.alloc_scope(crate::machine::core::Scope::child_under(scope));
    let scope_b = arena.alloc_scope(crate::machine::core::Scope::child_under(scope));
    let ctor_a = KType::UserType {
        kind: UserTypeKind::TypeConstructor { schema: std::rc::Rc::new(std::collections::HashMap::new()), param_names: vec!["Type".into()] },
        scope_id: ScopeId::from_raw(0, 0xAAAA),
        name: "Wrap".into(),
    };
    let ctor_b = KType::UserType {
        kind: UserTypeKind::TypeConstructor { schema: std::rc::Rc::new(std::collections::HashMap::new()), param_names: vec!["Type".into()] },
        scope_id: ScopeId::from_raw(0, 0xBBBB),
        name: "Wrap".into(),
    };
    scope_a.register_type("Wrap".into(), ctor_a.clone(), BindingIndex::BUILTIN);
    scope_b.register_type("Wrap".into(), ctor_b.clone(), BindingIndex::BUILTIN);

    let te = list_typeexpr("Wrap", vec![leaf("Number")]);
    let mut ela = Elaborator::new(scope_a);
    let kt_a = match elaborate_type_expr(&mut ela, &te) {
        ElabResult::Done(kt) => kt,
        other => panic!("expected Done, got {:?}", other),
    };
    let mut elb = Elaborator::new(scope_b);
    let kt_b = match elaborate_type_expr(&mut elb, &te) {
        ElabResult::Done(kt) => kt,
        other => panic!("expected Done, got {:?}", other),
    };
    assert_ne!(kt_a, kt_b);
}

#[test]
fn wrap_applied_arity_mismatch_unbound() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let ctor = KType::UserType {
        kind: UserTypeKind::TypeConstructor { schema: std::rc::Rc::new(std::collections::HashMap::new()), param_names: vec!["Type".into()] },
        scope_id: ScopeId::from_raw(0, 0xC0DE),
        name: "Wrap".into(),
    };
    scope.register_type("Wrap".into(), ctor, BindingIndex::BUILTIN);
    let te = list_typeexpr("Wrap", vec![leaf("Number"), leaf("Str")]);
    let mut el = Elaborator::new(scope);
    match elaborate_type_expr(&mut el, &te) {
        ElabResult::Unbound(msg) => {
            assert!(
                msg.contains("expects 1") && msg.contains("got 2"),
                "expected arity message naming counts, got: {msg}",
            );
        }
        other => panic!("expected Unbound, got {:?}", other),
    }
}

/// A forward reference to an in-flight `LET Wrap = ...` parks rather than
/// reporting `Unbound`, so FN-defs returning `Wrap<...>` resolve once the
/// producer fires.
#[test]
fn wrap_applied_parks_on_placeholder() {
    use crate::machine::execute::Scheduler;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = Scheduler::new();
    let dummy = sched.add_dispatch(
        crate::builtins::test_support::parse_one("LET _placeholder_target = 1"),
        scope,
    );
    scope.install_placeholder("Wrap".into(), dummy, BindingIndex::BUILTIN).expect("placeholder install");
    let te = list_typeexpr("Wrap", vec![leaf("Number")]);
    let mut el = Elaborator::new(scope);
    match elaborate_type_expr(&mut el, &te) {
        ElabResult::Park(ids) => {
            assert!(ids.contains(&dummy), "expected parked on the Wrap placeholder, got {:?}", ids);
        }
        other => panic!("expected Park, got {:?}", other),
    }
}

/// A Type-class leaf bound only in the value language reports the layering
/// vocabulary, not an unknown-name miss
/// (see [design/typing/functors.md](../../../../../design/typing/functors.md)).
#[test]
fn value_language_leaf_names_layering() {
    use crate::machine::model::values::KObject;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    scope
        .bind_value("Gee".into(), arena.alloc(KObject::Number(7.0)), BindingIndex::BUILTIN)
        .expect("bind_value");
    let mut el = Elaborator::new(scope);
    match elaborate_type_expr(&mut el, &leaf("Gee")) {
        ElabResult::Unbound(msg) => assert!(
            msg.contains("value-language only") && msg.contains("Gee"),
            "expected a value-language layering message naming `Gee`, got: {msg}",
        ),
        other => panic!("expected Unbound, got {:?}", other),
    }
}

#[test]
fn unbound_leaf_names_unknown_type() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut el = Elaborator::new(scope);
    match elaborate_type_expr(&mut el, &leaf("NopeType")) {
        ElabResult::Unbound(msg) => assert!(
            msg.contains("unknown type name") && msg.contains("NopeType"),
            "expected an unknown-type-name message naming `NopeType`, got: {msg}",
        ),
        other => panic!("expected Unbound, got {:?}", other),
    }
}

#[test]
fn functor_type_position_sigil_elaborates_to_kfunctor() {
    use crate::machine::model::ast::TypeParams;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let te = TypeExpr {
        name: "Functor".into(),
        params: TypeParams::Function {
            args: vec![leaf("Number")],
            ret: Box::new(leaf("Str")),
        },
        builtin_cache: std::cell::OnceCell::new(),
    };
    let mut el = Elaborator::new(scope);
    match elaborate_type_expr(&mut el, &te) {
        ElabResult::Done(KType::KFunctor { params, ret }) => {
            assert_eq!(params, vec![KType::Number]);
            assert_eq!(*ret, KType::Str);
        }
        other => panic!("expected Done(KFunctor), got {:?}", other),
    }
}

/// `:(Functor ...)` with a forward reference in an arg position parks on the
/// producer placeholder, mirroring the `Function` arm.
#[test]
fn functor_type_position_sigil_parks_on_forward_ref() {
    use crate::machine::execute::Scheduler;
    use crate::machine::model::ast::TypeParams;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = Scheduler::new();
    let dummy = sched.add_dispatch(
        crate::builtins::test_support::parse_one("LET _placeholder = 1"),
        scope,
    );
    scope.install_placeholder("MyType".into(), dummy, BindingIndex::BUILTIN).expect("placeholder install");
    let te = TypeExpr {
        name: "Functor".into(),
        params: TypeParams::Function {
            args: vec![leaf("MyType")],
            ret: Box::new(leaf("Number")),
        },
        builtin_cache: std::cell::OnceCell::new(),
    };
    let mut el = Elaborator::new(scope);
    match elaborate_type_expr(&mut el, &te) {
        ElabResult::Park(ids) => assert!(
            ids.contains(&dummy),
            "expected parked on the MyType placeholder, got {:?}",
            ids,
        ),
        other => panic!("expected Park, got {:?}", other),
    }
}

#[test]
fn constructor_apply_name_renders_surface_form() {
    let ctor = KType::UserType {
        kind: UserTypeKind::TypeConstructor { schema: std::rc::Rc::new(std::collections::HashMap::new()), param_names: vec!["Type".into()] },
        scope_id: ScopeId::from_raw(0, 0xC0DE),
        name: "Wrap".into(),
    };
    let app = KType::ConstructorApply {
        ctor: Box::new(ctor),
        args: vec![KType::Number],
    };
    assert_eq!(app.name(), ":(Wrap Number)");
}
