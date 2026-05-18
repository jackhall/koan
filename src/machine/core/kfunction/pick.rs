//! Dispatch-shape classification: the read-only view of how a `KFunction`'s
//! signature matches an `KExpression` for the purposes of late dispatch.
//!
//! - [`KFunction::accepts_for_wrap`] — tentative match used during the scope-chain
//!   walk in [`crate::machine::core::resolve_dispatch`].
//! - [`KFunction::classify_for_pick`] — post-pick per-slot bucketing into
//!   `eager_indices` / `wrap_indices` / `ref_name_indices` (see [`ClassifiedSlots`]).
//! - `lazy_eager_indices` — internal helper used by `classify_for_pick` to compute
//!   the lazy-candidate flag plus eager-index list in one pass.
//!
//! All three functions are read-only walks over `&self.signature` × `&expr.parts`
//! (plus `self.pre_run.is_some()` for the classifier). They share the
//! "bare-name" predicate ([`is_bare_name`]), which is the load-bearing
//! shape concept the auto-wrap and replay-park rails turn on.

use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr, TypeParams};
use crate::machine::model::types::{Argument, KType, SignatureElement};

use super::KFunction;

/// Per-slot classification produced by [`KFunction::classify_for_pick`]:
/// - `eager_indices`: `Some(indices)` iff the picked function is a *lazy candidate* (has at
///   least one `KType::KExpression` slot bound by an `ExpressionPart::Expression`); the
///   carried indices are the `Expression` parts in *non*-`KExpression` slots that must
///   evaluate eagerly. `None` when the function isn't a lazy candidate — the scheduler
///   then schedules every eager-shaped part (`Expression` / `ListLiteral` / `DictLiteral`)
///   as a sub-Dispatch.
/// - `wrap_indices`: bare-Identifier / bare-Type parts in non-literal-name slots to
///   auto-wrap as sub-Dispatches.
/// - `ref_name_indices`: bare-Identifier / bare-Type parts in literal-name slots
///   (`KType::Identifier` / `KType::TypeExprRef`) of a non-`pre_run` function; candidates
///   for replay-park.
///
/// `picked_has_pre_run` distinguishes binder-shaped expressions (literal-name slots are
/// declarations) from call-shaped expressions (literal-name slots are references that may
/// need to park). The three index vectors are disjoint by construction over disjoint
/// `(SignatureElement, ExpressionPart)` shapes — `classify_for_pick` is the sole producer.
pub struct ClassifiedSlots {
    pub eager_indices: Option<Vec<usize>>,
    pub wrap_indices: Vec<usize>,
    pub ref_name_indices: Vec<usize>,
    pub picked_has_pre_run: bool,
}

impl<'a> KFunction<'a> {
    /// Lazy-candidate shape check for this function: is `self` a viable lazy match for
    /// `expr`, and if so what are the indices of its eager-Expression parts? Returns `None`
    /// when this function isn't a lazy candidate (length mismatch, fixed-token mismatch, no
    /// `KExpression` slot binding an `Expression` part, or any other arg-type mismatch).
    /// Lazy means at least one `KType::KExpression` slot is bound by an
    /// `ExpressionPart::Expression`; the caller schedules the eager indices as deps and
    /// leaves the lazy ones in place for the receiving builtin to dispatch itself.
    pub fn lazy_eager_indices(&self, expr: &KExpression<'_>) -> Option<Vec<usize>> {
        let sig = &self.signature;
        if sig.elements.len() != expr.parts.len() {
            return None;
        }
        let mut eager_indices: Vec<usize> = Vec::new();
        let mut has_lazy_slot = false;
        for (i, (el, part)) in sig.elements.iter().zip(expr.parts.iter()).enumerate() {
            match (el, part) {
                (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) if s == t => {}
                (SignatureElement::Keyword(_), _) => return None,
                (SignatureElement::Argument(arg), part) => match (&arg.ktype, part) {
                    (KType::KExpression, ExpressionPart::Expression(_)) => {
                        has_lazy_slot = true;
                    }
                    (KType::KExpression, _) => return None,
                    (_, ExpressionPart::Expression(_)) => {
                        // Speculative: assume the eager-evaluated result will type-match at
                        // late dispatch. If not, dispatch will fail at that point.
                        eager_indices.push(i);
                    }
                    (_, other) => {
                        // Mirror `accepts_for_wrap`'s bare-name relaxation: a bare
                        // Identifier or bare leaf-Type part in any slot whose declared
                        // type isn't `Identifier` / `TypeExprRef` is auto-wrap-eligible.
                        // The auto-wrap pass (`apply_auto_wrap`) rewrites the part into
                        // a single-name sub-Dispatch that re-enters via the bare-name
                        // short-circuit before late dispatch matches the lifted value.
                        // Admitting the part here keeps the function's lazy candidacy
                        // intact when a sibling `KExpression+Expression` slot is the
                        // one driving laziness — without this, `SIG_WITH OrderedSig (...)`
                        // would lose its lazy candidacy on the `sig: Signature` /
                        // `Type(OrderedSig)` pairing and the `schedule_deps` None-arm
                        // would sub-Dispatch the bindings group, defeating the lazy
                        // contract for the `KExpression` slot.
                        if is_bare_name(other)
                            && !matches!(arg.ktype, KType::Identifier | KType::TypeExprRef)
                        {
                            continue;
                        }
                        if !arg.matches(other) {
                            return None;
                        }
                    }
                },
            }
        }
        if has_lazy_slot { Some(eager_indices) } else { None }
    }

