//! A `MATCH … UNDER`'s arms against its union clause, whose variants
//! [`variants`](crate::scope::variants) reads.
//!
//! [`arms_under`] checks a `MATCH … UNDER`'s arms against its clause beside the guards' check
//! ([`guards`](super::guards)), wherever the walk closes the clause: its labels and its coverage.
//!
//! See [elaborate/README.md](README.md).

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{
    BodyShape, Guard, ShapeError, Static, StaticType, TypeExpression, each_part, source_of,
    variant_label, variant_named, variants,
};
use crate::values::Chain;

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
            return Err(ShapeError::NotUnion {
                clause: union.into(),
                at,
            });
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
