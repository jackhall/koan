//! Shared `Action`-harness elaboration for a nominal type declarator's field-list schema —
//! the path UNION and NEWTYPE's record repr both walk: mark the binder in-flight, elaborate the
//! `(tag/field :Type, …)` list threading the binder name, then either fold the sealed pairs into
//! the carrier synchronously or defer one dep-finish over the parked producers + sigil sub-Dispatches.
//!
//! The two callers differ only in the parameters threaded through here (kind, diagnostic context,
//! field-name policy, error frame) and the `finalize` that folds the sealed `(name, KType)` pairs
//! into the right carrier (`finalize_union` / `finalize_record_newtype`).

use crate::machine::core::kfunction::action::{Action, BodyCtx, FinishCtx};
use crate::machine::core::PendingTypeEntry;
use crate::machine::execute::defer_field_list_action;
use crate::machine::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, FieldNameKind, KKind,
};
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::KType;
use crate::machine::DeliveredCarried;
use crate::machine::{BindingIndex, CarrierWitness, KError, KErrorKind, TraceFrame};
use crate::witnessed::Witnessed;

/// Fold the sealed `(name, KType)` pairs into the declarator's carrier; shared by the synchronous
/// and dep-finish paths. A plain `fn` pointer (not a closure) so it rides both the eager arm
/// and the deferred finish without `Clone`. The trailing slice is the dep carriers (parks then
/// owned) the field-list walk resolved — `&[]` on the synchronous arm, which has none.
pub(crate) type SchemaFinalize<'a> = fn(
    &FinishCtx<'a>,
    String,
    Vec<(String, KType<'a>)>,
    BindingIndex,
    &[&DeliveredCarried],
)
    -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError>;

/// Elaborate `schema_expr` as the named declarator's field list and fold or defer it. `kind` /
/// `context` / `name_kind` / `error_frame` parameterize the diagnostic and seal shape; `finalize`
/// builds the carrier from the sealed pairs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn nominal_schema_action<'a>(
    ctx: &BodyCtx<'a, '_>,
    name: String,
    schema_expr: crate::machine::model::ast::KExpression<'a>,
    kind: KKind,
    context: &'static str,
    name_kind: FieldNameKind,
    error_frame: TraceFrame,
    finalize: SchemaFinalize<'a>,
) -> Action<'a> {
    let bind_index = ctx.bind_index();
    let chain = ctx.chain.clone();
    // Mark this binder in-flight so a consumer referencing it (an earlier sibling still finalizing)
    // can park on our producer node. The guard's Drop removes the entry; the Pending path moves it
    // into the dep-finish closure.
    let pending_guard = ctx.scope.bindings().insert_pending_type(
        name.clone(),
        PendingTypeEntry {
            kind,
            scope_id: ctx.scope.id,
            schema_expr: schema_expr.clone(),
        },
    );
    // Seed the threaded set with this binder's name so a self-recursive declaration resolves to the
    // transient `RecursiveRef` rather than parking on its own placeholder.
    let mut elaborator = Elaborator::new(ctx.scope)
        .with_threaded([name.clone()])
        .with_chain(chain.clone());
    match parse_typed_field_list_via_elaborator(
        &schema_expr,
        context,
        name_kind,
        &mut elaborator,
        None,
    ) {
        FieldListOutcome::Done(fields) => {
            Action::Done(finalize(&ctx.finish_ctx(), name, fields, bind_index, &[]))
        }
        FieldListOutcome::Err(msg) => Action::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => {
            let finish_name = name.clone();
            defer_field_list_action(
                schema_expr,
                park_producers,
                sub_dispatches,
                context,
                name_kind,
                vec![name],
                chain,
                Some(pending_guard),
                Some(error_frame),
                Box::new(move |fctx, fields, carriers| {
                    finalize(fctx, finish_name, fields, bind_index, carriers)
                }),
            )
        }
    }
}
