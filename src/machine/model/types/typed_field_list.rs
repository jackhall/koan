//! Shared parser for `(<name> :<Type> <name> :<Type> ...)` schema expressions, used by
//! `UNION` (order discarded into a `HashMap<tag, KType>`) and `STRUCT` (order preserved for
//! positional construction).

use super::ktype::KType;
use super::recursive_group_window::RecursiveGroupWindow;
use super::registry::TypeRegistry;
use super::resolver::{elaborate_type_identifier, Elaborator, TypeResolution};
use crate::machine::core::read_resting;
use crate::machine::model::ast::{
    ExpressionPart, FieldSlot, KExpression, Part, WorkingExpression, WorkingPart,
};
use crate::machine::model::values::Carried;
use crate::machine::model::Record;
use crate::machine::{NodeId, Scope};
use crate::parse::parse_pair_list;
pub use crate::parse::FieldNameKind;
use crate::source::Spanned;
use std::collections::HashSet;
use std::rc::Rc;

/// The two nouns a field-list diagnostic needs. `list` names the whole schema, for errors about
/// the list as a unit ("UNION schema: forward type reference still unresolved…"); `member` names
/// one entry of it in the singular, for errors about a single slot ("the type of UNION variant
/// `Circle` must be a proper type"). Every caller states both, so a slot-level diagnostic names
/// the construct the user actually wrote rather than the walker they happen to share.
#[derive(Clone, Copy)]
pub struct FieldListContext {
    pub list: &'static str,
    pub member: &'static str,
}

impl FieldListContext {
    /// A `UNION`'s variant schema: `UNION Shape = (Circle :Number …)`.
    pub const UNION_SCHEMA: Self = Self {
        list: "UNION schema",
        member: "UNION variant",
    };

    /// A `NEWTYPE`'s record representation: `NEWTYPE Boxed = :{v :Str}`.
    pub const NEWTYPE_RECORD_REPR: Self = Self {
        list: "NEWTYPE record repr",
        member: "NEWTYPE repr field",
    };

    /// The parameter list of an `:(FN …)` function type.
    pub const FN_TYPE_PARAMETERS: Self = Self {
        list: "FN parameters",
        member: "FN parameter",
    };

    /// A structural record type `:{x :Number}` — standalone, or nested inside another field list.
    /// The anonymous-FN signature `FN :{x :Number} -> …` elaborates through this one: its `:{…}`
    /// resolves as an ordinary record type before `FN` ever sees it.
    pub const RECORD_TYPE: Self = Self {
        list: "record fields",
        member: "record-type field",
    };
}

pub enum FieldListOutcome<'a> {
    Done(Vec<(String, KType)>),
    /// `sub_dispatches` carries each sigil field's body as the scheduler's own node, in DFS walk
    /// order — the currency an [`OwnedDispatch`](crate::machine::core::OwnedDispatch) takes. The
    /// caller schedules them in that order and, on the dep-finish re-walk, feeds the resolved
    /// `Carried::Type`s back through a [`ResultFeed`] — the walk re-descends in the same order, so
    /// no slot index is needed. A body naming a co-declared sibling carries that sibling's handle
    /// as a resolved cell ([`rewrite_threaded_self_refs`]), which is why the node is a
    /// [`WorkingExpression`] rather than raw AST.
    Pending {
        park_producers: Vec<NodeId>,
        sub_dispatches: Vec<WorkingExpression<'a>>,
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

/// A field list's parts run, from either expression family. A parsed list arrives as
/// [`Ast`](FieldParts::Ast); one whose co-declared references [`rewrite_threaded_self_refs`] has
/// already sealed into resolved cells arrives as [`Threaded`](FieldParts::Threaded).
#[derive(Clone, Copy)]
pub enum FieldParts<'a> {
    Ast(&'a [Spanned<ExpressionPart<'a>>]),
    Threaded(&'a [Spanned<WorkingPart<'a>>]),
}

impl<'a> FieldParts<'a> {
    /// The field list a parsed node spells.
    pub fn of(expr: &KExpression<'a>) -> Self {
        FieldParts::Ast(expr.parts)
    }

    /// The field list a threaded node spells.
    pub fn threaded(expr: &WorkingExpression<'a>) -> Self {
        FieldParts::Threaded(expr.parts)
    }
}

