//! Shared deferral for typed field lists whose elaboration parked on a forward type or
//! scheduled sub-Dispatches for sigil field types — FN/FUNCTOR parameter lists, the
//! NEWTYPE record repr, the UNION schema, and the standalone record-type sigil.
//!
//! One Combine waits on `[park_producers ++ owned_subs]`; its finish re-walks the field
//! list through [`parse_typed_field_list_via_elaborator`], feeding the resolved
//! sub-Dispatch carriers back through that walker's `results` channel in DFS order, then
//! hands the sealed `(name, KType)` pairs to a caller-supplied `finalize` that folds them
//! into the right carrier (`KType::Record`, `KFunction`, union schema, …).

use std::rc::Rc;

use crate::machine::core::{LexicalFrame, PendingBinderGuard};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, FieldNameKind,
};
use crate::machine::model::KType;
use crate::machine::{
    BodyResult, CombineFinish, Frame, KError, KErrorKind, NodeId, SchedulerHandle, Scope,
};

/// Folds the elaborated `(name, KType)` pairs into the caller's carrier on the Combine's
/// `Done` arm.
pub(crate) type FieldListFinalize<'a> =
    Box<dyn FnOnce(&'a Scope<'a>, Vec<(String, KType<'a>)>) -> BodyResult<'a> + 'a>;

/// Schedule the sigil sub-Dispatches (in DFS order) and the Combine that re-walks `expr`
/// once they and `park_producers` resolve. `threaded` / `chain` rebuild the elaborator for
/// the re-walk; `pending_guard` (when present) rides into the closure so its Drop fires on
/// every finish arm; `error_frame` is attached to the user-facing `Err` arm.
#[allow(clippy::too_many_arguments)]
pub(crate) fn defer_field_list_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    expr: KExpression<'a>,
    park_producers: Vec<NodeId>,
    sub_dispatches: Vec<KExpression<'a>>,
    context: &'static str,
    name_kind: FieldNameKind,
    threaded: Vec<String>,
    chain: Option<Rc<LexicalFrame>>,
    pending_guard: Option<PendingBinderGuard<'a>>,
    error_frame: Option<Frame>,
    finalize: FieldListFinalize<'a>,
) -> BodyResult<'a> {
    let park_count = park_producers.len();
    let owned_subs: Vec<NodeId> = sub_dispatches
        .into_iter()
        .map(|sub| sched.add_dispatch(sub, scope))
        .collect();
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        // The guard's Drop clears the in-flight `pending_types` entry on every arm.
        let _pending_guard = pending_guard;
        // `results` = `[park results.. , owned-sub results..]`; the re-walk consumes only
        // the owned-sub carriers, in the DFS order they were scheduled above.
        let mut sub_results = results[park_count..].iter().copied();
        let mut elaborator = Elaborator::new(scope)
            .with_threaded(threaded.iter().cloned())
            .with_chain(chain.clone());
        match parse_typed_field_list_via_elaborator(
            &expr,
            context,
            name_kind,
            &mut elaborator,
            Some(&mut sub_results),
        ) {
            FieldListOutcome::Done(fields) => finalize(scope, fields),
            FieldListOutcome::Err(msg) => {
                let error = KError::new(KErrorKind::ShapeError(msg));
                BodyResult::Err(match error_frame {
                    Some(frame) => error.with_frame(frame),
                    None => error,
                })
            }
            // Every producer waited on is terminal by Combine invariant, so a second
            // park is a scheduling inconsistency rather than a recoverable forward ref.
            FieldListOutcome::Pending { .. } => BodyResult::Err(KError::new(KErrorKind::ShapeError(
                format!("{context}: forward type reference still unresolved after Combine wake"),
            ))),
        }
    });
    let combine_id = sched.add_combine(owned_subs, park_producers, scope, finish);
    BodyResult::DeferTo(combine_id)
}
