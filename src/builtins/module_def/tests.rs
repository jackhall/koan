//! `MODULE` declaration: the module value it binds and member access through it. The
//! type-declaration announcement its body performs is pinned in [`announcement`].

mod announcement;

use crate::builtins::test_support::{TestRun, lookup_module, parse_one};
use crate::machine::model::KObject;
use crate::machine::model::SigSchema;
use crate::machine::model::{Module, ModuleDraft};
use crate::machine::program_storage;
use crate::machine::run_root_storage;
use crate::machine::{BindingIndex, KErrorKind};

/// The binder name comes off the `Identifier` name part — a module binds value-side, so the
/// submit-time placeholder is tagged `Value`.
#[test]
fn binder_name_extracts_module_name() {
    let program = program_storage();
    let expr = parse_one(&program, "MODULE foo = (LET x = 1)");
    let name = crate::machine::model::binder::identifier_part_binder_name(&expr);
    assert_eq!(name, Some("foo"));
}

/// A Type-token module name is refused by the second overload, whose only job is the
/// respelling diagnostic — a module is a value, and the Type-token namespace names what can
/// type a field.
#[test]
fn type_token_module_name_errors_with_the_snake_case_respelling() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let err = test_run.run_one_err(parse_one(&program, "MODULE IntOrd = (LET x = 1)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("a module is a value") && msg.contains("`int_ord`")),
        "expected the snake_case respelling diagnostic, got {err}",
    );
    assert!(
        scope.bindings().data().get("IntOrd").is_none(),
        "the erroring overload binds nothing",
    );
}

/// A MODULE-body manifest member named `Type` collides with the builtin `Type`
/// meta-type. Builtins are immutable and unshadowable in either channel
/// ([`crate::machine::core::scope`] `shadows_builtin_type`), so `LET Type = Number`
/// raises `Rebind` naming `Type` rather than declaring the member. Modules and
/// signatures name their principal abstract type member `Carrier`
/// (see [design/typing/modules.md](../../../design/typing/modules.md)); this pins the
/// collision so the docs and the implementation cannot silently disagree.
#[test]
fn module_member_named_type_collides_with_builtin_type() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let err = test_run.run_one_err(parse_one(
        &program,
        "MODULE int_ord = ((LET Type = Number) (LET zero = 0))",
    ));
    assert!(
        matches!(&err.kind, KErrorKind::Rebind { name } if name == "Type"),
        "a MODULE member named `Type` must be a Rebind naming `Type`, got {err}",
    );
    assert!(
        scope.bindings().data().get("int_ord").is_none(),
        "the colliding module binds nothing",
    );
}

#[test]
fn module_binds_under_name_in_scope() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("MODULE foo = (LET x = 1)");
    assert!(
        matches!(scope.lookup("foo"), Some(KObject::Module(m)) if m.path == "foo"),
        "MODULE binds the module value on the value channel",
    );
    assert!(
        scope.resolve_type("foo").is_none(),
        "a module is a value — nothing lands in `types`",
    );
}

#[test]
fn bare_module_name_surfaces_as_object_value() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE foo = (LET x = 1)");
    // A module named in expression position reads back on the value channel's Object arm.
    let bare = test_run.run_one(parse_one(&program, "foo"));
    match bare {
        KObject::Module(module) => assert_eq!(module.path, "foo"),
        other => panic!(
            "bare module name must read back as an Object-arm module value, got {}",
            other.ktype().name(&test_run.types)
        ),
    }
    // PRINT returns the rendered string — a bare module renders as its path.
    let printed = test_run.run_one(parse_one(&program, "PRINT foo"));
    match printed {
        KObject::KString(s) => assert_eq!(*s, "foo"),
        other => panic!(
            "PRINT foo returns the path string, got {}",
            other.ktype().name(&test_run.types)
        ),
    }
}

/// A bare module name in list-element position name-resolves like any other bound
/// identifier, so the list holds the module values and memoizes their self-sig element type
/// — the same result the parenthesized `[(m)]` form produces.
#[test]
fn bare_module_names_in_list_resolve_and_memoize_self_sig() {
    use crate::machine::model::Held;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE int_ord = (LET compare = 7)");
    let listed = test_run.run_one(parse_one(&program, "[int_ord, int_ord]"));
    match listed {
        KObject::List(items, elem) => {
            // Ruling 12: a module's self-sig renders structurally, not by the module name.
            assert_eq!(
                elem.name(&test_run.types),
                ":(LIST OF SIG (compare: Number))",
                "the memoized element type is the module self-sig"
            );
            assert_eq!(items.elements().len(), 2);
            assert!(
                items
                    .elements()
                    .iter()
                    .all(|i| matches!(i, Held::Object(KObject::Module(m)) if m.path == "int_ord")),
                "each element is the Object-arm module value",
            );
        }
        other => panic!(
            "expected a list, got {}",
            other.ktype().name(&test_run.types)
        ),
    }
}

