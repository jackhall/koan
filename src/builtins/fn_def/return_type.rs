//! FN return-type pipeline: extraction → classification → carriage across the
//! dep-finish boundary → resolution at finish time.

use crate::builtins::resolve_or_await::{expect_type_terminal, resolve_at_wake, unbound_error};
use crate::machine::DepTerminal;
use crate::machine::LexicalFrame;
use crate::machine::ProducerId;
use crate::machine::core::RegionBrand;
use crate::machine::model::KExpression;
use crate::machine::model::TypeResolution;
use crate::machine::model::labels::TypeSymbol;
use crate::machine::model::{BinderSymbol, ExpressionPart};
use crate::machine::model::{DeferredReturn, ReturnType};
use crate::machine::model::{KObject, KType, Symbol};
use crate::machine::{KError, KErrorKind, Scope};
use crate::source::Spanned;
use std::rc::Rc;

use super::param_refs::{kexpression_references_any, type_expr_references_any};
use crate::machine::BoundArgs;
use crate::machine::model::RunRegistries;
use crate::machine::model::{StaticName, ValueSymbol};

/// The carrier union a deferral type slot takes, spelling the whole dimension once so the operand ×
/// result matrix is one registration per surface rather than a cartesian product. Every member is
/// captured raw: a bare `Type` token (`-> Number`, `OVER Elt`), a sigiled form (`-> :(LIST OF Str)`,
/// `OVER :Number`) and a record (`-> :{v :Number}`) all reach the body verbatim, so it resolves or
/// sub-dispatches them against its own scope rather than a name the surrounding one may not bind.
/// One constructor for both surfaces — `FN`'s return slot and `OP`'s operand / result slots — since
/// [`TypeSlotThunk::from_slot`] is the single read behind them and its arms *are* these members.
pub(crate) fn type_carrier_union(registries: &RunRegistries) -> KType {
    registries.types.union_of(&[
        KType::TYPE_NAME_TOKEN,
        KType::SIGILED_TYPE_EXPR,
        KType::RECORD_TYPE,
    ])
}

/// The normalized read of a deferral slot: whatever carrier member of the slot's union claimed the
/// part, reduced to the two shapes classification cares about. The slot captures raw rather than
/// sub-dispatching in the outer scope because the return's content may reference a parameter
/// unbound there.
pub(crate) enum TypeSlotThunk<'a> {
    /// A bare `Type` token, unresolved: the body forces it against its own scope and chain.
    UnresolvedName(TypeSymbol),
    /// A raw type expression to sub-dispatch: a `:(…)` capture's inner expression, or a `:{…}`
    /// capture re-wrapped as the single-part node that folds to its record type.
    RawTypeExpression(KExpression<'a>),
}

/// `Deferred` skips the outer-scope elaborator entirely: running it would surface
/// `Unbound` because the referenced parameter is not in the FN's lexical scope.
/// Per-call elaboration runs at the dispatch boundary instead.
pub(crate) enum ReturnTypeState<'a> {
    Done(KType),
    Pending {
        te: TypeSymbol,
        producers: Vec<ProducerId>,
    },
    Deferred(DeferredReturn<'a>),
    /// `Expression(_)` carrier (e.g. `-> :(Mo.Ty)`) that doesn't reference any FN
    /// parameter; safe to resolve once at FN-def time. Scheduling happens via
    /// `super::finalize::defer` so all owned-sub registration lives
    /// at one site.
    ExprToSubDispatch(KExpression<'a>),
}

pub(crate) enum ReturnTypeCapture<'a> {
    Resolved(KType),
    Unresolved(TypeSymbol),
    Deferred(DeferredReturn<'a>),
    /// The return type is a `:(…)` expression the deferral sub-dispatches; its result comes back at
    /// the dep index [`defer`](super::finalize::defer) recorded when it appended the request.
    ReturnTypeExpr,
}

/// Read the `return_type` slot from a `BodyCtx::args` record into a [`TypeSlotThunk`].
pub(crate) fn extract_return_type_raw<'a>(
    args: BoundArgs<'a, '_>,
    brand: RegionBrand<'a>,
) -> Result<TypeSlotThunk<'a>, KError> {
    TypeSlotThunk::from_slot(
        args,
        brand,
        &super::SLOTS.return_type,
        "FN return-type slot",
    )
}

