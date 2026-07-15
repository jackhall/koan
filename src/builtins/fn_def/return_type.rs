//! FN return-type pipeline: extraction → classification → carriage across the
//! dep-finish boundary → resolution at finish time.

use crate::builtins::resolve_or_await::{
    classify_type_hit, expect_type_terminal, resolve_at_wake, unbound_error,
};
use crate::machine::model::TypeResolution;
use crate::machine::model::{DeferredReturn, ReturnType};
use crate::machine::model::{KExpression, TypeIdentifier};
use crate::machine::model::{KObject, KType};
use crate::machine::DepTerminal;
use crate::machine::LexicalFrame;
use crate::machine::{KError, KErrorKind, NodeId, Scope};
use crate::scheduler::DepResults;
use std::rc::Rc;

use super::param_refs::{kexpression_references_any, type_expr_references_any};

/// `ExprCarrier` is captured raw rather than sub-dispatched in the outer scope because a
/// `:(…)` / dotted return's inner expression may reference a parameter unbound there.
pub(crate) enum ReturnTypeRaw<'a> {
    Resolved(KType<'a>),
    TypeExprCarrier(TypeIdentifier),
    ExprCarrier(KExpression<'a>),
}

/// `Deferred` skips the outer-scope elaborator entirely: running it would surface
/// `Unbound` because the referenced parameter is not in the FN's lexical scope.
/// Per-call elaboration runs at the dispatch boundary instead.
pub(crate) enum ReturnTypeState<'a> {
    Done(KType<'a>),
    Pending {
        te: TypeIdentifier,
        producers: Vec<NodeId>,
    },
    Deferred(DeferredReturn<'a>),
    /// `Expression(_)` carrier (e.g. `-> :(Mo.Ty)`) that doesn't reference any FN
    /// parameter; safe to resolve once at FN-def time. Scheduling happens via
    /// `super::finalize::defer` so all owned-sub registration lives
    /// at one site.
    ExprToSubDispatch(KExpression<'a>),
}

pub(crate) enum ReturnTypeCapture<'a> {
    Resolved(KType<'a>),
    Unresolved(String),
    Deferred(DeferredReturn<'a>),
    /// `owned_pos` is the return-type sub's index within the dep-finish's owned results — it is
    /// always the first owned dep, scheduled ahead of any signature subs, so `owned_pos == 0`.
    ReturnTypeExpr {
        owned_pos: usize,
    },
}

/// Read the `return_type` slot from a `BodyCtx::args` record into a `ReturnTypeRaw`.
pub(crate) fn extract_return_type_raw<'a>(args: &KObject<'a>) -> Result<ReturnTypeRaw<'a>, KError> {
    extract_type_slot_raw(args, "return_type", "FN return-type slot")
}

/// Read any type-denoting slot from a `BodyCtx::args` record into a [`ReturnTypeRaw`]. The two
/// carriers a type slot arrives on: a `ProperType` cell (`Number`, or an `Unresolved` name a
/// scope walk still has to resolve) and a raw `:(…)` sigil expression that sub-dispatches to its
/// type. `slot` names the args field; `label` names the surface in the shape error. Shared by
/// `FN`'s return slot and `OP`'s operand / return slots.
pub(crate) fn extract_type_slot_raw<'a>(
    args: &KObject<'a>,
    slot: &str,
    label: &str,
) -> Result<ReturnTypeRaw<'a>, KError> {
    use crate::machine::{arg_object, arg_type};
    if let Some(kt) = arg_type(args, slot) {
        match kt {
            KType::Unresolved(te) => Ok(ReturnTypeRaw::TypeExprCarrier(te.clone())),
            t => Ok(ReturnTypeRaw::Resolved(t.clone())),
        }
    } else if let Some(KObject::KExpression(e)) = arg_object(args, slot) {
        Ok(ReturnTypeRaw::ExprCarrier(e.clone()))
    } else {
        Err(KError::new(KErrorKind::ShapeError(format!(
            "{label} must be a type expression (e.g. `Number`, `:(LIST OF Str)`)"
        ))))
    }
}

