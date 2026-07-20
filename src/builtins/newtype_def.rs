//! `NEWTYPE <name> = <repr>` — declare a fresh nominal identity over a transparent
//! representation. The declaration writes only `bindings.types` (no value-side
//! schema carrier). Construction produces a [`KObject::Wrapped`] tagging the inner
//! value with the NEWTYPE identity; the `Wrapped.inner` is invariantly non-`Wrapped`
//! (newtype-over-newtype collapses to a single layer).
//!
//! Three registered overloads selected by the repr part-kind. A scalar / bare-leaf repr
//! (`= Number`, `= Foo`) resolves eagerly through the `:ProperType` slot. A non-record
//! sigil repr (`= :(LIST OF T)`) is captured *raw* through the `:SigiledTypeExpr` slot and
//! sub-dispatched to a resolved `KType` by the shared [`body`]. A record repr (`= :{…}`) is
//! captured *raw* through its own `:RecordType` slot and routed to [`body_record_repr`], so
//! the declarator owns its field-list elaboration and threads the binder name
//! ([`Elaborator::with_threaded`]): a self-reference (`:{next :Node}`) lowers to a
//! `RecursiveRef` and seals to a `SetLocal` back-edge — the same shared seal path
//! ([`finalize_nominal_member`], [`seal_recursive_refs`]) `UNION` uses, and the path a
//! `RECURSIVE TYPES` block routes its `NEWTYPE` members through.

use crate::machine::model::KKind;
use crate::machine::model::TypeRegistry;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::machine::model::KObject;
use crate::machine::model::KType;
use crate::machine::model::{
    finalize_nominal_member, seal_recursive_refs, FieldListContext, FieldNameKind, NominalSchema,
    Record, RecursiveSet, SchemaSealResult, SealOutcome,
};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::FinishCtx;
use crate::machine::{seal_type_identity, StepCarried};
use crate::machine::{BindingIndex, KError, KErrorKind, Scope, TraceFrame};
use crate::source::Spanned;

use super::{arg, kw, sig};

/// Seal a resolved `repr` into the NEWTYPE's identity and register it. A NEWTYPE is
/// non-recursive (its `repr` is already resolved), so it seals into a singleton set of one
/// member whose `kind` (`NewType`) is what `kind_of` reports for the sealed `SetRef`;
/// identity never descends `repr`.
///
/// The identity is owned data allocated into this scope's own region, so it seals as a resident
/// type carrier regardless of where `repr` was resolved from.
fn finalize_newtype<'a>(
    fctx: &FinishCtx<'a, '_>,
    name: String,
    repr: KType,
    bind_index: BindingIndex,
) -> Result<StepCarried<'a>, KError> {
    // The repr types the values the NEWTYPE wraps, so it must be a proper type; a bare
    // constructor of kind `* -> *` standing unapplied is a kind error.
    if let Some(message) = crate::machine::model::unsaturated_constructor_message(
        &repr,
        &format!("the representation type of NEWTYPE `{name}`"),
        fctx.types,
    ) {
        return Err(KError::new(KErrorKind::ShapeError(message)));
    }
    let scope = fctx.scope;
    let set = RecursiveSet::singleton(name.clone(), NominalSchema::NewType(Box::new(repr)));
    let identity = KType::SetRef { set, index: 0 };
    // Fused alloc + register: the identity is allocated into this scope's own region through the
    // single storage door and registered — one call returns the resident `&KType`.
    let kt_ref = scope.register_type_delivered(name, identity, bind_index)?;
    Ok(seal_type_identity(fctx.scope, kt_ref))
}

/// Seal the elaborated record fields into the NEWTYPE's [`RecursiveSet`] member as
/// `NominalSchema::NewType(KType::record(sealed))`. Transient `RecursiveRef(name)` field leaves
/// seal to `SetLocal(index)` against the member's set — the block's shared set when present (a
/// `RECURSIVE TYPES` member), else a fresh singleton (standalone self-recursion). Shared by the
/// synchronous and dep-finish paths.
fn finalize_record_newtype<'a>(
    fctx: &FinishCtx<'a, '_>,
    name: String,
    fields: Vec<(String, KType)>,
    bind_index: BindingIndex,
) -> Result<StepCarried<'a>, KError> {
    if fields.is_empty() {
        return Err(KError::new(KErrorKind::ShapeError(
            "NEWTYPE record repr must have at least one field".to_string(),
        )));
    }
    let scope = fctx.scope;
    let outcome = finalize_nominal_member(
        scope,
        &name,
        KKind::NewType,
        |set| {
            let missing = RefCell::new(Vec::new());
            let sealed_pairs: Vec<(String, KType)> = fields
                .into_iter()
                .map(|(field, kt)| (field, seal_recursive_refs(set, &kt, &missing, fctx.types)))
                .collect();
            let sealed = Record::from_pairs(sealed_pairs);
            match missing.into_inner().into_iter().next() {
                Some(m) => SchemaSealResult::Dangling(m),
                None => SchemaSealResult::Ok(NominalSchema::NewType(Box::new(KType::record(
                    Box::new(sealed),
                )))),
            }
        },
        bind_index,
    );
    match outcome {
        SealOutcome::Sealed(kt_ref) => Ok(seal_type_identity(scope, kt_ref)),
        SealOutcome::DanglingRef(missing) => Err(KError::new(KErrorKind::ShapeError(format!(
            "NEWTYPE `{name}` record repr references unsealed type `{missing}`",
        )))),
        SealOutcome::Rebind(e) => Err(e),
    }
}

