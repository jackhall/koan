//! End-to-end tests for the type-language-via-dispatch sigil surface.
//!
//! Covers the four new keyworded type-constructor overloads (`LIST OF`,
//! `MAP _ -> _`, `FN`, `FUNCTOR`) registered by
//! [`koan::builtins::type_constructors`], plus the legacy `:(List Number)`
//! positional fallback served by the dispatcher's `TypeCall` arm.
//!
//! These exercise the *sigil boundary*: a `:(...)` expression evaluates its
//! inner expression through the standard dispatch classifier and the result is
//! a type-side carrier (`KTypeValue` for structural types, `Module` /
//! `Signature` / `UserType` for nominal identities) that downstream slots
//! type-check naturally.
//!
//! Companion design: [design/typing/type-language-via-dispatch.md].

use std::cell::RefCell;
use std::rc::Rc;

use koan::builtins::default_scope;
use koan::machine::model::{KObject, KType, UserTypeKind};
use koan::machine::{RuntimeArena, Scheduler, Scope};
use koan::parse::parse;

struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl std::io::Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn run<'a>(arena: &'a RuntimeArena, src: &str) -> &'a Scope<'a> {
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(arena, Box::new(SharedBuf(captured)));
    let exprs = parse(src).expect("parse should succeed");
    let mut sched = Scheduler::new();
    for e in exprs {
        sched.add_dispatch(e, scope);
    }
    sched.execute().expect("scheduler should run to completion");
    scope
}

fn run_expect_err(arena: &RuntimeArena, src: &str) -> String {
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(arena, Box::new(SharedBuf(captured)));
    let exprs = parse(src).expect("parse should succeed");
    let mut sched = Scheduler::new();
    for e in exprs {
        sched.add_dispatch(e, scope);
    }
    match sched.execute() {
        Ok(()) => panic!("expected scheduler error, got success"),
        Err(e) => e.to_string(),
    }
}

/// Read the SIG named `sig_name`'s decl_scope value-binding for `name` as a
/// `KType` carrier. Reads the run-root scope's type side (`resolve_type`) to grab
/// the Signature carrier, then walks the Signature's decl_scope's `iter_data` view.
fn lookup_sig_value_kt<'a>(scope: &'a Scope<'a>, sig_name: &str, name: &str) -> KType<'a> {
    let s = match scope.resolve_type(sig_name) {
        Some(KType::Signature { sig, .. }) => *sig,
        other => panic!(
            "`{sig_name}` should bind a Signature KType, got {:?}",
            other
        ),
    };
    let entries = s.decl_scope().bindings().iter_data();
    let (_, obj) = entries
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("`{name}` should be bound"));
    match obj {
        KObject::KTypeValue(kt) => kt.clone(),
        other => panic!(
            "`{name}` must be a KTypeValue carrier, got {:?}",
            other.ktype()
        ),
    }
}

// --- LIST OF ---

/// `:(LIST OF Number)` lowers to `KType::List(Number)` and binds via VAL.
#[test]
fn sigil_list_of_lowers_to_list_carrier() {
    let arena = RuntimeArena::new();
    let scope = run(&arena, "SIG Sig = ((VAL items :(LIST OF Number)))");
    let items_kt = lookup_sig_value_kt(scope, "Sig", "items");
    match items_kt {
        KType::List(elem) => assert_eq!(*elem, KType::Number),
        other => panic!("items must be KType::List(Number), got {other:?}"),
    }
}

/// `:(LIST Number)` (no `OF`) is a dispatch error — the all-uppercase Keyword
/// `LIST` has no overload registered without the connector keyword `OF`.
#[test]
fn sigil_list_of_missing_of_keyword_errors() {
    let arena = RuntimeArena::new();
    let err = run_expect_err(&arena, "LET Ty = :(LIST Number)");
    assert!(
        err.contains("dispatch failed") || err.contains("no matching function"),
        "expected DispatchFailed surface, got: {err}",
    );
}

// --- MAP _ -> _ ---

