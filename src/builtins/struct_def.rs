use crate::machine::core::{PendingBinderGuard, PendingTypeEntry};
use crate::machine::model::types::{
    finalize_nominal_member, parse_typed_field_list_via_elaborator, seal_recursive_refs,
    Elaborator, FieldListOutcome, FieldNameKind, NominalKind, NominalSchema, Record,
    SchemaSealResult, SealOutcome,
};
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, Frame, KError, KErrorKind, NodeId,
    SchedulerHandle, Scope,
};

use crate::machine::model::ast::KExpression;

use super::{arg, err, kw, register_builtin_with_binder, sig};
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
    // Mark this binder in-flight so a consumer referencing it (an earlier sibling still
    // finalizing) can park on our producer node. The guard's Drop removes the entry; the
    // Park path moves the guard into the Combine-finish closure.
    let scope_id = scope.id;
    let pending_guard = scope.bindings().insert_pending_type(
        name.clone(),
        PendingTypeEntry {
            kind: NominalKind::Struct,
            scope_id,
            schema_expr: schema_expr.clone(),
        },
    );
    // Thread this binder's name so a self-reference resolves to the transient
    // `RecursiveRef` rather than parking on our own placeholder. The chain gates field type
    // names to this binder's lexical position — a field naming a later type is a position
    // error, and mutual recursion is expressed with a `RECURSIVE TYPES` block.
    let chain = sched.current_lexical_chain();
    let mut elaborator = Elaborator::new(scope)
        .with_threaded([name.clone()])
        .with_chain(chain.clone());
    let outcome = parse_typed_field_list_via_elaborator(
        &schema_expr,
        "STRUCT schema",
        FieldNameKind::Identifier,
        &mut elaborator,
    );
    // The STRUCT name binds at its own lexical position (non-nominal), so a forward
    // reference to it obeys source order like any other type name.
    let bind_index = chain
        .as_ref()
        .map(|c| BindingIndex::value(c.index))
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
            chain,
        ),
    }
}

/// Seal the elaborated fields into the STRUCT's [`RecursiveSet`] member and install the
/// `SetRef` identity into `bindings.types` — type-only, no value-side carrier. Transient
/// `RecursiveRef(name)` field leaves seal to `SetLocal(index)` against the member's set
/// (the SCC set if recursive, a fresh singleton otherwise). Shared by the synchronous and
/// Combine-finish paths.
fn finalize_struct<'a>(
    scope: &'a Scope<'a>,
    name: String,
    fields: Vec<(String, KType<'a>)>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    if fields.is_empty() {
        return err(KError::new(KErrorKind::ShapeError(
            "STRUCT schema must have at least one field".to_string(),
        )));
    }
    let scope_id = scope.id;
    let outcome = finalize_nominal_member(
        scope,
        &name,
        scope_id,
        NominalKind::Struct,
        |set| {
            let missing = std::cell::RefCell::new(Vec::new());
            // The `Vec`→`Record` boundary: the parser hands back declaration-ordered pairs
            // (duplicate-free, `parse_pair_list` rejects dups), wrapped once here.
            let sealed_pairs: Vec<(String, KType<'a>)> = fields
                .into_iter()
                .map(|(field, kt)| (field, seal_recursive_refs(set, &kt, &missing)))
                .collect();
            let sealed = Record::from_pairs(sealed_pairs);
            match missing.into_inner().into_iter().next() {
                Some(m) => SchemaSealResult::Dangling(m),
                None => SchemaSealResult::Ok(NominalSchema::Struct(sealed)),
            }
        },
        bind_index,
    );
    match outcome {
        SealOutcome::Sealed(kt_ref) => BodyResult::Value(
            scope
                .arena
                .alloc_object(KObject::KTypeValue(kt_ref.clone())),
        ),
        SealOutcome::DanglingRef(missing) => err(KError::new(KErrorKind::ShapeError(format!(
            "STRUCT `{name}` schema references unsealed type `{missing}`",
        )))),
        SealOutcome::Rebind(e) => err(e),
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
    chain: Option<std::rc::Rc<crate::machine::core::LexicalFrame>>,
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
        // Re-elaborate at the binder's captured lexical position so the gate is identical
        // to the synchronous path.
        let mut elaborator = Elaborator::new(scope)
            .with_threaded([name_for_finish.clone()])
            .with_chain(chain.clone());
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
    register_builtin_with_binder(
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
