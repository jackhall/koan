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

use super::super::{TypeDigest, empty_schema_digest, schema_content_digest};
use crate::machine::core::ScopeId;
use crate::machine::model::RunRegistries;
use crate::machine::model::TypeRegistry;
use crate::machine::model::TypeSymbol;
use crate::machine::model::types::{
    KKind, KType, Record, RecursiveGroupWindow, RelativeSchema, SigSchema, TypeMemberMap, TypeNode,
};

/// A fixture's Type-class name as the [`TypeSymbol`] the schema and node types key by. The pins
/// here compare digests and never render, so the pure probe constructor is enough.
fn type_symbol(text: &str) -> TypeSymbol {
    TypeSymbol::of(text).expect("a golden fixture names its members with Type tokens")
}

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
        pairs
            .into_iter()
            .map(|(n, t)| (crate::machine::model::Symbol::of(n), t)),
    ))
}

fn newtype(repr: KType) -> RelativeSchema {
    RelativeSchema::NewType(repr)
}

/// Seal a one-member window and hand back its member handle — the declarator shape every
/// standalone `NEWTYPE` / `UNION` / opaque mint takes.
fn singleton(name: &str, schema: RelativeSchema, types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(type_symbol(name), schema, None, types)
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
    let schema: TypeMemberMap = [
        (type_symbol("Empty"), KType::NULL),
        (type_symbol("Full"), KType::ANY),
    ]
    .into_iter()
    .collect();
    singleton(
        "Maybe",
        RelativeSchema::TypeConstructor {
            schema,
            param_names: vec![type_symbol("Elem")],
        },
        types,
    )
}

/// A generative set at a fixed nonce — opaque ascription's per-application mint.
fn generative(types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(
        type_symbol("Opaque"),
        newtype(KType::NUMBER),
        Some(ScopeId::from_raw(0, 0x0BAB)),
        types,
    )
}

/// A genuinely mutually-recursive pair, declared **out of canonical order** (`Odd` at declared
/// index 0, `Even` at 1) so the pins below record that declaration order is not identity.
///
/// Member identity is the computed strongly-connected component, and a mutually recursive pair is
/// one two-member component whose canonical order is the numeric order of the members' name
/// symbols — under which `Even` presents at position 0 and `Odd` at 1, and the intra-component
/// references re-encode against that order rather than the declared one. Every other pin in this
/// file is a singleton, whose component presentation is byte-identical to the whole-declaration
/// recipe.
///
/// Returns the member handles in declaration order: `[Odd, Even]`.
fn recursive_pair(types: &TypeRegistry) -> Vec<KType> {
    let window = RecursiveGroupWindow::new(vec![
        (type_symbol("Odd"), KKind::NewType),
        (type_symbol("Even"), KKind::NewType),
    ]);
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
        name: type_symbol(name),
        param_names: param_names.into_iter().map(type_symbol).collect(),
        nonce: None,
    })
}

/// A schema with a first-order member `Elem` and a higher-kinded member `Wrap` over `wrap_params`.
fn mixed_schema(types: &TypeRegistry, wrap_params: Vec<&str>) -> SigSchema {
    SigSchema {
        sig_id: Some(SIG_SOURCE),
        abstract_members: [
            (
                type_symbol("Elem"),
                abstract_member(types, "Elem", Vec::new()),
            ),
            (
                type_symbol("Wrap"),
                abstract_member(types, "Wrap", wrap_params),
            ),
        ]
        .into_iter()
        .collect(),
        manifest_members: TypeMemberMap::default(),
        value_slots: HashMap::default(),
    }
}

fn constructor_apply(types: &TypeRegistry, pairs: Vec<(&str, KType)>) -> KType {
    let both = types.intern(TypeNode::AbstractType {
        source: ScopeId::from_raw(0, 0xC70A),
        name: type_symbol("Both"),
        param_names: vec![type_symbol("Ok"), type_symbol("Error")],
        nonce: None,
    });
    types.constructor_apply(
        both,
        Record::from_pairs(
            pairs
                .into_iter()
                .map(|(n, t)| (crate::machine::model::Symbol::of(n), t)),
        ),
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
    let registries = RunRegistries::new();
    let types = &registries.types;
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
    let registries = RunRegistries::new();
    let types = &registries.types;
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
    let registries = RunRegistries::new();
    let types = &registries.types;
    let member = meters(types);
    assert_pinned(
        "Meters component",
        component_of(member, types),
        0x83213530_d0afb37d_b014e6b6_79d93030,
    );
    assert_handle_pinned(
        "Meters member reference",
        member,
        0x7e972522_67b987b4_99ab69ff_e1cbed47,
    );
}

#[test]
fn self_recursive_newtype_digests_are_pinned() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let member = chain(types);
    assert_pinned(
        "Chain component",
        component_of(member, types),
        0x3f0f31b1_6e34b5a7_85f073d7_1b930cd8,
    );
    assert_handle_pinned(
        "Chain member reference",
        member,
        0xdaf5788a_e4690a2b_c841781d_131fada4,
    );
}