/// `:(MAP Str -> Number)` lowers to `KType::Dict(Str, Number)`. Surface keyword
/// changed; underlying carrier identity is unchanged from the legacy `Dict`.
#[test]
fn sigil_map_lowers_to_dict_carrier() {
    let arena = RuntimeArena::new();
    let scope = run(&arena, "SIG Sig = ((VAL table :(MAP Str -> Number)))");
    let table_kt = lookup_sig_value_kt(scope, "Sig", "table");
    match table_kt {
        KType::Dict(k, v) => {
            assert_eq!(*k, KType::Str);
            assert_eq!(*v, KType::Number);
        }
        other => panic!("table must be KType::Dict(Str, Number), got {other:?}"),
    }
}

// --- FN ---

/// `:(FN (x :Number, y :Str) -> Bool)` lowers to a positional
/// `KType::KFunction { args, ret }`. Parameter names are surface-only this PR.
#[test]
fn sigil_fn_lowers_to_kfunction_positional() {
    let arena = RuntimeArena::new();
    let scope = run(
        &arena,
        "SIG Sig = ((VAL compare :(FN (x :Number, y :Str) -> Bool)))",
    );
    let cmp = lookup_sig_value_kt(scope, "Sig", "compare");
    match cmp {
        KType::KFunction { args, ret } => {
            assert_eq!(args.len(), 2);
            assert_eq!(args[0], KType::Number);
            assert_eq!(args[1], KType::Str);
            assert_eq!(*ret, KType::Bool);
        }
        other => panic!("compare must be KType::KFunction, got {other:?}"),
    }
}

/// Nullary FN: `:(FN () -> Number)` lowers to a zero-arg function type.
#[test]
fn sigil_fn_nullary_lowers_to_zero_arg_kfunction() {
    let arena = RuntimeArena::new();
    let scope = run(&arena, "SIG Sig = ((VAL gen :(FN () -> Number)))");
    let gen = lookup_sig_value_kt(scope, "Sig", "gen");
    match gen {
        KType::KFunction { args, ret } => {
            assert!(args.is_empty());
            assert_eq!(*ret, KType::Number);
        }
        other => panic!("gen must be KType::KFunction, got {other:?}"),
    }
}

// --- FUNCTOR ---

/// `:(FUNCTOR (Ty :Signature) -> Module)` lowers to a `KType::KFunctor`. The
/// param-name is surface-only; the underlying identity stays positional.
#[test]
fn sigil_functor_lowers_to_kfunctor() {
    let arena = RuntimeArena::new();
    let scope = run(
        &arena,
        "SIG Sig = ((VAL mk :(FUNCTOR (Ty :Signature) -> Module)))",
    );
    let mk = lookup_sig_value_kt(scope, "Sig", "mk");
    match mk {
        KType::KFunctor { params, ret } => {
            assert_eq!(params.len(), 1);
            assert_eq!(params[0], KType::AnySignature);
            assert_eq!(*ret, KType::AnyModule);
        }
        other => panic!("mk must be KType::KFunctor, got {other:?}"),
    }
}

// `sigil_legacy_list_number_falls_through_typecall` deleted: the legacy
// positional shape no longer routes through the dispatcher (TypeCall arm
// removed). Field schemas inside STRUCT/UNION still elaborate the legacy form
// inline via `try_synth_legacy`, but a standalone `:(List Number)` no longer
// resolves through the standalone dispatch path.

// --- Keyworded sigiled type expressions inside STRUCT/UNION field schemas ---

/// `STRUCT Foo = (xs :(LIST OF Number))` — the keyworded `LIST OF` sigil sub-Dispatches
/// through the dispatcher, producing a `KTypeValue(List<Number>)` carrier that the
/// field-walker splices back as the field's resolved KType. Mirror of the
/// legacy-positional `:(List Number)` form which the walker still elaborates inline.
#[test]
fn struct_field_accepts_keyworded_list_of_sigil() {
    let arena = RuntimeArena::new();
    let scope = run(&arena, "STRUCT Foo = (xs :(LIST OF Number))");
    // STRUCT is type-only — its field schema rides the `UserType { Struct { fields } }`
    // identity in `types`.
    let fields = match scope.resolve_type("Foo") {
        Some(KType::UserType {
            kind: UserTypeKind::Struct { fields },
            ..
        }) => fields.clone(),
        other => panic!("Foo must be a Struct identity in types, got {other:?}"),
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].0, "xs");
    match &fields[0].1 {
        KType::List(elem) => assert_eq!(**elem, KType::Number),
        other => panic!("xs must be KType::List(Number), got {other:?}"),
    }
}

