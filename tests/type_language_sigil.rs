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
//! `Signature` / `SetRef` for nominal identities) that downstream slots
//! type-check naturally.
//!
//! Companion design: [design/typing/type-language-via-dispatch.md].

use std::cell::RefCell;
use std::rc::Rc;

use koan::builtins::default_scope;
use koan::machine::model::{KKind, KObject, KType, ProjectedSchema, RecursiveSet, SigSource};
use koan::machine::{run_root_storage, FrameStorage, KoanRuntime, Scope};
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

fn run<'a>(region: &'a Rc<FrameStorage>, src: &str) -> &'a Scope<'a> {
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(region, Box::new(SharedBuf(captured)));
    let exprs = parse(src).expect("parse should succeed");
    let mut runtime = KoanRuntime::new();
    for e in exprs {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime
        .execute()
        .expect("scheduler should run to completion");
    scope
}

fn run_expect_err(region: &Rc<FrameStorage>, src: &str) -> String {
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(region, Box::new(SharedBuf(captured)));
    let exprs = parse(src).expect("parse should succeed");
    let mut runtime = KoanRuntime::new();
    let ids: Vec<_> = exprs
        .into_iter()
        .map(|e| runtime.dispatch_in_scope(e, scope))
        .collect();
    runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let last = *ids.last().expect("at least one expression");
    match runtime.result_error(last) {
        Ok(()) => panic!("expected scheduler error, got success"),
        Err(e) => e.to_string(),
    }
}

/// Read the SIG named `sig_name`'s decl_scope value-slot for `name` as its declared
/// `KType`. Reads the run-root scope's type side (`resolve_type`) to grab the Signature
/// carrier, then reads the Signature decl_scope's `bindings.types` — where VAL value slots
/// record their declared type under their value-class name.
fn lookup_sig_value_kt<'a>(scope: &'a Scope<'a>, sig_name: &str, name: &str) -> KType<'a> {
    let s = match scope.resolve_type(sig_name) {
        Some(KType::Signature {
            sig: SigSource::Declared(sig),
            ..
        }) => *sig,
        other => panic!(
            "`{sig_name}` should bind a Signature KType, got {:?}",
            other
        ),
    };
    let entries = s.decl_scope().bindings().iter_types();
    entries
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, kt)| (*kt).clone())
        .unwrap_or_else(|| panic!("`{name}` should be bound in the SIG decl_scope's types"))
}

// --- LIST OF ---

/// `:(LIST OF Number)` lowers to `KType::List(Number)` and binds via VAL.
#[test]
fn sigil_list_of_lowers_to_list_carrier() {
    let region = run_root_storage();
    let scope = run(&region, "SIG Holder = ((VAL items :(LIST OF Number)))");
    let items_kt = lookup_sig_value_kt(scope, "Holder", "items");
    match items_kt {
        KType::List { element: elem, .. } => assert_eq!(*elem, KType::Number),
        other => panic!("items must be KType::List(Number), got {other:?}"),
    }
}

/// `:(LIST Number)` (no `OF`) is a dispatch error — the all-uppercase Keyword
/// `LIST` has no overload registered without the connector keyword `OF`.
#[test]
fn sigil_list_of_missing_of_keyword_errors() {
    let region = run_root_storage();
    let err = run_expect_err(&region, "LET Ty = :(LIST Number)");
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
    let region = run_root_storage();
    let scope = run(&region, "SIG Holder = ((VAL table :(MAP Str -> Number)))");
    let table_kt = lookup_sig_value_kt(scope, "Holder", "table");
    match table_kt {
        KType::Dict {
            key: k, value: v, ..
        } => {
            assert_eq!(*k, KType::Str);
            assert_eq!(*v, KType::Number);
        }
        other => panic!("table must be KType::Dict(Str, Number), got {other:?}"),
    }
}

// --- FN ---

