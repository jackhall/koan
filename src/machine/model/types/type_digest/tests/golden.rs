//! Golden digest pins — literal `u128` values for every fixed handle and for representative
//! declarations built through the set API.
//!
//! Every assertion here compares a computed digest against a hex literal, so any edit to a digest
//! recipe fails loudly instead of silently re-identifying types. A failure message carries the
//! recomputed value, so an *intended* recipe change is a paste, and an unintended one is a bug
//! report.
//!
//! Permanence: every singleton pin below is permanent — a standalone declaration is a singleton
//! component under the per-SCC member-identity recipe, so its presentation is byte-identical
//! there. The one exception is called out at its own fixture.

use std::collections::HashMap;

use super::super::{empty_schema_digest, schema_content_digest, TypeDigest};
use crate::machine::core::ScopeId;
use crate::machine::model::types::{
    KKind, KType, Record, RecursiveGroupWindow, RelativeSchema, SigSchema, TypeNode,
};
use crate::machine::model::TypeRegistry;

#[track_caller]
fn assert_pinned(label: &str, actual: TypeDigest, expected: u128) {
    assert_eq!(
        actual.0, expected,
        "{label}: digest recipe moved — recomputed value is 0x{:032x}",
        actual.0
    );
}

#[track_caller]
fn assert_handle_pinned(label: &str, actual: KType, expected: u128) {
    assert_pinned(label, actual.digest(), expected);
}

fn record(types: &TypeRegistry, pairs: Vec<(&str, KType)>) -> KType {
    types.record(Record::from_pairs(
        pairs.into_iter().map(|(n, t)| (n.to_string(), t)),
    ))
}

fn newtype(repr: KType) -> RelativeSchema {
    RelativeSchema::NewType(repr)
}

/// Seal a one-member window and hand back its member handle — the declarator shape every
/// standalone `NEWTYPE` / `UNION` / opaque mint takes.
fn singleton(name: &str, schema: RelativeSchema, types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(name.into(), schema, None, types)
}

/// The component digest a member handle was derived from.
fn component_of(handle: KType, types: &TypeRegistry) -> TypeDigest {
    match types.node(handle) {
        TypeNode::SetMember { scc_digest, .. } => scc_digest,
        _ => panic!("a sealed member interns as a SetMember node"),
    }
}

/// The relative self-reference a singleton's own representation carries.
fn sibling(types: &TypeRegistry, index: usize) -> KType {
    types.intern(TypeNode::Sibling(index))
}

/// A non-recursive newtype: `Meters` over `Number`.
fn meters(types: &TypeRegistry) -> KType {
    singleton("Meters", newtype(KType::NUMBER), types)
}

/// A self-recursive newtype — the sibling reference in its representation is relative.
fn chain(types: &TypeRegistry) -> KType {
    singleton(
        "Chain",
        newtype(record(
            types,
            vec![("head", KType::NUMBER), ("tail", sibling(types, 0))],
        )),
        types,
    )
}

/// A newtype whose representation is a union naming itself — the binder shape a self-referencing
/// union declaration seals to.
fn recursive_union(types: &TypeRegistry) -> KType {
    singleton(
        "Tree",
        newtype(types.union_of(vec![KType::NUMBER, sibling(types, 0)])),
        types,
    )
}

/// A type constructor carrying parameter names.
fn constructor(types: &TypeRegistry) -> KType {
    let schema: HashMap<String, KType> = [
        ("Empty".to_string(), KType::NULL),
        ("Full".to_string(), KType::ANY),
    ]
    .into_iter()
    .collect();
    singleton(
        "Maybe",
        RelativeSchema::TypeConstructor {
            schema,
            param_names: vec!["Elem".to_string()],
        },
        types,
    )
}

/// A generative set at a fixed nonce — opaque ascription's per-application mint.
fn generative(types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(
        "Opaque".into(),
        newtype(KType::NUMBER),
        Some(ScopeId::from_raw(0, 0x0BAB)),
        types,
    )
}

