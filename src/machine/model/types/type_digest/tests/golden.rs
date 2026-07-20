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
use std::rc::Rc;

use super::super::{digest_of, empty_schema_digest, schema_content_digest, TypeDigest};
use crate::machine::core::ScopeId;
use crate::machine::model::types::{
    KKind, KType, NominalMember, NominalSchema, Record, RecursiveSet, SigSchema,
};

#[track_caller]
fn assert_pinned(label: &str, actual: TypeDigest, expected: u128) {
    assert_eq!(
        actual.0, expected,
        "{label}: digest recipe moved — recomputed value is 0x{:032x}",
        actual.0
    );
}

fn record(pairs: Vec<(&str, KType)>) -> KType {
    KType::record(Box::new(Record::from_pairs(
        pairs.into_iter().map(|(n, t)| (n.to_string(), t)),
    )))
}

fn newtype(repr: KType) -> NominalSchema {
    NominalSchema::NewType(Box::new(repr))
}

/// A non-recursive newtype: `Meters` over `Number`.
fn meters() -> Rc<RecursiveSet> {
    RecursiveSet::singleton("Meters".into(), newtype(KType::Number))
}

/// A self-recursive newtype — the sibling reference in its representation is a `SetLocal`.
fn chain() -> Rc<RecursiveSet> {
    RecursiveSet::singleton(
        "Chain".into(),
        newtype(record(vec![
            ("head", KType::Number),
            ("tail", KType::SetLocal(0)),
        ])),
    )
}

/// A newtype whose representation is a union naming itself — the binder shape a self-referencing
/// union declaration seals to.
fn recursive_union() -> Rc<RecursiveSet> {
    RecursiveSet::singleton(
        "Tree".into(),
        newtype(KType::union_of(vec![KType::Number, KType::SetLocal(0)])),
    )
}

/// A type constructor carrying parameter names.
fn constructor() -> Rc<RecursiveSet> {
    let schema: HashMap<String, KType> = [
        ("Empty".to_string(), KType::Null),
        ("Full".to_string(), KType::Any),
    ]
    .into_iter()
    .collect();
    RecursiveSet::singleton(
        "Maybe".into(),
        NominalSchema::TypeConstructor {
            schema,
            param_names: vec!["Elem".to_string()],
        },
    )
}

/// A generative set at a fixed nonce — opaque ascription's per-application mint.
fn generative() -> Rc<RecursiveSet> {
    let set = RecursiveSet::new_generative(
        vec![NominalMember::pending("Opaque".into(), KKind::NewType)],
        ScopeId::from_raw(0, 0x0BAB),
    );
    set.fill_member(0, newtype(KType::Number));
    Rc::new(set)
}

/// A genuinely mutually-recursive pair, declared **out of name order** (`Odd` at index 0, `Even`
/// at index 1) so the pins below record the declaration-order presentation.
///
/// These multi-member pins are the one set the per-SCC member-identity recipe deliberately
/// re-pins: member identity becomes the computed strongly-connected component, and a mutually
/// recursive pair is one two-member component whose canonical order is *name* order. This
/// fixture's declaration order differs from its name order, so its set digest and both member
/// `SetRef` digests change under that recipe — by design. Every other pin in this file is a
/// singleton and is permanent.
fn recursive_pair() -> Rc<RecursiveSet> {
    let set = RecursiveSet::new(vec![
        NominalMember::pending("Odd".into(), KKind::NewType),
        NominalMember::pending("Even".into(), KKind::NewType),
    ]);
    set.fill_member(0, newtype(record(vec![("pred", KType::SetLocal(1))])));
    set.fill_member(1, newtype(record(vec![("pred", KType::SetLocal(0))])));
    Rc::new(set)
}

fn member_ref(set: Rc<RecursiveSet>, index: usize) -> KType {
    KType::SetRef { set, index }
}

/// The abstract-member source shared by the signature-schema pins.
const SIG_SOURCE: ScopeId = ScopeId::from_raw(0, 0x51C0);

fn abstract_member(name: &str, param_names: Vec<&str>) -> KType {
    KType::AbstractType {
        source: SIG_SOURCE,
        name: name.into(),
        param_names: param_names.into_iter().map(str::to_string).collect(),
        nonce: None,
    }
}

/// A schema with a first-order member `Elem` and a higher-kinded member `Wrap` over `wrap_params`.
fn mixed_schema(wrap_params: Vec<&str>) -> SigSchema {
    SigSchema {
        sig_id: Some(SIG_SOURCE),
        abstract_members: [
            ("Elem".to_string(), abstract_member("Elem", Vec::new())),
            ("Wrap".to_string(), abstract_member("Wrap", wrap_params)),
        ]
        .into_iter()
        .collect(),
        manifest_members: HashMap::new(),
        value_slots: HashMap::new(),
    }
}