    /// Auto-wrap-permissive shape check. Speculatively admits two relaxations beyond the
    /// strict matcher:
    ///
    /// - Bare-Identifier and bare leaf-Type parts in any slot whose declared type isn't
    ///   `Identifier` or `TypeExprRef`. The auto-wrap pass rewrites these into single-name
    ///   sub-Dispatches that re-enter via the bare-name short-circuit and route through
    ///   the Identifier / TypeExprRef overload of `value_lookup`. Covers both
    ///   `MAKESET some_var` (Identifier) and `MAKESET IntOrd` (Type-token).
    /// - Parens-wrapped `Expression` parts in non-`KExpression` slots — *but only when*
    ///   the signature also has at least one `KExpression` slot bound by an `Expression`
    ///   part (i.e. the function is a [`Self::lazy_eager_indices`] candidate). The
    ///   post-pick scheduler then routes the non-`KExpression` slot's `Expression`
    ///   through `eager_indices` for sub-Dispatch while leaving the lazy
    ///   `KExpression+Expression` pair untouched, and splices the resulting `Future(_)`
    ///   back for strict re-matching. Covers shapes like `FN (...) -> Mo.Ty = (...)`
    ///   where the return-type slot is `Expression([ATTR Mo Ty])` and FN's `signature`/
    ///   `body` slots are also `Expression` parts. Functions without a `KExpression`
    ///   slot (e.g. `LIST_OF Mo.Ty`, `PLUS (deep_call) OP 1`) ride the
    ///   `resolve_dispatch::Deferred` path instead, where `schedule_eager_fallthrough`
    ///   sub-Dispatches every `Expression` part uniformly — equivalent end state without
    ///   the false-tentative-match noise that would otherwise show up here.
    ///
    /// All other slot/part pairings reuse the normal `Argument::matches` check.
    pub fn accepts_for_wrap(&self, expr: &KExpression<'_>) -> bool {
        let sig = &self.signature;
        if sig.elements.len() != expr.parts.len() {
            return false;
        }
        // Pre-compute whether this function has a `KExpression+Expression` lazy slot — gates
        // the Expression-in-non-KExpression-slot relaxation below so non-lazy candidates
        // keep their existing `Deferred` path.
        let has_lazy_kexpr_slot = sig.elements.iter().zip(expr.parts.iter()).any(|(el, part)| {
            matches!(
                (el, part),
                (
                    SignatureElement::Argument(Argument { ktype: KType::KExpression, .. }),
                    ExpressionPart::Expression(_),
                )
            )
        });
        for (el, part) in sig.elements.iter().zip(expr.parts.iter()) {
            match (el, part) {
                (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) if s == t => {}
                (SignatureElement::Keyword(_), _) => return false,
                (SignatureElement::Argument(arg), part) => {
                    if is_bare_name(part)
                        && !matches!(arg.ktype, KType::Identifier | KType::TypeExprRef)
                    {
                        continue;
                    }
                    if has_lazy_kexpr_slot
                        && matches!(part, ExpressionPart::Expression(_))
                        && !matches!(arg.ktype, KType::KExpression)
                    {
                        continue;
                    }
                    if !arg.matches(part) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Per-slot classification: classify `expr`'s slots against `self`'s signature into
    /// three disjoint index buckets — `eager_indices`, `wrap_indices`, `ref_name_indices` —
    /// plus a `picked_has_pre_run` flag. Identifier and bare leaf Type-token
    /// (`TypeParams::None`) parts are treated symmetrically — both name-shaped parts ride
    /// the same wrap-or-park rails, so `LET T = Number` and `LET y = z` (and their
    /// forward-reference variants) walk identical scheduler paths.
    ///
    /// Disjointness is guaranteed by construction: each slot's `(SignatureElement,
    /// ExpressionPart)` shape lands in at most one bucket. The classifier is the sole
    /// producer of these vectors; the downstream scheduler may rely on the invariant.
    pub fn classify_for_pick(&self, expr: &KExpression<'_>) -> ClassifiedSlots {
        let eager_indices = self.lazy_eager_indices(expr);
        let mut wrap_indices: Vec<usize> = Vec::new();
        let mut ref_name_indices: Vec<usize> = Vec::new();
        let picked_has_pre_run = self.pre_run.is_some();
        for (i, (el, part)) in self.signature.elements.iter().zip(expr.parts.iter()).enumerate() {
            let SignatureElement::Argument(arg) = el else { continue };
            if !is_bare_name(part) {
                continue;
            }
            match &arg.ktype {
                // Bare name in literal-name slot: replay-park iff the picked function isn't
                // a binder. Binders' literal-name slots are *declarations*; the slot already
                // owns the name and must not park on its own placeholder.
                KType::Identifier | KType::TypeExprRef => {
                    if !picked_has_pre_run {
                        ref_name_indices.push(i);
                    }
                }
                // Bare name in any other slot (including `Any`): auto-wrap. The wrap
                // rewrites the part into a sub-Dispatch that re-enters via the bare-name
                // short-circuit and routes through the Identifier / TypeExprRef overload of
                // `value_lookup`. Covers both `LET y = z` and `LET T = Number` /
                // `MAKESET IntOrd` symmetrically.
                _ => wrap_indices.push(i),
            }
        }
        ClassifiedSlots {
            eager_indices,
            wrap_indices,
            ref_name_indices,
            picked_has_pre_run,
        }
    }
}

/// True iff `part` is the "bare-name" shape — a bare `Identifier` or a leaf
/// `Type`-token (`TypeParams::None`). Both name-shaped parts ride the same
/// auto-wrap and replay-park rails, so the symmetry is load-bearing for
/// `LET T = Number` vs `LET y = z` walking identical scheduler paths.
fn is_bare_name(part: &ExpressionPart<'_>) -> bool {
    matches!(
        part,
        ExpressionPart::Identifier(_)
            | ExpressionPart::Type(TypeExpr { params: TypeParams::None, .. })
    )
}
