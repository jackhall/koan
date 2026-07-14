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

use crate::machine::model::types::KKind;
use std::cell::RefCell;

use crate::machine::core::kfunction::action::FinishCtx;
use crate::machine::execute::{seal_type_operand, StepCarried};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{
    finalize_nominal_member, seal_recursive_refs, FieldNameKind, NominalSchema, Record,
    RecursiveSet, SchemaSealResult, SealOutcome,
};
use crate::machine::model::values::KObject;
use crate::machine::model::KType;
use crate::machine::{BindingIndex, KError, KErrorKind, Scope, TraceFrame};
use crate::source::Spanned;

use super::{arg, kw, sig};
use crate::machine::DeliveredCarried;

/// Seal a resolved `repr` into the NEWTYPE's identity and register it. A NEWTYPE is
/// non-recursive (its `repr` is already resolved), so it seals into a singleton set of one
/// member whose `kind` (`NewType`) is what `kind_of` reports for the sealed `SetRef`;
/// identity never descends `repr`.
///
/// `repr` embeds `carrier`'s reach when `carrier` is `Some` — a bind-time `repr` argument (a
/// caller-supplied structural type) or a sigil repr's dep terminal (`defer_resolved_sigil`) can
/// both carry a borrow into a foreign region (a declared `Signature`, a nominal `SetRef`, ...).
/// `carrier` is `None` for a bare-leaf name resolved against scope bindings; the sealed identity
/// there is a fresh `SetRef` over its own set (never `'static`), so it takes
/// [`StepAllocator::alloc_type_checked`]'s runtime-audited seal, which passes because the identity
/// borrows at most its own region.
fn finalize_newtype<'a>(
    fctx: &FinishCtx<'a>,
    name: String,
    repr: KType<'a>,
    bind_index: BindingIndex,
    carrier: Option<&DeliveredCarried>,
) -> Result<StepCarried<'a>, KError> {
    let scope = fctx.scope;
    let scope_id = scope.id;
    let set = RecursiveSet::singleton(
        name.clone(),
        scope_id,
        NominalSchema::NewType(Box::new(repr)),
    );
    let identity = KType::SetRef { set, index: 0 };
    // Fused mint + alloc + register: the RHS carrier's reach is minted into this scope's arena (kept
    // mode) so `identity`'s SetRef (never `'static`, and possibly embedding `repr`'s foreign borrow —
    // see this fn's doc) is audited against it, the identity is allocated under it, and it is
    // registered — one call returns the resident `&KType` plus the same token. The token names no
    // foreign reach when `carrier` is `None`, which degrades the reaching check to the dest-only case.
    let (kt_ref, stored) = scope.register_type_delivered(name, identity, carrier, bind_index)?;
    let sealed = match carrier {
        // Cross the identity across the build brand as a declared operand, folding the carrier's
        // reach onto the placement's witness — rather than capturing `kt_ref` into a fold closure.
        // `stored` is the identity's own token (the carrier's host reach `kt_ref` was audited
        // against), replayed whole so the operand's witness carries the derived home-borrow bit.
        Some(c) => seal_type_operand(fctx.scope, fctx.ctx.frame(), kt_ref, stored, &[c]),
        // A bare-leaf name reaches no foreign region: `kt_ref` is region-pure over its own set, so it
        // seals with no carrier to fold and no capture to cross.
        None => fctx.ctx.alloc_type_checked(kt_ref.clone())?,
    };
    Ok(sealed)
}