/// Entry point used by STRUCT / UNION / FN. Routes each field type through the
/// scheduler-aware [`elaborate_type_identifier`], accumulating parking producers and
/// pending sub-Dispatches across the whole walk so the caller installs one dep-finish for
/// the merged set. `name_kind` selects valid field-name tokens (STRUCT / UNION pass
/// `Identifier`; FN passes `IdentifierOrType` to accept capitalized type-parameter
/// names).
///
/// `results` is `None` on the first walk (each sigil field schedules a sub-Dispatch) and
/// `Some` on the re-walk (each consumes the next resolved carrier in DFS order). The
/// re-walk re-descends in the same deterministic order, so positional consumption needs no
/// slot index and nested field-lists fall out for free.
///
/// The two part families walk the same [`walk_field_list`]; this is the door that picks the
/// instantiation, and the one a nested record recurses back through when it descends into the
/// other family.
pub fn parse_typed_field_list_via_elaborator<'a, 'f>(
    parts: FieldParts<'a>,
    context: FieldListContext,
    name_kind: FieldNameKind,
    elaborator: &mut Elaborator<'_, 'a>,
    results: Option<&mut ResultFeed<'_, 'f>>,
    types: &TypeRegistry,
) -> FieldListOutcome<'a> {
    match parts {
        FieldParts::Ast(parts) => {
            walk_field_list(parts, context, name_kind, elaborator, results, types)
        }
        FieldParts::Threaded(parts) => {
            walk_field_list(parts, context, name_kind, elaborator, results, types)
        }
    }
}

