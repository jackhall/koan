//! The raw-capture derivation a slot type is erased through, and the law it obeys over union
//! carriers.

use proptest::prelude::*;

use crate::parse::builtin_shapes::lazy::LazyKinds;
use crate::parse::builtin_shapes::{raw_kind_of, raw_kinds_over};
use crate::type_lattice::KType;

/// The carriers a generated union draws from, beside the kind each keeps raw: the three
/// raw-capture leaves, and three that stage nothing. A name-token carrier keeps nothing raw — a
/// bare token is not an eager shape, so it never stages and needs no kind.
const MEMBERS: &[(KType, LazyKinds)] = &[
    (KType::KEXPRESSION, LazyKinds::CODE),
    (KType::SIGILED_TYPE_EXPR, LazyKinds::TYPE_EXPR),
    (KType::RECORD_TYPE, LazyKinds::RECORD_TYPE),
    (KType::TYPE_NAME_TOKEN, LazyKinds::EMPTY),
    (KType::IDENTIFIER, LazyKinds::EMPTY),
    (KType::NUMBER, LazyKinds::EMPTY),
];

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// The derivation distributes over union members: a union-typed slot contributes every
    /// member's kind and nothing else, so a bucket that spells its raw capture as a union keeps
    /// raw everything that union admits. A union of carriers that stage nothing at all — name
    /// tokens, scalars — keeps nothing raw, and a bare carrier keeps its own kind.
    #[test]
    fn the_kind_derivation_distributes_over_union_members(
        chosen in prop::sample::subsequence(MEMBERS.to_vec(), 1..=MEMBERS.len()),
    ) {
        for (ktype, kinds) in &chosen {
            prop_assert_eq!(raw_kind_of(*ktype), *kinds);
        }

        let members: Vec<KType> = chosen.iter().map(|(ktype, _)| *ktype).collect();
        let expected = chosen
            .iter()
            .fold(LazyKinds::EMPTY, |kinds, (_, member)| kinds.with(*member));
        prop_assert_eq!(raw_kinds_over(&members), expected);
    }
}