#[test]
fn self_referencing_union_digests_are_pinned() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let member = recursive_union(types);
    assert_pinned(
        "Tree component",
        component_of(member, types),
        0x1f308e1f_4c36139e_ea0ced12_0a8a762c,
    );
    assert_handle_pinned(
        "Tree member reference",
        member,
        0xa473d50b_0f3635a5_2c9af860_10e12ba9,
    );
}

#[test]
fn type_constructor_digests_are_pinned() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let member = constructor(types);
    assert_pinned(
        "Maybe component",
        component_of(member, types),
        0x0d692950_d63184af_895aa39c_407cf9fd,
    );
    assert_handle_pinned(
        "Maybe member reference",
        member,
        0x249c1453_407da9af_bdd40194_e3dc04a1,
    );
}

#[test]
fn generative_set_digests_are_pinned() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let member = generative(types);
    assert_pinned(
        "Opaque component",
        component_of(member, types),
        0x746c9f5e_77020e8c_eff6fdab_72a3d539,
    );
    assert_handle_pinned(
        "Opaque member reference",
        member,
        0xb63930e4_bfafd6aa_47bed6ff_8d85f845,
    );
}

/// The multi-member pins. See [`recursive_pair`]: the pair is one two-member component presented
/// in name-symbol order while this fixture declares it out of that order. Every other pin in this
/// file is a singleton and is permanent.
#[test]
fn recursive_pair_digests_are_pinned() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let members = recursive_pair(types);
    assert_pinned(
        "Odd/Even component",
        component_of(members[0], types),
        0xdca7f03c_fb26b0ae_bd30ebeb_06cfbecb,
    );
    assert_handle_pinned(
        "Odd member reference (component position 1)",
        members[0],
        0x0aded42d_a7d17f50_333f7582_bb26e664,
    );
    assert_handle_pinned(
        "Even member reference (component position 0)",
        members[1],
        0xa878d115_f2884c95_14c03ca5_396df755,
    );
}

/// A `ConstructorApply`'s arguments are a name-keyed `Record` fed name-sorted, so the insertion
/// order of the arguments record is presentation: both orders land on one pinned value.
#[test]
fn constructor_apply_digest_is_pinned_and_order_blind() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let declared = constructor_apply(types, vec![("Ok", KType::NUMBER), ("Error", KType::STR)]);
    let reversed = constructor_apply(types, vec![("Error", KType::STR), ("Ok", KType::NUMBER)]);
    assert_handle_pinned(
        "Both(Ok = Number, Error = Str)",
        declared,
        0x740b934e_d6d9a6e8_6db7cb80_1246dd74,
    );
    assert_handle_pinned(
        "Both applied in reverse argument order",
        reversed,
        0x740b934e_d6d9a6e8_6db7cb80_1246dd74,
    );
}

/// A schema's abstract members feed `byte(0)` for a first-order member and `byte(1)` + parameter
/// count + sorted parameter names for a higher-kinded one. So both parameter orders of `Wrap` land
/// on one pinned value, and making `Wrap` first-order lands on a different one.
#[test]
fn schema_abstract_member_digests_are_pinned() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert_pinned(
        "schema with higher-kinded Wrap",
        schema_content_digest(&mixed_schema(types, vec!["Inner", "Outer"]), types),
        0xa840be7d_0e0e19e2_4b6cca13_32304af0,
    );
    assert_pinned(
        "schema with Wrap's parameters reordered",
        schema_content_digest(&mixed_schema(types, vec!["Outer", "Inner"]), types),
        0xa840be7d_0e0e19e2_4b6cca13_32304af0,
    );
    assert_pinned(
        "schema with first-order Wrap",
        schema_content_digest(&mixed_schema(types, Vec::new()), types),
        0x76aa2294_c71a336a_eb294e3b_bd0abfc9,
    );
}
