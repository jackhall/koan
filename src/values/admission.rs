//! Slot admission: whether a type slot takes a value, a raw AST part, or a working part — the type
//! dispatch reads off a raw part — the one rule a construction is checked by, the one rule
//! a member sealed behind an opaque view's barrier is checked by, and the one rule each container
//! kind's memo is derived by.
//!
//! A value is checked by the one lattice relation over its memoized type, never by walking its
//! contents. A raw part is checked by shape, since an unevaluated literal has no value yet.
//!
//! [`unsealed`] is the one peel: a value sealed behind an opaque view read through each seal whose
//! bound reveals the payload's kind.

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::{ExpressionPart, KLiteral};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    Collector, KKind, KType, NodeSchema, TypeNode, TypeRegistry, Variance, admits_with,
    is_subtype_of, join, satisfied_by,
};

use super::{Knotted, Value, WorkingPart};

/// Whether `slot` takes `value`: one relation over the value's memoized type. A slot reading a
/// quantifier admits by unification against that type under a fresh collector, which checks the
/// shape alone: a variable's bound, and two slots of one call agreeing on it, are what the caller's
/// own collector checks when it solves.
pub fn satisfies<X: Knotted>(
    slot: KType,
    value: &Value<'_, '_, X>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> bool {
    let carried = value.ktype();
    if types.contains_quantified(slot) {
        let mut collector = Collector::new(scratch, 0);
        return admits_with(types, scratch, slot, carried, Variance::Co, &mut collector).is_ok();
    }
    satisfied_by(types, scratch, slot, carried)
}

/// What a construction `(Head payload)` refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstructionRefused {
    /// The head names nothing a construction builds: no newtype, no family with a representation.
    /// An application of a family is no head either.
    NotConstructible(KType),
    /// The payload's type does not satisfy the newtype's representation.
    Misfit {
        identity: KType,
        representation: KType,
    },
    /// The payload's type cannot be solved against the family's representation: a structural
    /// mismatch, or contributions to one parameter with no maximum.
    Unsolved { family: KType, payload: KType },
}

/// The identity a construction whose head denotes `head` produces over a payload of type `payload`.
/// Every construction is checked here — an ordinary one through
/// [`Tagged::construct`](super::Tagged::construct), a knot's tagged node by the tie over its derived
/// payload type.
///
/// A newtype gives `head` itself, when `payload` satisfies its representation. A family with a
/// representation gives its application at the least arguments `payload` solves the representation
/// to, a parameter the payload does not reach taking `Never`: `Boxed` over a number gives
/// `:(Boxed {Type = Number})`.
pub fn construction(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    head: KType,
    payload: KType,
) -> Result<KType, ConstructionRefused> {
    match types.node(head) {
        TypeNode::SetMember {
            schema: NodeSchema::NewType(representation),
            ..
        } => {
            if satisfied_by(types, scratch, representation, payload) {
                Ok(head)
            } else {
                Err(ConstructionRefused::Misfit {
                    identity: head,
                    representation,
                })
            }
        }
        TypeNode::SetMember {
            schema:
                NodeSchema::TypeConstructor {
                    representation: Some(representation),
                    param_names,
                },
            ..
        } => {
            let mut collector = Collector::least(scratch, param_names.len());
            let solved = admits_with(
                types,
                scratch,
                representation,
                payload,
                Variance::Co,
                &mut collector,
            )
            .ok()
            .and_then(|()| collector.solve(types).ok())
            .ok_or(ConstructionRefused::Unsolved {
                family: head,
                payload,
            })?;
            let mut arguments = BumpVec::with_capacity_in(param_names.len(), scratch);
            arguments.extend(
                param_names
                    .iter()
                    .zip(solved.iter())
                    .map(|(name, ktype)| (BinderSymbol::Type(*name), *ktype)),
            );
            Ok(types.constructor_apply(scratch, head, &arguments))
        }
        _ => Err(ConstructionRefused::NotConstructible(head)),
    }
}

/// Whether a construction through `head` solves its identity from its payload — `head` names a
/// family with a representation — rather than taking `head` itself: a newtype, or a head that
/// constructs nothing, which the check then refuses. The tie reads the latter two as a cut.
pub fn solves_identity(types: &TypeRegistry<'_>, head: KType) -> bool {
    matches!(
        types.node(head),
        TypeNode::SetMember {
            schema: NodeSchema::TypeConstructor {
                representation: Some(_),
                ..
            },
            ..
        }
    )
}

/// What sealing a payload under an opaque mint refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SealRefused {
    /// The identity is no per-application mint: not a nonced abstract type, nor an application of
    /// one.
    NotAMint(KType),
    /// The payload's type does not satisfy what the source binds the member to.
    Misfit { mint: KType, witness: KType },
}

