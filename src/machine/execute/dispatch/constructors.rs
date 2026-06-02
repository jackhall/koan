//! Struct + tagged-union construction dispatch. Both the
//! `ConstructorCall` fast lane (single_poll) and the `FunctionValueCall`
//! fast lane (fn_value) route a resolved verb-carrier here. Args resolve
//! through per-value eager sub-Dispatches; when all are bound, `construct`
//! validates types and emits the `KObject::Struct` / `KObject::Tagged`
//! directly — no bucket lookup, no `BodyResult::Tail` re-dispatch.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::kfunction::SchedulerHandle;
use crate::machine::core::source::Spanned;
use crate::machine::core::ScopeId;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::KType;
use crate::machine::model::{KObject, Record};
use crate::machine::{NodeId, Scope};

use super::super::nodes::{NodeOutput, NodeStep};
use super::single_poll::{CtorKind, CtorState, CtorTrack};
use super::{DispatchCtx, DispatchState, Initialized};

pub(in crate::machine::execute) mod struct_value;
pub(in crate::machine::execute) mod tagged_union;

/// Direct-construct a struct from the schema read off its `KType::UserType`
/// identity — `fields` came straight from `bindings.types[name]`, no value-side
/// schema carrier. Reorders the call-site args into declaration order, then
/// `launch`es the per-value eager subs.
#[allow(clippy::too_many_arguments)]
pub(in crate::machine::execute) fn dispatch_construct_struct<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    name: String,
    scope_id: ScopeId,
    fields: Rc<Record<KType<'a>>>,
    record_fields: Vec<(String, ExpressionPart<'a>)>,
    scope: &'a Scope<'a>,
    idx: usize,
) -> NodeStep<'a> {
    let value_parts = match struct_value::prepare_value_parts(&fields, record_fields) {
        Ok(v) => v,
        Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
    };
    launch(
        ctx,
        value_parts,
        CtorKind::Struct {
            name,
            scope_id,
            fields,
        },
        scope,
        idx,
    )
}

/// Direct-construct a tagged-union value from the schema read off its
/// `KType::UserType` identity. Shared by named UNIONs (`Tagged` kind) and the
/// builtin `Result` constructor (`TypeConstructor` kind) — both carry the schema
/// payload on the identity.
pub(in crate::machine::execute) fn dispatch_construct_tagged<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    name: String,
    scope_id: ScopeId,
    schema: Rc<HashMap<String, KType<'a>>>,
    args_parts: Vec<Spanned<ExpressionPart<'a>>>,
    scope: &'a Scope<'a>,
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
            name,
            scope_id,
            tag,
        },
        scope,
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
    let track = CtorTrack {
        subs,
        staged_values,
        kind,
    };
    let init = Initialized {
        pre_subs: Vec::new(),
    };
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
        CtorKind::Struct {
            name,
            scope_id,
            fields,
        } => struct_value::construct(name, *scope_id, fields, values),
        CtorKind::Tagged {
            schema,
            name,
            scope_id,
            tag,
        } => {
            debug_assert_eq!(values.len(), 1);
            tagged_union::construct(schema, name, *scope_id, tag.clone(), values[0])
        }
    };
    match result {
        Ok(obj) => NodeStep::Done(NodeOutput::Value(scope.arena.alloc(obj))),
        Err(e) => NodeStep::Done(NodeOutput::Err(e)),
    }
}
