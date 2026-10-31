//! A union's variants, as a `MATCH … UNDER` clause names them: the members of the clause's
//! `Union` node, in stored order. A label names the member of its name: a declared variant by its
//! own name, and a variant applied to arguments, as `:(Number AS Option)` applies each, by its
//! constructor's. A structural member, `Number` in `:(Maybe | Number)`, has no name, so no label
//! names it. Read through the lattice's public node doors only.
//!
//! [`arms_under`] checks a `MATCH … UNDER`'s arms against its clause beside the guards' check
//! ([`guards`](super::guards)), wherever the walk closes the clause: its labels and its coverage.
//!
//! See [elaborate/README.md](README.md).

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{
    BodyShape, Guard, ShapeError, Static, StaticType, TypeExpression, each_part, source_of,
};
use crate::symbols::Symbol;
use crate::type_lattice::{KType, TypeNode};
use crate::values::Chain;

/// `union`'s members in stored order, or `None` where `union` is no `Union` node.
pub fn variants<'tier>(
    types: &Chain<'tier, '_, '_, '_>,
    union: KType,
) -> Option<impl Iterator<Item = KType> + 'tier> {
    match types.node(union) {
        TypeNode::Union { members } => Some(members.iter()),
        _ => None,
    }
}

/// The member of `union` that `label` names, or `None` where none does or `union` is no union.
pub fn variant_named(types: &Chain<'_, '_, '_, '_>, union: KType, label: Symbol) -> Option<KType> {
    variants(types, union)?.find(|member| variant_label(types, *member) == Some(label))
}

/// The name a label gives `variant`: a declared variant's own, an applied one's constructor's, and
/// `None` for a structural member.
pub fn variant_label(types: &Chain<'_, '_, '_, '_>, variant: KType) -> Option<Symbol> {
    let declared = match types.node(variant) {
        TypeNode::ConstructorApply { constructor, .. } => constructor,
        _ => variant,
    };
    match types.node(declared) {
        TypeNode::SetMember { name, .. } => Some(name.symbol()),
        _ => None,
    }
}

/// Check each `MATCH … UNDER` among `shape`'s `expressions` whose union clause `typed` closes:
/// the clause is a union, each label names one of its variants, and, where no `_` arm is written,
/// each of its variants has an arm — refusing the first failure in program order. A clause the load
/// leaves unknown is checked where the binding that closes it is walked.
pub fn arms_under<'graph, 'o>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    shape: &'graph BodyShape<'graph>,
    expressions: &[&TypeExpression<'graph>],
    typed: impl Fn(&TypeExpression<'graph>) -> StaticType<'o>,
) -> Result<(), ShapeError<'o>> {
    for expression in expressions.iter().filter(|expression| expression.clause) {
        let Static::Closed(union) = typed(expression) else {
            continue;
        };
        let statement = &shape.body()[expression.statement as usize];
        let at = source_of(shape, statement, expression.site).unwrap_or(statement.source);
        if variants(types, union).is_none() {
            return Err(ShapeError::NotUnion { clause: union, at });
        }
        let node = clause_node(statement, expression.part);
        let mut labels = BumpVec::new_in(scratch);
        let mut defaulted = false;
        for guard in arm_guards(shape, node) {
            match guard {
                Guard::Label(label) if variant_named(types, union, label).is_none() => {
                    return Err(ShapeError::NoVariant { label, union, at });
                }
                Guard::Label(label) => labels.push(label),
                Guard::Default => defaulted = true,
                Guard::Type(_) | Guard::Value(_) => {
                    unreachable!("a `MATCH … UNDER` arm is guarded by a label or `_`")
                }
            }
        }
        let uncovered = (variants(types, union).into_iter().flatten()).find(|variant| {
            variant_label(types, *variant).is_none_or(|label| !labels.contains(&label))
        });
        if let Some(variant) = uncovered.filter(|_| !defaulted) {
            return Err(ShapeError::Uncovered {
                union,
                variant,
                at: node.source,
            });
        }
    }
    Ok(())
}

/// The `MATCH … UNDER` node within `statement` whose union clause is `clause`.
fn clause_node<'graph>(
    statement: &'graph KExpression<'graph>,
    clause: &ExpressionPart<'graph>,
) -> &'graph KExpression<'graph> {
    let holds = |node: &KExpression<'_>| matches!(node.parts, [_, _, _, written, _, _] if std::ptr::eq(&written.value, clause));
    let mut found = holds(statement).then_some(statement);
    each_part(statement, |part| {
        if let ExpressionPart::Expression(node) = part
            && holds(node)
        {
            found = Some(node);
        }
    });
    found.expect("a union clause is written in its `MATCH … UNDER`")
}

/// The guard of each arm of the `MATCH` `node` of `shape`, in written order.
pub fn arm_guards<'graph>(
    shape: &'graph BodyShape<'graph>,
    node: &'graph KExpression<'graph>,
) -> impl Iterator<Item = Guard<'graph>> + 'graph {
    arm_shapes(shape, node).map(|arm| arm.arm().expect("an arm knows it is one").guard)
}

/// The block of each arm of the `MATCH` `node` of `shape`, in written order.
pub fn arm_shapes<'graph>(
    shape: &'graph BodyShape<'graph>,
    node: &'graph KExpression<'graph>,
) -> impl Iterator<Item = &'graph BodyShape<'graph>> + 'graph {
    let Some(ExpressionPart::DictLiteral(arms)) = node.parts.last().map(|arms| &arms.value) else {
        unreachable!("a `MATCH`'s last part is its arm set")
    };
    (arms.iter()).map(|(_, arm)| shape.nested(shape.site(arm)).expect("an arm has a shape"))
}
