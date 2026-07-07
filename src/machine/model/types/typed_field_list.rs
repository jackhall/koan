//! Shared parser for `(<name> :<Type> <name> :<Type> ...)` schema expressions, used by
//! `UNION` (order discarded into a `HashMap<tag, KType>`) and `STRUCT` (order preserved for
//! positional construction).

use super::ktype::KType;
use super::resolver::{elaborate_type_identifier, Elaborator, TypeResolution};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::values::Carried;
use crate::machine::model::Parseable;
use crate::machine::model::Record;
use crate::machine::{NodeId, Scope};
use crate::parse::parse_pair_list;
pub use crate::parse::FieldNameKind;
use crate::source::Spanned;
use std::collections::HashSet;

pub enum FieldListOutcome<'a> {
    Done(Vec<(String, KType<'a>)>),
    /// `sub_dispatches` carries each sigil field's wrapped expression in DFS walk
    /// order. The caller schedules them in that order and, on the dep-finish re-walk,
    /// feeds the resolved `Carried::Type`s back through a [`ResultFeed`] — the walk
    /// re-descends in the same order, so no slot index is needed.
    Pending {
        park_producers: Vec<NodeId>,
        sub_dispatches: Vec<KExpression<'a>>,
    },
    Err(String),
}

/// Walk-order feed of resolved sub-dispatch carriers for the dep-finish re-walk: the
/// re-walk replays the first walk's DFS traversal and [`pop`](ResultFeed::pop)s each
/// carrier back in. A concrete cursor (not a `dyn Iterator`) so it reborrows cleanly when a
/// nested record recurses through the shared walker.
pub struct ResultFeed<'b, 'a> {
    results: &'b [Carried<'a>],
    pos: usize,
}

impl<'b, 'a> ResultFeed<'b, 'a> {
    pub fn new(results: &'b [Carried<'a>]) -> Self {
        ResultFeed { results, pos: 0 }
    }

    fn pop(&mut self) -> Option<Carried<'a>> {
        let next = self.results.get(self.pos).copied();
        if next.is_some() {
            self.pos += 1;
        }
        next
    }
}

/// Entry point used by STRUCT / UNION / FN / FUNCTOR. Routes each field type through the
/// scheduler-aware [`elaborate_type_identifier`], accumulating parking producers and
/// pending sub-Dispatches across the whole walk so the caller installs one dep-finish for
/// the merged set. `name_kind` selects valid field-name tokens (STRUCT / UNION pass
/// `Identifier`; FN / FUNCTOR pass `IdentifierOrType` to accept capitalized type-parameter
/// names).
///
/// `results` is `None` on the first walk (each sigil field schedules a sub-Dispatch) and
/// `Some` on the re-walk (each consumes the next resolved carrier in DFS order). The
/// re-walk re-descends in the same deterministic order, so positional consumption needs no
/// slot index and nested field-lists fall out for free.
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
            ExpressionPart::Type(t) => match elaborate_type_identifier(elaborator, t) {
                TypeResolution::Done(kt) => Ok(kt),
                TypeResolution::Park(producers) => {
                    parks.extend(producers);
                    // Placeholder, discarded under Pending; lets the walk collect every
                    // parking producer in one pass.
                    Ok(KType::Any)
                }
                TypeResolution::Unbound(msg) => Err(format!("{msg} in {context} for `{}`", name)),
            },
            // Sigils sub-Dispatch through the standalone dispatcher, which carries no SCC
            // context, so self-references are pre-resolved to `RecursiveRef` carriers first
            // (see `rewrite_threaded_self_refs`).
            ExpressionPart::SigiledTypeExpr(boxed) => {
                match results.as_mut().and_then(|feed| feed.pop()) {
                    // Re-walk: the `Type`-arm is the single guard rejecting a sub that
                    // resolved to a value-by-expression.
                    Some(Carried::Type(kt)) => Ok(kt.clone()),
                    Some(Carried::Object(other)) => Err(format!(
                        "{context} type for `{}` resolved to non-type value `{}`",
                        name,
                        other.summarize(),
                    )),
                    None if results.is_some() => Err(format!(
                        "{context}: dep-finish re-walk found fewer resolved sub-dispatches than slots",
                    )),
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
            // A nested record type `:{…}` elaborates inline through this same walker,
            // sharing the elaborator and `results` feed; its parks / sub-dispatches merge
            // into the outer set. No sub-Dispatch of the record node, no slot bookkeeping.
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
            // A spliced cell is adopted into the elaborating scope (folding its reach),
            // then routed through type/non-type handling.
            ExpressionPart::Spliced { cell, .. } => match elaborator.scope.adopt_sealed(cell) {
                Carried::Type(kt) => Ok(kt.clone()),
                Carried::Object(other) => Err(format!(
                    "{context} type for `{}` resolved to non-type value `{}`",
                    name,
                    other.summarize(),
                )),
            },
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

/// Pre-resolve self-references inside a keyworded sigil body before it sub-Dispatches into
/// the standalone dispatcher, which carries no SCC threading context. Every bare
/// `Type(name)` leaf whose `name` is in `threaded` becomes a `Spliced` cell sealing a
/// `RecursiveRef(name)` carrier, so `STRUCT Tree = (children :(LIST OF Tree))` lowers `Tree`
/// to `RecursiveRef` instead of parking on its own placeholder and closing a
/// scheduler-deadlock cycle. Recurses into nested sigils and records; non-threaded names
/// are left for the dispatcher.
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
                    // Minted fresh in this scope's region and spliced into a sub-dispatched
                    // expression (it crosses into another node), so it travels as a cell: a
                    // region-resident type carrier reaching nothing foreign (empty reach).
                    let obj = scope.brand().alloc_ktype(KType::RecursiveRef(t.render()));
                    // A region-resident carrier: the delivery envelope's pin is this scope's own
                    // region owner (the seal-resident veneer), not a separate producer frame.
                    ExpressionPart::Spliced {
                        cell: scope
                            .seal_resident_delivered(scope.resident_type_carrier(obj, None, false)),
                    }
                }
                ExpressionPart::SigiledTypeExpr(b) => ExpressionPart::SigiledTypeExpr(Box::new(
                    rewrite_threaded_self_refs(b, threaded, scope),
                )),
                // A record nested inside a sub-dispatched sigil must thread its
                // self-references the same way.
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
