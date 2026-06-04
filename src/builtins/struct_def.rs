use std::rc::Rc;

use crate::machine::core::{PendingBinderGuard, PendingTypeEntry};
use crate::machine::model::types::UserTypeKind;
use crate::machine::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, FieldNameKind,
};
use crate::machine::model::{KObject, KType, Record};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, Frame, KError, KErrorKind, NodeId,
    SchedulerHandle, Scope,
};

use crate::machine::model::ast::KExpression;

use super::{arg, err, kw, register_nominal_binder, sig};
use crate::machine::core::kfunction::argument_bundle::{
    extract_bare_type_name, extract_kexpression,
};

/// `STRUCT <name:TypeExprRef> = (<schema>)` — declare a named record type.
///
/// Schema is a parens-wrapped expression of `<field:Identifier> :<type:Type>` pairs.
/// Order is preserved so `struct_value::apply` can canonicalize named-arg pairs at
/// construction. Empty schemas, unknown type names, duplicate fields, and malformed
/// triples surface as `ShapeError`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match extract_bare_type_name(&bundle, "name", "STRUCT") {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let schema_expr = match extract_kexpression(&mut bundle, "schema") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "STRUCT schema slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    // Register in `pending_types` so a fellow in-flight binder parking on our
    // placeholder can detect a closing cycle and install our identity without
    // re-entering dispatch. The guard's Drop removes the entry; the Park path
    // moves the guard into the Combine-finish closure.
    let scope_id = scope.id;
    let pending_guard = scope.bindings().insert_pending_type(
        name.clone(),
        PendingTypeEntry {
            kind: UserTypeKind::struct_sentinel(),
            scope_id,
            schema_expr: schema_expr.clone(),
            edges: Vec::new(),
        },
    );
    // Thread this binder's name so a self-reference resolves to `RecursiveRef`
    // rather than parking on our own placeholder. `with_current_decl` arms the
    // SCC edge-recording / cycle-detection arm.
    let mut elaborator = Elaborator::new(scope)
        .with_threaded([name.clone()])
        .with_current_decl(name.clone(), UserTypeKind::struct_sentinel(), scope_id);
    let outcome = parse_typed_field_list_via_elaborator(
        &schema_expr,
        "STRUCT schema",
        FieldNameKind::Identifier,
        &mut elaborator,
    );
    // Nominal binder: the placeholder install stamped `nominal_binder: true`;
    // the type-only `register_type_upsert` must carry the same flag for visibility
    // consistency.
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::nominal(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    match outcome {
        FieldListOutcome::Done(fields) => finalize_struct(scope, name, fields, bind_index),
        FieldListOutcome::Err(msg) => err(KError::new(KErrorKind::ShapeError(msg))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => defer_struct_via_combine(
            scope,
            sched,
            name,
            schema_expr,
            park_producers,
            sub_dispatches,
            pending_guard,
            bind_index,
        ),
    }
}

/// Fold the elaborated fields into the `UserType { Struct { fields } }` identity and
/// upsert it into `bindings.types` — type-only, no value-side carrier. The schema rides
/// the identity, so construction reads it via a fresh `types[name]` lookup. Shared by the
/// synchronous and Combine-finish paths.
fn finalize_struct<'a>(
    scope: &'a Scope<'a>,
    name: String,
    fields: Vec<(String, KType<'a>)>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    // Idempotent-finalize guard: a parallel finalize (cycle-close + Combine-finish, or
    // two Combine-finishes) may already have upserted the schema-bearing identity. The
    // cycle-close pre-install carries a payload-empty identity, so the guard must
    // distinguish them: only short-circuit on a populated `Struct { fields }` payload.
    let bindings = scope.bindings();
    if let Some(KType::UserType {
        kind: UserTypeKind::Struct { fields },
        ..
    }) = bindings.lookup_type(&name, None)
    {
        if !fields.is_empty() {
            return BodyResult::Value(scope.arena.alloc_object(KObject::KTypeValue(
                bindings.lookup_type(&name, None).unwrap().clone(),
            )));
        }
    }
    if fields.is_empty() {
        return err(KError::new(KErrorKind::ShapeError(
            "STRUCT schema must have at least one field".to_string(),
        )));
    }
    let scope_id = scope.id;
    let identity = KType::UserType {
        kind: UserTypeKind::Struct {
            // The `Vec`→`Record` boundary: the parser hands back declaration-ordered
            // pairs (duplicate-free, `parse_pair_list` rejects dups), wrapped once here.
            fields: Rc::new(Record::from_pairs(fields)),
        },
        scope_id,
        name: name.clone(),
    };
    match scope.register_type_upsert(name, identity, bind_index) {
        Ok(kt_ref) => BodyResult::Value(
            scope
                .arena
                .alloc_object(KObject::KTypeValue(kt_ref.clone())),
        ),
        Err(e) => err(e),
    }
}

/// Schedule a `Combine` over `park_producers` plus owned sub-Dispatches for
/// sigiled-type-expression slots (`xs :(LIST OF Number)`), then re-run schema
/// elaboration in the finish closure.
///
/// Combine layout: `[park_producers ++ owned_subs...]`. `splice_layout[k] =
/// (slot_idx, results_pos)` splices `results[results_pos]` into
/// `schema_expr.parts[slot_idx]` as `Future(_)` before the re-walk. `pending_guard`
/// moves into the closure so its Drop fires regardless of which finish arm runs.
#[allow(clippy::too_many_arguments)]
fn defer_struct_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    name: String,
    schema_expr: KExpression<'a>,
    park_producers: Vec<NodeId>,
    sub_dispatches: Vec<(usize, KExpression<'a>)>,
    pending_guard: PendingBinderGuard<'a>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    use crate::machine::model::ast::ExpressionPart;
    let name_for_finish = name.clone();
    // Build splice_layout before scheduling so each sub-Dispatch's `results_pos`
    // matches its position in the combined deps vector.
    let park_count = park_producers.len();
    let mut owned_subs: Vec<NodeId> = Vec::with_capacity(sub_dispatches.len());
    let mut splice_layout: Vec<(usize, usize)> = Vec::with_capacity(sub_dispatches.len());
    for (slot_idx, sub_expr) in sub_dispatches {
        let id = sched.add_dispatch(sub_expr, scope);
        splice_layout.push((slot_idx, park_count + owned_subs.len()));
        owned_subs.push(id);
    }
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        // Hold the guard so its Drop runs on any closure exit.
        let _pending_guard = pending_guard;
        // Splice sub-Dispatch results into the schema as `Future(_)` carriers
        // for the re-walk's `Future(KTypeValue(_))` arm.
        let mut spliced_parts = schema_expr.parts.clone();
        for &(slot_idx, results_pos) in &splice_layout {
            let obj = results[results_pos];
            if !matches!(obj, KObject::KTypeValue(_)) {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "STRUCT schema slot at part-index {slot_idx} expected a type \
                     expression, got a {} value",
                    obj.ktype().name(),
                ))));
            }
            spliced_parts[slot_idx].value = ExpressionPart::Future(obj);
        }
        let spliced_schema = KExpression::new(spliced_parts);
        // All producers have terminalized; no `current_decl` seeding needed since
        // cycle detection only matters for in-flight binders that might park.
        let mut elaborator = Elaborator::new(scope).with_threaded([name_for_finish.clone()]);
        match parse_typed_field_list_via_elaborator(
            &spliced_schema,
            "STRUCT schema",
            FieldNameKind::Identifier,
            &mut elaborator,
        ) {
            FieldListOutcome::Done(fields) => {
                finalize_struct(scope, name_for_finish.clone(), fields, bind_index)
            }
            FieldListOutcome::Err(msg) => BodyResult::Err(
                KError::new(KErrorKind::ShapeError(msg)).with_frame(Frame::bare(
                    "<struct>",
                    format!("STRUCT {} schema", name_for_finish),
                )),
            ),
            FieldListOutcome::Pending { .. } => {
                BodyResult::Err(KError::new(KErrorKind::ShapeError(
                    "STRUCT schema elaboration parked again after Combine wake".to_string(),
                )))
            }
        }
    });
    let combine_id = sched.add_combine(owned_subs, park_producers, scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Dispatch-time placeholder extractor: pulls the bare-leaf name from the
/// `Type(_)` token at `parts[1]`. Parameterized forms aren't supported until
/// functors land.
pub(crate) fn binder_name(expr: &KExpression<'_>) -> Option<String> {
    expr.binder_name_from_type_part()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_nominal_binder(
        scope,
        "STRUCT",
        sig(
            KType::Type,
            vec![
                kw("STRUCT"),
                arg("name", KType::TypeExprRef),
                kw("="),
                arg("schema", KType::KExpression),
            ],
        ),
        body,
        Some(binder_name),
    );
}

#[cfg(test)]
mod tests;
