//! Generic walker for ordered `<Identifier> <slot>` field/parameter lists.
//!
//! [`parse_pair_list`] handles `<Identifier> <slot>` PAIRS — typed field declarations
//! (STRUCT, SIG, FN signature). The Design-B type sigil consumes the `:`, so a typed
//! parameter `xs :Number` lands as `[Identifier("xs"), Type(Number)]`. Identifier
//! validation and duplicate-name detection live here; the per-slot interpretation is
//! supplied by a `parse_slot` closure.

use crate::machine::model::ast::{ExpressionPart, KExpression};

/// Which token shapes are accepted as a field/parameter *name* by [`parse_pair_list`].
///
/// STRUCT / record fields are lowercase user identifiers, so they require `Identifier`.
/// FN / FUNCTOR parameters may be conventionally capitalized (`Ty`, `er`), which lexes
/// as a `Type` token, so they opt into `IdentifierOrType`. UNION variant tags *are*
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
pub fn parse_pair_list<'a, T>(
    expr: &KExpression<'a>,
    context: &str,
    name_kind: FieldNameKind,
    mut parse_slot: impl FnMut(&ExpressionPart<'a>, &str) -> Result<T, String>,
) -> Result<Vec<(String, T)>, String> {
    let parts = &expr.parts;
    if !parts.len().is_multiple_of(2) {
        return Err(format!(
            "{context} must be `<name> <slot>` pairs; got {} parts (not a multiple of 2)",
            parts.len(),
        ));
    }
    let mut out: Vec<(String, T)> = Vec::with_capacity(parts.len() / 2);
    let mut i = 0;
    while i < parts.len() {
        let name = match (&parts[i].value, name_kind) {
            (
                ExpressionPart::Identifier(s),
                FieldNameKind::Identifier | FieldNameKind::IdentifierOrType,
            ) => s.clone(),
            // Capitalized names (`Ty`, `er` params; `Some`, `Ok` variant tags) lex as
            // `Type` tokens; admitted under `IdentifierOrType` (FN / FUNCTOR) and `Type`
            // (UNION tags), never for STRUCT / record fields.
            (ExpressionPart::Type(t), FieldNameKind::IdentifierOrType | FieldNameKind::Type) => {
                t.render()
            }
            // A lowercase tag under the `Type` policy — tags must be capitalized type names.
            (other, FieldNameKind::Type) => {
                return Err(format!(
                    "{context} variant tag must be a capitalized type name, got {}",
                    other.summarize(),
                ));
            }
            (other, _) => {
                return Err(format!(
                    "{context} name must be a bare identifier, got {}",
                    other.summarize(),
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
    use crate::machine::model::ast::TypeIdentifier;
    use crate::source::Spanned;

    /// `[name, slot]` parts where the name rides as a `Type` token (e.g. a capitalized
    /// FUNCTOR param `Ty`) and the slot is an arbitrary leaf, here a `Type` too.
    fn type_named_pair<'a>() -> KExpression<'a> {
        KExpression::new(vec![
            Spanned::bare(ExpressionPart::Type(TypeIdentifier::leaf("Ty".into()))),
            Spanned::bare(ExpressionPart::Type(TypeIdentifier::leaf(
                "Signature".into(),
            ))),
        ])
    }

    #[test]
    fn identifier_or_type_accepts_type_token_name() {
        let expr = type_named_pair();
        let out = parse_pair_list(
            &expr,
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
        let expr = type_named_pair();
        let result = parse_pair_list(&expr, "STRUCT schema", FieldNameKind::Identifier, |_, _| {
            Ok::<_, String>(())
        });
        assert!(
            matches!(&result, Err(msg) if msg.contains("bare identifier")),
            "Type-token name must be rejected under Identifier-only, got {result:?}",
        );
    }
}
