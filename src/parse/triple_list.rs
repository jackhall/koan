//! Generic walker for ordered `<Identifier> <slot>` field/parameter lists.
//!
//! [`parse_pair_list`] handles `<Identifier> <slot>` PAIRS — typed field declarations
//! (STRUCT, SIG, FN signature). The Design-B type sigil consumes the `:`, so a typed
//! parameter `xs :Number` lands as `[Identifier("xs"), Type(Number)]`. Identifier
//! validation and duplicate-name detection live here; the per-slot interpretation is
//! supplied by a `parse_slot` closure.

use crate::machine::model::ast::{FieldSlot, Part};
use crate::machine::model::labels::{BinderSymbol, LabelInterner};
use crate::source::Spanned;

/// Which token shapes are accepted as a field/parameter *name* by [`parse_pair_list`].
///
/// STRUCT / record fields are lowercase user identifiers, so they require `Identifier`.
/// FN parameters may be capitalized (`Ty`, `Er`) when they name a type or a
/// signature value, which lexes as a `Type` token, so they opt into `IdentifierOrType`. UNION variant tags *are*
/// types (`Some`, `Ok`) and so require `Type` — a lowercase tag is rejected. Each admitted
/// token hands over the symbol its own parse minted, so the name carries its class already.
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
    labels: &LabelInterner,
    mut parse_slot: impl FnMut(&P, BinderSymbol) -> Result<T, String>,
) -> Result<Vec<(BinderSymbol, T)>, String> {
    if !parts.len().is_multiple_of(2) {
        return Err(format!(
            "{context} must be `<name> <slot>` pairs; got {} parts (not a multiple of 2)",
            parts.len(),
        ));
    }
    let mut out: Vec<(BinderSymbol, T)> = Vec::with_capacity(parts.len() / 2);
    let mut i = 0;
    while i < parts.len() {
        let name = match (parts[i].value.field_slot(), name_kind) {
            (FieldSlot::Name(v), FieldNameKind::Identifier | FieldNameKind::IdentifierOrType) => {
                BinderSymbol::Value(v)
            }
            // Capitalized names (`Ty`, `Er` params; `Some`, `Ok` variant tags) lex as
            // `Type` tokens; admitted under `IdentifierOrType` (FN) and `Type`
            // (UNION tags), never for STRUCT / record fields.
            (FieldSlot::Type(t), FieldNameKind::IdentifierOrType | FieldNameKind::Type) => {
                BinderSymbol::Type(t)
            }
            // A lowercase tag under the `Type` policy — tags must be capitalized type names.
            (_, FieldNameKind::Type) => {
                return Err(format!(
                    "{context} variant tag must be a capitalized type name, got {}",
                    parts[i].value.summarize(labels),
                ));
            }
            _ => {
                return Err(format!(
                    "{context} name must be a bare identifier, got {}",
                    parts[i].value.summarize(labels),
                ));
            }
        };
        if out.iter().any(|(n, _)| *n == name) {
            return Err(format!(
                "duplicate name `{}` in {context}",
                labels.render(name.symbol()),
            ));
        }
        let slot = parse_slot(&parts[i + 1].value, name)?;
        out.push((name, slot));
        i += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::core::{RegionBrand, program_storage};
    use crate::machine::model::ast::{ExpressionPart, KExpression};
    use crate::machine::model::labels::TypeSymbol;
    use crate::source::Spanned;

    /// `[name, slot]` parts where the name rides as a `Type` token (e.g. a capitalized
    /// FN param `Ty`) and the slot is an arbitrary leaf, here a `Type` too.
    fn type_named_pair<'a>(brand: RegionBrand<'a>, labels: &LabelInterner) -> KExpression<'a> {
        KExpression::new(
            brand,
            &[
                Spanned::bare(ExpressionPart::Type(declared("Ty", labels))),
                Spanned::bare(ExpressionPart::Type(declared("Signature", labels))),
            ],
        )
    }

    /// A Type token declared into `labels`, so the walker's own render resolves its spelling.
    fn declared(text: &str, labels: &LabelInterner) -> TypeSymbol {
        TypeSymbol::declared(text, labels).expect("a fixture name is a Type token")
    }

    #[test]
    fn identifier_or_type_accepts_type_token_name() {
        let program = program_storage();
        let labels = LabelInterner::new();
        let expr = type_named_pair(program.brand().region(), &labels);
        let out = parse_pair_list(
            expr.parts,
            "FN parameters",
            FieldNameKind::IdentifierOrType,
            &labels,
            |p, _| match p {
                ExpressionPart::Type(t) => Ok(labels.render(t.symbol())),
                _ => Err("unexpected slot".to_string()),
            },
        )
        .expect("Type-token name accepted under IdentifierOrType");
        assert_eq!(
            out,
            vec![(
                BinderSymbol::Type(declared("Ty", &labels)),
                "Signature".to_string()
            )]
        );
    }

    #[test]
    fn identifier_only_rejects_type_token_name() {
        let program = program_storage();
        let labels = LabelInterner::new();
        let expr = type_named_pair(program.brand().region(), &labels);
        let result = parse_pair_list(
            expr.parts,
            "STRUCT schema",
            FieldNameKind::Identifier,
            &labels,
            |_, _| Ok::<_, String>(()),
        );
        assert!(
            matches!(&result, Err(msg) if msg.contains("bare identifier")),
            "Type-token name must be rejected under Identifier-only, got {result:?}",
        );
    }
}
