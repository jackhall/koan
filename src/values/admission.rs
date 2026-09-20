//! Slot admission: whether a type slot takes a value, a raw AST part, or a working part — the type
//! dispatch reads off a raw part — the one rule a newtype construction is checked by, the one rule
//! a member sealed behind an opaque view's barrier is checked by, and the one rule each container
//! kind's memo is derived by.
//!
//! A value is checked by the one lattice relation over its memoized type, never by walking its
//! contents. A raw part is checked by shape, since an unevaluated literal has no value yet.

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::{BinderSymbol, ExpressionPart, KLiteral};
use crate::type_lattice::{
    Collector, KKind, KType, NodeSchema, TypeNode, TypeRegistry, Variance, admits_with, join,
    satisfied_by,
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

/// What a newtype construction `(Head payload)` refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstructionRefused {
    /// The head names no newtype: not a declared nominal, or one whose schema is a type constructor.
    NotNewType(KType),
    /// The payload's type does not satisfy the newtype's representation.
    Misfit {
        identity: KType,
        representation: KType,
    },
}

/// The identity a construction whose head denotes `head` produces over a payload of type `payload`:
/// `head` itself, when it is a newtype whose representation `payload` satisfies. Every construction
/// is checked here — an ordinary one through [`Tagged::construct`](super::Tagged::construct), a
/// knot's tagged node by the tie over its derived payload type.
pub fn construction(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    head: KType,
    payload: KType,
) -> Result<KType, ConstructionRefused> {
    let TypeNode::SetMember {
        schema: NodeSchema::NewType(representation),
        ..
    } = types.node(head)
    else {
        return Err(ConstructionRefused::NotNewType(head));
    };
    if satisfied_by(types, scratch, representation, payload) {
        Ok(head)
    } else {
        Err(ConstructionRefused::Misfit {
            identity: head,
            representation,
        })
    }
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
    let minted = match types.node(mint) {
        TypeNode::AbstractType { nonce, .. } => nonce.is_some(),
        TypeNode::ConstructorApply { constructor, .. } => matches!(
            types.node(constructor),
            TypeNode::AbstractType { nonce: Some(_), .. }
        ),
        _ => false,
    };
    if !minted {
        return Err(SealRefused::NotAMint(mint));
    }
    if satisfied_by(types, scratch, witness, payload) {
        Ok(mint)
    } else {
        Err(SealRefused::Misfit { mint, witness })
    }
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
/// a kind slot takes a type token only for `ProperType` and `AnyType`; a quantified slot is the top
/// and takes every shape. A function, nominal, signature, shape, constructor-application, deferred
/// or sibling slot admits no raw part — only a resolved value.
pub fn admits_part(slot: KType, part: &ExpressionPart<'_>, types: &TypeRegistry<'_>) -> bool {
    match types.node(slot) {
        TypeNode::Any | TypeNode::Quantified { .. } => true,
        TypeNode::Never => false,
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
