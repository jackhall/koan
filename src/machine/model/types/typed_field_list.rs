//! Shared parser for `(<name> :<Type> <name> :<Type> ...)` schema expressions, used by
//! `UNION` (order discarded into a `HashMap<tag, KType>`) and `STRUCT` (order preserved for
//! positional construction).

use super::ktype::KType;
use super::resolver::{elaborate_type_expr, ElabResult, Elaborator};
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::KObject;
use crate::machine::model::Parseable;
use crate::machine::model::Record;
use crate::machine::{NodeId, Scope};
use crate::parse::parse_pair_list;
pub use crate::parse::FieldNameKind;
use std::collections::HashSet;

pub enum FieldListOutcome<'a> {
    Done(Vec<(String, KType<'a>)>),
    /// `sub_dispatches` carries each sigil field's wrapped expression in DFS walk
    /// order. The caller schedules them in that order and, on the Combine re-walk,
    /// feeds the resolved `KObject::KTypeValue`s back through a [`ResultFeed`] — the walk
    /// re-descends in the same order, so no slot index is needed.
    Pending {
        park_producers: Vec<NodeId>,
        sub_dispatches: Vec<KExpression<'a>>,
    },
    Err(String),
}

/// Walk-order feed of resolved sub-dispatch carriers for the Combine re-walk. The first
/// walk records one sub-Dispatch per sigil field (in DFS order, descending into nested
/// records); the re-walk replays the same traversal and [`pop`](ResultFeed::pop)s each
/// resolved carrier back in. A concrete cursor (rather than a `dyn Iterator`) so it
/// reborrows cleanly when a nested record recurses through the shared walker.
pub struct ResultFeed<'r, 'a> {
    results: &'r [&'a KObject<'a>],
    pos: usize,
}

impl<'r, 'a> ResultFeed<'r, 'a> {
    pub fn new(results: &'r [&'a KObject<'a>]) -> Self {
        ResultFeed { results, pos: 0 }
    }

    /// The next resolved carrier in walk order, or `None` once exhausted.
    fn pop(&mut self) -> Option<&'a KObject<'a>> {
        let next = self.results.get(self.pos).copied();
        if next.is_some() {
            self.pos += 1;
        }
        next
    }
}

/// Entry point used by STRUCT / UNION / FN / FUNCTOR. Routes each field type through the
/// scheduler-aware [`elaborate_type_expr`], accumulating parking producers and
/// pending sub-Dispatches across the whole walk so the caller can install one
/// Combine for the merged set. `name_kind` selects which token shapes are valid as a
/// field/parameter name (STRUCT / UNION pass `Identifier`; FN / FUNCTOR pass
/// `IdentifierOrType` so capitalized type-parameter names like `Ty` are accepted).
///
/// `results` is `None` on the first walk (each sigil field schedules a sub-Dispatch,
/// collected into `Pending.sub_dispatches`) and `Some(iter)` on the Combine re-walk
/// (each sigil field consumes the next resolved `KObject::KTypeValue` from `iter` in DFS
/// walk order instead of re-scheduling). Because the re-walk re-descends the field list
/// in the same deterministic order the first walk produced the subs, positional
/// consumption needs no slot index — and nested field-lists fall out for free.
pub fn parse_typed_field_list_via_elaborator<'a>(
    expr: &KExpression<'a>,
    context: &str,
    name_kind: FieldNameKind,
    elaborator: &mut Elaborator<'_, 'a>,
    mut results: Option<&mut ResultFeed<'_, 'a>>,
) -> FieldListOutcome<'a> {
    let mut parks: Vec<NodeId> = Vec::new();
    let mut sub_dispatches: Vec<KExpression<'a>> = Vec::new();
    let parsed = parse_pair_list(expr, context, name_kind, |part, name| {
        match part {
            ExpressionPart::Type(t) => match elaborate_type_expr(elaborator, t) {
                ElabResult::Done(kt) => Ok(kt),
                ElabResult::Park(producers) => {
                    parks.extend(producers);
                    // Placeholder; discarded under the Pending outcome. Lets the walk
                    // continue so every parking producer is collected in one pass.
                    Ok(KType::Any)
                }
                ElabResult::Unbound(msg) => Err(format!("{msg} in {context} for `{}`", name)),
            },
            // Sigils (`:(LIST OF Tree)`, `:(MAP Tree -> _)`) sub-Dispatch through the
            // standalone dispatcher, which carries no SCC context, so self-references
            // are pre-resolved to `RecursiveRef` carriers first — `STRUCT Tree =
            // (children :(LIST OF Tree))` must lower Tree to `RecursiveRef("Tree")`.
            ExpressionPart::SigiledTypeExpr(boxed) => {
                match results.as_mut().and_then(|feed| feed.pop()) {
                    // Re-walk: take the resolved carrier. The KTypeValue check is the
                    // single guard that a sub returning a value-by-expression is rejected.
                    Some(KObject::KTypeValue(kt)) => Ok(kt.clone()),
                    Some(other) => Err(format!(
                        "{context} type for `{}` resolved to non-type value `{}`",
                        name,
                        other.summarize(),
                    )),
                    None if results.is_some() => Err(format!(
                        "{context}: Combine re-walk found fewer resolved sub-dispatches than slots",
                    )),
                    // First walk: pre-resolve threaded self-refs, then schedule a sub-Dispatch.
                    None => {
                        let rewritten = rewrite_threaded_self_refs(
                            boxed,
                            &elaborator.threaded,
                            elaborator.scope,
                        );
                        sub_dispatches.push(KExpression::new(vec![Spanned::bare(
                            ExpressionPart::SigiledTypeExpr(Box::new(rewritten)),
                        )]));
                        Ok(KType::Any)
                    }
                }
            }
            // A nested record type `:{…}` elaborates *inline* through this same walker,
            // sharing the elaborator (so a self-reference threads) and the `results`
            // iterator (so a deferred inner sigil consumes in DFS order). Its own
            // parks / sub-dispatches merge into the outer set — there is no sub-Dispatch
            // of the record node itself, and nesting needs no slot bookkeeping.
            ExpressionPart::RecordType(boxed) => {
                match parse_typed_field_list_via_elaborator(
                    boxed,
                    "record fields",
                    FieldNameKind::Identifier,
                    elaborator,
                    results.as_deref_mut(),
                ) {
                    FieldListOutcome::Done(pairs) => {
                        Ok(KType::Record(Box::new(Record::from_pairs(pairs))))
                    }
                    FieldListOutcome::Err(msg) => Err(msg),
                    FieldListOutcome::Pending {
                        park_producers,
                        sub_dispatches: inner_subs,
                    } => {
                        parks.extend(park_producers);
                        sub_dispatches.extend(inner_subs);
                        Ok(KType::Any)
                    }
                }
            }
            ExpressionPart::Future(KObject::KTypeValue(kt)) => Ok(kt.clone()),
            ExpressionPart::Future(other) => Err(format!(
                "{context} type for `{}` resolved to non-type value `{}`",
                name,
                other.summarize(),
            )),
            other => Err(format!(
                "{context} type for `{}` must be a type name token, got {}",
                name,
                other.summarize()
            )),
        }
    });
    match parsed {
        Err(msg) => FieldListOutcome::Err(msg),
        Ok(fields) => {
            if !parks.is_empty() || !sub_dispatches.is_empty() {
                FieldListOutcome::Pending {
                    park_producers: parks,
                    sub_dispatches,
                }
            } else {
                FieldListOutcome::Done(fields)
            }
        }
    }
}