/// A resolved repr finalizes synchronously; a bare-leaf name resolves against the scope chain,
/// parks on an in-flight producer (a `DepRequest::Existing` dep-finish), or errors; a raw sigil repr
/// sub-dispatches via [`defer_resolved_sigil`].
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::builtins::resolve_or_await::{classify_name_lookup, resolve_or_await};
    use crate::machine::{arg_object, arg_type, require_bare_type_name, Action};

    let name = crate::try_action!(require_bare_type_name(
        ctx.args, "name", "NEWTYPE", ctx.types
    ));
    let chain = ctx.chain.clone();
    let bind_index = ctx.bind_index();
    if let Some(te) = crate::machine::arg_unresolved_type(ctx.args, "repr") {
        let te = te.clone();
        resolve_or_await(
            ctx.scope,
            "NEWTYPE repr slot",
            move |scope| {
                classify_name_lookup(
                    scope.resolve_type_with_chain(te.as_str(), chain.as_deref()),
                    te.as_str(),
                )
            },
            // A bare-leaf name resolved against scope bindings, not a dep terminal.
            move |fctx, kt| Action::Done(finalize_newtype(fctx, name, kt, bind_index)),
            ctx.types,
        )
    } else if let Some(repr_kt) = arg_type(ctx.args, "repr") {
        Action::Done(finalize_newtype(
            &ctx.finish_ctx(),
            name,
            repr_kt.clone(),
            bind_index,
        ))
    } else if let Some(KObject::KExpression(inner)) = arg_object(ctx.args, "repr") {
        defer_resolved_sigil(name, inner.clone(), bind_index)
    } else {
        Action::Done(Err(KError::new(KErrorKind::ShapeError(
            "NEWTYPE repr slot must be a type expression (e.g. `Number`, `Foo`)".to_string(),
        ))))
    }
}

/// A non-record sigil repr (`NEWTYPE Stream = :(LIST OF Number)`): re-wrap the captured sigil,
/// sub-dispatch it, and seal a plain NewType over the resolved `KType` at dep-finish.
fn defer_resolved_sigil<'a>(
    name: String,
    inner: KExpression<'a>,
    bind_index: BindingIndex,
) -> crate::machine::Action<'a> {
    use crate::builtins::resolve_or_await::dispatch_type_then;
    use crate::machine::Action;
    let wrapped = KExpression::new(vec![Spanned::bare(ExpressionPart::SigiledTypeExpr(
        Box::new(inner),
    ))]);
    dispatch_type_then(wrapped, "NEWTYPE repr slot", move |fctx, kt| {
        Action::Done(finalize_newtype(fctx, name, kt, bind_index))
    })
}

/// Body of the record-repr overload `NEWTYPE <name> = :{…}`: elaborate the `:{…}` field list
/// (threading the binder name + pending guard), folding via [`finalize_record_newtype`] or deferring
/// through the shared `nominal_schema_action` field-list path.
pub fn body_record_repr<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use super::nominal_schema::nominal_schema_action;
    use crate::machine::{arg_object, require_bare_type_name, Action};

    let name = crate::try_action!(require_bare_type_name(
        ctx.args, "name", "NEWTYPE", ctx.types
    ));
    let fields = match arg_object(ctx.args, "repr") {
        Some(KObject::KExpression(e)) => e.clone(),
        _ => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(
                "NEWTYPE record repr slot must be a record type `:{…}`".to_string(),
            ))))
        }
    };
    let error_frame = TraceFrame::bare("<newtype>", format!("NEWTYPE {name}"));
    nominal_schema_action(
        ctx,
        name,
        fields,
        FieldListContext::NEWTYPE_RECORD_REPR,
        FieldNameKind::Identifier,
        error_frame,
        finalize_record_newtype,
    )
}

/// Mint a type-constructor family: a singleton [`RecursiveSet`] of one
/// [`KKind::TypeConstructor`] member, filled with an empty variant schema (identity ignores it)
/// and the declared `param_names`.
pub(crate) fn mint_type_constructor(member_name: String, param_names: Vec<String>) -> KType {
    let set = RecursiveSet::singleton(
        member_name,
        NominalSchema::TypeConstructor {
            schema: HashMap::new(),
            param_names,
        },
    );
    KType::SetRef { set, index: 0 }
}