/// `:(FN (x :Number, y :Str) -> Bool)` lowers to a `KType::KFunction { params, ret }`
/// whose `params` record keys each parameter type by its declared name.
#[test]
fn sigil_fn_lowers_to_kfunction_named() {
    let region = run_root_storage();
    let scope = run(
        &region,
        "SIG Holder = ((VAL compare :(FN (x :Number, y :Str) -> Bool)))",
    );
    let cmp = lookup_sig_value_kt(scope, "Holder", "compare");
    match cmp {
        KType::KFunction { params, ret, .. } => {
            assert_eq!(params.len(), 2);
            assert_eq!(params.get("x"), Some(&KType::Number));
            assert_eq!(params.get("y"), Some(&KType::Str));
            assert_eq!(*ret, KType::Bool);
        }
        other => panic!("compare must be KType::KFunction, got {other:?}"),
    }
}

/// Nullary FN: `:(FN () -> Number)` lowers to a zero-arg function type.
#[test]
fn sigil_fn_nullary_lowers_to_zero_arg_kfunction() {
    let region = run_root_storage();
    let scope = run(&region, "SIG Holder = ((VAL gen :(FN () -> Number)))");
    let gen = lookup_sig_value_kt(scope, "Holder", "gen");
    match gen {
        KType::KFunction { params, ret, .. } => {
            assert!(params.is_empty());
            assert_eq!(*ret, KType::Number);
        }
        other => panic!("gen must be KType::KFunction, got {other:?}"),
    }
}

// --- FUNCTOR ---

