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
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, FieldNameKind, ResultFeed,
};
use crate::machine::model::{KType, Record};
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
    sched: &mut dyn SchedulerHandle<'a, 'a>,
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
        let mut feed = ResultFeed::new(&results[park_count..]);
        let mut elaborator = Elaborator::new(scope)
            .with_threaded(threaded.iter().cloned())
            .with_chain(chain.clone());
        match parse_typed_field_list_via_elaborator(
            &expr,
            context,
            name_kind,
            &mut elaborator,
            Some(&mut feed),
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
            FieldListOutcome::Pending { .. } => {
                BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "{context}: forward type reference still unresolved after Combine wake"
                ))))
            }
        }
    });
    let combine_id = sched.add_combine(owned_subs, park_producers, scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Elaborate a standalone `:{…}` record type to `KObject::KTypeValue(KType::Record(_))`.
/// The `fields` expression is the record's `(name :Type, …)` field list. A record type at a
/// value/type position declares no binder, so the elaborator threads no self-reference; a
/// field naming a forward type parks and a sigil field type sub-dispatches, both deferred
/// through one Combine (the field walker's own re-walk handles nested records).
pub(crate) fn elaborate_record_value<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a, 'a>,
    fields: KExpression<'a>,
    chain: Option<Rc<LexicalFrame>>,
) -> BodyResult<'a> {
    fn fold<'a>(scope: &'a Scope<'a>, pairs: Vec<(String, KType<'a>)>) -> BodyResult<'a> {
        let record = Record::from_pairs(pairs);
        BodyResult::ktype(scope.arena.alloc_ktype(KType::Record(Box::new(record))))
    }
    let mut elaborator = Elaborator::new(scope).with_chain(chain.clone());
    match parse_typed_field_list_via_elaborator(
        &fields,
        "record fields",
        FieldNameKind::Identifier,
        &mut elaborator,
        None,
    ) {
        FieldListOutcome::Done(pairs) => fold(scope, pairs),
        FieldListOutcome::Err(msg) => BodyResult::Err(KError::new(KErrorKind::ShapeError(msg))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => defer_field_list_via_combine(
            scope,
            sched,
            fields,
            park_producers,
            sub_dispatches,
            "record fields",
            FieldNameKind::Identifier,
            Vec::new(),
            chain,
            None,
            None,
            Box::new(fold),
        ),
    }
}