/// Pre-resolve self-references inside a keyworded sigil body before it sub-Dispatches
/// into the standalone dispatcher, which carries no SCC threading context. Every bare
/// `Type(name)` leaf whose `name` is in `threaded` becomes a `Future(KTypeValue(
/// RecursiveRef(name)))` carrier — the same type-side transport `:(LIST OF Number)`
/// rides — so `STRUCT Tree = (children :(LIST OF Tree))` lowers `Tree` to
/// `RecursiveRef("Tree")` instead of parking on its own placeholder and closing a
/// scheduler-deadlock cycle. Recurses into nested sigils (`:(LIST OF (LIST OF Tree))`,
/// `:(MAP Tree -> Number)`); non-threaded names are left for the dispatcher to resolve.
fn rewrite_threaded_self_refs<'a>(
    inner: &KExpression<'a>,
    threaded: &HashSet<String>,
    scope: &Scope<'a>,
) -> KExpression<'a> {
    let parts = inner
        .parts
        .iter()
        .map(|p| {
            let value = match &p.value {
                ExpressionPart::Type(t) if threaded.contains(t.as_str()) => {
                    let obj = scope
                        .arena
                        .alloc_object(KObject::KTypeValue(KType::RecursiveRef(t.render())));
                    ExpressionPart::Future(obj)
                }
                ExpressionPart::SigiledTypeExpr(b) => ExpressionPart::SigiledTypeExpr(Box::new(
                    rewrite_threaded_self_refs(b, threaded, scope),
                )),
                // A `:{…}` nested inside a sub-dispatched sigil (`:(LIST OF :{x :Self})`)
                // must thread its self-references the same way before it sub-dispatches.
                ExpressionPart::RecordType(b) => ExpressionPart::RecordType(Box::new(
                    rewrite_threaded_self_refs(b, threaded, scope),
                )),
                other => other.clone(),
            };
            Spanned {
                value,
                span: p.span,
            }
        })
        .collect();
    KExpression::new(parts)
}