fn constructor_apply(pairs: Vec<(&str, KType)>) -> KType {
    KType::constructor_apply(
        Box::new(KType::AbstractType {
            source: ScopeId::from_raw(0, 0xC70A),
            name: "Both".into(),
            param_names: vec!["Ok".into(), "Error".into()],
            nonce: None,
        }),
        Record::from_pairs(pairs.into_iter().map(|(n, t)| (n.to_string(), t))),
    )
}

/// The nine leaf types. Each is a bare domain tag, so these are the most load-bearing pins in the
/// file: they are the leaves every composite digest is built from.
#[test]
fn leaf_digests_are_pinned() {
    assert_pinned(
        "Number",
        digest_of(&KType::Number),
        0xe21d67f1_7aa25f92_e072c1bb_1f72fc48,
    );
    assert_pinned(
        "Str",
        digest_of(&KType::Str),
        0xda8a6add_c7627c0f_ae4be842_dfbe13ab,
    );
    assert_pinned(
        "Bool",
        digest_of(&KType::Bool),
        0x01210944_fd6fb8f8_0c9ba36e_1de8e0e1,
    );
    assert_pinned(
        "Null",
        digest_of(&KType::Null),
        0xbc9d88bb_75d5fb35_a4fd343e_749a380c,
    );
    assert_pinned(
        "Identifier",
        digest_of(&KType::Identifier),
        0x41b73c3e_2391bbb4_6b850e4f_e740cb84,
    );
    assert_pinned(
        "KExpression",
        digest_of(&KType::KExpression),
        0x63c296ef_dbe5d41c_9969ddda_6b0b311c,
    );
    assert_pinned(
        "SigiledTypeExpr",
        digest_of(&KType::SigiledTypeExpr),
        0xf6d652dc_848e0f69_4a152496_ddd88b44,
    );
    assert_pinned(
        "RecordType",
        digest_of(&KType::RecordType),
        0x387dfced_dc0a5d96_da3b29a5_dde0f32e,
    );
    assert_pinned(
        "Any",
        digest_of(&KType::Any),
        0xd9f70f99_49f95b5c_44d7ce99_10aa1972,
    );
}

/// The five kind values, each a tag plus its stable `kkind_tag` byte.
#[test]
fn of_kind_digests_are_pinned() {
    assert_pinned(
        "OfKind ProperType",
        digest_of(&KType::OfKind(KKind::ProperType)),
        0xe082d96a_231e2f4c_af1e256b_459a681f,
    );
    assert_pinned(
        "OfKind Signature",
        digest_of(&KType::OfKind(KKind::Signature)),
        0xa74d105b_68705a5a_4c93c325_b2bb4032,
    );
    assert_pinned(
        "OfKind AnyType",
        digest_of(&KType::OfKind(KKind::AnyType)),
        0x6230fb6f_d4cb83ad_59072aad_08f93e54,
    );
    assert_pinned(
        "OfKind NewType",
        digest_of(&KType::OfKind(KKind::NewType)),
        0x3079a661_6197d2a5_46103cc5_f0cbfeaa,
    );
    assert_pinned(
        "OfKind TypeConstructor",
        digest_of(&KType::OfKind(KKind::TypeConstructor)),
        0x1522ec89_d5fd3ca8_2db00c80_75beafb3,
    );
}

/// The two fixed composites the container builtins lower to.
#[test]
fn fixed_composite_digests_are_pinned() {
    assert_pinned(
        "List<Any>",
        digest_of(&KType::list(Box::new(KType::Any))),
        0x9d40af7c_078f46c4_bd4a8f94_98f5fd63,
    );
    assert_pinned(
        "Dict<Any, Any>",
        digest_of(&KType::dict(Box::new(KType::Any), Box::new(KType::Any))),
        0xf9b9d64d_aa69edda_e7a59f82_4e0f5015,
    );
}

/// The module-lattice top, both halves: the zero-member schema content digest and the signature
/// that wraps it with no `WITH` pins.
#[test]
fn empty_signature_digests_are_pinned() {
    assert_pinned(
        "empty schema content",
        empty_schema_digest(),
        0xca37d6c1_0e957006_5c08a0d2_ad8b02f8,
    );
    assert_pinned(
        "empty signature",
        digest_of(&KType::empty_signature()),
        0xaba2f8c7_47c0f5ed_9783abfe_50ce36c0,
    );
}