/// `NEWTYPE (<Param>… AS <Name>)` — declare a type-constructor family (declaration-by-example),
/// mirroring the application surface with the concrete arguments replaced by the parameter names.
/// Mints a [`KKind::TypeConstructor`] `SetRef` under `Name` in the declaring scope, so it
/// satisfies a higher-kinded slot declared over the same parameter names and applies through
/// `AS`. Reuses the shared `TYPE` declaration parser. Valid in any scope (top level, MODULE body)
/// — no SIG-body gate.
pub fn body_constructor_family<'a>(
    ctx: &crate::machine::BodyCtx<'a, '_>,
) -> crate::machine::Action<'a> {
    use crate::machine::{require_kexpression, Action};

    let decl = match require_kexpression(ctx.args, "NEWTYPE", "decl") {
        Ok(decl) => decl,
        Err(e) => return Action::Done(Err(e)),
    };
    let (param_names, member_name) = match crate::builtins::type_decl::parse_hk_decl(&decl) {
        Ok(pair) => pair,
        Err(e) => return Action::Done(Err(e)),
    };
    let kt = mint_type_constructor(member_name.clone(), param_names);
    // Bind through the fused alloc + register path, mirroring `type_decl::bind_abstract_member`.
    let bind_index = ctx.bind_index();
    let kt_ref = match ctx
        .scope
        .register_user_type_delivered(member_name, kt, bind_index)
    {
        Ok(kt_ref) => kt_ref,
        Err(e) => return Action::Done(Err(e)),
    };
    let carrier = ctx.scope.resident_type_carrier(kt_ref);
    Action::Done(Ok(StepCarried::born(carrier)))
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    // Three overloads, selected by the repr part-kind. Construction lives in the `TypeCall`
    // fast lane via `constructors::dispatch_construct_newtype`.
    let scalar_sig = || {
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                kw("NEWTYPE"),
                arg("name", KType::OfKind(KKind::ProperType)),
                kw("="),
                arg("repr", KType::OfKind(KKind::ProperType)),
            ],
        )
    };
    let sigil_sig = || {
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                kw("NEWTYPE"),
                arg("name", KType::OfKind(KKind::ProperType)),
                kw("="),
                arg("repr", KType::SigiledTypeExpr),
            ],
        )
    };
    let record_sig = || {
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                kw("NEWTYPE"),
                arg("name", KType::OfKind(KKind::ProperType)),
                kw("="),
                arg("repr", KType::RecordType),
            ],
        )
    };
    use crate::builtins::register_builtin_full;
    let binder: crate::machine::BinderNameFn = super::type_part_binder_name;
    let binder_kind = crate::machine::BindKind::Type;
    // Scalar / bare-leaf repr (`= Number`, `= Foo`) and non-record sigil repr (`= :(LIST OF T)`)
    // share `body`; the record repr (`= :{…}`) routes to `body_record_repr`.
    register_builtin_full(
        scope,
        "NEWTYPE",
        scalar_sig(),
        body,
        Some((binder, binder_kind)),
        None,
        types,
    );
    register_builtin_full(
        scope,
        "NEWTYPE",
        sigil_sig(),
        body,
        Some((binder, binder_kind)),
        None,
        types,
    );
    register_builtin_full(
        scope,
        "NEWTYPE",
        record_sig(),
        body_record_repr,
        Some((binder, binder_kind)),
        None,
        types,
    );
    // Constructor-family declarator `NEWTYPE (Type AS Wrapper)`. Its keyword set is `{NEWTYPE}`
    // (no `=`), so it lands in its own dispatch bucket, disjoint from the three `{NEWTYPE, =}`
    // overloads above. The `KExpression` slot captures `(Type AS Wrapper)` raw — the inner `AS`
    // is not sub-dispatched (same as TYPE's higher-kinded overload).
    let constructor_family_sig = sig(
        KType::OfKind(KKind::AnyType),
        vec![kw("NEWTYPE"), arg("decl", KType::KExpression)],
    );
    register_builtin_full(
        scope,
        "NEWTYPE",
        constructor_family_sig,
        body_constructor_family,
        Some((
            crate::builtins::type_decl::binder_name,
            crate::machine::BindKind::Type,
        )),
        None,
        types,
    );
}

#[cfg(test)]
mod tests {

    use crate::builtins::test_support::{binds_module, parse_one, TestRun};
    use crate::machine::model::{KKind, NominalSchema, ProjectedSchema, RecursiveSet};
    use crate::machine::model::{KObject, KType, Record};
    use crate::machine::run_root_storage;
    use crate::machine::{KErrorKind, Scope};
    use std::rc::Rc;

