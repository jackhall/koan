//! `NEWTYPE <name> = <repr>` — declare a fresh nominal identity over a transparent
//! representation. The declaration writes only `bindings.types` (no value-side
//! schema carrier). Construction produces a [`KObject::Wrapped`] tagging the inner
//! value with the NEWTYPE identity; the `Wrapped.inner` is invariantly non-`Wrapped`
//! (newtype-over-newtype collapses to a single layer).
//!
//! Three registered overloads selected by the repr part-kind. A scalar / bare-leaf repr
//! (`= Number`, `= Foo`) resolves eagerly through the `:TypeExprRef` slot. A non-record
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
use std::rc::Rc;

use crate::machine::core::kfunction::argument_bundle::{
    extract_bare_type_name, extract_kexpression, extract_ktype,
};
use crate::machine::core::source::Spanned;
use crate::machine::core::{ApplyOutcome, LexicalFrame, PendingTypeEntry};
use crate::machine::execute::defer_field_list_via_combine;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{
    finalize_nominal_member, parse_typed_field_list_via_elaborator, seal_recursive_refs,
    Elaborator, FieldListOutcome, FieldNameKind, NominalMember, NominalSchema, Record,
    RecursiveSet, SchemaSealResult, SealOutcome,
};
use crate::machine::model::values::{Carried, KObject};
use crate::machine::model::KType;
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, Frame, KError, KErrorKind, Resolution,
    SchedulerHandle, Scope,
};

use super::{arg, err, kw, sig};
#[cfg(not(feature = "action-harness"))]
use super::register_builtin_with_binder;

/// Body of `NEWTYPE <name> = <repr>`, shared by the scalar (`:TypeExprRef` repr) and
/// non-record-sigil (`:SigiledTypeExpr` repr) overloads. Branches on the repr carrier: a
/// `Type`-arm `KType` — either resolved (scalar path → [`finalize_newtype`]) or a bare-leaf
/// [`KType::Unresolved`] name (scope-chain walk → finalize) — or an `Object`-arm
/// `KExpression` from the `:SigiledTypeExpr` slot (a structural sigil like `:(LIST OF T)` →
/// [`defer_resolved_sigil`]). The record repr `:{…}` is a distinct `RecordType` part routed
/// to [`body_record_repr`]. Every path writes the sealed `SetRef` identity into
/// `bindings.types` and yields it on the type channel.
pub fn body<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match extract_bare_type_name(&bundle, "name", "NEWTYPE") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    // Gated to the NEWTYPE's lexical position: a repr naming a later type is a position
    // error, like any other forward type reference.
    let chain = sched.current_lexical_chain();
    let bind_index = chain
        .as_ref()
        .map(|c| BindingIndex::value(c.index))
        .unwrap_or(BindingIndex::BUILTIN);
    // A `Type`-arm carrier splits two ways: a resolved `KType` for builtin leaves /
    // structural shapes, `KType::Unresolved` for bare-leaf names. An `Object`-arm
    // `KExpression` is a raw-captured sigil. Peek before extracting so we route to the
    // right helper — both consume the slot.
    if bundle.get_type("repr").is_some() {
        match extract_ktype(&mut bundle, "repr") {
            // Bare-leaf carrier (`NEWTYPE Bar = Foo` where `Foo` is user-declared):
            // walk the scope chain for the resolved identity.
            Some(KType::Unresolved(te)) => {
                if let Some(kt) = sched
                    .current_scope()
                    .resolve_type_with_chain(te.as_str(), chain.as_deref())
                {
                    return finalize_newtype(sched.current_scope(), name, kt.clone(), bind_index);
                }
                // The repr names a type that is still finalizing in this scheduler — e.g. a
                // record-repr dependency whose `:{…}` defers its own finalize behind a
                // sub-dispatch, so this dependent's body can run first. Park on the
                // producer and re-resolve at Combine-finish, once its identity is in
                // `types`. A name with no in-flight producer is a genuine forward/unknown
                // reference — a position error.
                if let Resolution::Placeholder(producer) = sched
                    .current_scope()
                    .resolve_with_chain(te.as_str(), chain.as_deref())
                {
                    let finish: CombineFinish<'a> = Box::new(move |_sched, _results| match _sched
                        .current_scope()
                        .resolve_type_with_chain(te.as_str(), chain.as_deref())
                    {
                        Some(kt) => {
                            finalize_newtype(_sched.current_scope(), name, kt.clone(), bind_index)
                        }
                        None => err(KError::new(KErrorKind::ShapeError(format!(
                            "NEWTYPE repr slot = unknown type name `{}`",
                            te.as_str(),
                        )))),
                    });
                    let combine_id = sched.add_combine_here(Vec::new(), vec![producer], finish);
                    return BodyResult::DeferTo(combine_id);
                }
                err(KError::new(KErrorKind::ShapeError(format!(
                    "NEWTYPE repr slot = unknown type name `{}`",
                    te.as_str(),
                ))))
            }
            Some(repr) => finalize_newtype(sched.current_scope(), name, repr, bind_index),
            None => unreachable!("get_type(repr) then extract_ktype must succeed"),
        }
    } else if matches!(bundle.get("repr"), Some(KObject::KExpression(_))) {
        // Raw-captured sigil repr from the `:SigiledTypeExpr` overload — a structural repr
        // (`:(LIST OF Number)`) with no self-reference to thread. Sub-dispatch it to a resolved
        // `KType` and seal a plain Newtype over it. A record repr `:{…}` is a distinct part
        // routed to its own overload ([`body_record_repr`]), so it never reaches here.
        let inner = match extract_kexpression(&mut bundle, "repr") {
            Some(e) => e,
            None => unreachable!("get(KExpression) then extract_kexpression must succeed"),
        };
        defer_resolved_sigil(sched, name, inner, bind_index)
    } else {
        err(KError::new(KErrorKind::ShapeError(
            "NEWTYPE repr slot must be a type expression (e.g. `Number`, `Foo`)".to_string(),
        )))
    }
}

