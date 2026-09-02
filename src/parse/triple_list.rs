//! Generic walker for ordered `<Identifier> <slot>` field/parameter lists.
//!
//! [`parse_pair_list`] handles `<Identifier> <slot>` PAIRS — typed field declarations
//! (STRUCT, SIG, FN signature). The Design-B type sigil consumes the `:`, so a typed
//! parameter `xs :Number` lands as `[Identifier("xs"), Type(Number)]`. Identifier
//! validation and duplicate-name detection live here; the per-slot interpretation is
//! supplied by a `parse_slot` closure.

use crate::machine::model::RunRegistries;
use crate::machine::model::ast::{FieldSlot, Part, part_summary};
use crate::machine::model::labels::{BinderSymbol, TypeSymbol};
use crate::source::Spanned;

/// Which token shapes are accepted as a field/parameter *name* by [`parse_pair_list`].
///
/// Record / STRUCT fields and FN parameters take `IdentifierOrType`: a name may be a lowercase
/// identifier (`x`) or capitalized (`Ty`, `Er`) when it names a type or a signature value, which
/// lexes as a `Type` token. UNION variant tags *are* types (`Some`, `Ok`) and so require `Type` — a
/// lowercase tag is rejected. Each admitted token hands over the symbol its own parse minted, so
/// the name carries its class already.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldNameKind {
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
    registries: &RunRegistries,
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
            (FieldSlot::Name(v), FieldNameKind::IdentifierOrType) => BinderSymbol::Value(v),
            // Capitalized names (`Ty`, `Er` params; `Some`, `Ok` variant tags) lex as
            // `Type` tokens, admitted under both policies.
            (FieldSlot::Type(t), FieldNameKind::IdentifierOrType | FieldNameKind::Type) => {
                BinderSymbol::Type(t)
            }
            // A lowercase tag under the `Type` policy — tags must be capitalized type names.
            (_, FieldNameKind::Type) => {
                return Err(format!(
                    "{context} variant tag must be a capitalized type name, got {}",
                    part_summary(&parts[i].value, registries),
                ));
            }
            _ => {
                return Err(format!(
                    "{context} name must be a bare identifier, got {}",
                    part_summary(&parts[i].value, registries),
                ));
            }
        };
        if out.iter().any(|(n, _)| *n == name) {
            return Err(format!(
                "duplicate name `{}` in {context}",
                registries.labels.render(name.symbol()),
            ));
        }
        let slot = parse_slot(&parts[i + 1].value, name)?;
        out.push((name, slot));
        i += 2;
    }
    Ok(out)
}

/// The variant tags of a `<tag> <slot>` pair list, read without touching a slot — the pre-scan a
/// declarator runs to announce its window's members before any payload elaborates.
///
/// A tag *is* a type name, so this door classifies as one: a tag position that is not a `Type`
/// token is a shape error here, and the names it hands back are `TypeSymbol`s with no arm left for
/// a caller to discard.
pub fn parse_type_tag_names<'a, P: Part<'a>>(
    parts: &'a [Spanned<P>],
    context: &str,
    registries: &RunRegistries,
) -> Result<Vec<TypeSymbol>, String> {
    if !parts.len().is_multiple_of(2) {
        return Err(format!(
            "{context} must be `<name> <slot>` pairs; got {} parts (not a multiple of 2)",
            parts.len(),
        ));
    }
    let mut out: Vec<TypeSymbol> = Vec::with_capacity(parts.len() / 2);
    let mut i = 0;
    while i < parts.len() {
        let FieldSlot::Type(tag) = parts[i].value.field_slot() else {
            return Err(format!(
                "{context} variant tag must be a capitalized type name, got {}",
                part_summary(&parts[i].value, registries),
            ));
        };
        if out.contains(&tag) {
            return Err(format!(
                "duplicate name `{}` in {context}",
                registries.labels.render(tag.symbol()),
            ));
        }
        out.push(tag);
        i += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::core::{RegionBrand, program_storage};
    use crate::machine::model::ast::{ExpressionPart, KExpression};
    use crate::machine::model::labels::{LabelInterner, ValueSymbol};
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
        let registries = RunRegistries::new();
        let labels = &registries.labels;
        let expr = type_named_pair(program.brand().region(), labels);
        let out = parse_pair_list(
            expr.parts,
            "FN parameters",
            FieldNameKind::IdentifierOrType,
            &registries,
            |p, _| match p {
                ExpressionPart::Type(t) => Ok(labels.render(t.symbol())),
                _ => Err("unexpected slot".to_string()),
            },
        )
        .expect("Type-token name accepted under IdentifierOrType");
        assert_eq!(
            out,
            vec![(
                BinderSymbol::Type(declared("Ty", labels)),
                "Signature".to_string()
            )]
        );
    }

    /// The pre-scan door hands its tags back as the `Type` tokens they are, so a caller has no
    /// other class to discard.
    #[test]
    fn type_tag_names_are_classified_type_tokens() {
        let program = program_storage();
        let registries = RunRegistries::new();
        let labels = &registries.labels;
        let expr = type_named_pair(program.brand().region(), labels);
        let tags = parse_type_tag_names(expr.parts, "UNION schema", &registries)
            .expect("a Type-token tag is admitted");
        assert_eq!(tags, vec![declared("Ty", labels)]);
    }

    #[test]
    fn type_tag_names_reject_an_identifier_tag() {
        let program = program_storage();
        let registries = RunRegistries::new();
        let labels = &registries.labels;
        let expr = KExpression::new(
            program.brand().region(),
            &[
                Spanned::bare(ExpressionPart::Identifier(
                    ValueSymbol::declared("some", labels).expect("a fixture name is a value token"),
                )),
                Spanned::bare(ExpressionPart::Type(declared("Number", labels))),
            ],
        );
        let result = parse_type_tag_names(expr.parts, "UNION schema", &registries);
        assert!(
            matches!(&result, Err(msg) if msg.contains("capitalized type name")),
            "a lowercase tag must be rejected, got {result:?}",
        );
    }

    #[test]
    fn type_tag_names_reject_a_duplicate_tag() {
        let program = program_storage();
        let registries = RunRegistries::new();
        let labels = &registries.labels;
        let region = program.brand().region();
        let expr = KExpression::new(
            region,
            &[
                Spanned::bare(ExpressionPart::Type(declared("Ty", labels))),
                Spanned::bare(ExpressionPart::Type(declared("Number", labels))),
                Spanned::bare(ExpressionPart::Type(declared("Ty", labels))),
                Spanned::bare(ExpressionPart::Type(declared("Str", labels))),
            ],
        );
        let result = parse_type_tag_names(expr.parts, "UNION schema", &registries);
        assert!(
            matches!(&result, Err(msg) if msg.contains("duplicate name")),
            "a repeated tag must be rejected, got {result:?}",
        );
    }
}