#[test]
fn non_recursive_newtype_digests_are_pinned() {
    assert_pinned(
        "Meters set",
        meters().digest().expect("sealed on fill"),
        0xa5bab723_08985b67_fdc176d5_b9e836b1,
    );
    assert_pinned(
        "Meters member reference",
        digest_of(&member_ref(meters(), 0)),
        0xaa9dc344_ea08a395_63635ec0_be611e20,
    );
}

#[test]
fn self_recursive_newtype_digests_are_pinned() {
    assert_pinned(
        "Chain set",
        chain().digest().expect("sealed on fill"),
        0xaaab8251_b184aebe_af32f73c_592df0cf,
    );
    assert_pinned(
        "Chain member reference",
        digest_of(&member_ref(chain(), 0)),
        0xcdfbfaac_8fae50c8_850808f7_27df0fa2,
    );
}

#[test]
fn self_referencing_union_digests_are_pinned() {
    assert_pinned(
        "Tree set",
        recursive_union().digest().expect("sealed on fill"),
        0xd0d777d4_90760cd1_778b02fd_6ecdf5ca,
    );
    assert_pinned(
        "Tree member reference",
        digest_of(&member_ref(recursive_union(), 0)),
        0xeee5c699_feb5a1c8_4b913ea2_313272cd,
    );
}

#[test]
fn type_constructor_digests_are_pinned() {
    assert_pinned(
        "Maybe set",
        constructor().digest().expect("sealed on fill"),
        0x8fce3135_01caf69c_dae1dfba_79f02281,
    );
    assert_pinned(
        "Maybe member reference",
        digest_of(&member_ref(constructor(), 0)),
        0x5ebc2110_fa8a5b65_ae71cd45_ee6636cf,
    );
}

#[test]
fn generative_set_digests_are_pinned() {
    assert_pinned(
        "Opaque set",
        generative().digest().expect("sealed on fill"),
        0xedf73e8f_68d1d2a2_5ceee390_0def9a25,
    );
    assert_pinned(
        "Opaque member reference",
        digest_of(&member_ref(generative(), 0)),
        0x339743c3_12b34d96_42134765_aef171a0,
    );
}

/// The multi-member pins. See [`recursive_pair`]: these three values are the one group the
/// per-SCC member-identity recipe re-pins by design, because this fixture declares its members
/// out of name order. Every other pin in this file is a singleton and is permanent.
#[test]
fn recursive_pair_digests_are_pinned() {
    assert_pinned(
        "Odd/Even set",
        recursive_pair().digest().expect("sealed on last fill"),
        0x01700733_6176f236_527afc27_a9fd0300,
    );
    assert_pinned(
        "Odd member reference",
        digest_of(&member_ref(recursive_pair(), 0)),
        0x2166b6f6_d1cbcc06_3c877d08_cc7aa3b9,
    );
    assert_pinned(
        "Even member reference",
        digest_of(&member_ref(recursive_pair(), 1)),
        0xa1126c4e_a1acc069_f0fad305_2ec89f4b,
    );
}

/// A `ConstructorApply`'s args are a name-keyed `Record` fed name-sorted, so the insertion order
/// of the args record is presentation: both orders land on one pinned value.
#[test]
fn constructor_apply_digest_is_pinned_and_order_blind() {
    let declared = constructor_apply(vec![("Ok", KType::Number), ("Error", KType::Str)]);
    let reversed = constructor_apply(vec![("Error", KType::Str), ("Ok", KType::Number)]);
    assert_pinned(
        "Both(Ok = Number, Error = Str)",
        digest_of(&declared),
        0xeadbdff7_6b59c1f3_70761787_1f06cd46,
    );
    assert_pinned(
        "Both applied in reverse argument order",
        digest_of(&reversed),
        0xeadbdff7_6b59c1f3_70761787_1f06cd46,
    );
}

/// A schema's abstract members feed `byte(0)` for a first-order member and `byte(1)` + parameter
/// count + sorted parameter names for a higher-kinded one. So both parameter orders of `Wrap` land
/// on one pinned value, and making `Wrap` first-order lands on a different one.
#[test]
fn schema_abstract_member_digests_are_pinned() {
    assert_pinned(
        "schema with higher-kinded Wrap",
        schema_content_digest(&mixed_schema(vec!["Inner", "Outer"])),
        0x74c887c4_2b7bdd55_7a481826_b15078ee,
    );
    assert_pinned(
        "schema with Wrap's parameters reordered",
        schema_content_digest(&mixed_schema(vec!["Outer", "Inner"])),
        0x74c887c4_2b7bdd55_7a481826_b15078ee,
    );
    assert_pinned(
        "schema with first-order Wrap",
        schema_content_digest(&mixed_schema(Vec::new())),
        0xdcaf6f29_107c1417_c55d837b_7e90fe20,
    );
}