fn walk_field_list<'a, 'f, P: Part<'a>>(
    parts: &'a [Spanned<P>],
    context: FieldListContext,
    name_kind: FieldNameKind,
    elaborator: &mut Elaborator<'_, 'a>,
    mut results: Option<&mut ResultFeed<'_, 'f>>,
    types: &TypeRegistry,
) -> FieldListOutcome<'a> {
    let mut parks: Vec<NodeId> = Vec::new();
    let mut sub_dispatches: Vec<WorkingExpression<'a>> = Vec::new();
    let FieldListContext {
        list: context_list,
        member: context_member,
    } = context;
    let parsed = parse_pair_list(parts, context_list, name_kind, |part, name| {
        // Every field types a value, so each field type must be a proper type; a bare
        // constructor of kind `* -> *` standing unapplied is a kind error. Applied to each
        // elaborated field on the way out, so the four arms below share one verdict — the
        // `KType::ANY` placeholders a `Pending` walk yields are proper and pass, and the
        // re-walk checks the resolved type they stand for.
        let checked = |kt: KType| match super::sig_schema::unsaturated_constructor_message(
            kt,
            &format!("the type of {context_member} `{name}`"),
            types,
        ) {
            Some(message) => Err(message),
            None => Ok(kt),
        };
        match part.field_slot() {
            FieldSlot::Type(t) => match elaborate_type_identifier(elaborator, &t, types) {
                TypeResolution::Done(kt) => checked(kt),
                TypeResolution::Park(producers) => {
                    parks.extend(producers);
                    // Placeholder, discarded under Pending; lets the walk collect every
                    // parking producer in one pass.
                    Ok(KType::ANY)
                }
                TypeResolution::Unbound(msg) => {
                    Err(format!("{msg} in {context_list} for `{}`", name))
                }
            },
            // A co-declared sibling `rewrite_threaded_self_refs` already sealed in. The cell rests
            // in the declarator's scope, which is parked on this very walk, so the handle is read
            // through the pin-less door: a `KType` is an interned registry handle borrowing nothing.
            FieldSlot::Resolved(cell) => read_resting(&cell, |carried| match carried {
                Carried::Type(kt) => checked(kt),
                other => Err(format!(
                    "{context_list} type for `{}` resolved to non-type value `{}`",
                    name,
                    other.summarize(types),
                )),
            }),
            // A sigil body whose co-declared references are already threaded dispatches as it
            // stands — the rewrite that produced it did what this arm's `Ast` peer does below.
            FieldSlot::ThreadedSigil(body) => {
                match results.as_mut().and_then(|feed| feed.pop()) {
                    Some(Carried::Type(kt)) => checked(kt),
                    Some(other @ (Carried::Object(_) | Carried::UnresolvedType(_))) => {
                        Err(format!(
                            "{context_list} type for `{}` resolved to non-type value `{}`",
                            name,
                            other.summarize(types),
                        ))
                    }
                    None if results.is_some() => Err(format!(
                        "{context_list}: dep-finish re-walk found fewer resolved sub-dispatches than slots",
                    )),
                    None => {
                        sub_dispatches.push(*body);
                        Ok(KType::ANY)
                    }
                }
            }
            // A threaded record body elaborates inline exactly as its `Ast` peer does, and for the
            // same reason: a record type is folded here, never sub-Dispatched as a whole.
            FieldSlot::ThreadedRecord(body) => {
                match parse_typed_field_list_via_elaborator(
                    FieldParts::threaded(body),
                    FieldListContext::RECORD_TYPE,
                    FieldNameKind::Identifier,
                    elaborator,
                    results.as_deref_mut(),
                    types,
                ) {
                    FieldListOutcome::Done(pairs) => Ok(types.record(Record::from_pairs(pairs))),
                    FieldListOutcome::Err(msg) => Err(msg),
                    FieldListOutcome::Pending {
                        park_producers,
                        sub_dispatches: inner_subs,
                    } => {
                        parks.extend(park_producers);
                        sub_dispatches.extend(inner_subs);
                        Ok(KType::ANY)
                    }
                }
            }
            // Sigils sub-Dispatch through the standalone dispatcher, which carries no window
            // context, so co-declared references are pre-resolved to sibling carriers first
            // (see `rewrite_threaded_self_refs`).
            FieldSlot::AstSigil(boxed) => {
                // `:(Tree Leaf)` while `Tree` is the binder under seal: a sibling-variant
                // reference. It cannot sub-dispatch (parking would deadlock on this very
                // seal's producer), so it lowers straight to the variant's relative `Sibling`
                // handle against the ambient window.
                if let [first, second] = boxed.parts {
                    if let (ExpressionPart::Type(head), ExpressionPart::Type(tag)) =
                        (&first.value, &second.value)
                    {
                        if elaborator.threaded.contains(head.as_str()) {
                            let window = elaborator.window().ok_or_else(|| {
                                format!(
                                    "{context_list}: `{}` names a co-declared member with no \
                                     open declaration window",
                                    tag.render(),
                                )
                            })?;
                            return Ok(window.sibling(
                                &tag.render(),
                                crate::machine::model::KKind::NewType,
                                types,
                            ));
                        }
                    }
                }
                match results.as_mut().and_then(|feed| feed.pop()) {
                    // Re-walk: the `Type`-arm is the single guard rejecting a sub that
                    // resolved to a value-by-expression.
                    Some(Carried::Type(kt)) => checked(kt),
                    Some(other @ (Carried::Object(_) | Carried::UnresolvedType(_))) => {
                        Err(format!(
                            "{context_list} type for `{}` resolved to non-type value `{}`",
                            name,
                            other.summarize(types),
                        ))
                    }
                    None if results.is_some() => Err(format!(
                        "{context_list}: dep-finish re-walk found fewer resolved sub-dispatches than slots",
                    )),
                    // The body dispatches directly — the sigil wrapper's own handler does no more
                    // than hand its body to the dispatch entry.
                    None => {
                        sub_dispatches.push(rewrite_threaded_self_refs(
                            boxed,
                            &elaborator.threaded,
                            elaborator.scope,
                            elaborator.window().as_ref(),
                            types,
                        ));
                        Ok(KType::ANY)
                    }
                }
            }
            // A nested record type `:{…}` elaborates inline through this same walker,
            // sharing the elaborator and `results` feed; its parks / sub-dispatches merge
            // into the outer set. No sub-Dispatch of the record node, no slot bookkeeping.
            FieldSlot::AstRecord(boxed) => {
                match parse_typed_field_list_via_elaborator(
                    FieldParts::of(boxed),
                    FieldListContext::RECORD_TYPE,
                    FieldNameKind::Identifier,
                    elaborator,
                    results.as_deref_mut(),
                    types,
                ) {
                    FieldListOutcome::Done(pairs) => Ok(types.record(Record::from_pairs(pairs))),
                    FieldListOutcome::Err(msg) => Err(msg),
                    FieldListOutcome::Pending {
                        park_producers,
                        sub_dispatches: inner_subs,
                    } => {
                        parks.extend(park_producers);
                        sub_dispatches.extend(inner_subs);
                        Ok(KType::ANY)
                    }
                }
            }
            FieldSlot::Name(_) | FieldSlot::Other => Err(format!(
                "{context_list} type for `{}` must be a type name token, got {}",
                name,
                Part::summarize(part)
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

/// True iff any `Type` leaf in `inner`'s subtree names a co-declared sibling — the gate deciding
/// whether a body needs rewriting at all. A body with none crosses to the working form as one slice
/// copy that keeps its cached shape.
fn names_threaded_self_ref(inner: &KExpression<'_>, threaded: &HashSet<String>) -> bool {
    inner.parts.iter().any(|part| match &part.value {
        ExpressionPart::Type(t) => threaded.contains(t.as_str()),
        ExpressionPart::Expression(body)
        | ExpressionPart::SigiledTypeExpr(body)
        | ExpressionPart::RecordType(body)
        | ExpressionPart::QuotedExpression(body) => names_threaded_self_ref(body, threaded),
        _ => false,
    })
}

/// The scheduler's node for a keyworded sigil body, with self-references pre-resolved: the
/// standalone dispatcher the body sub-Dispatches into carries no window context, so every bare
/// `Type(name)` leaf whose `name` is in `threaded` is written in as a resolved cell sealing that
/// name's relative sibling handle against `window`. `STRUCT Tree = (children :(LIST OF Tree))` then
/// lowers `Tree` to a `Sibling` back-edge instead of parking on its own placeholder and closing a
/// scheduler-deadlock cycle. Non-threaded names — and, with no open window, every name — are left
/// for the dispatcher.
///
/// Recurses into nested sigils **and nested record types**, which reach their own sub-Dispatches the
/// same window-less way.
fn rewrite_threaded_self_refs<'a>(
    inner: &KExpression<'a>,
    threaded: &HashSet<String>,
    scope: &Scope<'a>,
    window: Option<&Rc<RecursiveGroupWindow>>,
    types: &TypeRegistry,
) -> WorkingExpression<'a> {
    let brand = scope.brand();
    let Some(window) = window.filter(|_| names_threaded_self_ref(inner, threaded)) else {
        return WorkingExpression::from_ast(brand, *inner);
    };
    let parts = inner
        .parts
        .iter()
        .map(|p| {
            let value = match &p.value {
                ExpressionPart::Type(t) if threaded.contains(t.as_str()) => {
                    // The sibling handle is minted against the window here, where the window is in
                    // hand — the sub-dispatch it crosses into cannot reach one. The cell is a
                    // resident seal in this scope's own region: a type carrier reaching nothing
                    // foreign, so it rests with no coverage to lodge anywhere.
                    let sibling =
                        window.sibling(&t.render(), crate::machine::model::KKind::NewType, types);
                    WorkingPart::Spliced {
                        cell: scope.seal_resident::<crate::machine::model::CarriedFamily>(
                            Carried::Type(sibling),
                        ),
                    }
                }
                // A nested sigil threads its own self-references and rides as a synthesized node:
                // its slot dispatches the rewritten body, which is what the sigil wrapper's handler
                // does with a body it is handed.
                ExpressionPart::SigiledTypeExpr(body)
                    if names_threaded_self_ref(body, threaded) =>
                {
                    WorkingPart::Expression(brand.alloc_value(rewrite_threaded_self_refs(
                        body,
                        threaded,
                        scope,
                        Some(window),
                        types,
                    )))
                }
                // A nested record type threads its own self-references too, and keeps its
                // record-type slot class: its body is a field list its handler elaborates, so it
                // cannot ride the transparent sigil arm above.
                ExpressionPart::RecordType(body) if names_threaded_self_ref(body, threaded) => {
                    WorkingPart::RecordType(brand.alloc_value(rewrite_threaded_self_refs(
                        body,
                        threaded,
                        scope,
                        Some(window),
                        types,
                    )))
                }
                other => WorkingPart::Ast(*other),
            };
            Spanned {
                value,
                span: p.span,
            }
        })
        .collect();
    WorkingExpression::new(brand, parts)
}

/// The declared names of a `<name> <slot>` pair list, without elaborating any slot — what a
/// declarator pre-scans to announce its window's members before walking their types, so a
/// reference to a later-declared sibling already has a stable relative index.
pub fn pair_list_names(
    expr: &KExpression<'_>,
    context: &'static str,
    name_kind: FieldNameKind,
) -> Result<Vec<String>, String> {
    parse_pair_list(expr.parts, context, name_kind, |_, _| Ok(())).map(|pairs| {
        pairs
            .into_iter()
            .map(|(name, ())| name)
            .collect::<Vec<String>>()
    })
}