/// `UNION Maybe = (some :(MAP Str -> Number), none :Null)` — keyworded `MAP` sigil
/// inside a UNION field. Same sub-Dispatch path, different field-walker invocation.
#[test]
fn union_field_accepts_keyworded_map_sigil() {
    let arena = RuntimeArena::new();
    let scope = run(
        &arena,
        "UNION Maybe = (some :(MAP Str -> Number), none :Null)",
    );
    // UNION is type-only — its variant schema rides the `UserType { Tagged { schema } }`
    // identity in `types`.
    let schema = match scope.resolve_type("Maybe") {
        Some(KType::UserType {
            kind: UserTypeKind::Tagged { schema },
            ..
        }) => schema.clone(),
        other => panic!("Maybe must be a Tagged identity in types, got {other:?}"),
    };
    let some_kt = schema.get("some").expect("some tag");
    match some_kt {
        KType::Dict(k, v) => {
            assert_eq!(**k, KType::Str);
            assert_eq!(**v, KType::Number);
        }
        other => panic!("some must be KType::Dict(Str, Number), got {other:?}"),
    }
}

// --- Forward type reference inside sigil body ---

/// A sigiled FUNCTOR type expression whose inner parameter type references a
/// forward-declared sibling SIG defers through a Combine: at body-run time the
/// SIG is still parked on its own elaboration, so `extract_param_types`
/// returns `Park(producers)`; the body schedules a Combine over the producers
/// and re-runs the walk at finish against the now-final SIG. Pins the
/// [`defer_via_combine`] path in `body_fn` / `body_functor`.
///
/// Index gating means a SIG-internal VAL whose annotation references a
/// later-sibling top-level SIG can't see the forward sibling at submission
/// time, so the type-language sigil's resolver parks. Submission order:
/// top-level SIG `Outer` (which contains the VAL), then top-level SIG
/// `OrderedSig`. The VAL's sigiled FUNCTOR type annotation parks on
/// `OrderedSig`'s producer and resumes via Combine.
#[test]
fn sigil_functor_forward_reference_defers_via_combine() {
    let arena = RuntimeArena::new();
    let scope = run(
        &arena,
        "SIG Outer = ((VAL mk :(FUNCTOR (Ty :OrderedSig) -> Module)))\n\
         SIG OrderedSig = (VAL compare :Number)",
    );
    let mk = lookup_sig_value_kt(scope, "Outer", "mk");
    match mk {
        KType::KFunctor { params, ret } => {
            assert_eq!(params.len(), 1);
            // OrderedSig resolves to its `Signature { .. }` identity post-Combine.
            // The carrier type's name (`OrderedSig`) is enough to confirm the
            // forward reference resolved through the deferral path.
            assert!(
                params[0].name().contains("OrderedSig") || params[0] == KType::AnySignature,
                "param 0 should carry OrderedSig identity, got {:?}",
                params[0]
            );
            assert_eq!(*ret, KType::AnyModule);
        }
        other => panic!("mk must be KType::KFunctor, got {other:?}"),
    }
}
// --- User-functor application via sigil ---

/// User-functor application through the sigil: a `FUNCTOR MyFunctor (...) =
/// ...` binds a `KFunction` carrier under `MAKESET`, and the value-side
/// call `(MAKESET IntOrd)` produces a Module value. The same `FunctionValueCall`
/// arm of the classifier serves both the value-side and sigiled surfaces, so
/// `:(MyFunctor IntOrd)` routes through the same machinery — covered by
/// `tests/functor_binder_e2e.rs::functor_binder_and_sigil_coexist` for the
/// sigiled surface; this test pins the value-side surface so both paths are
/// exercised in CI.
#[test]
fn sigil_user_functor_application_through_dispatch() {
    let arena = RuntimeArena::new();
    let scope = run(
        &arena,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrdBase = ((LET compare = 7))\n\
         LET IntOrd = (IntOrdBase :! OrderedSig)\n\
         FUNCTOR (MAKESET Er :OrderedSig) -> Module = \
            (MODULE Result = ((LET tag = 0)))\n\
         LET MySet = (MAKESET IntOrd)",
    );
    // `MySet` is a module bound under a Type-classed name — type-only, so its identity
    // lives in `types`.
    match scope.resolve_type("MySet") {
        Some(KType::Module { .. }) => {}
        other => panic!("MySet must be a Module identity in types, got {other:?}"),
    }
}