/// The identity a payload of type `payload` takes when sealed under `mint`, the per-application
/// abstract type an opaque ascription minted for a member the source binds to `witness`.
///
/// The barrier's rule, beside [`construction`]: an abstract type records no representation for a
/// construction to check against, so what is checked is the source's own binding. Sealing happens
/// where a view is built, never where a koan program writes a construction.
pub fn sealing(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    mint: KType,
    witness: KType,
    payload: KType,
) -> Result<KType, SealRefused> {
    if !is_mint(types, mint) {
        return Err(SealRefused::NotAMint(mint));
    }
    if satisfied_by(types, scratch, witness, payload) {
        Ok(mint)
    } else {
        Err(SealRefused::Misfit { mint, witness })
    }
}

/// Whether `ktype` is a per-application mint: a nonced abstract type, or an application of one.
fn is_mint(types: &TypeRegistry<'_>, ktype: KType) -> bool {
    match types.node(ktype) {
        TypeNode::AbstractType { nonce, .. } => nonce.is_some(),
        TypeNode::ConstructorApply { constructor, .. } => matches!(
            types.node(constructor),
            TypeNode::AbstractType { nonce: Some(_), .. }
        ),
        _ => false,
    }
}

/// `value` read through its seal where the seal's bound licenses it. An opaque view's seal is
/// transparent exactly where the member's bound reveals the kind of value it holds — where the mint
/// lies under its payload's kind. A seal bounded by `Value`, or by a union spanning kinds, stays.
/// A seal re-tags rather than wraps ([`Tagged::seal`](super::Tagged::seal)), so there is one layer
/// to read through. Equality and dict keys read a value through this.
pub fn unsealed<'graph, 'cell, X: Knotted>(
    value: Value<'graph, 'cell, X>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Value<'graph, 'cell, X> {
    let Value::Tagged(tagged) = value else {
        return value;
    };
    let payload = *tagged.payload();
    let revealed = is_mint(types, tagged.ktype())
        && kind_of(&payload, types, scratch)
            .is_some_and(|kind| is_subtype_of(types, scratch, tagged.ktype(), kind));
    if revealed { payload } else { value }
}

/// The top of `value`'s own kind — `Number` for a number, `LIST OF Any` for a list, the empty
/// record for a record — or `None` for a knot member, which no seal reads through.
fn kind_of<X: Knotted>(
    value: &Value<'_, '_, X>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Option<KType> {
    Some(match value {
        Value::Number(_) => KType::NUMBER,
        Value::Bool(_) => KType::BOOL,
        Value::Null => KType::NULL,
        Value::Str(_) => KType::STR,
        Value::Expression(_) => KType::KEXPRESSION,
        Value::Type(_) => KType::ANY_TYPE,
        Value::List(_) => KType::LIST_OF_ANY,
        Value::Dict(_) => KType::DICT_ANY_ANY,
        Value::Record(_) => types.record(scratch, &[]),
        Value::Tagged(tagged) => tagged.ktype(),
        Value::Knotted(_) => return None,
    })
}

/// The type dispatch matches a raw part on, and the one a diagnostic naming the slot renders. `None`
/// for a keyword, which fills no slot. A literal is its leaf, a container literal the join of its
/// elements' part types — the rule its value will memoize once it evaluates — a name token
/// `Identifier`, a parenthesized or quoted body `KExpression`, a sigiled body its slot leaf and a
/// type token `ProperType`, the kind a type name denotes.
pub fn part_ktype(
    part: &ExpressionPart<'_>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Option<KType> {
    let element =
        |part: &ExpressionPart<'_>| part_ktype(part, types, scratch).unwrap_or(KType::ANY);
    Some(match part {
        ExpressionPart::Keyword(_) => return None,
        ExpressionPart::Literal(KLiteral::Number(_)) => KType::NUMBER,
        ExpressionPart::Literal(KLiteral::String(_)) => KType::STR,
        ExpressionPart::Literal(KLiteral::Boolean(_)) => KType::BOOL,
        ExpressionPart::Literal(KLiteral::Null) => KType::NULL,
        ExpressionPart::ListLiteral(items) => list_type(types, scratch, items.iter().map(element)),
        ExpressionPart::DictLiteral(pairs) => dict_type(
            types,
            scratch,
            pairs
                .iter()
                .map(|(key, value)| (element(key), element(value))),
        ),
        ExpressionPart::RecordLiteral(fields) => record_type(
            types,
            scratch,
            fields.iter().map(|(name, value)| (*name, element(value))),
        ),
        ExpressionPart::Identifier(_) => KType::IDENTIFIER,
        ExpressionPart::Expression(_) | ExpressionPart::QuotedExpression(_) => KType::KEXPRESSION,
        ExpressionPart::SigiledTypeExpr(_) => KType::SIGILED_TYPE_EXPR,
        ExpressionPart::RecordType(_) => KType::RECORD_TYPE,
        ExpressionPart::Type(_) => KType::PROPER_TYPE,
    })
}

fn joined(
    items: impl Iterator<Item = KType>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> KType {
    items.fold(KType::NEVER, |acc, item| join(types, scratch, acc, item))
}

/// The type a list over cells of types `elements` memoizes: the list of their join, `Never` for
/// none. Every list's memo is derived here — [`List::new`](super::List::new)'s over its cells, and
/// the tie's over a data node's staged cells.
pub fn list_type(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    elements: impl Iterator<Item = KType>,
) -> KType {
    types.list(joined(elements, types, scratch))
}

/// The type a dict over entries of key and value types `entries` memoizes: the dict of each side's
/// join. Every dict's memo is derived here, over the entries a repeated key leaves.
pub fn dict_type(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    entries: impl Iterator<Item = (KType, KType)>,
) -> KType {
    let (keys, values) = entries.fold((KType::NEVER, KType::NEVER), |(keys, values), entry| {
        (
            join(types, scratch, keys, entry.0),
            join(types, scratch, values, entry.1),
        )
    });
    types.dict(keys, values)
}

/// The type a record over `fields` memoizes: the record of each field's type. Every record's memo
/// is derived here.
pub fn record_type(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    fields: impl ExactSizeIterator<Item = (BinderSymbol, KType)>,
) -> KType {
    let mut field_types = BumpVec::with_capacity_in(fields.len(), scratch);
    field_types.extend(fields);
    types.record(scratch, &field_types)
}

/// Whether `slot` takes a raw part, by shape. An unevaluated container literal admits on its kind
/// alone, since its element types are unknown until it runs; a union admits what any member admits;
/// a family top admits what some concrete type of its family admits, as a union does; a kind slot
/// takes a type token only for `ProperType` and `AnyType`; a quantified slot takes what its bound
/// takes. A function, nominal, signature, shape, constructor-application, deferred or sibling slot
/// admits no raw part — only a resolved value.
pub fn admits_part(slot: KType, part: &ExpressionPart<'_>, types: &TypeRegistry<'_>) -> bool {
    match types.node(slot) {
        TypeNode::Any => true,
        TypeNode::Quantified { bound, .. } => admits_part(bound, part, types),
        TypeNode::Never => false,
        TypeNode::AnyValue => matches!(
            part,
            ExpressionPart::Literal(_)
                | ExpressionPart::ListLiteral(_)
                | ExpressionPart::DictLiteral(_)
                | ExpressionPart::RecordLiteral(_)
        ),
        TypeNode::AnyCode => matches!(
            part,
            ExpressionPart::Identifier(_)
                | ExpressionPart::Type(_)
                | ExpressionPart::Expression(_)
                | ExpressionPart::QuotedExpression(_)
                | ExpressionPart::SigiledTypeExpr(_)
                | ExpressionPart::RecordType(_)
        ),
        TypeNode::Number => matches!(part, ExpressionPart::Literal(KLiteral::Number(_))),
        TypeNode::Str => matches!(part, ExpressionPart::Literal(KLiteral::String(_))),
        TypeNode::Bool => matches!(part, ExpressionPart::Literal(KLiteral::Boolean(_))),
        TypeNode::Null => matches!(part, ExpressionPart::Literal(KLiteral::Null)),
        TypeNode::List { .. } => matches!(part, ExpressionPart::ListLiteral(_)),
        TypeNode::Dict { .. } => matches!(part, ExpressionPart::DictLiteral(_)),
        TypeNode::Record { .. } => matches!(part, ExpressionPart::RecordLiteral(_)),
        TypeNode::Identifier => matches!(part, ExpressionPart::Identifier(_)),
        TypeNode::NameToken => {
            matches!(
                part,
                ExpressionPart::Identifier(_) | ExpressionPart::Type(_)
            )
        }
        TypeNode::TypeNameToken => matches!(part, ExpressionPart::Type(_)),
        TypeNode::KExpression => matches!(
            part,
            ExpressionPart::Expression(_) | ExpressionPart::QuotedExpression(_)
        ),
        TypeNode::SigiledTypeExpr => matches!(part, ExpressionPart::SigiledTypeExpr(_)),
        TypeNode::RecordType => matches!(part, ExpressionPart::RecordType(_)),
        TypeNode::OfKind(kind) => {
            matches!(part, ExpressionPart::Type(_))
                && matches!(kind, KKind::ProperType | KKind::AnyType)
        }
        TypeNode::Union { members } => members
            .iter()
            .any(|member| admits_part(*member, part, types)),
        TypeNode::KFunction { .. }
        | TypeNode::SetMember { .. }
        | TypeNode::AbstractType { .. }
        | TypeNode::Signature { .. }
        | TypeNode::ExpressionShape { .. }
        | TypeNode::ConstructorApply { .. }
        | TypeNode::DeferredReturn(_)
        | TypeNode::Sibling(_) => false,
    }
}

/// Whether `slot` takes a working part: an AST part by shape, a spliced value by its type. A node
/// the scheduler synthesized and a staging hole denote no value yet, so only an `Any` slot takes one.
pub fn admits<X: Knotted>(
    slot: KType,
    part: &WorkingPart<'_, '_, X>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> bool {
    match part {
        WorkingPart::Ast(part) => admits_part(slot, part, types),
        WorkingPart::Spliced { value, .. } => satisfies(slot, value, types, scratch),
        WorkingPart::Expression(_) | WorkingPart::RecordType(_) | WorkingPart::StagedSlot => {
            slot == KType::ANY
        }
    }
}
