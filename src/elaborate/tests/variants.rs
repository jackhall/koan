//! A union's variants as a `MATCH … UNDER` clause reads them: a plain union's declared members, a
//! parameterized union's families, bare or applied, each named by its label, and no variants for
//! a type that is no union.

use crate::type_lattice::KType;

use super::super::{variant_label, variant_named, variants};
use super::{Program, brought};

const OPTION: &str = "UNION (Elem -> Option) = #{Some: Elem, None: Null}";

/// The labels of `union`'s variants, in stored order.
fn labels(program: &Program<'_, '_, '_>, union: KType) -> Vec<String> {
    variants(program.types, union)
        .expect("a union")
        .map(|variant| {
            let label = variant_label(program.types, variant).expect("a named variant");
            program.symbols.display(label).to_string()
        })
        .collect()
}

#[test]
fn a_unions_variants_are_its_members_each_named_by_its_label() {
    brought(
        &format!(
            "UNION Maybe = #{{Some: Number, None: Null}}\n{OPTION}\n\
             LET NumberOption = :(Number -> Option)\nLET Mixed = :(Maybe | Number)\n\
             NEWTYPE Meters = Number"
        ),
        |program| {
            let named = |union: &str| {
                let mut labels = labels(&program, program.bound(union));
                labels.sort();
                labels
            };
            for union in ["Maybe", "Option", "NumberOption"] {
                assert_eq!(named(union), ["None", "Some"], "{union}");
            }
            let some = program.type_name("Some").symbol();
            let maybe = program.bound("Maybe");
            let hit = variant_named(program.types, maybe, some).expect("`Some` names a variant");
            assert_eq!(variant_label(program.types, hit), Some(some));
            let bogus = program.type_name("Bogus").symbol();
            assert_eq!(variant_named(program.types, maybe, bogus), None);
            assert!(
                variant_named(program.types, program.bound("NumberOption"), some).is_some(),
                "an applied variant is named by its constructor"
            );
            let mixed = program.bound("Mixed");
            assert_eq!(
                variants(program.types, mixed).expect("a union").count(),
                3,
                "a union of a union and a structural member flattens"
            );
            assert!(
                (variants(program.types, mixed).expect("a union")).any(|member| variant_label(
                    program.types,
                    member
                )
                .is_none()),
                "a structural member has no label"
            );
            assert!(variants(program.types, program.bound("Meters")).is_none());
            assert!(variants(program.types, KType::NUMBER).is_none());
        },
    );
}
