//! A `MATCH … UNDER`'s arms against its union clause's variants.
//!
//! [`arms_under`] checks a `MATCH … UNDER`'s arms against its clause beside the guards' check
//! ([`guards`](super::guards)), once, where the program loads: its labels and its coverage, over
//! the variants the clause's text shows. A family applied at a run-bound name shows its variants
//! whatever the name is bound to, and a union declared over one shows its declaration's; a bare
//! run-bound name shows none, so it is no union.
//!
//! See [elaborate/README.md](README.md).

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{
    AsWritten, BodyShape, Guard, ShapeError, Static, StaticType, TypeExpression, each_part,
    source_of,
};
use crate::symbols::Symbol;
use crate::type_lattice::Parametric;

/// Check each `MATCH … UNDER` among `shape`'s `expressions` whose union clause `typed` knows: the
/// clause shows variants (`shown`: each one's label, beside the variant), each label names one of
/// them, and, where no `_` arm is written, each has an arm — refusing the first failure in program
/// order. A clause the load does not know, past a quote's chain, is code another load checks.
pub fn arms_under<'graph, 'o, 'x>(
    scratch: BumpAllocator<'x>,
    shape: &'graph BodyShape<'graph>,
    expressions: &[&TypeExpression<'graph>],
    typed: impl Fn(&TypeExpression<'graph>) -> StaticType<'o>,
    shown: impl Fn(Parametric) -> Option<BumpVec<'x, (Option<Symbol>, Parametric)>>,
    written: impl Fn(Parametric) -> Option<AsWritten>,
) -> Result<(), ShapeError<'o>> {
    for expression in expressions.iter().filter(|expression| expression.clause) {
        let union: Parametric = match typed(expression) {
            Static::Closed(union) => union.into(),
            Static::Rigid { value, .. } => value,
            Static::Unknown => continue,
        };
        let statement = &shape.body()[expression.statement as usize];
        let at = source_of(shape, statement, expression.site).unwrap_or(statement.source);
        let Some(variants) = shown(union) else {
            return Err(ShapeError::NotUnion {
                clause: union,
                written: written(union),
                at,
            });
        };
        let node = clause_node(statement, expression.part);
        let mut labels = BumpVec::new_in(scratch);
        let mut defaulted = false;
        for guard in arm_guards(shape, node) {
            match guard {
                Guard::Label(label) if !variants.iter().any(|(named, _)| *named == Some(label)) => {
                    return Err(ShapeError::NoVariant { label, union, at });
                }
                Guard::Label(label) => labels.push(label),
                Guard::Default => defaulted = true,
                Guard::Type(_) | Guard::Value(_) => {
                    unreachable!("a `MATCH … UNDER` arm is guarded by a label or `_`")
                }
            }
        }
        let uncovered = (variants.iter())
            .find(|(label, _)| label.is_none_or(|label| !labels.contains(&label)))
            .map(|(_, variant)| *variant);
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