/// Seal a resolved `repr` into the NEWTYPE's identity and register it. A NEWTYPE is
/// non-recursive (its `repr` is already resolved), so it seals into a singleton set of one
/// member whose `kind` (`Newtype`) is what `kind_of` reports for the sealed `SetRef`;
/// identity never descends `repr`.
fn finalize_newtype<'a>(
    scope: &Scope<'a>,
    name: String,
    repr: KType<'a>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    let scope_id = scope.id;
    let member = NominalMember::pending(name.clone(), scope_id, KKind::Newtype);
    member.fill(NominalSchema::Newtype(Box::new(repr)));
    let set = Rc::new(RecursiveSet::new(vec![member]));
    let identity = KType::SetRef { set, index: 0 };
    let kt_ref: &'a KType = scope.arena.alloc_ktype(identity);
    match scope
        .bindings()
        .try_register_type(&name, kt_ref, bind_index)
    {
        Ok(ApplyOutcome::Applied) => BodyResult::ktype(scope.arena.alloc_ktype(kt_ref.clone())),
        // Finalize sites run post-Combine outside the re-entrant hot path, so borrow
        // contention here is a programming error. Surface as a structured error rather
        // than panicking — a future re-entrant caller still gets a recoverable diag.
        Ok(ApplyOutcome::Conflict) => err(KError::new(KErrorKind::ShapeError(format!(
            "NEWTYPE `{name}` registration deferred = bindings borrow contention",
        )))),
        Err(e) => err(e),
    }
}

/// Body of the record-repr overload `NEWTYPE <name> = :{…}`. The `:RecordType` repr slot
/// captures the field list raw (as a `KObject::KExpression`), so the declarator owns its
/// elaboration and threads its own binder name through a recursive `:{next :Node}` — the
/// reason this is a distinct overload from the shared [`body`] rather than a peek inside it.
pub fn body_record_repr<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match extract_bare_type_name(&bundle, "name", "NEWTYPE") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let chain = sched.current_lexical_chain();
    let bind_index = chain
        .as_ref()
        .map(|c| BindingIndex::value(c.index))
        .unwrap_or(BindingIndex::BUILTIN);
    let fields = match extract_kexpression(&mut bundle, "repr") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "NEWTYPE record repr slot must be a record type `:{…}`".to_string(),
            )))
        }
    };
    elaborate_record_repr(sched, name, fields, bind_index, chain)
}

