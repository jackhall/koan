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
use crate::machine::{KError, KErrorKind, Scope};

use super::super::nodes::{DispatchCombineFinish, NodeOutput, NodeStep};
use super::outcome::{DispatchDep, DispatchOutcome};
use super::single_poll::CtorKind;
use super::{harness, DispatchCtx};

pub(in crate::machine::execute) mod tagged_union;

/// Construct a newtype value (record-repr or scalar). `value_parts` is the whole value
/// expression (`expr.parts[1..]`); a single redundant `(...)` paren group unwraps so
/// `(Distance 3.0)` / `Distance (3.0)` construct identically and `Distance ()` is arity-zero.
/// The parts are launched as one value cell whose finish type-checks against the member's
/// `repr` and wraps with `identity`.
pub(in crate::machine::execute) fn dispatch_construct_newtype<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    identity: &'run KType<'run>,
    mut value_parts: Vec<Spanned<ExpressionPart<'run>>>,
    idx: usize,
) -> NodeStep<'run> {
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
pub(in crate::machine::execute) fn dispatch_construct_record_newtype<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    identity: &'run KType<'run>,
    record_fields: Vec<(String, ExpressionPart<'run>)>,
    idx: usize,
) -> NodeStep<'run> {
    let field_names: Vec<String> = record_fields.iter().map(|(n, _)| n.clone()).collect();
    let value_parts: Vec<ExpressionPart<'run>> =
        record_fields.into_iter().map(|(_, p)| p).collect();
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
fn construct_newtype<'run>(
    identity: &'run KType<'run>,
    value: &KObject<'run>,
) -> Result<KObject<'run>, KError> {
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
pub(in crate::machine::execute) fn dispatch_construct_tagged<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    set: Rc<RecursiveSet<'run>>,
    index: usize,
    schema: Rc<HashMap<String, KType<'run>>>,
    args_parts: Vec<Spanned<ExpressionPart<'run>>>,
    idx: usize,
) -> NodeStep<'run> {
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

/// Decide a constructor park: every value part is a fresh sub-Dispatch dep (a single-part
/// `Expression` wrapping routes through normal classification), and a freshly-minted sub is never
/// terminal in the same step (submission is enqueue-then-drain), so there is no inline-ready case —
/// the slot always parks as a [`DispatchOutcome::Combine`]. The finish reads the resolved deps in
/// declaration order and materializes the value via [`finish`]; dep errors propagate frameless.
fn launch<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    value_parts: Vec<ExpressionPart<'run>>,
    kind: CtorKind<'run>,
    idx: usize,
) -> NodeStep<'run> {
    debug_assert!(
        !value_parts.is_empty(),
        "launch requires at least one value part (arity-zero is rejected upstream)"
    );
    let deps: Vec<DispatchDep<'run>> = value_parts
        .into_iter()
        .map(|part| DispatchDep::Dispatch(KExpression::new(vec![Spanned::bare(part)])))
        .collect();
    let combine_finish: DispatchCombineFinish<'run> = Box::new(move |ctx, results, _idx| {
        let values: Vec<&'run KObject<'run>> = results.iter().map(|c| c.object()).collect();
        finish(ctx.current_scope(), &kind, &values)
    });
    let outcome = DispatchOutcome::Combine {
        deps,
        dep_error_frame: None,
        finish: combine_finish,
    };
    harness::apply_dispatch_outcome(ctx, outcome, idx)
}

/// All value subs have completed. Read each, materialize the kind-keyed
/// payload, and arena-allocate the produced `KObject`.
pub(in crate::machine::execute::dispatch) fn finish<'run>(
    scope: &Scope<'run>,
    kind: &CtorKind<'run>,
    values: &[&'run KObject<'run>],
) -> NodeStep<'run> {
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