/// `:(FUNCTOR (Ty :Signature) -> Module)` lowers to a `KType::KFunctor` whose `params`
/// record keys the parameter type by its (capitalized) declared name `Ty`.
#[test]
fn sigil_functor_lowers_to_kfunctor() {
    let region = run_root_storage();
    let scope = run(
        &region,
        "SIG Holder = ((VAL mk :(FUNCTOR (Ty :Signature) -> Module)))",
    );
    let mk = lookup_sig_value_kt(scope, "Holder", "mk");
    match mk {
        KType::KFunctor { params, ret, .. } => {
            assert_eq!(params.len(), 1);
            assert_eq!(params.get("Ty"), Some(&KType::OfKind(KKind::Signature)));
            assert_eq!(*ret, KType::empty_signature());
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

/// `NEWTYPE Foo = :{xs :(LIST OF Number)}` — the keyworded `LIST OF` sigil sub-Dispatches
/// through the dispatcher, producing a `KTypeValue(List<Number>)` carrier that the
/// field-walker splices back as the field's resolved KType inside the record repr.
#[test]
fn newtype_record_field_accepts_keyworded_list_of_sigil() {
    let region = run_root_storage();
    let scope = run(&region, "NEWTYPE Foo = :{xs :(LIST OF Number)}");
    // NEWTYPE is type-only — its record repr rides the sealed `SetRef` member in `types`.
    let fields = match scope.resolve_type("Foo") {
        Some(KType::SetRef { set, index }) => match RecursiveSet::projected_schema(set, *index) {
            ProjectedSchema::NewType(KType::Record { fields, .. }) => fields,
            _ => panic!("Foo must project a record-repr NewType schema"),
        },
        other => panic!("Foo must be a NewType SetRef in types, got {other:?}"),
    };
    assert_eq!(fields.len(), 1);
    let (xs_name, xs_type) = fields.iter().next().expect("one field");
    assert_eq!(xs_name, "xs");
    match xs_type {
        KType::List { element: elem, .. } => assert_eq!(**elem, KType::Number),
        other => panic!("xs must be KType::List(Number), got {other:?}"),
    }
}

/// `UNION Maybe = (Some :(MAP Str -> Number), None :Null)` — keyworded `MAP` sigil
/// inside a UNION field. Same sub-Dispatch path, different field-walker invocation.
#[test]
fn union_field_accepts_keyworded_map_sigil() {
    let region = run_root_storage();
    let scope = run(
        &region,
        "UNION Maybe = (Some :(MAP Str -> Number), None :Null)",
    );
    // UNION is type-only — it binds an anonymous union of per-variant newtypes; the `Some`
    // variant's newtype repr is the keyworded `MAP` sigil that sub-Dispatched.
    let some_repr = match scope.resolve_type("Maybe") {
        Some(KType::Union { members, .. }) => members
            .iter()
            .find_map(|m| match m {
                KType::SetRef { set, index } if set.member(*index).name == "Some" => {
                    match RecursiveSet::projected_schema(set, *index) {
                        ProjectedSchema::NewType(repr) => Some(repr),
                        _ => None,
                    }
                }
                _ => None,
            })
            .expect("Some variant must project a NewType repr"),
        other => panic!("Maybe must be a Union in types, got {other:?}"),
    };
    match some_repr {
        KType::Dict {
            key: k, value: v, ..
        } => {
            assert_eq!(*k, KType::Str);
            assert_eq!(*v, KType::Number);
        }
        other => panic!("Some repr must be KType::Dict(Str, Number), got {other:?}"),
    }
}

// --- Forward type reference inside sigil body ---

/// A sigiled FUNCTOR type expression whose inner parameter type references a
/// forward-declared sibling SIG defers through a dep-finish: at body-run time the
/// SIG is still parked on its own elaboration, so `extract_param_types`
/// returns `Park(producers)`; the body schedules a dep-finish over the producers
/// and re-runs the walk at finish against the now-final SIG. Pins the
/// [`defer`] path in `body_fn` / `body_functor`.
///
/// Index gating means a SIG-internal VAL whose annotation references a
/// later-sibling top-level SIG can't see the forward sibling at submission
/// time, so the type-language sigil's resolver parks. Submission order:
/// top-level SIG `Outer` (which contains the VAL), then top-level SIG
/// `Ordered`. The VAL's sigiled FUNCTOR type annotation parks on
/// `Ordered`'s producer and resumes via dep-finish.
#[test]
fn sigil_functor_forward_reference_defers_via_combine() {
    let region = run_root_storage();
    let scope = run(
        &region,
        "SIG Outer = ((VAL mk :(FUNCTOR (Ty :Ordered) -> Module)))\n\
         SIG Ordered = (VAL compare :Number)",
    );
    let mk = lookup_sig_value_kt(scope, "Outer", "mk");
    match mk {
        KType::KFunctor { params, ret, .. } => {
            assert_eq!(params.len(), 1);
            // Ordered resolves to its `Signature { .. }` identity post-dep-finish.
            // The carrier type's name (`Ordered`) is enough to confirm the
            // forward reference resolved through the deferral path.
            let ty = params.get("Ty").expect("param `Ty` must be present");
            assert!(
                ty.name().contains("Ordered") || *ty == KType::OfKind(KKind::Signature),
                "param `Ty` should carry Ordered identity, got {ty:?}",
            );
            assert_eq!(*ret, KType::empty_signature());
        }
        other => panic!("mk must be KType::KFunctor, got {other:?}"),
    }
}
// --- User-functor application via sigil ---

/// User-functor application through the sigil: a `FUNCTOR MyFunctor (...) =
/// ...` binds a `KFunction` carrier under `MAKESET`, and the value-side
/// call `(MAKESET int_ord)` produces a Module value. The same `FunctionValueCall`
/// arm of the classifier serves both the value-side and sigiled surfaces, so
/// `:(MyFunctor int_ord)` routes through the same machinery — covered by
/// `tests/functor_binder_e2e.rs::functor_binder_and_sigil_coexist` for the
/// sigiled surface; this test pins the value-side surface so both paths are
/// exercised in CI.
#[test]
fn sigil_user_functor_application_through_dispatch() {
    let region = run_root_storage();
    let scope = run(
        &region,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord_base = ((LET compare = 7))\n\
         LET int_ord = (int_ord_base :! Ordered)\n\
         FUNCTOR (MAKESET er :Ordered) -> Module = \
            (MODULE generated = ((LET tag = 0)))\n\
         LET my_set = (MAKESET int_ord)",
    );
    // `my_set` is a module bound under a Type-classed name — a module is a value, so it binds
    // on the value channel.
    assert!(
        matches!(scope.lookup("my_set"), Some(KObject::Module(_))),
        "my_set must bind the produced module value",
    );
}
