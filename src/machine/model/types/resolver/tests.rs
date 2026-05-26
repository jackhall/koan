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

/// B2: `Wrap<Number>` where `Wrap` is bound in `bindings.types` as a
/// `KType::UserType { kind: UserTypeKind::TypeConstructor { param_names: ["Type"] }, .. }`
/// elaborates to `KType::ConstructorApply { ctor: <that UserType>, args: [Number] }`.
/// Pins the constructor-application arm in `elaborate_type_expr`.
#[test]
fn wrap_applied_elaborates_to_constructor_apply() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // Register a TypeConstructor under `Wrap` directly (mirrors what
    // `ascribe.rs:body_opaque` mints at runtime).
    let ctor = KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
        scope_id: ScopeId::from_raw(0, 0xC0DE),
        name: "Wrap".into(),
    };
    scope.register_type("Wrap".into(), ctor.clone(), BindingIndex::BUILTIN);
    // Surface form: `Wrap<Number>` — a TypeExpr with name "Wrap" + List params.
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

/// B2: two opaque ascriptions of the same SIG mint distinct `Wrap` constructors;
/// the `ConstructorApply`s produced by elaborating against each per-call scope
/// must therefore differ structurally. Pins the per-call generativity property
/// extending into the applied-form layer.
#[test]
fn wrap_applied_distinct_per_ascription() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let scope_a = arena.alloc_scope(crate::machine::core::Scope::child_under(scope));
    let scope_b = arena.alloc_scope(crate::machine::core::Scope::child_under(scope));
    let ctor_a = KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
        scope_id: ScopeId::from_raw(0, 0xAAAA),
        name: "Wrap".into(),
    };
    let ctor_b = KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
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
    // Structural inequality: ctor identities differ by scope_id.
    assert_ne!(kt_a, kt_b);
}

/// Arity mismatch surfaces a focused error rather than building a wrong-shape
/// `ConstructorApply`. Pins the elaborator's arity check.
#[test]
fn wrap_applied_arity_mismatch_unbound() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let ctor = KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
        scope_id: ScopeId::from_raw(0, 0xC0DE),
        name: "Wrap".into(),
    };
    scope.register_type("Wrap".into(), ctor, BindingIndex::BUILTIN);
    // Two args against a single-param constructor: arity mismatch.
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

/// Confirms that a parked-on placeholder for `name` (LET binding hasn't run yet)
/// reports `ElabResult::Park`, not `Unbound`. Pins the forward-reference path
/// added to the constructor-application arm so FN-defs in a SIG body whose
/// return type is `Wrap<...>` correctly park on the in-flight `LET Wrap = ...`.
#[test]
fn wrap_applied_parks_on_placeholder() {
    use crate::machine::execute::Scheduler;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // Install a placeholder for `Wrap` (NodeId 0xDEAD won't dispatch; the test
    // only inspects the elaborator's response).
    let mut sched = Scheduler::new();
    // Reserve a NodeId for the placeholder. Use `add_dispatch` on a no-op expr so
    // the scheduler has a real node id to install.
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

/// A bare leaf that resolves to a *value-language* binding (a name in `bindings.data`,
/// not `bindings.types`, and not a builtin type) reports the type-language/value-language
/// layering rather than reading as an unknown-name miss. Pins the `Resolution::Value`
/// arm's diagnostic vocabulary (design/typing/functors.md).
#[test]
fn value_language_leaf_names_layering() {
    use crate::machine::model::values::KObject;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // `Gee` is a Type-class token, but bound in the value language.
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

/// A genuinely-unbound bare leaf (not in any map, not a builtin) reports an
/// unknown-type-name miss naming the offender verbatim.
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

/// Stage 1: bare `:(Functor (params) -> R)` elaboration lowers to
/// `KType::KFunctor { params, ret }` with the right structural shape. Parallels
/// the `Function` arm's path; only the head keyword and the resulting variant
/// differ.
#[test]
fn functor_type_position_sigil_elaborates_to_kfunctor() {
    use crate::machine::model::ast::TypeParams;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // `:(Functor (Number) -> Str)` — a 1-ary functor type with primitive slots.
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

/// Stage 1: `:(Functor ...)` with a forward reference inside an arg position
/// parks on the producer placeholder, matching the `Function` arm's forward-ref
/// coverage. Closes the symmetry: the new arm must inherit Park/Done/Unbound
/// precedence verbatim.
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

/// `name()` round-trip — pin the diagnostic surface form `ctor<arg>`.
#[test]
fn constructor_apply_name_renders_surface_form() {
    let ctor = KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
        scope_id: ScopeId::from_raw(0, 0xC0DE),
        name: "Wrap".into(),
    };
    let app = KType::ConstructorApply {
        ctor: Box::new(ctor),
        args: vec![KType::Number],
    };
    assert_eq!(app.name(), ":(Wrap Number)");
}