/// A genuinely mutually-recursive pair, declared **out of name order** (`Odd` at declared index 0,
/// `Even` at 1) so the pins below record that declaration order is not identity.
///
/// These are the three values ruling 10 deliberately re-pins. Member identity is the computed
/// strongly-connected component, and a mutually recursive pair is one two-member component whose
/// canonical order is *name* order — so `Even` presents at position 0 and `Odd` at 1, and the
/// intra-component references re-encode against that order rather than the declared one. Every
/// other pin in this file is a singleton, whose component presentation is byte-identical to the
/// whole-declaration recipe, and is permanent.
///
/// Returns the member handles in declaration order: `[Odd, Even]`.
fn recursive_pair(types: &TypeRegistry) -> Vec<KType> {
    let window = RecursiveGroupWindow::new(
        vec![
            ("Odd".into(), KKind::NewType),
            ("Even".into(), KKind::NewType),
        ],
        None,
    );
    window.fill_member(
        0,
        newtype(record(types, vec![("pred", sibling(types, 1))])),
        types,
    );
    window
        .fill_member(
            1,
            newtype(record(types, vec![("pred", sibling(types, 0))])),
            types,
        )
        .expect("the last fill seals")
        .members
}

/// The abstract-member source shared by the signature-schema pins.
const SIG_SOURCE: ScopeId = ScopeId::from_raw(0, 0x51C0);

fn abstract_member(types: &TypeRegistry, name: &str, param_names: Vec<&str>) -> KType {
    types.intern(TypeNode::AbstractType {
        source: SIG_SOURCE,
        name: name.into(),
        param_names: param_names.into_iter().map(str::to_string).collect(),
        nonce: None,
    })
}

/// A schema with a first-order member `Elem` and a higher-kinded member `Wrap` over `wrap_params`.
fn mixed_schema(types: &TypeRegistry, wrap_params: Vec<&str>) -> SigSchema {
    SigSchema {
        sig_id: Some(SIG_SOURCE),
        abstract_members: [
            (
                "Elem".to_string(),
                abstract_member(types, "Elem", Vec::new()),
            ),
            (
                "Wrap".to_string(),
                abstract_member(types, "Wrap", wrap_params),
            ),
        ]
        .into_iter()
        .collect(),
        manifest_members: HashMap::new(),
        value_slots: HashMap::new(),
    }
}

fn constructor_apply(types: &TypeRegistry, pairs: Vec<(&str, KType)>) -> KType {
    let both = types.intern(TypeNode::AbstractType {
        source: ScopeId::from_raw(0, 0xC70A),
        name: "Both".into(),
        param_names: vec!["Ok".into(), "Error".into()],
        nonce: None,
    });
    types.constructor_apply(
        both,
        Record::from_pairs(pairs.into_iter().map(|(n, t)| (n.to_string(), t))),
    )
}

/// The nine leaf types. Each is a bare domain tag, so these are the most load-bearing pins in the
/// file: they are the leaves every composite digest is built from.
#[test]
fn leaf_digests_are_pinned() {
    assert_handle_pinned(
        "Number",
        KType::NUMBER,
        0xe21d67f1_7aa25f92_e072c1bb_1f72fc48,
    );
    assert_handle_pinned("Str", KType::STR, 0xda8a6add_c7627c0f_ae4be842_dfbe13ab);
    assert_handle_pinned("Bool", KType::BOOL, 0x01210944_fd6fb8f8_0c9ba36e_1de8e0e1);
    assert_handle_pinned("Null", KType::NULL, 0xbc9d88bb_75d5fb35_a4fd343e_749a380c);
    assert_handle_pinned(
        "Identifier",
        KType::IDENTIFIER,
        0x41b73c3e_2391bbb4_6b850e4f_e740cb84,
    );
    assert_handle_pinned(
        "KExpression",
        KType::KEXPRESSION,
        0x63c296ef_dbe5d41c_9969ddda_6b0b311c,
    );
    assert_handle_pinned(
        "SigiledTypeExpr",
        KType::SIGILED_TYPE_EXPR,
        0xf6d652dc_848e0f69_4a152496_ddd88b44,
    );
    assert_handle_pinned(
        "RecordType",
        KType::RECORD_TYPE,
        0x387dfced_dc0a5d96_da3b29a5_dde0f32e,
    );
    assert_handle_pinned("Any", KType::ANY, 0xd9f70f99_49f95b5c_44d7ce99_10aa1972);
}

