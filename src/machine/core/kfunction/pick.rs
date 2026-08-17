//! Dispatch-shape classification: read-only view of how a `KFunction`'s
//! signature matches the expression a slot is dispatching for late dispatch.
//!
//! The classifiers share the "bare-name" predicate ([`is_bare_name`]) — the
//! load-bearing shape concept the auto-wrap rail turns on.

use crate::machine::model::TypeRegistry;
use crate::machine::model::{Argument, KType, SignatureElement};
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};

use super::KFunction;

/// Per-slot classification produced by [`KFunction::classify_for_pick`]:
/// - `eager_indices`: `Some(indices)` when the picked function is a *lazy candidate* — the
///   carried indices are the eager `Expression` / `ListLiteral` / `DictLiteral` / `RecordLiteral`
///   parts in *non*-`KExpression` slots. `None` when not a lazy candidate, so every eager-shaped
///   part schedules as its own sub-Dispatch.
/// - `wrap_indices`: bare-Identifier / bare-Type parts in non-literal-name slots to
///   auto-wrap as sub-Dispatches. A literal-name slot (`KType::Identifier` /
///   `KType::OfKind(KKind::ProperType)`) is excluded: it owns its token — a declaration's name,
///   or name-data the body reads — so the token rides to the bind unresolved.
///
/// The two index sets are disjoint by construction over disjoint `(SignatureElement, WorkingPart)`
/// shapes — `classify_for_pick` is the sole producer.
pub struct ClassifiedSlots {
    pub eager_indices: Option<Vec<usize>>,
    pub wrap_indices: Vec<usize>,
}

