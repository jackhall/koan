//! Newtype + tagged-union construction dispatch. Both the `TypeCall` fast lane (single_poll)
//! and the `FunctionValueCall` fast lane (fn_value) route a resolved verb-carrier here. Args
//! resolve through per-value eager sub-Dispatches; when all are bound, `finish` validates
//! types and emits the `KObject::Wrapped` / `KObject::Tagged` directly — no bucket lookup, no
//! `BodyResult::Tail` re-dispatch. Reusing the eager-subs `CtorState` machinery (rather than a
//! standalone `Combine`) is load-bearing: it stages an already-ready value in place and parks
//! a deferred one on the construction node itself, so a newtype built from a still-pending
//! reference (`(Boxed (p))` where `p` is a sibling construction) finalizes correctly.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::kfunction::SchedulerHandle;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{KType, ProjectedSchema, RecursiveSet};
use crate::machine::model::values::NonWrappedRef;
use crate::machine::model::KObject;
use crate::machine::{KError, KErrorKind, NodeId, Scope};

use super::super::nodes::{NodeOutput, NodeStep};
use super::single_poll::{CtorKind, CtorState, CtorTrack};
use super::{DispatchCtx, DispatchState, Initialized};

pub(in crate::machine::execute) mod tagged_union;

/// Construct a newtype value (record-repr or scalar). `value_parts` is the whole value
/// expression (`expr.parts[1..]`); a single redundant `(...)` paren group unwraps so
/// `(Distance 3.0)` / `Distance (3.0)` construct identically and `Distance ()` is arity-zero.
/// The parts are launched as one value cell whose finish type-checks against the member's
/// `repr` and wraps with `identity`.
pub(in crate::machine::execute) fn dispatch_construct_newtype<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    identity: &'a KType<'a>,
    mut value_parts: Vec<Spanned<ExpressionPart<'a>>>,
    idx: usize,
) -> NodeStep<'a> {
    if let [Spanned {
        value: ExpressionPart::Expression(inner),
        ..
    }] = value_parts.as_slice()
    {
        value_parts = inner.parts.clone();
    }
    if value_parts.is_empty() {
        return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::ArityMismatch {
            expected: 1,
            got: 0,
        })));
    }
    // One value cell. A single part dispatches directly (a bare `(p)` reference resolves
    // in place when ready, the way tagged construction dispatches its lone value); a
    // multi-part value (`Bar (Foo 3.0)`) is wrapped so `launch` dispatches it as one unit.
    let value_cell = if value_parts.len() == 1 {
        value_parts.into_iter().next().expect("len == 1").value
    } else {
        ExpressionPart::Expression(Box::new(KExpression::new(value_parts)))
    };
    launch(ctx, vec![value_cell], CtorKind::Newtype { identity }, idx)
}

/// Direct-construct a record-repr newtype from a named record-literal body. Launches one
/// value cell per field — a literal field stages in place, so a record over literal fields
/// binds synchronously (the property the retired struct path relied on, and which a chained
/// `(Boxed (p))` depends on). The finish builds the `KObject::Record` and wraps it.
pub(in crate::machine::execute) fn dispatch_construct_record_newtype<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    identity: &'a KType<'a>,
    record_fields: Vec<(String, ExpressionPart<'a>)>,
    idx: usize,
) -> NodeStep<'a> {
    let field_names: Vec<String> = record_fields.iter().map(|(n, _)| n.clone()).collect();
    let value_parts: Vec<ExpressionPart<'a>> = record_fields.into_iter().map(|(_, p)| p).collect();
    launch(
        ctx,
        value_parts,
        CtorKind::RecordNewtype {
            identity,
            field_names,
        },
        idx,
    )
}

/// Type-check `value` against the newtype member's projected `repr` and wrap it with
/// `identity`, peeling any `Wrapped` layer (the single-layer collapse invariant).
fn construct_newtype<'a>(
    identity: &'a KType<'a>,
    value: &KObject<'a>,
) -> Result<KObject<'a>, KError> {
    let (set, index) = match identity {
        KType::SetRef { set, index } => (set, *index),
        _ => unreachable!("TypeCall fast lane routed a non-SetRef identity into newtype construct"),
    };
    let repr = match RecursiveSet::projected_schema(set, index) {
        ProjectedSchema::Newtype(repr) => repr,
        _ => unreachable!("newtype construct ran on a non-Newtype member"),
    };
    if !repr.matches_value(value) {
        return Err(KError::new(KErrorKind::TypeMismatch {
            arg: "value".to_string(),
            expected: repr.name(),
            got: value.ktype().name(),
        }));
    }
    Ok(KObject::Wrapped {
        inner: NonWrappedRef::peel(value),
        type_id: identity,
    })
}