/// The five kind values, each a tag plus its stable `kkind_tag` byte.
#[test]
fn of_kind_digests_are_pinned() {
    assert_handle_pinned(
        "OfKind ProperType",
        KType::of_kind(KKind::ProperType),
        0xe082d96a_231e2f4c_af1e256b_459a681f,
    );
    assert_handle_pinned(
        "OfKind Signature",
        KType::of_kind(KKind::Signature),
        0xa74d105b_68705a5a_4c93c325_b2bb4032,
    );
    assert_handle_pinned(
        "OfKind AnyType",
        KType::of_kind(KKind::AnyType),
        0x6230fb6f_d4cb83ad_59072aad_08f93e54,
    );
    assert_handle_pinned(
        "OfKind NewType",
        KType::of_kind(KKind::NewType),
        0x3079a661_6197d2a5_46103cc5_f0cbfeaa,
    );
    assert_handle_pinned(
        "OfKind TypeConstructor",
        KType::of_kind(KKind::TypeConstructor),
        0x1522ec89_d5fd3ca8_2db00c80_75beafb3,
    );
}

/// The two fixed composites the container builtins lower to.
#[test]
fn fixed_composite_digests_are_pinned() {
    let types = TypeRegistry::new();
    assert_handle_pinned(
        "List<Any>",
        types.list(KType::ANY),
        0x9d40af7c_078f46c4_bd4a8f94_98f5fd63,
    );
    assert_handle_pinned(
        "Dict<Any, Any>",
        types.dict(KType::ANY, KType::ANY),
        0xf9b9d64d_aa69edda_e7a59f82_4e0f5015,
    );
}

/// The module-lattice top, both halves: the zero-member schema content digest and the signature
/// that wraps it.
#[test]
fn empty_signature_digests_are_pinned() {
    let types = TypeRegistry::new();
    assert_pinned(
        "empty schema content",
        empty_schema_digest(),
        0xca37d6c1_0e957006_5c08a0d2_ad8b02f8,
    );
    assert_handle_pinned(
        "empty signature",
        types.signature(SigSchema::empty()),
        0x1660d74d_20447364_cde2f1b9_3ed245f6,
    );
}

#[test]
fn non_recursive_newtype_digests_are_pinned() {
    let types = TypeRegistry::new();
    let member = meters(&types);
    assert_pinned(
        "Meters component",
        component_of(member, &types),
        0xa5bab723_08985b67_fdc176d5_b9e836b1,
    );
    assert_handle_pinned(
        "Meters member reference",
        member,
        0xaa9dc344_ea08a395_63635ec0_be611e20,
    );
}

#[test]
fn self_recursive_newtype_digests_are_pinned() {
    let types = TypeRegistry::new();
    let member = chain(&types);
    assert_pinned(
        "Chain component",
        component_of(member, &types),
        0xaaab8251_b184aebe_af32f73c_592df0cf,
    );
    assert_handle_pinned(
        "Chain member reference",
        member,
        0xcdfbfaac_8fae50c8_850808f7_27df0fa2,
    );
}

#[test]
fn self_referencing_union_digests_are_pinned() {
    let types = TypeRegistry::new();
    let member = recursive_union(&types);
    assert_pinned(
        "Tree component",
        component_of(member, &types),
        0xd0d777d4_90760cd1_778b02fd_6ecdf5ca,
    );
    assert_handle_pinned(
        "Tree member reference",
        member,
        0xeee5c699_feb5a1c8_4b913ea2_313272cd,
    );
}

