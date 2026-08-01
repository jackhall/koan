//! Shared `Action`-harness elaboration for a nominal type declarator's field-list schema —
//! the path UNION and NEWTYPE's record repr both walk: elaborate the `(tag/field :Type, …)` list
//! threading the binder name, then either fold the sealed pairs into the carrier synchronously or
//! defer one dep-finish over the parked producers + sigil sub-Dispatches.
//!
//! The two callers differ only in the parameters threaded through here (diagnostic context,
//! field-name policy, error frame) and the `finalize` that folds the sealed `(name, KType)` pairs
//! into the right carrier (`finalize_union` / `finalize_record_newtype`).

use crate::machine::core::bindings::WriteOp;
use crate::machine::model::KType;
use crate::machine::model::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListContext, FieldListOutcome,
    FieldNameKind, FieldParts,
};
use crate::machine::{Action, BodyCtx, FinishCtx};
use crate::machine::{DeclarationSite, KError, KErrorKind, TraceFrame};
use crate::machine::{FieldListDeferral, StepCarried};

/// Fold the sealed `(name, KType)` pairs into the declarator's carrier and the `types` write that
/// installs its identity; shared by the synchronous and dep-finish paths. A plain `fn` pointer (not a closure) so it rides both the eager arm
/// and the deferred finish without `Clone`.
pub(crate) type SchemaFinalize<'a> = fn(
    &FinishCtx<'a, '_>,
    String,
    std::rc::Rc<crate::machine::model::RecursiveGroupWindow>,
    Vec<(String, KType)>,
    DeclarationSite,
) -> Result<(StepCarried<'a>, Vec<WriteOp>), KError>;

/// Elaborate `schema_expr` as the named declarator's field list and fold or defer it.
/// `context` / `name_kind` / `error_frame` parameterize the diagnostic and seal shape; `finalize`
/// builds the carrier from the sealed pairs.
///
/// `window` is the declaration window the schema's co-declared references resolve against — the
/// enclosing `RECURSIVE TYPES` block's when this declaration is one of its members, else one this
/// declaration opens and seals itself.
#[allow(clippy::too_many_arguments)]
pub(crate) fn nominal_schema_action<'a>(
    ctx: &BodyCtx<'a, '_>,
    name: String,
    window: std::rc::Rc<crate::machine::model::RecursiveGroupWindow>,
    schema_expr: crate::machine::model::KExpression<'a>,
    context: FieldListContext,
    name_kind: FieldNameKind,
    error_frame: TraceFrame,
    finalize: SchemaFinalize<'a>,
) -> Action<'a> {
    let site = ctx.declaration_site();
    let chain = ctx.chain.clone();
    // Seed the threaded set with this binder's name so a self-recursive declaration resolves
    // through the window rather than parking on its own placeholder.
    let mut elaborator = Elaborator::new(ctx.scope)
        .with_threaded([name.clone()])
        .with_window(std::rc::Rc::clone(&window))
        .with_chain(chain.clone());
    match parse_typed_field_list_via_elaborator(
        FieldParts::of(&schema_expr),
        context,
        name_kind,
        &mut elaborator,
        None,
        ctx.types,
    ) {
        FieldListOutcome::Done(fields) => {
            Action::done_writing(finalize(&ctx.finish_ctx(), name, window, fields, site))
        }
        FieldListOutcome::Err(msg) => Action::done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => {
            let finish_name = name.clone();
            let finish_window = std::rc::Rc::clone(&window);
            FieldListDeferral::new(
                FieldParts::of(&schema_expr),
                park_producers,
                sub_dispatches,
                context,
                name_kind,
            )
            .with_threaded([name])
            .with_window(window)
            .with_chain(chain)
            .with_error_frame(error_frame)
            .action(Box::new(move |fctx, fields| {
                finalize(fctx, finish_name, finish_window, fields, site)
            }))
        }
    }
}