/// Elaborate + seal a record repr (`:{…}`), threading the binder name so a self-reference
/// (`NEWTYPE Node = :{next :Node}`) lowers to a transient `RecursiveRef` and seals to a
/// `SetLocal`. Mirrors the retired `STRUCT` declarator, sealing `NominalSchema::Newtype(Record)`
/// rather than `Struct`. `fields` is the bare field list (`(name :Type, …)`) carried by the
/// `:{…}` `RecordType` part.
fn elaborate_record_repr<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    name: String,
    fields: KExpression<'a>,
    bind_index: BindingIndex,
    chain: Option<Rc<LexicalFrame>>,
) -> BodyResult<'a> {
    // Mark this binder in-flight so a consumer referencing it (an earlier sibling still
    // finalizing) can park on our producer node; the guard's Drop removes the entry, and the
    // deferred path moves it into the Combine-finish closure.
    let pending_guard = sched.current_scope().bindings().insert_pending_type(
        name.clone(),
        PendingTypeEntry {
            kind: KKind::Newtype,
            scope_id: sched.current_scope().id,
            schema_expr: fields.clone(),
        },
    );
    let mut elaborator = Elaborator::new(sched.current_scope())
        .with_threaded([name.clone()])
        .with_chain(chain.clone());
    let outcome = parse_typed_field_list_via_elaborator(
        &fields,
        "NEWTYPE record repr",
        FieldNameKind::Identifier,
        &mut elaborator,
        None,
    );
    match outcome {
        FieldListOutcome::Done(sealed) => {
            finalize_record_newtype(sched.current_scope(), name, sealed, bind_index)
        }
        FieldListOutcome::Err(msg) => err(KError::new(KErrorKind::ShapeError(msg))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => {
            let name_for_finish = name.clone();
            defer_field_list_via_combine(
                sched,
                fields,
                park_producers,
                sub_dispatches,
                "NEWTYPE record repr",
                FieldNameKind::Identifier,
                vec![name.clone()],
                chain,
                Some(pending_guard),
                Some(Frame::bare("<newtype>", format!("NEWTYPE {name}"))),
                Box::new(move |scope, sealed| {
                    finalize_record_newtype(scope, name_for_finish, sealed, bind_index)
                }),
            )
        }
    }
}

/// Seal the elaborated record fields into the NEWTYPE's [`RecursiveSet`] member as
/// `NominalSchema::Newtype(KType::Record(sealed))`. Transient `RecursiveRef(name)` field leaves
/// seal to `SetLocal(index)` against the member's set — the block's shared set when present (a
/// `RECURSIVE TYPES` member), else a fresh singleton (standalone self-recursion). Shared by the
/// synchronous and Combine-finish paths.
fn finalize_record_newtype<'a>(
    scope: &Scope<'a>,
    name: String,
    fields: Vec<(String, KType<'a>)>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    if fields.is_empty() {
        return err(KError::new(KErrorKind::ShapeError(
            "NEWTYPE record repr must have at least one field".to_string(),
        )));
    }
    let scope_id = scope.id;
    let outcome = finalize_nominal_member(
        scope,
        &name,
        scope_id,
        KKind::Newtype,
        |set| {
            let missing = RefCell::new(Vec::new());
            let sealed_pairs: Vec<(String, KType<'a>)> = fields
                .into_iter()
                .map(|(field, kt)| (field, seal_recursive_refs(set, &kt, &missing)))
                .collect();
            let sealed = Record::from_pairs(sealed_pairs);
            match missing.into_inner().into_iter().next() {
                Some(m) => SchemaSealResult::Dangling(m),
                None => SchemaSealResult::Ok(NominalSchema::Newtype(Box::new(KType::Record(
                    Box::new(sealed),
                )))),
            }
        },
        bind_index,
    );
    match outcome {
        SealOutcome::Sealed(kt_ref) => BodyResult::ktype(scope.arena.alloc_ktype(kt_ref.clone())),
        SealOutcome::DanglingRef(missing) => err(KError::new(KErrorKind::ShapeError(format!(
            "NEWTYPE `{name}` record repr references unsealed type `{missing}`",
        )))),
        SealOutcome::Rebind(e) => err(e),
    }
}