#[test]
fn type_constructor_digests_are_pinned() {
    let types = TypeRegistry::new();
    let member = constructor(&types);
    assert_pinned(
        "Maybe component",
        component_of(member, &types),
        0x8fce3135_01caf69c_dae1dfba_79f02281,
    );
    assert_handle_pinned(
        "Maybe member reference",
        member,
        0x5ebc2110_fa8a5b65_ae71cd45_ee6636cf,
    );
}

#[test]
fn generative_set_digests_are_pinned() {
    let types = TypeRegistry::new();
    let member = generative(&types);
    assert_pinned(
        "Opaque component",
        component_of(member, &types),
        0xedf73e8f_68d1d2a2_5ceee390_0def9a25,
    );
    assert_handle_pinned(
        "Opaque member reference",
        member,
        0x339743c3_12b34d96_42134765_aef171a0,
    );
}

/// The multi-member pins. See [`recursive_pair`]: these three values are the ones ruling 10
/// re-pins, because the pair is one two-member component presented in name order while this
/// fixture declares it out of name order. Every other pin in this file is a singleton and is
/// permanent.
#[test]
fn recursive_pair_digests_are_pinned() {
    let types = TypeRegistry::new();
    let members = recursive_pair(&types);
    assert_pinned(
        "Odd/Even component",
        component_of(members[0], &types),
        0xd547f989_55b718b9_e72fdf74_0b0e0543,
    );
    assert_handle_pinned(
        "Odd member reference (component position 1)",
        members[0],
        0x4774dbbc_3769d00a_46eb440c_7a598c51,
    );
    assert_handle_pinned(
        "Even member reference (component position 0)",
        members[1],
        0xf9e68404_41cf37d0_2b93447e_d808b399,
    );
}

/// A `ConstructorApply`'s arguments are a name-keyed `Record` fed name-sorted, so the insertion
/// order of the arguments record is presentation: both orders land on one pinned value.
#[test]
fn constructor_apply_digest_is_pinned_and_order_blind() {
    let types = TypeRegistry::new();
    let declared = constructor_apply(&types, vec![("Ok", KType::NUMBER), ("Error", KType::STR)]);
    let reversed = constructor_apply(&types, vec![("Error", KType::STR), ("Ok", KType::NUMBER)]);
    assert_handle_pinned(
        "Both(Ok = Number, Error = Str)",
        declared,
        0xeadbdff7_6b59c1f3_70761787_1f06cd46,
    );
    assert_handle_pinned(
        "Both applied in reverse argument order",
        reversed,
        0xeadbdff7_6b59c1f3_70761787_1f06cd46,
    );
}

/// A schema's abstract members feed `byte(0)` for a first-order member and `byte(1)` + parameter
/// count + sorted parameter names for a higher-kinded one. So both parameter orders of `Wrap` land
/// on one pinned value, and making `Wrap` first-order lands on a different one.
#[test]
fn schema_abstract_member_digests_are_pinned() {
    let types = TypeRegistry::new();
    assert_pinned(
        "schema with higher-kinded Wrap",
        schema_content_digest(&mixed_schema(&types, vec!["Inner", "Outer"]), &types),
        0x74c887c4_2b7bdd55_7a481826_b15078ee,
    );
    assert_pinned(
        "schema with Wrap's parameters reordered",
        schema_content_digest(&mixed_schema(&types, vec!["Outer", "Inner"]), &types),
        0x74c887c4_2b7bdd55_7a481826_b15078ee,
    );
    assert_pinned(
        "schema with first-order Wrap",
        schema_content_digest(&mixed_schema(&types, Vec::new()), &types),
        0xdcaf6f29_107c1417_c55d837b_7e90fe20,
    );
}
