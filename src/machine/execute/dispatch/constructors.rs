//! Struct + tagged-union construction dispatch. Both the
//! `ConstructorCall` fast lane (single_poll) and the `FunctionValueCall`
//! fast lane (fn_value) route a resolved verb-carrier here. Args resolve
//! through per-value eager sub-Dispatches; when all are bound, `construct`
//! validates types and emits the `KObject::Struct` / `KObject::Tagged`
//! directly — no bucket lookup, no `BodyResult::Tail` re-dispatch.

use std::rc::Rc;

use crate::machine::core::kfunction::SchedulerHandle;
use crate::machine::core::source::Spanned;
use crate::machine::model::Parseable;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::KObject;
use crate::machine::{KError, KErrorKind, NodeId, Scope};

use super::super::nodes::{NodeOutput, NodeStep};
use super::single_poll::{CtorKind, CtorState, CtorTrack};
use super::{DispatchCtx, DispatchState, Initialized};

pub(in crate::machine::execute) mod struct_value;
pub(in crate::machine::execute) mod tagged_union;

/// Branch on the resolved verb's carrier variant. `StructType` and
/// `TaggedUnionType` use the direct-construct path; other variants
/// surface a `TypeMismatch` (the caller usually filters these earlier).
pub(in crate::machine::execute) fn dispatch_construct<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    carrier: &'a KObject<'a>,
    args_parts: Vec<Spanned<ExpressionPart<'a>>>,
    scope: &'a Scope<'a>,
    idx: usize,
) -> NodeStep<'a> {
    match carrier {
        KObject::StructType { name, scope_id, fields } => {
            let name = name.clone();
            let scope_id = *scope_id;
            let fields = Rc::clone(fields);
            let value_parts = match struct_value::prepare_value_parts(&fields, args_parts) {
                Ok(v) => v,
                Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
            };
            launch(
                ctx,
                value_parts,
                CtorKind::Struct { name, scope_id, fields },
                scope,
                idx,
            )
        }
        KObject::TaggedUnionType { schema, name, scope_id } => {
            let schema = Rc::clone(schema);
            let name = name.clone();
            let scope_id = *scope_id;
            let (tag, value_part) = match tagged_union::prepare_args(args_parts) {
                Ok(v) => v,
                Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
            };
            launch(
                ctx,
                vec![value_part],
                CtorKind::Tagged { schema, name, scope_id, tag },
                scope,
                idx,
            )
        }
        other => NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::TypeMismatch {
            arg: "verb".to_string(),
            expected: "constructible Type".to_string(),
            got: other.summarize(),
        }))),
    }
}

/// Stage each value part as a sub-Dispatch (single-part `Expression`
/// wrapping routes through normal classification). If every sub
/// short-circuits at install time, construct in place; otherwise park as
/// a `CtorState` with an eager-subs track.
fn launch<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    value_parts: Vec<ExpressionPart<'a>>,
    kind: CtorKind<'a>,
    scope: &'a Scope<'a>,
    idx: usize,
) -> NodeStep<'a> {
    let n = value_parts.len();
    let mut staged_values: Vec<Option<&'a KObject<'a>>> = vec![None; n];
    let mut subs: Vec<(usize, NodeId)> = Vec::new();
    for (i, part) in value_parts.into_iter().enumerate() {
        let sub_expr = KExpression::new(vec![Spanned::bare(part)]);
        let sub_id = ctx.add_dispatch(sub_expr, scope);
        if ctx.is_result_ready(sub_id) {
            let outcome = ctx.read_result(sub_id);
            match outcome {
                Ok(v) => {
                    staged_values[i] = Some(v);
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
        return finish(scope, &kind, &values);
    }
    let track = CtorTrack { subs, staged_values, kind };
    let init = Initialized { pre_subs: Vec::new() };
    ctx.replace_with_parked_dispatch(DispatchState::ConstructorCall(CtorState::with_track(
        init, track,
    )))
}

/// All value subs have completed. Read each, materialize the kind-keyed
/// payload, and arena-allocate the produced `KObject`.
pub(in crate::machine::execute::dispatch) fn finish<'a>(
    scope: &'a Scope<'a>,
    kind: &CtorKind<'a>,
    values: &[&'a KObject<'a>],
) -> NodeStep<'a> {
    let result = match kind {
        CtorKind::Struct { name, scope_id, fields } => {
            struct_value::construct(name, *scope_id, fields, values)
        }
        CtorKind::Tagged { schema, name, scope_id, tag } => {
            debug_assert_eq!(values.len(), 1);
            tagged_union::construct(schema, name, *scope_id, tag.clone(), values[0])
        }
    };
    match result {
        Ok(obj) => NodeStep::Done(NodeOutput::Value(scope.arena.alloc(obj))),
        Err(e) => NodeStep::Done(NodeOutput::Err(e)),
    }
}