#[test]
fn module_in_list_surfaces_as_object_element_memoized_to_self_sig() {
    use crate::machine::model::Held;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    // A parenthesized module expression evaluates to the Object-arm module value, so the list
    // element is `Held::Object` memoized as the module's self-sig, which (ruling 12) renders
    // structurally as `SIG (compare: Number)` rather than by the module name.
    let listed = test_run.run_one(parse_one(&program, "[(int_ord)]"));
    match listed {
        KObject::List(items, elem) => {
            assert_eq!(
                elem.name(&test_run.types),
                ":(LIST OF SIG (compare: Number))",
                "element memoizes to the module self-sig"
            );
            assert_eq!(items.elements().len(), 1);
            assert!(
                matches!(&items.elements()[0], Held::Object(KObject::Module(m)) if m.path == "int_ord"),
                "the list element is the Object-arm module value",
            );
        }
        other => panic!(
            "expected a list, got {}",
            other.ktype().name(&test_run.types)
        ),
    }
}

#[test]
fn module_member_access_via_attr() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE foo = (LET x = 1)");
    let result = test_run.run_one(parse_one(&program, "foo.x"));
    assert!(matches!(result, KObject::Number(n) if *n == 1.0));
}

#[test]
fn module_with_multiple_statements_in_parens() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE foo = ((LET x = 1) (LET y = 2))");
    assert!(
        matches!(test_run.run_one(parse_one(&program, "foo.x")), KObject::Number(n) if *n == 1.0)
    );
    assert!(
        matches!(test_run.run_one(parse_one(&program, "foo.y")), KObject::Number(n) if *n == 2.0)
    );
}

#[test]
fn module_member_function_via_let_fn() {
    // `LET <name> = (FN ...)` binds under a clean identifier; bare FN lands under
    // its signature key and isn't reachable as `foo.<name>` via ATTR.
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("MODULE foo = (LET double = FN (DOUBLE x :Number) -> Number = (x))");
    let foo = lookup_module(scope, "foo", &test_run.types);
    assert!(foo.child_scope().bindings().data().contains_key("double"));
}

#[test]
fn module_unknown_member_errors() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE foo = (LET x = 1)");
    let err = test_run.run_one_err(parse_one(&program, "foo.bogus"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("foo") && msg.contains("`bogus`")),
        "expected ShapeError naming foo and bogus, got {err}",
    );
}

#[test]
fn nested_module_accessible_via_chained_attr() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE outer =\n  MODULE inner = (LET x = 7)");
    let result = test_run.run_one(parse_one(&program, "outer.inner.x"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

/// MODULE body parks on an outer-scheduler placeholder for a sibling forward
/// reference instead of erroring as `UnboundName`.
#[test]
fn module_body_parks_on_outer_placeholder() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("LET y = 7\nMODULE foo = (LET x = y)");
    let result = test_run.run_one(parse_one(&program, "foo.x"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

/// A failing body statement must not bind `foo` in the parent scope.
#[test]
fn module_body_error_short_circuits_finalize() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("MODULE foo = (LET x = nonexistent_name)");
    assert!(
        scope.bindings().data().get("foo").is_none(),
        "foo must not bind when its body errors",
    );
}

/// Pre-seed the `foo` module value through the value-side door, then re-dispatch
/// `MODULE foo = ...`. The finalize guard reads `data`, short-circuits on the existing
/// binding, and leaves the pre-seeded `&Module` pointer intact.
#[test]
fn module_finalize_short_circuits_on_idempotent_state() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let child = scope.alloc_child_under_module("foo", None);
    // Every mint carries its self-sig (2d eager-seal invariant), so a manually pre-seeded
    // module derives its interface from the same (empty) draft production would, before the
    // value exists.
    let draft = ModuleDraft::empty();
    let self_sig = test_run
        .types
        .signature(SigSchema::raw_self_sig(child, &draft));
    let module: &Module<'_> = Module::alloc_at_child_scope("foo", child, draft, self_sig);
    let sealed = scope.seal_module(module);
    scope
        .bind_value_direct(
            "foo".into(),
            sealed,
            BindingIndex::value(0),
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("pre-seed the module value binding");
    test_run.run("MODULE foo = (LET y = 2)");
    let foo = lookup_module(scope, "foo", &test_run.types);
    assert!(std::ptr::eq(foo, module));
}

/// Miri audit-slate: exercises the MODULE dep-finish continuation's captured
/// `child_scope: &'a Scope<'a>` and finalize writes under tree borrows.
#[test]
fn module_body_dispatch_does_not_dangle() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("LET y = 7\nMODULE foo = ((LET x = y) (LET z = 11))");
    let foo = lookup_module(scope, "foo", &test_run.types);
    let inner = foo.child_scope();
    assert!(matches!(inner.lookup("x"), Some(KObject::Number(n)) if *n == 7.0));
    assert!(matches!(inner.lookup("z"), Some(KObject::Number(n)) if *n == 11.0));
}