/// A non-record sigil repr (`NEWTYPE Stream = :(LIST OF Number)`): no self-reference to thread, so
/// re-wrap the captured sigil and sub-dispatch it to a resolved `KType`, then seal a plain Newtype
/// over the result at Combine-finish.
fn defer_resolved_sigil<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    name: String,
    inner: KExpression<'a>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    let wrapped = KExpression::new(vec![Spanned::bare(ExpressionPart::SigiledTypeExpr(
        Box::new(inner),
    ))]);
    let sub = sched.add_dispatch_here(wrapped);
    let finish: CombineFinish<'a> = Box::new(move |_sched, results| match results[0] {
        Carried::Type(kt) => finalize_newtype(_sched.current_scope(), name, kt.clone(), bind_index),
        Carried::Object(other) => BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
            "NEWTYPE repr sigil resolved to a non-type value `{}`",
            other.ktype().name(),
        )))),
    });
    let combine_id = sched.add_combine_here(vec![sub], Vec::new(), finish);
    BodyResult::DeferTo(combine_id)
}

/// `Action`-harness twin of [`body`]: a resolved repr finalizes synchronously; a bare-leaf name
/// resolves against the scope chain, parks on an in-flight producer (a `Dep::Existing` Combine), or
/// errors; a raw sigil repr sub-dispatches via [`defer_resolved_sigil_action`].
#[cfg(feature = "action-harness")]
pub fn body_action<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{
        arg_object, arg_type, body_result_to_action, require_bare_type_name, Action, Cont, Dep,
    };

    let name = crate::try_action!(require_bare_type_name(ctx.args, "name", "NEWTYPE"));
    let chain = ctx.chain.clone();
    let bind_index = ctx.bind_index();
    if let Some(repr_kt) = arg_type(ctx.args, "repr") {
        match repr_kt {
            KType::Unresolved(te) => {
                let te = te.clone();
                if let Some(kt) = ctx
                    .scope
                    .resolve_type_with_chain(te.as_str(), chain.as_deref())
                {
                    return body_result_to_action(finalize_newtype(
                        ctx.scope,
                        name,
                        kt.clone(),
                        bind_index,
                    ));
                }
                // The repr names a type still finalizing in this scheduler: park on its producer
                // and re-resolve at Combine-finish. A name with no in-flight producer is a genuine
                // forward/unknown reference.
                if let Resolution::Placeholder(producer) = ctx
                    .scope
                    .resolve_with_chain(te.as_str(), chain.as_deref())
                {
                    let chain_for_finish = chain.clone();
                    let finish: Cont<'a> = Box::new(move |fctx, _results| {
                        match fctx
                            .scope
                            .resolve_type_with_chain(te.as_str(), chain_for_finish.as_deref())
                        {
                            Some(kt) => body_result_to_action(finalize_newtype(
                                fctx.scope,
                                name,
                                kt.clone(),
                                bind_index,
                            )),
                            None => Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                                "NEWTYPE repr slot = unknown type name `{}`",
                                te.as_str(),
                            ))))),
                        }
                    });
                    return Action::Combine {
                        deps: vec![Dep::Existing(producer)],
                        finish,
                    };
                }
                Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "NEWTYPE repr slot = unknown type name `{}`",
                    te.as_str(),
                )))))
            }
            other => body_result_to_action(finalize_newtype(
                ctx.scope,
                name,
                other.clone(),
                bind_index,
            )),
        }
    } else if let Some(KObject::KExpression(inner)) = arg_object(ctx.args, "repr") {
        defer_resolved_sigil_action(name, inner.clone(), bind_index)
    } else {
        Action::Done(Err(KError::new(KErrorKind::ShapeError(
            "NEWTYPE repr slot must be a type expression (e.g. `Number`, `Foo`)".to_string(),
        ))))
    }
}

