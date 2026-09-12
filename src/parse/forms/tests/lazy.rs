//! The kind derivation the table⟺registration law reads a live slot type through, and the law it
//! obeys over union carriers.

use proptest::prelude::*;
use workgraph::witnessed::RegionHandle;
use workgraph::witnessed::doctest_fixture::fresh_cart;

use crate::parse::forms::lazy::LazyKinds;
use crate::type_lattice::{KType, TypeNode, TypeRegistry};

/// The kind an exact raw-capture slot type stands for; `None` for a slot type that captures
/// nothing raw. A name-token carrier answers `None`: a bare token is not an eager shape, so it
/// never stages and needs no stamp to stay raw.
fn exact_kind_of(ktype: KType) -> Option<LazyKinds> {
    match ktype {
        KType::KEXPRESSION => Some(LazyKinds::CODE),
        KType::SIGILED_TYPE_EXPR => Some(LazyKinds::TYPE_EXPR),
        KType::RECORD_TYPE => Some(LazyKinds::RECORD_TYPE),
        _ => None,
    }
}

/// The kinds a slot type stands for, distributed over union members: a union carrier slot admits
/// every carrier spelling it lists, so its bucket's stamp must carry each member's kind. This is
/// what forces a union-slot builtin's bucket to declare correct lazy slots.
fn kind_of(ktype: KType, types: &TypeRegistry) -> Option<LazyKinds> {
    if let Some(kind) = exact_kind_of(ktype) {
        return Some(kind);
    }
    let kinds = types.with_node(ktype, |node| match node {
        TypeNode::Union { members } => members
            .iter()
            .filter_map(|member| exact_kind_of(*member))
            .fold(LazyKinds::EMPTY, LazyKinds::with),
        _ => LazyKinds::EMPTY,
    });
    (!kinds.is_empty()).then_some(kinds)
}

/// The carriers a generated union draws from: the three that stage raw, and three that do not.
const MEMBERS: &[(KType, Option<LazyKinds>)] = &[
    (KType::KEXPRESSION, Some(LazyKinds::CODE)),
    (KType::SIGILED_TYPE_EXPR, Some(LazyKinds::TYPE_EXPR)),
    (KType::RECORD_TYPE, Some(LazyKinds::RECORD_TYPE)),
    (KType::TYPE_NAME_TOKEN, None),
    (KType::IDENTIFIER, None),
    (KType::NUMBER, None),
];

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// The derivation distributes over union members: a union-typed slot contributes every member's
    /// kind and nothing else, so a bucket that spells its raw capture as a union is held to a table
    /// entry covering all of them. A union that stages nothing at all — name tokens, scalars —
    /// answers `None`, so its bucket needs no table entry, and a bare carrier keeps its own kind.
    #[test]
    fn the_kind_derivation_distributes_over_union_members(
        chosen in prop::sample::subsequence(MEMBERS.to_vec(), 1..=MEMBERS.len()),
    ) {
        let cart = fresh_cart();
        let bump = RegionHandle::from_owner(&*cart).allocator();
        let types = &TypeRegistry::in_region(bump);

        for (ktype, kind) in &chosen {
            prop_assert_eq!(kind_of(*ktype, types), *kind);
        }

        let members: Vec<KType> = chosen.iter().map(|(ktype, _)| *ktype).collect();
        let expected = chosen
            .iter()
            .filter_map(|(_, kind)| *kind)
            .fold(LazyKinds::EMPTY, LazyKinds::with);
        let union = types.union_of(bump, &members);
        prop_assert_eq!(
            kind_of(union, types),
            (!expected.is_empty()).then_some(expected),
        );
    }
}
