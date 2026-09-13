//! Slot admission: whether a type slot takes a value, a raw AST part, or a working part — and the
//! type dispatch reads off a raw part.
//!
//! A value is checked by the one lattice relation over its memoized type, never by walking its
//! contents. A raw part is checked by shape, since an unevaluated literal has no value yet.

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::{ExpressionPart, KLiteral};
use crate::type_lattice::{
    Collector, KKind, KType, TypeNode, TypeRegistry, Variance, admits_with, join, satisfied_by,
};

use super::{Value, WorkingPart};

/// Whether `slot` takes `value`: one relation over the value's memoized type. A slot reading a
/// quantifier admits by unification against that type under a fresh collector, which checks the
/// shape alone: a variable's bound, and two slots of one call agreeing on it, are what the caller's
/// own collector checks when it solves.
pub fn satisfies(
    slot: KType,
    value: &Value<'_, '_>,
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
        ExpressionPart::ListLiteral(items) => {
            types.list(joined(items.iter().map(element), types, scratch))
        }
        ExpressionPart::DictLiteral(pairs) => types.dict(
            joined(pairs.iter().map(|(key, _)| element(key)), types, scratch),
            joined(
                pairs.iter().map(|(_, value)| element(value)),
                types,
                scratch,
            ),
        ),
        ExpressionPart::RecordLiteral(fields) => {
            let mut field_types = BumpVec::with_capacity_in(fields.len(), scratch);
            field_types.extend(fields.iter().map(|(name, value)| (*name, element(value))));
            types.record(scratch, &field_types)
        }
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
pub fn admits(
    slot: KType,
    part: &WorkingPart<'_, '_>,
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
