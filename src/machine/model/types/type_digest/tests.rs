//! Content-addressing invariants for the node digest recipes: same content digests equal
//! regardless of field order, distinct content digests apart, and nonced content stays distinct.
//!
//! Each type is built through the registry (`KType` is a bare content handle), and its digest is
//! read back with [`KType::digest`]. Two independently built types with the same content intern to
//! one handle, so a digest compare and a handle compare say the same thing.
//!
//! The recursive-group recipes are exercised where they are computed, in
//! [`recursive_group_window`](super::super::recursive_group_window)'s tests — a component digest
//! is only meaningful alongside the Tarjan condensation that decides which members share one.

mod golden;

use super::*;
use crate::machine::core::ScopeId;
use crate::machine::model::types::{KType, Record, TypeNode};
use crate::machine::model::TypeRegistry;

fn record(types: &TypeRegistry, pairs: Vec<(&str, KType)>) -> KType {
    types.record(Record::from_pairs(
        pairs.into_iter().map(|(n, t)| (n.to_string(), t)),
    ))
}

#[test]
fn same_content_built_twice_digests_equal() {
    let types = TypeRegistry::new();
    let r1 = record(&types, vec![("x", KType::NUMBER), ("y", KType::STR)]);
    let r2 = record(&types, vec![("x", KType::NUMBER), ("y", KType::STR)]);
    assert_eq!(r1.digest(), r2.digest());

    let u1 = types.union_of(vec![KType::NUMBER, KType::STR]);
    let u2 = types.union_of(vec![KType::NUMBER, KType::STR]);
    assert_eq!(u1.digest(), u2.digest());
}

#[test]
fn record_digest_is_order_blind_but_binds_name_to_type() {
    let types = TypeRegistry::new();
    let ordered = record(&types, vec![("x", KType::NUMBER), ("y", KType::STR)]);
    let reversed = record(&types, vec![("y", KType::STR), ("x", KType::NUMBER)]);
    assert_eq!(
        ordered.digest(),
        reversed.digest(),
        "record identity ignores declaration order"
    );

    let x_number = record(&types, vec![("x", KType::NUMBER)]);
    let y_number = record(&types, vec![("y", KType::NUMBER)]);
    assert_ne!(
        x_number.digest(),
        y_number.digest(),
        "the field name is bound to its type in the digest"
    );
}

#[test]
fn union_digest_is_order_blind() {
    let types = TypeRegistry::new();
    let forward = types.union_of(vec![KType::NUMBER, KType::STR]);
    let reversed = types.union_of(vec![KType::STR, KType::NUMBER]);
    assert_eq!(forward.digest(), reversed.digest());
}

#[test]
fn leaves_and_composites_digest_distinctly_by_shape() {
    let types = TypeRegistry::new();
    assert_ne!(KType::NUMBER.digest(), KType::STR.digest());
    assert_ne!(KType::BOOL.digest(), KType::NULL.digest());
    assert_ne!(
        types.list(KType::NUMBER).digest(),
        types.list(KType::STR).digest(),
    );
    // A list of X and a dict keyed on X differ by domain tag even if payloads overlap.
    assert_ne!(
        types.list(KType::NUMBER).digest(),
        types.dict(KType::NUMBER, KType::NUMBER).digest(),
    );
}

/// `AbstractType` digests its whole content: the generativity `nonce`, the binder `source`, the
/// name, and the parameter names as a *set*. Two same-named members at different orders — `TYPE
/// Elt` versus `TYPE (X AS Elt)` — are different declarations and stay distinct, while parameter
/// *order* is presentation.
#[test]
fn abstract_type_digest_keys_on_full_content() {
    let types = TypeRegistry::new();
    let source = ScopeId::from_raw(0, 0xA11C);
    let member = |param_names: Vec<&str>| {
        types.intern(TypeNode::AbstractType {
            source,
            name: "Elt".into(),
            param_names: param_names.into_iter().map(str::to_string).collect(),
            nonce: None,
        })
    };

    // Order separates a first-order member from a same-named constructor.
    assert_ne!(member(vec![]).digest(), member(vec!["X"]).digest());
    assert_ne!(member(vec![]), member(vec!["X"]));
    // Arity separates two constructors.
    assert_ne!(member(vec!["X"]).digest(), member(vec!["X", "Y"]).digest(),);
    // A renamed parameter is a different interface.
    assert_ne!(member(vec!["X"]).digest(), member(vec!["Y"]).digest());
    // Parameter *order* is immaterial — identity is the name set.
    assert_eq!(
        member(vec!["X", "Y"]).digest(),
        member(vec!["Y", "X"]).digest(),
    );
    assert_eq!(member(vec!["X", "Y"]), member(vec!["Y", "X"]));

    // A different name is a different member.
    assert_ne!(
        member(vec![]).digest(),
        types
            .intern(TypeNode::AbstractType {
                source,
                name: "Other".into(),
                param_names: Vec::new(),
                nonce: None,
            })
            .digest(),
    );
}

