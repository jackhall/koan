//! Generic walker for ordered `<Identifier> <slot>` field/parameter lists.
//!
//! [`parse_pair_list`] handles `<Identifier> <slot>` PAIRS — typed field declarations
//! (STRUCT, SIG, FN signature). The Design-B type sigil consumes the `:`, so a typed
//! parameter `xs :Number` lands as `[Identifier("xs"), Type(Number)]`. Identifier
//! validation and duplicate-name detection live here; the per-slot interpretation is
//! supplied by a `parse_slot` closure.

use crate::machine::model::ast::{FieldSlot, Part};
use crate::source::Spanned;

/// Which token shapes are accepted as a field/parameter *name* by [`parse_pair_list`].
///
/// STRUCT / record fields are lowercase user identifiers, so they require `Identifier`.
/// FN parameters may be capitalized (`Ty`, `Er`) when they name a type or a
/// signature value, which lexes as a `Type` token, so they opt into `IdentifierOrType`. UNION variant tags *are*
/// types (`Some`, `Ok`) and so require `Type` — a lowercase tag is rejected. In every
/// type-token case the name string is read via `TypeIdentifier::render()`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldNameKind {
    Identifier,
    IdentifierOrType,
    Type,
}

/// `context` is woven into error messages; `name_kind` selects which token shapes are
/// valid as a name. Empty `parts` yields an empty `Vec`.
///
/// Generic over the part family so a parsed field list and a self-reference-threaded one walk the
/// same code: both name positions read through [`FieldSlot`], which each family answers in the one
/// shared vocabulary.
pub fn parse_pair_list<'a, P: Part<'a>, T>(
    parts: &'a [Spanned<P>],
    context: &str,
    name_kind: FieldNameKind,
    mut parse_slot: impl FnMut(&P, &str) -> Result<T, String>,
) -> Result<Vec<(String, T)>, String> {
    if !parts.len().is_multiple_of(2) {
        return Err(format!(
            "{context} must be `<name> <slot>` pairs; got {} parts (not a multiple of 2)",
            parts.len(),
        ));
    }
    let mut out: Vec<(String, T)> = Vec::with_capacity(parts.len() / 2);
    let mut i = 0;
    while i < parts.len() {
        let name = match (parts[i].value.field_slot(), name_kind) {
            (FieldSlot::Name(s), FieldNameKind::Identifier | FieldNameKind::IdentifierOrType) => {
                s.to_string()
            }
            // Capitalized names (`Ty`, `Er` params; `Some`, `Ok` variant tags) lex as
            // `Type` tokens; admitted under `IdentifierOrType` (FN) and `Type`
            // (UNION tags), never for STRUCT / record fields.
            (FieldSlot::Type(t), FieldNameKind::IdentifierOrType | FieldNameKind::Type) => {
                t.render()
            }
            // A lowercase tag under the `Type` policy — tags must be capitalized type names.
            (_, FieldNameKind::Type) => {
                return Err(format!(
                    "{context} variant tag must be a capitalized type name, got {}",
                    parts[i].value.summarize(),
                ));
            }
            _ => {
                return Err(format!(
                    "{context} name must be a bare identifier, got {}",
                    parts[i].value.summarize(),
                ));
            }
        };
        if out.iter().any(|(n, _)| n == &name) {
            return Err(format!("duplicate name `{}` in {context}", name));
        }
        let slot = parse_slot(&parts[i + 1].value, &name)?;
        out.push((name, slot));
        i += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::core::{RegionBrand, program_storage};
    use crate::machine::model::ast::{ExpressionPart, KExpression, TypeIdentifier};
    use crate::source::Spanned;

    /// `[name, slot]` parts where the name rides as a `Type` token (e.g. a capitalized
    /// FN param `Ty`) and the slot is an arbitrary leaf, here a `Type` too.
    fn type_named_pair<'a>(brand: RegionBrand<'a>) -> KExpression<'a> {
        KExpression::new(
            brand,
            vec![
                Spanned::bare(ExpressionPart::Type(TypeIdentifier::leaf("Ty"))),
                Spanned::bare(ExpressionPart::Type(TypeIdentifier::leaf("Signature"))),
            ],
        )
    }

    #[test]
    fn identifier_or_type_accepts_type_token_name() {
        let program = program_storage();
        let expr = type_named_pair(program.brand().region());
        let out = parse_pair_list(
            expr.parts,
            "FN parameters",
            FieldNameKind::IdentifierOrType,
            |p, _| match p {
                ExpressionPart::Type(t) => Ok(t.render()),
                _ => Err("unexpected slot".to_string()),
            },
        )
        .expect("Type-token name accepted under IdentifierOrType");
        assert_eq!(out, vec![("Ty".to_string(), "Signature".to_string())]);
    }

    #[test]
    fn identifier_only_rejects_type_token_name() {
        let program = program_storage();
        let expr = type_named_pair(program.brand().region());
        let result = parse_pair_list(
            expr.parts,
            "STRUCT schema",
            FieldNameKind::Identifier,
            |_, _| Ok::<_, String>(()),
        );
        assert!(
            matches!(&result, Err(msg) if msg.contains("bare identifier")),
            "Type-token name must be rejected under Identifier-only, got {result:?}",
        );
    }
}