/// Seal the elaborated record fields into the NEWTYPE's [`RecursiveSet`] member as
/// `NominalSchema::NewType(KType::record(sealed))`. Transient `RecursiveRef(name)` field leaves
/// seal to `SetLocal(index)` against the member's set — the block's shared set when present (a
/// `RECURSIVE TYPES` member), else a fresh singleton (standalone self-recursion). Shared by the
/// synchronous and dep-finish paths.
fn finalize_record_newtype<'a>(
    fctx: &FinishCtx<'a>,
    name: String,
    fields: Vec<(String, KType<'a>)>,
    bind_index: BindingIndex,
    carriers: &[&DeliveredCarried],
) -> Result<StepCarried<'a>, KError> {
    if fields.is_empty() {
        return Err(KError::new(KErrorKind::ShapeError(
            "NEWTYPE record repr must have at least one field".to_string(),
        )));
    }
    let scope = fctx.scope;
    let scope_id = scope.id;
    let outcome = finalize_nominal_member(
        scope,
        &name,
        scope_id,
        KKind::NewType,
        |set| {
            let missing = RefCell::new(Vec::new());
            let sealed_pairs: Vec<(String, KType<'a>)> = fields
                .into_iter()
                .map(|(field, kt)| (field, seal_recursive_refs(set, &kt, &missing)))
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
        // Cross the sealed identity as a declared operand and fold the field carriers' reach onto
        // the placement's witness — `kt_ref` seals over its own set (empty foreign reach), the
        // carriers supply whatever the record fields reach.
        SealOutcome::Sealed(kt_ref) => Ok(seal_type_operand(
            scope,
            fctx.ctx.frame(),
            kt_ref,
            scope.checked_reach_of_type(kt_ref),
            carriers,
        )),
        SealOutcome::DanglingRef(missing) => Err(KError::new(KErrorKind::ShapeError(format!(
            "NEWTYPE `{name}` record repr references unsealed type `{missing}`",
        )))),
        SealOutcome::Rebind(e) => Err(e),
    }
}

/// A resolved repr finalizes synchronously; a bare-leaf name resolves against the scope chain,
/// parks on an in-flight producer (a `DepRequest::Existing` dep-finish), or errors; a raw sigil repr
/// sub-dispatches via [`defer_resolved_sigil`].
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::builtins::resolve_or_await::{classify_name_lookup, resolve_or_await};
    use crate::machine::core::kfunction::action::{
        arg_object, arg_type, require_bare_type_name, Action,
    };

    let name = crate::try_action!(require_bare_type_name(ctx.args, "name", "NEWTYPE"));
    let chain = ctx.chain.clone();
    let bind_index = ctx.bind_index();
    if let Some(repr_kt) = arg_type(ctx.args, "repr") {
        match repr_kt {
            KType::Unresolved(te) => {
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
                    move |fctx, kt| {
                        Action::Done(finalize_newtype(fctx, name, kt, bind_index, None))
                    },
                )
            }
            // A bind-time `repr` argument: any caller-supplied structural carrier, so
            // `arg_carrier` names its own foreign reach if it has one.
            other => Action::Done(finalize_newtype(
                &ctx.finish_ctx(),
                name,
                other.clone(),
                bind_index,
                ctx.arg_carrier("repr"),
            )),
        }
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
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::builtins::resolve_or_await::dispatch_type_then;
    use crate::machine::core::kfunction::action::Action;
    let wrapped = KExpression::new(vec![Spanned::bare(ExpressionPart::SigiledTypeExpr(
        Box::new(inner),
    ))]);
    dispatch_type_then(wrapped, "NEWTYPE repr slot", move |fctx, kt, carrier| {
        Action::Done(finalize_newtype(fctx, name, kt, bind_index, Some(carrier)))
    })
}

/// Body of the record-repr overload `NEWTYPE <name> = :{…}`: elaborate the `:{…}` field list
/// (threading the binder name + pending guard), folding via [`finalize_record_newtype`] or deferring
/// through the shared `nominal_schema_action` field-list path.
pub fn body_record_repr<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::nominal_schema::nominal_schema_action;
    use crate::machine::core::kfunction::action::{arg_object, require_bare_type_name, Action};

    let name = crate::try_action!(require_bare_type_name(ctx.args, "name", "NEWTYPE"));
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
        KKind::NewType,
        "NEWTYPE record repr",
        FieldNameKind::Identifier,
        error_frame,
        finalize_record_newtype,
    )
}

pub fn register<'a>(scope: &'a Scope<'a>) {
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
    let binder: crate::machine::core::kfunction::BinderNameFn = super::type_part_binder_name;
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
    );
    register_builtin_full(
        scope,
        "NEWTYPE",
        sigil_sig(),
        body,
        Some((binder, binder_kind)),
        None,
    );
    register_builtin_full(
        scope,
        "NEWTYPE",
        record_sig(),
        body_record_repr,
        Some((binder, binder_kind)),
        None,
    );
}

#[cfg(test)]
mod tests {

    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::machine::core::run_root_storage;
    use crate::machine::execute::KoanRuntime;
    use crate::machine::model::types::{KKind, NominalSchema, ProjectedSchema, RecursiveSet};
    use crate::machine::model::{KObject, KType};
    use crate::machine::{KErrorKind, Scope};
    use std::rc::Rc;