/// Generativity rides `nonce`, not `source`: an opaque-ascription mint folds the per-application
/// module id in, so two applications of one SIG member — same `source`, same name — stay distinct,
/// and both stay distinct from the SIG-body declaration they were threaded from.
#[test]
fn abstract_type_nonce_is_generative() {
    let types = TypeRegistry::new();
    let source = ScopeId::from_raw(0, 0xA11C);
    let mint = |nonce: Option<ScopeId>| {
        types.intern(TypeNode::AbstractType {
            source,
            name: "Elt".into(),
            param_names: Vec::new(),
            nonce,
        })
    };
    let declared = mint(None);
    let first = mint(Some(ScopeId::from_raw(0, 0x01)));
    let second = mint(Some(ScopeId::from_raw(0, 0x02)));

    assert_ne!(first.digest(), second.digest());
    assert_ne!(first, second);
    assert_ne!(declared.digest(), first.digest());
    assert_ne!(declared, first);
    // The nonce is the only difference that matters — same nonce, same identity.
    assert_eq!(
        first.digest(),
        mint(Some(ScopeId::from_raw(0, 0x01))).digest()
    );
}

/// A SIG's abstract-member encoding feeds the member's parameter names, so two signatures
/// differing only in what a higher-kinded member calls its parameter are distinct interfaces —
/// the digest-side counterpart of `sig_subtype`'s name-agreement check. Order within one
/// member's list is presentation: the names feed sorted, so a reordered declaration is the same
/// interface.
#[test]
fn schema_digest_binds_abstract_member_param_names() {
    use crate::machine::model::types::SigSchema;
    let types = TypeRegistry::new();
    let sig_id = ScopeId::from_raw(0, 0x51C0);
    let schema = |param_names: Vec<&str>| SigSchema {
        sig_id: Some(sig_id),
        abstract_members: [(
            "Wrap".to_string(),
            types.intern(TypeNode::AbstractType {
                source: sig_id,
                name: "Wrap".into(),
                param_names: param_names.into_iter().map(str::to_string).collect(),
                nonce: None,
            }),
        )]
        .into_iter()
        .collect(),
        manifest_members: HashMap::new(),
        value_slots: HashMap::new(),
    };
    assert_ne!(
        schema_content_digest(&schema(vec!["Elem"]), &types),
        schema_content_digest(&schema(vec!["Item"]), &types),
        "a renamed parameter is a different interface",
    );
    assert_ne!(
        schema_content_digest(&schema(vec!["Elem"]), &types),
        schema_content_digest(&schema(vec![]), &types),
        "a first-order member and a constructor member are different interfaces",
    );
    assert_ne!(
        schema_content_digest(&schema(vec!["Elem"]), &types),
        schema_content_digest(&schema(vec!["Elem", "Item"]), &types),
        "arity is part of the interface",
    );
    assert_eq!(
        schema_content_digest(&schema(vec!["Elem", "Item"]), &types),
        schema_content_digest(&schema(vec!["Item", "Elem"]), &types),
        "a member's parameter identity is its name set, not its declaration order",
    );
}

/// `ConstructorApply` identity is `(ctor, args)` with `Record`'s order-blind semantics: the same
/// name-to-type map is one application however the args record was built.
#[test]
fn constructor_apply_digest_is_order_blind() {
    let types = TypeRegistry::new();
    let ctor = types.intern(TypeNode::AbstractType {
        source: ScopeId::from_raw(0, 0xC70A),
        name: "Both".into(),
        param_names: vec!["Ok".into(), "Error".into()],
        nonce: None,
    });
    let apply = |pairs: Vec<(&str, KType)>| {
        types.constructor_apply(
            ctor,
            Record::from_pairs(pairs.into_iter().map(|(n, t)| (n.to_string(), t))),
        )
    };
    let declared = apply(vec![("Ok", KType::NUMBER), ("Error", KType::STR)]);
    let reversed = apply(vec![("Error", KType::STR), ("Ok", KType::NUMBER)]);
    assert_eq!(declared.digest(), reversed.digest());
    assert_eq!(declared, reversed);
    // The name-to-type binding still holds: swapping which parameter takes which type differs.
    assert_ne!(
        declared.digest(),
        apply(vec![("Ok", KType::STR), ("Error", KType::NUMBER)]).digest(),
    );
}