impl<'a> KFunction<'a> {
    /// Lazy-candidate shape check. Lazy means at least one `KType::KExpression` slot is
    /// bound by an `ExpressionPart::Expression`; the caller schedules the returned eager
    /// indices as deps and leaves the lazy ones in place for the receiving builtin to
    /// dispatch itself. Returns `None` when `self` isn't a lazy candidate.
    ///
    /// A raw-capture slot admits only the AST shape it captures, so those arms read through
    /// [`WorkingPart::Ast`]; the scheduler's own slots (a resolved sub-result, a staging hole, a
    /// synthesized nested node) are ordinary value slots and fall to the tail arm.
    pub fn lazy_eager_indices<'e>(
        &self,
        expr: &WorkingExpression<'e>,
        types: &TypeRegistry,
    ) -> Option<Vec<usize>> {
        let sig = &self.signature;
        if sig.elements().len() != expr.parts.len() {
            return None;
        }
        let mut eager_indices: Vec<usize> = Vec::new();
        let mut has_lazy_slot = false;
        for (i, (el, part)) in sig.elements().iter().zip(expr.parts.iter()).enumerate() {
            match (el, &part.value) {
                (SignatureElement::Keyword(s), WorkingPart::Ast(ExpressionPart::Keyword(t)))
                    if *s == *t => {}
                (SignatureElement::Keyword(_), _) => return None,
                (SignatureElement::Argument(arg), part_value) => match (arg.ktype, part_value) {
                    (KType::KEXPRESSION, WorkingPart::Ast(ExpressionPart::Expression(_))) => {
                        has_lazy_slot = true;
                    }
                    // A `#(...)` quote in a `:KExpression` slot is captured raw — the body is data,
                    // so it must never be sub-dispatched.
                    (KType::KEXPRESSION, WorkingPart::Ast(ExpressionPart::QuotedExpression(_))) => {
                        has_lazy_slot = true;
                    }
                    (KType::KEXPRESSION, _) => return None,
                    // `:SigiledTypeExpr` is the lazy sibling of `:KExpression` for a `:(...)`
                    // part — captured raw (`resolve_for`), never sub-dispatched here.
                    (
                        KType::SIGILED_TYPE_EXPR,
                        WorkingPart::Ast(ExpressionPart::SigiledTypeExpr(_)),
                    ) => {
                        has_lazy_slot = true;
                    }
                    (KType::SIGILED_TYPE_EXPR, _) => return None,
                    // `:RecordType` is the lazy sibling for a `:{…}` part — captured raw so
                    // the NEWTYPE record-repr declarator owns its field-list elaboration.
                    (KType::RECORD_TYPE, WorkingPart::Ast(ExpressionPart::RecordType(_))) => {
                        has_lazy_slot = true;
                    }
                    (KType::RECORD_TYPE, _) => return None,
                    // A node the scheduler synthesized (an operator-chain fold's accumulator) is
                    // an eager operand exactly as a parsed `(...)` is; it is already a working
                    // expression, so staging dispatches it with no AST crossing.
                    (_, WorkingPart::Expression(_))
                    | (_, WorkingPart::Ast(ExpressionPart::Expression(_)))
                    | (_, WorkingPart::Ast(ExpressionPart::SigiledTypeExpr(_)))
                    | (_, WorkingPart::Ast(ExpressionPart::RecordType(_)))
                    | (_, WorkingPart::Ast(ExpressionPart::ListLiteral(_)))
                    | (_, WorkingPart::Ast(ExpressionPart::DictLiteral(_)))
                    | (_, WorkingPart::Ast(ExpressionPart::RecordLiteral(_))) => {
                        // Speculative: assume the eager-evaluated result will type-match
                        // at late dispatch. SigiledTypeExpr / RecordType ride the Expression
                        // path — sub-dispatch produces a type-side Spliced the slot validates.
                        // A container literal (List/Dict/Record) in a non-`:KExpression` slot is
                        // reached here only after the `arg.ktype` match above has ruled out
                        // `KEXPRESSION` (whose own List-literal arm — the unary-operator-run raw
                        // case — and Record/Dict slot-typed siblings are handled above): its
                        // substrate (a `Record`'s, once other containers convert) is born only
                        // through the fold door, which no slot but the scheduled dep-finish path
                        // reaches, so it must stage the same as an `Expression` part rather than
                        // fall through unstaged to `resolve()`.
                        eager_indices.push(i);
                    }
                    (_, other) => {
                        // Admit bare names in non-literal-name slots so a sibling
                        // `KExpression+Expression` slot can still drive lazy candidacy —
                        // e.g. a builtin pairing a bare-name-typed slot (`:Signature` /
                        // `Type(...)`) with a lazy `:KExpression` slot would otherwise lose it.
                        if is_bare_name(other)
                            && !matches!(arg.ktype, KType::IDENTIFIER | KType::PROPER_TYPE)
                        {
                            continue;
                        }
                        if !slot_admits(arg, other, types) {
                            return None;
                        }
                    }
                },
            }
        }
        if has_lazy_slot {
            Some(eager_indices)
        } else {
            None
        }
    }

    /// Per-slot classification of `expr` against `self`'s signature into the two index
    /// buckets of [`ClassifiedSlots`]. Disjointness is guaranteed by construction — each
    /// `(SignatureElement, WorkingPart)` shape lands in at most one bucket — and the
    /// downstream scheduler relies on it.
    pub fn classify_for_pick<'e>(
        &self,
        expr: &WorkingExpression<'e>,
        types: &TypeRegistry,
    ) -> ClassifiedSlots {
        let eager_indices = self.lazy_eager_indices(expr, types);
        let mut wrap_indices: Vec<usize> = Vec::new();
        for (i, (el, part)) in self
            .signature
            .elements()
            .iter()
            .zip(expr.parts.iter())
            .enumerate()
        {
            let SignatureElement::Argument(arg) = el else {
                continue;
            };
            if !is_bare_name(&part.value) {
                continue;
            }
            // A literal-name slot owns its token; every other slot's bare name resolves.
            if !matches!(arg.ktype, KType::IDENTIFIER | KType::PROPER_TYPE) {
                wrap_indices.push(i);
            }
        }
        ClassifiedSlots {
            eager_indices,
            wrap_indices,
        }
    }
}

/// Whether `part` satisfies `arg`'s declared parameter type — the one admissibility reader over a
/// dispatch-path slot. An AST slot classifies by part shape
/// ([`KType::accepts_part`](crate::machine::model::KType::accepts_part)); a resolved sub-result
/// classifies by the carrier resting in its cell, opened at that cell's own brand. A synthesized
/// nested node and a staging hole denote no value yet, so neither satisfies a slot.
pub fn slot_admits(arg: &Argument, part: &WorkingPart<'_>, types: &TypeRegistry) -> bool {
    match part {
        WorkingPart::Ast(ast) => arg.matches(ast, types),
        WorkingPart::Spliced { cell } => arg.ktype.accepts_cell(cell, types),
        WorkingPart::Expression(_) | WorkingPart::RecordType(_) | WorkingPart::StagedSlot => false,
    }
}

/// True iff `part` is the "bare-name" shape — a bare `Identifier` or a leaf
/// `Type`-token. Both name-shaped parts ride the same auto-wrap and
/// dispatch-park rails, so the symmetry is load-bearing for `LET T = Number`
/// vs `LET y = z` walking identical scheduler paths.
fn is_bare_name(part: &WorkingPart<'_>) -> bool {
    matches!(
        part.as_ast(),
        Some(ExpressionPart::Identifier(_) | ExpressionPart::Type(_))
    )
}
