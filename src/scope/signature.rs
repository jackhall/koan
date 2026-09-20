//! What a callable's signature declares for its body: `<name> :<Type>` pairs and a record schema's
//! fields as parameters, `FOR ALL` names as type parameters, `_` as nothing.
//!
//! The signature's *types* are read where the form runs, so they are mentions of the enclosing
//! shape; only the names are the body's.

use crate::memory::BumpVec;
use crate::parse::{ExpressionPart, KExpression};
use crate::symbols::{BinderSymbol, WILDCARD};

/// The declared name of the `<name> :<Type>` pair starting at `index`, and whether one starts there:
/// `Some(None)` for a `_` pair, which declares nothing.
pub(crate) fn pair_name(run: &KExpression<'_>, index: usize) -> Option<Option<BinderSymbol>> {
    let parts = run.parts;
    let name = match parts[index].value {
        ExpressionPart::Identifier(name) => Some(BinderSymbol::Value(name)),
        ExpressionPart::Type(name) => Some(BinderSymbol::Type(name)),
        ExpressionPart::Keyword(symbol) if symbol == WILDCARD.symbol() => None,
        _ => return None,
    };
    let typed = parts.get(index + 1).is_some_and(|part| {
        matches!(
            part.value,
            ExpressionPart::Type(_)
                | ExpressionPart::Expression(_)
                | ExpressionPart::SigiledTypeExpr(_)
                | ExpressionPart::RecordType(_)
        )
    });
    typed.then_some(name)
}

/// The run of pairs a signature part holds: a `:{…}` schema's field list or an `EXPR` head.
pub(crate) fn signature_run<'graph>(
    part: &ExpressionPart<'graph>,
) -> Option<&'graph KExpression<'graph>> {
    match part {
        ExpressionPart::RecordType(node) | ExpressionPart::Expression(node) => {
            Some(node.reference())
        }
        _ => None,
    }
}

/// Push every name `part` declares as a signature onto `into`.
pub(crate) fn declare_parameters(part: &ExpressionPart<'_>, into: &mut BumpVec<'_, BinderSymbol>) {
    let Some(run) = signature_run(part) else {
        return;
    };
    let mut index = 0;
    while index < run.parts.len() {
        match pair_name(run, index) {
            Some(name) => {
                into.extend(name);
                index += 2;
            }
            None => index += 1,
        }
    }
}

/// Push every type parameter a `FOR ALL` group declares onto `into`.
pub(crate) fn declare_quantifiers(part: &ExpressionPart<'_>, into: &mut BumpVec<'_, BinderSymbol>) {
    let ExpressionPart::Expression(group) = part else {
        return;
    };
    into.extend(group.parts.iter().filter_map(|part| match part.value {
        ExpressionPart::Type(name) => Some(BinderSymbol::Type(name)),
        _ => None,
    }));
}

/// The statements of a body part, if it is a parenthesized body.
pub(crate) fn body_of<'graph>(
    part: &ExpressionPart<'graph>,
) -> Option<&'graph KExpression<'graph>> {
    match part {
        ExpressionPart::Expression(node) => Some(node.reference()),
        _ => None,
    }
}