    /// `(set, record-fields)` of a sealed record-repr newtype, read raw off its `SetRef`
    /// identity so assertions see `SetLocal` / `List(SetLocal)` back-edges before projection.
    fn record_fields<'a>(
        scope: &'a Scope<'a>,
        name: &str,
    ) -> (Rc<RecursiveSet<'a>>, Vec<(String, KType<'a>)>) {
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
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Distance = Number");
        let types = scope.bindings().types();
        let (kt, _, _) = types
            .get("Distance")
            .expect("Distance should be in bindings.types");
        match **kt {
            KType::SetRef { ref set, index } => {
                assert_eq!(set.member(index).name, "Distance");
                assert_eq!(set.member(index).kind, KKind::NewType);
                match RecursiveSet::projected_schema(set, index) {
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
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Distance = Number");
        let result = run_one(scope, parse_one("Distance (3.0)"));
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
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Distance = Number");
        let err = run_one_err(scope, parse_one("Distance (\"hi\")"));
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
        let scope = run_root_silent(&region);
        run(
            scope,
            "NEWTYPE Point = :{x :Number, y :Number}\nNEWTYPE Boxed = Point",
        );
        // No placeholder may survive the declaration run: a leaked one corrupts the next
        // scheduler on this REPL-persistent scope.
        assert!(
            scope.bindings().placeholders().is_empty(),
            "NEWTYPE declarations must leave no value-side placeholder, got {:?}",
            *scope.bindings().placeholders(),
        );
        let result = run_one(scope, parse_one("(Boxed (Point {x = 1, y = 2}))"));
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
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Boxed = Nope");
        assert!(
            scope.bindings().placeholders().is_empty(),
            "a failed NEWTYPE must not leak its placeholder, got {:?}",
            *scope.bindings().placeholders(),
        );
        let err = run_one_err(scope, parse_one("(Boxed (3.0))"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(n) if n == "Boxed"),
            "expected UnboundName(Boxed) after failed declaration, got {err}",
        );
    }

    /// A self-recursive record repr seals its self-reference to a `SetLocal` back-edge into the
    /// declaring member's singleton set — the binder name is threaded through the field-list
    /// elaboration. (`next :Node` has no base case, so the type is uninhabited by a finite value;
    /// this pins the seal shape, not construction.)
    #[test]
    fn record_repr_self_recursion_seals_set_local() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Node = :{value :Number, next :Node}");
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
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Tree = :{children :(LIST OF Tree)}");
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
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Outer = :{inner :{owner :Outer}}");
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
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Nums = :(LIST OF Number)");
        let result = run_one(scope, parse_one("(Nums [1.0, 2.0])"));
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
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Foo = Number\nNEWTYPE Bar = Foo");
        let result = run_one(scope, parse_one("Bar (Foo (3.0))"));
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
        let scope = run_root_silent(&region);
        run(
            scope,
            "NEWTYPE Distance = Number\n\
             FN (TAKES_NUM x :Number) -> Str = (\"num\")\n\
             FN (TAKES_DIST x :Distance) -> Str = (\"dist\")",
        );
        let r1 = run_one(scope, parse_one("TAKES_DIST (Distance (3.0))"));
        match r1 {
            KObject::KString(s) => assert_eq!(s, "dist"),
            other => panic!("expected \"dist\", got {:?}", other.ktype()),
        }
        let r2 = run_one(scope, parse_one("TAKES_NUM (3.0)"));
        match r2 {
            KObject::KString(s) => assert_eq!(s, "num"),
            other => panic!("expected \"num\", got {:?}", other.ktype()),
        }
        let mut sched1 = KoanRuntime::new();
        let root = sched1.dispatch_in_scope(parse_one("TAKES_NUM (Distance (3.0))"), scope);
        sched1
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let err = sched1
            .result_error(root)
            .expect_err("TAKES_NUM on Distance should fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on Number-slot Distance, got {err}",
        );
        let mut sched2 = KoanRuntime::new();
        let root2 = sched2.dispatch_in_scope(parse_one("TAKES_DIST (3.0)"), scope);
        sched2
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let err2 = sched2
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
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Distance = Number\nLET x = 3.0");
        let result = run_one(scope, parse_one("Distance (x)"));
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
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Distance = Number");
        let err = run_one_err(scope, parse_one("Distance ()"));
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
        let scope = run_root_silent(&region);
        run(
            scope,
            "NEWTYPE Distance = Number\n\
             FN (MAKE_NUM x :Number) -> Number = (x)",
        );
        let result = run_one(scope, parse_one("Distance (MAKE_NUM 3.0)"));
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
}