impl<'a> TypeSlotThunk<'a> {
    /// Normalize a deferral slot's raw capture. The slot is a carrier union, so exactly one member
    /// claimed the part and the read is a match over the arms that union lists — never a probe for
    /// a resolved cell, since every member is part-kind-exact and captures raw.
    ///
    /// `slot` names the args field; `label` names the surface in the shape errors. Shared by `FN`'s
    /// return slot and `OP`'s operand / result slots, which take the same
    /// [`type_carrier_union`] — so the arms here are exactly that union's members. A value-named
    /// slot (`-> er`) is no member of it: the shape never binds, and its targeted message comes
    /// from the dispatch-miss diagnosis table.
    pub(crate) fn from_slot(
        args: BoundArgs<'a, '_>,
        brand: RegionBrand<'a>,
        slot: &StaticName<ValueSymbol>,
        label: &str,
    ) -> Result<TypeSlotThunk<'a>, KError> {
        if let Some(BinderSymbol::Type(te)) = args.name(slot) {
            return Ok(TypeSlotThunk::UnresolvedName(te));
        }
        if let Some(KObject::KExpression(e)) = args.object(slot) {
            // A sigil capture hands over its *inner* expression, which dispatches on its own.
            return Ok(TypeSlotThunk::RawTypeExpression(e.node()));
        }
        if let Some(fields) = args.record_type(slot) {
            // A record capture hands over the field list, which does not; re-wrap it as the
            // single-part node whose `RecordType` dispatch shape folds to the record type.
            return Ok(TypeSlotThunk::RawTypeExpression(KExpression::new(
                brand,
                &[Spanned::bare(ExpressionPart::RecordType(fields))],
            )));
        }
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
    raw: TypeSlotThunk<'a>,
    param_names: &[Symbol],
    scope: &Scope<'a>,
    chain: Option<Rc<LexicalFrame>>,
    label: &str,
    registries: &RunRegistries,
) -> Result<ReturnTypeState<'a>, KError> {
    match raw {
        TypeSlotThunk::UnresolvedName(te) => {
            if type_expr_references_any(te, param_names) {
                return Ok(ReturnTypeState::Deferred(DeferredReturn::Type(te)));
            }
            // Gated to the FN's lexical position — a return type naming a later type is a
            // position error, like any other forward reference.
            match scope.resolve_type_identifier(te, chain, registries) {
                TypeResolution::Done(kt) => Ok(ReturnTypeState::Done(kt)),
                TypeResolution::Park(producers) => Ok(ReturnTypeState::Pending { te, producers }),
                // `resolve_type_identifier` already tries the builtin fallback internally, so an
                // `Unbound` here is neither a type binder nor a builtin — a hard miss.
                TypeResolution::Unbound(name) => Err(unbound_error(
                    label,
                    &crate::machine::model::unknown_type_name(name, registries),
                )),
            }
        }
        TypeSlotThunk::RawTypeExpression(e) => {
            if kexpression_references_any(&e, param_names) {
                Ok(ReturnTypeState::Deferred(DeferredReturn::Expression(e)))
            } else {
                Ok(ReturnTypeState::ExprToSubDispatch(e))
            }
        }
    }
}

pub(super) fn make_capture<'a>(te: TypeSymbol) -> ReturnTypeCapture<'a> {
    ReturnTypeCapture::Unresolved(te)
}

/// Park-arm outcomes from `Scope::resolve_type_identifier` are protocol errors here: every
/// parked producer is terminal by the dep-finish invariant, so a second park would
/// loop forever and is surfaced as a structured error — see [`resolve_at_wake`].
pub(super) fn resolve_capture_at_finish<'a>(
    capture: ReturnTypeCapture<'a>,
    scope: &Scope<'a>,
    results: &[DepTerminal<'_>],
    return_type_dep: Option<usize>,
    registries: &RunRegistries,
) -> Result<ReturnType<'a>, KError> {
    match capture {
        ReturnTypeCapture::Resolved(kt) => Ok(ReturnType::Resolved(kt)),
        ReturnTypeCapture::Unresolved(te) => {
            resolve_at_wake(scope, "FN return-type slot", registries, |s, registries| {
                s.resolve_type_identifier(te, None, registries)
            })
            .map(ReturnType::Resolved)
        }
        ReturnTypeCapture::Deferred(d) => Ok(ReturnType::Deferred(d)),
        ReturnTypeCapture::ReturnTypeExpr => {
            // The resolved return type is owned content, cloned out of the sub-dispatch's terminal,
            // and is folded straight into the `KFunction` `finalize_fn_with_kind` builds (via
            // `user_sig`).
            let dep_index = return_type_dep
                .expect("a ReturnTypeExpr capture is built beside the request that carries it");
            let kt = expect_type_terminal(results, dep_index, "FN return-type slot", registries)?;
            Ok(ReturnType::Resolved(kt))
        }
    }
}