/// `Action`-side [`defer_resolved_sigil`]: re-wrap the captured sigil, sub-dispatch it, and seal a
/// plain Newtype over the resolved `KType` at Combine-finish.
#[cfg(feature = "action-harness")]
fn defer_resolved_sigil_action<'a>(
    name: String,
    inner: KExpression<'a>,
    bind_index: BindingIndex,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{
        body_result_to_action, Action, Cont, Dep, DepPlacement,
    };
    let wrapped = KExpression::new(vec![Spanned::bare(ExpressionPart::SigiledTypeExpr(Box::new(
        inner,
    )))]);
    let finish: Cont<'a> = Box::new(move |fctx, results| match results[0] {
        Carried::Type(kt) => {
            body_result_to_action(finalize_newtype(fctx.scope, name, kt.clone(), bind_index))
        }
        Carried::Object(other) => Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
            "NEWTYPE repr sigil resolved to a non-type value `{}`",
            other.ktype().name(),
        ))))),
    });
    Action::Combine {
        deps: vec![Dep::Dispatch {
            expr: wrapped,
            placement: DepPlacement::OwnScope,
        }],
        finish,
    }
}

/// `Action`-harness twin of [`body_record_repr`]: elaborate the `:{…}` field list (threading the
/// binder name + pending guard), folding via [`finalize_record_newtype`] or deferring through
/// [`defer_field_list_action`].
#[cfg(feature = "action-harness")]
pub fn body_record_repr_action<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{arg_object, require_bare_type_name, Action};
    use super::nominal_schema::nominal_schema_action;

    let name = crate::try_action!(require_bare_type_name(ctx.args, "name", "NEWTYPE"));
    let fields = match arg_object(ctx.args, "repr") {
        Some(KObject::KExpression(e)) => e.clone(),
        _ => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(
                "NEWTYPE record repr slot must be a record type `:{…}`".to_string(),
            ))))
        }
    };
    let error_frame = Frame::bare("<newtype>", format!("NEWTYPE {name}"));
    nominal_schema_action(
        ctx,
        name,
        fields,
        KKind::Newtype,
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
            KType::OfKind(KKind::Any),
            vec![
                kw("NEWTYPE"),
                arg("name", KType::OfKind(KKind::Proper)),
                kw("="),
                arg("repr", KType::OfKind(KKind::Proper)),
            ],
        )
    };
    let sigil_sig = || {
        sig(
            KType::OfKind(KKind::Any),
            vec![
                kw("NEWTYPE"),
                arg("name", KType::OfKind(KKind::Proper)),
                kw("="),
                arg("repr", KType::SigiledTypeExpr),
            ],
        )
    };
    let record_sig = || {
        sig(
            KType::OfKind(KKind::Any),
            vec![
                kw("NEWTYPE"),
                arg("name", KType::OfKind(KKind::Proper)),
                kw("="),
                arg("repr", KType::RecordType),
            ],
        )
    };
    #[cfg(feature = "action-harness")]
    {
        use crate::builtins::register_action_builtin_full;
        let binder = super::type_part_binder_name;
        register_action_builtin_full(scope, "NEWTYPE", scalar_sig(), body_action, Some(binder), None, false);
        register_action_builtin_full(scope, "NEWTYPE", sigil_sig(), body_action, Some(binder), None, false);
        register_action_builtin_full(
            scope,
            "NEWTYPE",
            record_sig(),
            body_record_repr_action,
            Some(binder),
            None,
            false,
        );
    }
    #[cfg(not(feature = "action-harness"))]
    {
        // Scalar / bare-leaf repr (`= Number`, `= Foo`) and non-record sigil repr (`= :(LIST OF T)`)
        // share `body`; the record repr (`= :{…}`) routes to `body_record_repr`.
        register_builtin_with_binder(scope, "NEWTYPE", scalar_sig(), body, Some(super::type_part_binder_name));
        register_builtin_with_binder(scope, "NEWTYPE", sigil_sig(), body, Some(super::type_part_binder_name));
        register_builtin_with_binder(
            scope,
            "NEWTYPE",
            record_sig(),
            body_record_repr,
            Some(super::type_part_binder_name),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::machine::execute::Scheduler;
    use crate::machine::model::types::{KKind, NominalSchema, ProjectedSchema, RecursiveSet};
    use crate::machine::model::{KObject, KType};
    use crate::machine::{KErrorKind, RuntimeArena, Scope};

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
                    Some(NominalSchema::Newtype(repr)) => match repr.as_ref() {
                        KType::Record(record) => {
                            let fields =
                                record.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                            (Rc::clone(set), fields)
                        }
                        other => panic!("expected {name} to carry a record repr, got {other:?}"),
                    },
                    other => panic!("expected {name} to carry a Newtype schema, got {other:?}"),
                }
            }
            other => panic!("expected {name} to be a SetRef identity, got {other:?}"),
        }
    }

    /// NEWTYPE writes the `SetRef` identity into `bindings.types` and nothing into
    /// `bindings.data` — the declaration has no payload value to bind.
    #[test]
    fn declare_mints_newtype_identity() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Distance = Number");
        let types = scope.bindings().types();
        let (kt, _) = types
            .get("Distance")
            .expect("Distance should be in bindings.types");
        match **kt {
            KType::SetRef { ref set, index } => {
                assert_eq!(set.member(index).name, "Distance");
                assert_eq!(set.member(index).kind, KKind::Newtype);
                match RecursiveSet::projected_schema(set, index) {
                    ProjectedSchema::Newtype(repr) => assert_eq!(repr, KType::Number),
                    _ => panic!("expected a Newtype schema"),
                }
            }
            ref other => panic!("expected Newtype SetRef identity, got {other:?}"),
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Distance = Number");
        let result = run_one(scope, parse_one("Distance (3.0)"));
        match result {
            KObject::Wrapped { inner, type_id } => {
                match **type_id {
                    KType::SetRef { ref set, index } => {
                        assert_eq!(set.member(index).name, "Distance");
                        assert_eq!(set.member(index).kind, KKind::Newtype);
                    }
                    ref other => panic!("expected Newtype SetRef type_id, got {other:?}"),
                }
                assert!(matches!(inner.get(), KObject::Number(n) if *n == 3.0));
            }
            other => panic!("expected Wrapped, got {:?}", other.ktype()),
        }
    }

    /// `Distance("hi")` (Number repr, Str value) surfaces as `TypeMismatch`.
    #[test]
    fn construct_rejects_non_matching_repr() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
            Some(&KType::List(Box::new(KType::SetLocal(tree_idx)))),
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "NEWTYPE Outer = :{inner :{owner :Outer}}");
        let (set, fields) = record_fields(scope, "Outer");
        let outer_idx = set.index_of("Outer").expect("Outer is its own set member");
        let inner_ty = fields
            .iter()
            .find(|(f, _)| f == "inner")
            .map(|(_, t)| t)
            .expect("inner field present");
        match inner_ty {
            KType::Record(rec) => assert_eq!(
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
    /// sigil to a resolved `KType` and seals a plain Newtype over it. Regression guard for the
    /// overload split — this used to ride the `:TypeExprRef` overload's speculative sub-dispatch.
    #[test]
    fn sigil_repr_non_record_seals_newtype_over_resolved_type() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
    /// Rejection lands as `DispatchFailed` out of `Scheduler::execute` (the per-slot
    /// type check filters the only candidate, scope chain runs out without a match)
    /// — drive the scheduler directly rather than `run_one_err`, which expects a
    /// per-slot Err result.
    #[test]
    fn dispatch_distinguishes_distance_from_number() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let mut sched1 = Scheduler::new();
        sched1.add_dispatch(parse_one("TAKES_NUM (Distance (3.0))"), scope);
        let err = sched1
            .execute()
            .expect_err("TAKES_NUM on Distance should fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on Number-slot Distance, got {err}",
        );
        let mut sched2 = Scheduler::new();
        sched2.add_dispatch(parse_one("TAKES_DIST (3.0)"), scope);
        let err2 = sched2
            .execute()
            .expect_err("TAKES_DIST on raw Number should fail dispatch");
        assert!(
            matches!(&err2.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed on Distance-slot Number, got {err2}",
        );
    }

    /// `Distance(x)` resolves the inner identifier inside the Combine's dispatched
    /// dep before the finish closure runs — pins the non-trivial-dispatch path.
    #[test]
    fn construct_with_identifier_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
    /// arithmetic operators today (per TUTORIAL.md § "No arithmetic, comparison, or
    /// logical operators"), so a user-fn call stands in for non-trivial dispatch.
    #[test]
    fn construct_with_operator_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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
