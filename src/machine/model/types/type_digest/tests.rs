//! Content-addressing invariants for [`digest_of`] / [`set_digest`]: same content digests
//! equal regardless of allocation and field/member order, distinct content digests apart, the
//! two generative exceptions stay distinct, and a set seals its digest on the last member fill.

use super::*;
use crate::machine::core::ScopeId;
use crate::machine::model::types::{
    KKind, KType, NominalMember, NominalSchema, Record, RecursiveSet,
};

fn record(pairs: Vec<(&str, KType<'static>)>) -> KType<'static> {
    KType::record(Box::new(Record::from_pairs(
        pairs.into_iter().map(|(n, t)| (n.to_string(), t)),
    )))
}

fn newtype_singleton(
    name: &str,
    scope: ScopeId,
    repr: KType<'static>,
) -> std::rc::Rc<RecursiveSet<'static>> {
    RecursiveSet::singleton(name.into(), scope, NominalSchema::NewType(Box::new(repr)))
}

#[test]
fn same_content_built_twice_digests_equal() {
    let r1 = record(vec![("x", KType::Number), ("y", KType::Str)]);
    let r2 = record(vec![("x", KType::Number), ("y", KType::Str)]);
    assert_eq!(digest_of(&r1), digest_of(&r2));

    let u1 = KType::union_of(vec![KType::Number, KType::Str]);
    let u2 = KType::union_of(vec![KType::Number, KType::Str]);
    assert_eq!(digest_of(&u1), digest_of(&u2));
}

#[test]
fn record_digest_is_order_blind_but_binds_name_to_type() {
    let ordered = record(vec![("x", KType::Number), ("y", KType::Str)]);
    let reversed = record(vec![("y", KType::Str), ("x", KType::Number)]);
    assert_eq!(
        digest_of(&ordered),
        digest_of(&reversed),
        "record identity ignores declaration order"
    );

    let x_number = record(vec![("x", KType::Number)]);
    let y_number = record(vec![("y", KType::Number)]);
    assert_ne!(
        digest_of(&x_number),
        digest_of(&y_number),
        "the field name is bound to its type in the digest"
    );
}

#[test]
fn union_digest_is_order_blind() {
    let forward = KType::union_of(vec![KType::Number, KType::Str]);
    let reversed = KType::union_of(vec![KType::Str, KType::Number]);
    assert_eq!(digest_of(&forward), digest_of(&reversed));
}

#[test]
fn leaves_and_composites_digest_distinctly_by_shape() {
    assert_ne!(digest_of(&KType::Number), digest_of(&KType::Str));
    assert_ne!(digest_of(&KType::Bool), digest_of(&KType::Null));
    assert_ne!(
        digest_of(&KType::list(Box::new(KType::Number))),
        digest_of(&KType::list(Box::new(KType::Str))),
    );
    // A list of X and a dict keyed on X differ by domain tag even if payloads overlap.
    assert_ne!(
        digest_of(&KType::list(Box::new(KType::Number))),
        digest_of(&KType::dict(
            Box::new(KType::Number),
            Box::new(KType::Number)
        )),
    );
}

#[test]
fn functor_digest_matches_by_shape_and_stays_apart_from_function() {
    let functor = |ret| {
        KType::functor_type(
            Record::from_pairs(vec![("x".to_string(), KType::Number)]),
            Box::new(ret),
            None,
        )
    };
    assert_eq!(
        digest_of(&functor(KType::Str)),
        digest_of(&functor(KType::Str))
    );

    let function = KType::function_type(
        Record::from_pairs(vec![("x".to_string(), KType::Number)]),
        Box::new(KType::Str),
    );
    assert_ne!(
        digest_of(&functor(KType::Str)),
        digest_of(&function),
        "functor and function of the same shape stay distinct by variant tag"
    );
    // `body` exclusion is enforced structurally (`digest_of` never reads it) and pinned by
    // the Eq/Hash body-exclusion test in `machine::core::kfunction::tests`.
}

#[test]
fn independently_built_sets_unify_and_exclude_scope_id() {
    // Same name + schema, different scope ids: the digest excludes `scope_id`, so they unify.
    let s1 = newtype_singleton("Foo", ScopeId::from_raw(7, 1), KType::Number);
    let s2 = newtype_singleton("Foo", ScopeId::from_raw(9, 2), KType::Number);
    assert!(s1.digest().is_some());
    assert_eq!(
        s1.digest(),
        s2.digest(),
        "content unifies across allocations"
    );
    assert_eq!(
        digest_of(&KType::SetRef {
            set: s1.clone(),
            index: 0
        }),
        digest_of(&KType::SetRef { set: s2, index: 0 }),
    );

    // A different member name is different content.
    let bar = newtype_singleton("Bar", ScopeId::from_raw(7, 1), KType::Number);
    assert_ne!(s1.digest(), bar.digest());
}

#[test]
fn generative_sets_never_unify() {
    let generative = |nonce: ScopeId| {
        let set = RecursiveSet::new_generative(
            vec![NominalMember::pending("Op".into(), nonce, KKind::NewType)],
            nonce,
        );
        set.fill_member(0, NominalSchema::NewType(Box::new(KType::Number)));
        set
    };
    let g1 = generative(ScopeId::from_raw(1, 1));
    let g2 = generative(ScopeId::from_raw(2, 2));
    assert_ne!(
        g1.digest(),
        g2.digest(),
        "distinct nonces fold to distinct digests"
    );

    // A content-addressed set of the same shape is distinct from any generative mint.
    let plain = newtype_singleton("Op", ScopeId::from_raw(1, 1), KType::Number);
    assert_ne!(g1.digest(), plain.digest());
}

#[test]
fn multi_member_set_seals_digest_on_last_fill() {
    let set = RecursiveSet::new(vec![
        NominalMember::pending("A".into(), ScopeId::SENTINEL, KKind::NewType),
        NominalMember::pending("B".into(), ScopeId::SENTINEL, KKind::NewType),
    ]);
    assert!(set.digest().is_none(), "unsealed before any fill");
    set.fill_member(0, NominalSchema::NewType(Box::new(KType::Number)));
    assert!(
        set.digest().is_none(),
        "still unsealed after one of two fills"
    );
    set.fill_member(1, NominalSchema::NewType(Box::new(KType::Str)));
    assert!(set.digest().is_some(), "sealed once every member filled");
}

#[test]
fn schema_embedding_external_setref_digests_deterministically() {
    let build = || {
        let inner = newtype_singleton("Inner", ScopeId::from_raw(3, 3), KType::Number);
        let outer = newtype_singleton(
            "Outer",
            ScopeId::from_raw(4, 4),
            KType::SetRef {
                set: inner,
                index: 0,
            },
        );
        outer.digest().expect("sealed on fill")
    };
    assert_eq!(
        build(),
        build(),
        "a set over an external SetRef is content-addressed"
    );
}