    /// `(set, record-fields)` of a sealed record-repr newtype, read raw off its `SetRef`
    /// identity so assertions see `SetLocal` / `List(SetLocal)` back-edges before projection.
    fn record_fields<'a>(
        scope: &'a Scope<'a>,
        name: &str,
    ) -> (Rc<RecursiveSet>, Vec<(String, KType)>) {
        match scope.resolve_type(name) {
            Some(KType::SetRef { set, index }) => {
                let member = set.member(*index);
                let borrow = member.schema();
                match borrow.as_ref() {
                    Some(NominalSchema::NewType(repr)) => match repr.as_ref() {
                        KType::Record { fields: record, .. } => {
                            let fields =
                                record.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                            (Rc::clone(set), fields)
                        }
                        other => panic!("expected {name} to carry a record repr, got {other:?}"),
                    },
                    other => panic!("expected {name} to carry a NewType schema, got {other:?}"),
                }
            }
            other => panic!("expected {name} to be a SetRef identity, got {other:?}"),
        }
    }

    /// NEWTYPE writes the `SetRef` identity into `bindings.types` and nothing into
    /// `bindings.data` — the declaration has no payload value to bind.
    #[test]
    fn declare_mints_newtype_identity() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("NEWTYPE Distance = Number");
        let types = scope.bindings().types();
        let (kt, _) = types
            .get("Distance")
            .expect("Distance should be in bindings.types");
        match **kt {
            KType::SetRef { ref set, index } => {
                assert_eq!(set.member(index).name, "Distance");
                assert_eq!(set.member(index).kind, KKind::NewType);
                match RecursiveSet::projected_schema(set, index, &test_run.types) {
                    ProjectedSchema::NewType(repr) => assert_eq!(repr, KType::Number),
                    _ => panic!("expected a NewType schema"),
                }
            }
            ref other => panic!("expected NewType SetRef identity, got {other:?}"),
        }
        drop(types);
        let data = scope.bindings().data();
        assert!(
            data.get("Distance").is_none(),
            "NEWTYPE must not write a value-side carrier",
        );
    }

    /// `Distance(3.0)` returns a `Wrapped` whose `ktype()` is `Distance` and whose
    /// `inner` is the bare `Number`.
    #[test]
    fn construct_wraps_repr_matching_value() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Distance = Number");
        let result = test_run.run_one(parse_one("Distance (3.0)"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::SetRef { ref set, index } => {
                        assert_eq!(set.member(index).name, "Distance");
                        assert_eq!(set.member(index).kind, KKind::NewType);
                    }
                    ref other => panic!("expected NewType SetRef type_id, got {other:?}"),
                }
                assert!(matches!(inner.get(), KObject::Number(n) if *n == 3.0));
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// `Distance("hi")` (Number repr, Str value) surfaces as `TypeMismatch`.
    #[test]
    fn construct_rejects_non_matching_repr() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Distance = Number");
        let err = test_run.run_one_err(parse_one("Distance (\"hi\")"));
        assert!(
            matches!(&err.kind, KErrorKind::TypeMismatch { expected, got, .. }
                if expected == "Number" && got == "Str"),
            "expected TypeMismatch(Number, Str), got {err}",
        );
    }

    /// A record-repr NEWTYPE and a NEWTYPE depending on it, declared in the *same*
    /// scheduler, then constructed. The dependency's `:{…}` defers its finalize behind a
    /// sub-dispatch, so the dependent's body would run first; it must park on the
    /// dependency's producer rather than error on an unresolved repr (which previously
    /// leaked a stale value-side placeholder that panicked the next construction).
    #[test]
    fn dependent_newtype_parks_on_record_repr_dependency() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("NEWTYPE Point = :{x :Number, y :Number}\nNEWTYPE Boxed = Point");
        // No placeholder may survive the declaration run: a leaked one corrupts the next
        // scheduler on this REPL-persistent scope.
        assert!(
            scope.bindings().placeholders().is_empty(),
            "NEWTYPE declarations must leave no value-side placeholder, got {:?}",
            *scope.bindings().placeholders(),
        );
        let result = test_run.run_one(parse_one("(Boxed (Point {x = 1, y = 2}))"));
        assert!(
            matches!(result, KObject::Wrapped { .. }),
            "expected Wrapped, got {:?}",
            result.ktype(),
        );
    }

    /// A NEWTYPE whose repr names a genuinely unknown type errors — and clears the
    /// value-side placeholder its dispatch installed, so a later construction of the same
    /// name fails cleanly (unbound) rather than tripping over a leaked producer `NodeId`.
    #[test]
    fn unknown_repr_errors_without_leaking_placeholder() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("NEWTYPE Boxed = Nope");
        assert!(
            scope.bindings().placeholders().is_empty(),
            "a failed NEWTYPE must not leak its placeholder, got {:?}",
            *scope.bindings().placeholders(),
        );
        let err = test_run.run_one_err(parse_one("(Boxed (3.0))"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(n) if n == "Boxed"),
            "expected UnboundName(Boxed) after failed declaration, got {err}",
        );
    }

    /// Two record-repr `NEWTYPE`s of one name in one scope are two declarations, not one: the
    /// second statement's own `BindingIndex` differs from the index stored beside the installed
    /// identity, so the seal mints a fresh singleton and the install raises `Rebind`.
    /// `enter_block` is what gives the statements their distinct lexical indices.
    #[test]
    fn same_scope_record_repr_redeclare_rebinds() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        let exprs = crate::parse::parse("NEWTYPE Foo = :{x :Number}\nNEWTYPE Foo = :{x :Str}")
            .expect("parse should succeed");
        let ids = test_run.runtime.enter_block(scope.id, exprs, scope);
        test_run
            .runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        assert!(
            test_run.runtime.result_error(ids[0]).is_ok(),
            "the first declaration should succeed, got {:?}",
            test_run.runtime.result_error(ids[0]).err(),
        );
        let err = test_run
            .runtime
            .result_error(ids[1])
            .expect_err("redeclaring Foo in the same scope should error");
        assert!(
            matches!(&err.kind, KErrorKind::Rebind { name } if name == "Foo"),
            "expected Rebind naming Foo, got {err}",
        );
    }

    /// A self-recursive record repr seals its self-reference to a `SetLocal` back-edge into the
    /// declaring member's singleton set — the binder name is threaded through the field-list
    /// elaboration. (`next :Node` has no base case, so the type is uninhabited by a finite value;
    /// this pins the seal shape, not construction.)
    #[test]
    fn record_repr_self_recursion_seals_set_local() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("NEWTYPE Node = :{value :Number, next :Node}");
        let (set, fields) = record_fields(scope, "Node");
        let node_idx = set.index_of("Node").expect("Node is its own set member");
        assert_eq!(
            fields.iter().find(|(f, _)| f == "value").map(|(_, t)| t),
            Some(&KType::Number),
            "value stays a builtin leaf",
        );
        assert_eq!(
            fields.iter().find(|(f, _)| f == "next").map(|(_, t)| t),
            Some(&KType::SetLocal(node_idx)),
            "next seals to a SetLocal self-reference",
        );
        assert!(scope.bindings().pending_types().is_empty());
    }

    /// A `:(LIST OF Self)` field threads the self-reference through the deferred sigil-field path:
    /// `children` seals to `List(SetLocal(Tree))`. (Construction is the same seal-shape concern the
    /// retired struct path pinned — a bare recursive record has no nullable base, and an empty list
    /// literal types as `List(Str)`, both orthogonal to the recursion threading proven here.)
    #[test]
    fn record_repr_list_of_self_field_seals_set_local() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("NEWTYPE Tree = :{children :(LIST OF Tree)}");
        let (set, fields) = record_fields(scope, "Tree");
        let tree_idx = set.index_of("Tree").expect("Tree is its own set member");
        assert_eq!(
            set.len(),
            1,
            "a self-recursive type seals into a singleton set"
        );
        assert_eq!(
            fields.iter().find(|(f, _)| f == "children").map(|(_, t)| t),
            Some(&KType::list(Box::new(KType::SetLocal(tree_idx)))),
            "children seals its self-reference to List(SetLocal(Tree))",
        );
        assert!(scope.bindings().pending_types().is_empty());
    }

    /// A record type nested as a field type elaborates *inline* through the shared field
    /// walker (no whole-`:{…}` sub-Dispatch), so the outer binder name threads into the
    /// inner record: `owner :Outer` seals to a `SetLocal` back-edge. The retired
    /// sub-Dispatch path could not thread here — it handed the record a fresh elaborator
    /// with an empty threaded set.
    #[test]
    fn nested_record_field_threads_self_reference() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("NEWTYPE Outer = :{inner :{owner :Outer}}");
        let (set, fields) = record_fields(scope, "Outer");
        let outer_idx = set.index_of("Outer").expect("Outer is its own set member");
        let inner_ty = fields
            .iter()
            .find(|(f, _)| f == "inner")
            .map(|(_, t)| t)
            .expect("inner field present");
        match inner_ty {
            KType::Record { fields: rec, .. } => assert_eq!(
                rec.get("owner"),
                Some(&KType::SetLocal(outer_idx)),
                "the nested record's `owner` threads to a SetLocal back-edge into Outer",
            ),
            other => panic!("expected `inner` to be a record type, got {other:?}"),
        }
        assert!(scope.bindings().pending_types().is_empty());
    }

    /// A non-record sigil repr (`= :(LIST OF Number)`) routes through the same
    /// `:SigiledTypeExpr` overload but has no self-reference to thread: it sub-dispatches the
    /// sigil to a resolved `KType` and seals a plain NewType over it. Regression guard for the
    /// overload split — this used to ride the `:ProperType` overload's speculative sub-dispatch.
    #[test]
    fn sigil_repr_non_record_seals_newtype_over_resolved_type() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Nums = :(LIST OF Number)");
        let result = test_run.run_one(parse_one("(Nums [1.0, 2.0])"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::SetRef { ref set, index } => {
                        assert_eq!(set.member(index).name, "Nums")
                    }
                    ref other => panic!("expected Nums identity, got {other:?}"),
                }
                assert!(
                    matches!(inner.get(), KObject::List(..)),
                    "inner is the bare list, got {:?}",
                    inner.get().ktype(),
                );
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// `Bar(Foo(3.0))` produces a single-layer `Wrapped { type_id: Bar,
    /// inner: Number(3.0) }` — pins the collapse invariant.
    #[test]
    fn newtype_over_newtype_collapses() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Foo = Number\nNEWTYPE Bar = Foo");
        let result = test_run.run_one(parse_one("Bar (Foo (3.0))"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::SetRef { ref set, index } => assert_eq!(set.member(index).name, "Bar"),
                    ref other => panic!("expected Bar identity, got {other:?}"),
                }
                // Critical: `inner` must be the bare Number, NOT another Wrapped.
                assert!(
                    matches!(inner.get(), KObject::Number(n) if *n == 3.0),
                    "expected bare Number inner, got {:?}",
                    inner.get().ktype(),
                );
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// `Distance` and `Number` are observably distinct at dispatch.
    ///
    /// Rejection lands as `DispatchFailed` out of `KoanRuntime::execute` (the per-slot
    /// type check filters the only candidate, scope chain runs out without a match)
    /// — drive the scheduler directly rather than `run_one_err`, which expects a
    /// per-slot Err result.
    #[test]
    fn dispatch_distinguishes_distance_from_number() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run(
            "NEWTYPE Distance = Number\n\
             FN (TAKES_NUM x :Number) -> Str = (\"num\")\n\
             FN (TAKES_DIST x :Distance) -> Str = (\"dist\")",
        );
        let r1 = test_run.run_one(parse_one("TAKES_DIST (Distance (3.0))"));
        match r1 {
            KObject::KString(s) => assert_eq!(s, "dist"),
            other => panic!("expected \"dist\", got {:?}", other.ktype()),
        }
        let r2 = test_run.run_one(parse_one("TAKES_NUM (3.0)"));
        match r2 {
            KObject::KString(s) => assert_eq!(s, "num"),
            other => panic!("expected \"num\", got {:?}", other.ktype()),
        }
        let root = test_run
            .runtime
            .dispatch_in_scope(parse_one("TAKES_NUM (Distance (3.0))"), scope);
        test_run
            .runtime
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let err = test_run
            .runtime
            .result_error(root)
            .expect_err("TAKES_NUM on Distance should fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on Number-slot Distance, got {err}",
        );
        let root2 = test_run
            .runtime
            .dispatch_in_scope(parse_one("TAKES_DIST (3.0)"), scope);
        test_run
            .runtime
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let err2 = test_run
            .runtime
            .result_error(root2)
            .expect_err("TAKES_DIST on raw Number should fail dispatch");
        assert!(
            matches!(&err2.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on Distance-slot Number, got {err2}",
        );
    }

    /// `Distance(x)` resolves the inner identifier inside the dep-finish's dispatched
    /// dep before the finish closure runs — pins the non-trivial-dispatch path.
    #[test]
    fn construct_with_identifier_value() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Distance = Number\nLET x = 3.0");
        let result = test_run.run_one(parse_one("Distance (x)"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::SetRef { ref set, index } => {
                        assert_eq!(set.member(index).name, "Distance")
                    }
                    ref other => panic!("expected Distance identity, got {other:?}"),
                }
                assert!(matches!(inner.get(), KObject::Number(n) if *n == 3.0));
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// Pins the pre-dispatch arity guard: `Distance ()` rejects with `ArityMismatch`.
    #[test]
    fn construct_arity_zero_rejects() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Distance = Number");
        let err = test_run.run_one_err(parse_one("Distance ()"));
        assert!(
            matches!(
                &err.kind,
                KErrorKind::ArityMismatch {
                    expected: 1,
                    got: 0
                }
            ),
            "expected ArityMismatch(1, 0) on Distance(), got {err}",
        );
    }

    /// Pins the "any sub-expression in the value position" path. Koan has no
    /// arithmetic operators today (per tutorial/README.md § "What isn't in the
    /// language yet"), so a user-fn call stands in for non-trivial dispatch.
    #[test]
    fn construct_with_operator_value() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run(
            "NEWTYPE Distance = Number\n\
             FN (MAKE_NUM x :Number) -> Number = (x)",
        );
        let result = test_run.run_one(parse_one("Distance (MAKE_NUM 3.0)"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::SetRef { ref set, index } => {
                        assert_eq!(set.member(index).name, "Distance")
                    }
                    ref other => panic!("expected Distance identity, got {other:?}"),
                }
                assert!(matches!(inner.get(), KObject::Number(n) if *n == 3.0));
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// `NEWTYPE (Type AS Wrapper)` mints a `TypeConstructor` `SetRef` in the declaring scope's
    /// type table: kind `TypeConstructor`, `param_names == ["Type"]`, empty schema, and no
    /// value-side entry.
    #[test]
    fn constructor_family_mints_declared_identity() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("NEWTYPE (Type AS Wrapper)");
        let types = scope.bindings().types();
        let (kt, _) = types
            .get("Wrapper")
            .expect("Wrapper should be in bindings.types");
        match **kt {
            KType::SetRef { ref set, index } => {
                let member = set.member(index);
                assert_eq!(member.name, "Wrapper");
                assert_eq!(member.kind, KKind::TypeConstructor);
                let borrow = member.schema();
                match borrow.as_ref() {
                    Some(NominalSchema::TypeConstructor {
                        schema,
                        param_names,
                    }) => {
                        assert!(
                            schema.is_empty(),
                            "a constructor family has an empty schema"
                        );
                        assert_eq!(*param_names, vec!["Type".to_string()]);
                    }
                    other => panic!("expected a TypeConstructor schema, got {other:?}"),
                }
            }
            ref other => panic!("expected a TypeConstructor SetRef identity, got {other:?}"),
        }
        drop(types);
        assert!(
            scope.bindings().data().get("Wrapper").is_none(),
            "a constructor-family declaration writes no value-side carrier",
        );
    }

    /// After `NEWTYPE (Type AS Wrapper)`, applying it with `:(Number AS Wrapper)` lowers to a
    /// `ConstructorApply { ctor: <Wrapper SetRef>, args: {Type = Number} }` — `AS` fills the
    /// constructor's sole parameter by name.
    #[test]
    fn constructor_family_applies_with_as() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE (Type AS Wrapper)");
        let result = test_run.run_one_type(parse_one(":(Number AS Wrapper)"));
        match result {
            KType::ConstructorApply { ctor, args, .. } => {
                match ctor.as_ref() {
                    KType::SetRef { set, index } => {
                        assert_eq!(set.member(*index).kind, KKind::TypeConstructor);
                    }
                    other => panic!("expected a SetRef ctor, got {other:?}"),
                }
                assert_eq!(
                    *args,
                    Record::from_pairs([("Type".to_string(), KType::Number)]),
                );
            }
            other => panic!("expected ConstructorApply, got {other:?}"),
        }
    }

    /// A `NEWTYPE (Type AS Wrap)` declared inside a MODULE body supplies a matching-arity
    /// higher-kinded `TYPE (Type AS Wrap)` SIG slot: `int_list :| Monad` ascribes.
    #[test]
    fn constructor_family_declared_inside_module_satisfies_hk_slot() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        let src = "SIG Monad = ((TYPE (Type AS Wrap)))\n\
                   MODULE int_list = ((NEWTYPE (Type AS Wrap)))\n\
                   LET view = (int_list :| Monad)";
        let exprs = crate::parse::parse(src).expect("parse should succeed");
        let mut ids = Vec::new();
        for expr in exprs {
            ids.push(test_run.runtime.dispatch_in_scope(expr, scope));
        }
        test_run
            .runtime
            .execute()
            .expect("scheduler should succeed");
        for (i, id) in ids.iter().enumerate() {
            if let Err(e) = test_run.runtime.result_error(*id) {
                panic!("expr {i} errored: {e}");
            }
        }
        assert!(
            binds_module(scope, "view"),
            "int_list must satisfy Monad and bind a view module",
        );
    }

    /// `NEWTYPE (One Two AS Wrapper)` — two parameters before `AS` — declares an arity-2
    /// family through the shared `TYPE` declaration parser.
    #[test]
    fn constructor_family_arity_above_one_declares() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("NEWTYPE (One Two AS Wrapper)");
        let kt = scope
            .resolve_type("Wrapper")
            .expect("Wrapper must bind a type");
        assert_eq!(
            crate::machine::model::constructor_param_names(kt, &test_run.types),
            Some(vec!["One".to_string(), "Two".to_string()]),
        );
    }

    /// `NEWTYPE (Type Wrapper)` — no `AS` keyword — is a shape error from the shared parser.
    #[test]
    fn constructor_family_missing_as_rejects() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let err = test_run.run_one_err(parse_one("NEWTYPE (Type Wrapper)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(_)),
            "expected a shape error for a missing AS, got {err}",
        );
    }

    /// `Wrapper (3.0)` over a `NEWTYPE (Type AS Wrapper)` family constructs a `Wrapped`
    /// whose payload is the bare `Number` and whose `type_id` is
    /// `ConstructorApply(Wrapper, {Type = Number})` — the value inhabits `:(Number AS Wrapper)`.
    #[test]
    fn apply_construct_wraps_and_stamps() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE (Type AS Wrapper)");
        let result = test_run.run_one(parse_one("Wrapper (3.0)"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::ConstructorApply {
                        ref ctor, ref args, ..
                    } => {
                        match ctor.as_ref() {
                            KType::SetRef { set, index } => {
                                assert_eq!(set.member(*index).name, "Wrapper");
                                assert_eq!(set.member(*index).kind, KKind::TypeConstructor);
                            }
                            other => panic!("expected a SetRef ctor, got {other:?}"),
                        }
                        assert_eq!(
                            *args,
                            Record::from_pairs([("Type".to_string(), KType::Number)]),
                        );
                    }
                    ref other => panic!("expected a ConstructorApply type_id, got {other:?}"),
                }
                assert!(matches!(inner.get(), KObject::Number(n) if *n == 3.0));
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// `Wrapper (Distance (3.0))` collapses the inner `Wrapped` payload — the stored `inner`
    /// is the bare `Number`, never a nested `Wrapped` (the single-layer invariant) — while the
    /// stamped arg keeps the full `Distance` nominal identity: args are `[Distance's SetRef]`.
    #[test]
    fn apply_construct_collapses_wrapped_payload() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE Distance = Number\nNEWTYPE (Type AS Wrapper)");
        let result = test_run.run_one(parse_one("Wrapper (Distance (3.0))"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                // Single-layer invariant: the collapsed inner is the bare Number, not a Wrapped.
                assert!(
                    matches!(inner.get(), KObject::Number(n) if *n == 3.0),
                    "inner must be the bare Number, got {:?}",
                    inner.get().ktype(),
                );
                match **type_id {
                    KType::ConstructorApply { ref args, .. } => {
                        assert_eq!(args.len(), 1);
                        // The stamped arg keeps the Distance identity (a NewType SetRef).
                        match args.get("Type") {
                            Some(KType::SetRef { set, index }) => {
                                assert_eq!(set.member(*index).name, "Distance");
                                assert_eq!(set.member(*index).kind, KKind::NewType);
                            }
                            other => panic!("expected the Distance SetRef arg, got {other:?}"),
                        }
                    }
                    ref other => panic!("expected a ConstructorApply type_id, got {other:?}"),
                }
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// `Wrapper ()` — no value — is an arity-zero rejection.
    #[test]
    fn apply_construct_arity_zero_rejects() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE (Type AS Wrapper)");
        let err = test_run.run_one_err(parse_one("Wrapper ()"));
        assert!(
            matches!(
                &err.kind,
                KErrorKind::ArityMismatch {
                    expected: 1,
                    got: 0
                }
            ),
            "expected ArityMismatch(1, 0), got {err}",
        );
    }

    /// A `FN` param typed `:(Number AS Wrapper)` admits a matching `Wrapper (3.0)` value,
    /// rejects a bare `3.0` (not a `Wrapped`), and rejects a `(Str AS Wrapper)` value
    /// (`Wrapper ("s")` — the stamped arg is `Str`, not `Number`).
    #[test]
    fn applied_type_dispatches() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run(
            "NEWTYPE (Type AS Wrapper)\n\
             FN (UNPACK x :(Number AS Wrapper)) -> Str = (\"hit\")",
        );
        // A matching applied-type value dispatches.
        let hit = test_run.run_one(parse_one("UNPACK (Wrapper (3.0))"));
        assert!(
            matches!(hit, KObject::KString(s) if s == "hit"),
            "Wrapper (3.0) must dispatch, got {:?}",
            hit.ktype(),
        );
        // A bare Number is not a Wrapped value — dispatch fails.
        expect_dispatch_failure(&mut test_run, "UNPACK 3.0");
        // A (Str AS Wrapper) value: the stamped arg is Str, not Number — dispatch fails.
        expect_dispatch_failure(&mut test_run, "UNPACK (Wrapper (\"s\"))");
    }

    /// Run `probe` against the bundle's scope and assert it fails dispatch (a slot-terminal
    /// `DispatchFailed`, not a fatal execute error).
    fn expect_dispatch_failure(test_run: &mut TestRun<'_>, probe: &str) {
        let scope = test_run.scope;
        let root = test_run.runtime.dispatch_in_scope(parse_one(probe), scope);
        test_run
            .runtime
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let err = test_run
            .runtime
            .result_error(root)
            .expect_err("probe should fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed for `{probe}`, got {err}",
        );
    }

    /// Two `Wrapper (3.0)` values compare `==` true (structural `Wrapped` equality); a
    /// `Wrapper (3.0)` and a `Wrapper ("x")` compare false.
    #[test]
    fn applied_values_are_value_equal() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE (Type AS Wrapper)");
        match test_run.run_one(parse_one("(Wrapper (3.0)) == (Wrapper (3.0))")) {
            KObject::Bool(b) => assert!(*b, "two Wrapper (3.0) must compare equal"),
            other => panic!("expected Bool, got {:?}", other.ktype()),
        }
        match test_run.run_one(parse_one("(Wrapper (3.0)) == (Wrapper (\"x\"))")) {
            KObject::Bool(b) => assert!(!*b, "Wrapper (3.0) and Wrapper (\"x\") must differ"),
            other => panic!("expected Bool, got {:?}", other.ktype()),
        }
    }

    /// A record-literal payload rides through as a single positional value; ATTR projects a
    /// field of it through the `Wrapped` layer.
    #[test]
    fn attr_projects_record_payload() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("NEWTYPE (Type AS Wrapper)\nLET w = (Wrapper ({x = 1.0}))");
        let result = test_run.run_one(parse_one("w.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 1.0));
    }

    /// A `:(Any AS Wrapper)` param admits every instantiation of the family — `Wrapper (3.0)`
    /// and `Wrapper ("s")` both dispatch (the `Any` slot arg admits any stamped arg) — while a
    /// bare `3.0` (not a `Wrapped`) still fails.
    #[test]
    fn applied_any_slot_admits_all_instantiations() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run(
            "NEWTYPE (Type AS Wrapper)\n\
             FN (ANYUNPACK x :(Any AS Wrapper)) -> Str = (\"hit\")",
        );
        for probe in ["ANYUNPACK (Wrapper (3.0))", "ANYUNPACK (Wrapper (\"s\"))"] {
            let hit = test_run.run_one(parse_one(probe));
            assert!(
                matches!(hit, KObject::KString(s) if s == "hit"),
                "`{probe}` must dispatch, got {:?}",
                hit.ktype(),
            );
        }
        expect_dispatch_failure(&mut test_run, "ANYUNPACK 3.0");
    }

    /// A `TYPE`-declared abstract constructor slot names a kind but constructs nothing:
    /// constructing over it is a `ShapeError`, not a `Wrapped` value.
    #[test]
    fn abstract_constructor_slot_rejects_construction() {
        use crate::machine::BindingIndex;
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        let kt = KType::AbstractType {
            source: scope.id,
            name: "Abstract".into(),
            param_names: vec!["Type".into()],
            nonce: None,
        };
        scope.register_builtin_type("Abstract".into(), kt, BindingIndex::BUILTIN);
        let err = test_run.run_one_err(parse_one("Abstract (3.0)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("abstract constructor slot declared by TYPE")),
            "expected the abstract-constructor-slot ShapeError, got {err}",
        );
    }
}