/// Direct-construct a tagged-union value from the projected schema of its sealed
/// `RecursiveSet` member. Shared by named UNIONs (`Tagged` kind) and the builtin `Result`
/// constructor (`TypeConstructor` kind) — both reference a sealed member.
#[allow(clippy::too_many_arguments)]
pub(in crate::machine::execute) fn dispatch_construct_tagged<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    set: Rc<RecursiveSet<'a>>,
    index: usize,
    schema: Rc<HashMap<String, KType<'a>>>,
    args_parts: Vec<Spanned<ExpressionPart<'a>>>,
    idx: usize,
) -> NodeStep<'a> {
    let (tag, value_part) = match tagged_union::prepare_args(args_parts) {
        Ok(v) => v,
        Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
    };
    launch(
        ctx,
        vec![value_part],
        CtorKind::Tagged {
            schema,
            set,
            index,
            tag,
        },
        idx,
    )
}

/// Stage each value part as a sub-Dispatch (single-part `Expression`
/// wrapping routes through normal classification). If every sub
/// short-circuits at install time, construct in place; otherwise park as
/// a `CtorState` with an eager-subs track.
fn launch<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    value_parts: Vec<ExpressionPart<'a>>,
    kind: CtorKind<'a>,
    idx: usize,
) -> NodeStep<'a> {
    let n = value_parts.len();
    let mut staged_values: Vec<Option<&'a KObject<'a>>> = vec![None; n];
    let mut subs: Vec<(usize, NodeId)> = Vec::new();
    for (i, part) in value_parts.into_iter().enumerate() {
        let sub_expr = KExpression::new(vec![Spanned::bare(part)]);
        let sub_id = ctx.add_dispatch_here(sub_expr);
        if ctx.is_result_ready(sub_id) {
            let outcome = ctx.read_result(sub_id);
            match outcome {
                Ok(v) => {
                    staged_values[i] = Some(v.object());
                    ctx.free(sub_id.index());
                }
                Err(e) => {
                    let err = e.clone_for_propagation();
                    ctx.free(sub_id.index());
                    return NodeStep::Done(NodeOutput::Err(err));
                }
            }
        } else {
            ctx.add_owned_edge(sub_id, NodeId(idx));
            subs.push((i, sub_id));
        }
    }
    if subs.is_empty() {
        let values: Vec<&'a KObject<'a>> = staged_values.into_iter().map(|o| o.unwrap()).collect();
        return finish(ctx.current_scope(), &kind, &values);
    }
    let track = CtorTrack {
        subs,
        staged_values,
        kind,
    };
    let init = Initialized {
        pre_subs: Vec::new(),
    };
    ctx.replace_with_parked_dispatch(DispatchState::TypeCall(Box::new(CtorState::with_track(
        init, track,
    ))))
}

/// All value subs have completed. Read each, materialize the kind-keyed
/// payload, and arena-allocate the produced `KObject`.
pub(in crate::machine::execute::dispatch) fn finish<'a>(
    scope: &Scope<'a>,
    kind: &CtorKind<'a>,
    values: &[&'a KObject<'a>],
) -> NodeStep<'a> {
    let result = match kind {
        CtorKind::Newtype { identity } => {
            debug_assert_eq!(values.len(), 1);
            construct_newtype(identity, values[0])
        }
        CtorKind::RecordNewtype {
            identity,
            field_names,
        } => {
            let record = crate::machine::model::Record::from_pairs(
                field_names
                    .iter()
                    .cloned()
                    .zip(values.iter().map(|v| v.deep_clone())),
            );
            construct_newtype(identity, &KObject::record(record))
        }
        CtorKind::Tagged {
            schema,
            set,
            index,
            tag,
        } => {
            debug_assert_eq!(values.len(), 1);
            tagged_union::construct(schema, set, *index, tag.clone(), values[0])
        }
    };
    match result {
        Ok(obj) => NodeStep::Done(NodeOutput::value(scope.arena.alloc_object(obj))),
        Err(e) => NodeStep::Done(NodeOutput::Err(e)),
    }
}
