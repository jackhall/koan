//! Shared `Action`-harness elaboration for a nominal type declarator's field-list schema:
//! elaborate the `(tag/field :Type, …)` list threading the binder name, then either fold the
//! sealed pairs into the carrier synchronously or defer one dep-finish over the parked binder
//! claims + sigil sub-Dispatches.
//!
//! A declarator states the parameters threaded through here (diagnostic context, field-name
//! policy, error frame) and the `finalize` that folds the sealed `(name, KType)` pairs into its
//! own carrier.

use crate::machine::core::bindings::WriteOp;
use crate::machine::model::DeclWindow;
use crate::machine::model::KType;
use crate::machine::model::{
    Elaborator, FieldListContext, FieldListOutcome, FieldNameKind, FieldParts,
    parse_typed_field_list_via_elaborator,
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
    &DeclWindow<'a>,
    Vec<(String, KType)>,
    DeclarationSite,
) -> Result<(StepCarried<'a>, Vec<WriteOp<'a>>), KError>;

/// Elaborate `schema_expr` as the named declarator's field list and fold or defer it.
/// `context` / `name_kind` / `error_frame` parameterize the diagnostic and seal shape; `finalize`
/// builds the carrier from the sealed pairs.
///
/// `window` is the declaration window the schema's co-declared references resolve against — the
/// enclosing module body's announced one when this declaration is one of its members, else one this
/// declaration opens and seals itself.
#[allow(clippy::too_many_arguments)]
pub(crate) fn nominal_schema_action<'a>(
    ctx: &BodyCtx<'_, 'a, '_>,
    name: String,
    window: DeclWindow<'a>,
    schema_expr: crate::machine::model::KExpression<'a>,
    context: FieldListContext,
    name_kind: FieldNameKind,
    error_frame: TraceFrame,
    finalize: SchemaFinalize<'a>,
) -> Action<'a> {
    let site = ctx.declaration_site();
    let chain = ctx.chain.clone();
    // Seed the threaded set with this binder's name and every other name the window announces, so
    // a reference to any of them resolves through the window rather than parking on a placeholder
    // — and, for a sigil field whose body sub-dispatches into the window-less standalone
    // dispatcher, is pre-resolved to a sibling cell before it leaves.
    let outcome = {
        let mut elaborator = Elaborator::new(ctx.scope)
            .with_threaded(std::iter::once(name.clone()).chain(window.view().threadable_names()))
            .with_window(window.view())
            .with_chain(chain.clone());
        parse_typed_field_list_via_elaborator(
            FieldParts::of(&schema_expr),
            context,
            name_kind,
            &mut elaborator,
            None,
            ctx.types(),
        )
    };
    match outcome {
        FieldListOutcome::Done(fields) => {
            Action::done_writing(finalize(&ctx.finish_ctx(), name, &window, fields, site))
        }
        FieldListOutcome::Err(msg) => Action::done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            awaited_producers,
            sub_dispatches,
        } => {
            let finish_name = name.clone();
            let threaded: Vec<String> = std::iter::once(name)
                .chain(window.view().threadable_names())
                .collect();
            FieldListDeferral::new(
                FieldParts::of(&schema_expr),
                awaited_producers,
                sub_dispatches,
                context,
                name_kind,
            )
            .with_threaded(threaded)
            .with_window(window)
            .with_chain(chain)
            .with_error_frame(error_frame)
            .action(Box::new(move |fctx, window, fields| {
                let window = window.expect("a nominal declarator always carries its window");
                finalize(fctx, finish_name, window, fields, site)
            }))
        }
    }
}