/// Classify a type-slot carrier. The parameter-name scan runs first so a match
/// short-circuits eager elaboration and the carrier survives verbatim to the dispatch
/// boundary. `label` names the slot in the unbound-name error; `param_names` is empty for a
/// slot no parameter can reference (`OP`'s operand / return), which never classifies `Deferred`.
pub(crate) fn classify_return_type<'a>(
    raw: ReturnTypeRaw<'a>,
    param_names: &[String],
    scope: &Scope<'a>,
    chain: Option<Rc<LexicalFrame>>,
    label: &str,
) -> Result<ReturnTypeState<'a>, KError> {
    match raw {
        ReturnTypeRaw::Resolved(kt) => Ok(ReturnTypeState::Done(kt)),
        ReturnTypeRaw::TypeExprCarrier(te) => {
            if type_expr_references_any(&te, param_names) {
                return Ok(ReturnTypeState::Deferred(DeferredReturn::Type(te)));
            }
            // Gated to the FN's lexical position — a return type naming a later type is a
            // position error, like any other forward reference.
            match classify_type_hit(scope.resolve_type_identifier(&te, chain)) {
                TypeResolution::Done(kt) => Ok(ReturnTypeState::Done(kt)),
                TypeResolution::Park(producers) => Ok(ReturnTypeState::Pending { te, producers }),
                // `resolve_type_identifier` already tries the builtin fallback internally, so an
                // `Unbound` here is neither a type binder nor a builtin — a hard miss.
                TypeResolution::Unbound(detail) => Err(unbound_error(label, &detail)),
            }
        }
        ReturnTypeRaw::ExprCarrier(e) => {
            if kexpression_references_any(&e, param_names) {
                Ok(ReturnTypeState::Deferred(DeferredReturn::Expression(e)))
            } else {
                Ok(ReturnTypeState::ExprToSubDispatch(e))
            }
        }
    }
}

pub(super) fn make_capture<'a>(te: TypeIdentifier) -> ReturnTypeCapture<'a> {
    ReturnTypeCapture::Unresolved(te.render())
}

/// Park-arm outcomes from `Scope::resolve_type_identifier` are protocol errors here: every
/// parked producer is terminal by the dep-finish invariant, so a second park would
/// loop forever and is surfaced as a structured error — see [`resolve_at_wake`].
pub(super) fn resolve_capture_at_finish<'a>(
    capture: ReturnTypeCapture<'a>,
    scope: &Scope<'a>,
    results: DepResults<'_, &DepTerminal<'a>>,
) -> Result<ReturnType<'a>, KError> {
    match capture {
        ReturnTypeCapture::Resolved(kt) => Ok(ReturnType::Resolved(kt)),
        ReturnTypeCapture::Unresolved(name) => {
            let te = TypeIdentifier::leaf(name);
            resolve_at_wake(scope, "FN return-type slot", |s| {
                classify_type_hit(s.resolve_type_identifier(&te, None))
            })
            .map(ReturnType::Resolved)
        }
        ReturnTypeCapture::Deferred(d) => Ok(ReturnType::Deferred(d)),
        ReturnTypeCapture::ReturnTypeExpr { owned_pos } => {
            let (kt, carrier) = expect_type_terminal(&results, owned_pos, "FN return-type slot")?;
            // The resolved return type can embed a borrow into the sub-dispatch's producer region (a
            // declared `Signature`, a nominal `SetRef`, ...); it is folded straight into the `KFunction`
            // `finalize_fn_with_kind` builds (via `user_sig`), whose own terminal carrier seals with an
            // empty foreign reach — sound only because the captured scope's reach-set transitively pins
            // everything its bindings reach. The parameter-type slots already fold this way via
            // `adopt_sealed` at signature elaboration; this fold gives the return-type slot the same
            // property before `finalize_fn_with_kind` runs.
            let _ = scope.host_reach_of(carrier);
            Ok(ReturnType::Resolved(kt))
        }
    }
}
