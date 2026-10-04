//! What a callable's signature declares for its body: the `<name> :<Type>` pairs of a quoted `EXPR`
//! head and a record schema's fields as parameters, the names of a `FOR ALL` list or dict — bounded
//! or not — as type parameters, `_` and a rank as nothing.
//!
//! The signature's *types*, a `FOR ALL` name's bound among them, are read where the form runs, so
//! they are mentions of the enclosing shape; only the names are the body's.

use crate::memory::BumpVec;
use crate::parse::{
    SlotLabel, declarator_parameters, next_is_type_slot, quantifier_entries, slot_label,
};

pub(crate) use crate::parse::quoted_body;
use crate::parse::{ExpressionPart, KExpression};
use crate::symbols::BinderSymbol;

/// The label of the `<label> :<Type>` pair starting at `index`, if one starts there: a name, `_`, or
/// a rank. Only a name declares anything.
pub(crate) fn pair_label(run: &KExpression<'_>, index: usize) -> Option<SlotLabel> {
    let label = slot_label(&run.parts[index].value)?;
    next_is_type_slot(run.parts, index + 1).then_some(label)
}

/// The run of pairs a signature part holds: a `:{…}` schema's field list or a quoted `EXPR` head.
pub(crate) fn signature_run<'graph>(
    part: &ExpressionPart<'graph>,
) -> Option<&'graph KExpression<'graph>> {
    match part {
        ExpressionPart::RecordType(node) | ExpressionPart::QuotedExpression(node) => {
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
        match pair_label(run, index) {
            Some(label) => {
                into.extend(label.name());
                index += 2;
            }
            None => index += 1,
        }
    }
}

/// Push every type parameter a `FOR ALL` group declares onto `into`, bounded or not. A malformed
/// entry declares nothing; the elaborator refuses it.
pub(crate) fn declare_quantifiers(part: &ExpressionPart<'_>, into: &mut BumpVec<'_, BinderSymbol>) {
    into.extend(quantifier_entries(part).filter_map(|entry| entry.name.map(BinderSymbol::Type)));
}

/// Push every type parameter a `(<P>… AS <Name>)` declarator names onto `into` — a parameterized
/// `UNION`'s, which its variants' payloads read.
pub(crate) fn declare_family_parameters(
    part: &ExpressionPart<'_>,
    into: &mut BumpVec<'_, BinderSymbol>,
) {
    into.extend(declarator_parameters(part).map(BinderSymbol::Type));
}

/// The bound parts a `FOR ALL` group writes, in written order. A bound is a type expression read
/// where the form runs, so its names are mentions of the enclosing shape.
pub(crate) fn quantifier_bounds<'g>(
    part: &ExpressionPart<'g>,
) -> impl Iterator<Item = &'g ExpressionPart<'g>> {
    quantifier_entries(part).filter_map(|entry| entry.bound)
}

/// The statements of an in-place body part, if it is a parenthesized body.
pub(crate) fn body_of<'graph>(
    part: &ExpressionPart<'graph>,
) -> Option<&'graph KExpression<'graph>> {
    match part {
        ExpressionPart::Expression(node) => Some(node.reference()),
        _ => None,
    }
}
