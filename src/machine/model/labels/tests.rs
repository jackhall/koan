use super::*;
use crate::machine::model::BindKind;

#[test]
fn equal_text_yields_equal_symbols() {
    assert_eq!(Symbol::of("field"), Symbol::of("field"));
}

#[test]
fn distinct_text_yields_distinct_symbols() {
    assert_ne!(Symbol::of("x"), Symbol::of("y"));
    assert_ne!(Symbol::of(""), Symbol::of("x"));
}

#[test]
fn intern_round_trips_through_resolve() {
    let interner = LabelInterner::new();
    let symbol = interner.intern("width");
    assert_eq!(interner.resolve(symbol).as_deref(), Some("width"));
    assert_eq!(symbol, Symbol::of("width"));
}

#[test]
fn interning_twice_records_one_entry() {
    let interner = LabelInterner::new();
    let first = interner.intern("height");
    let second = interner.intern("height");
    assert_eq!(first, second);
    assert_eq!(interner.len(), 1);
}

#[test]
fn resolving_an_uninterned_symbol_misses() {
    let interner = LabelInterner::new();
    assert_eq!(interner.resolve(Symbol::of("never-seen")), None);
}

/// The identity hasher accepts only `write_u128`; `Symbol`'s derived `Hash` must route through it,
/// which the table's every insert and lookup depends on.
#[test]
fn symbol_hashes_through_the_identity_hasher() {
    use std::collections::HashMap;
    let mut map: HashMap<Symbol, u8, IdentityBuildHasher> = HashMap::default();
    map.insert(Symbol::of("a"), 1);
    map.insert(Symbol::of("b"), 2);
    assert_eq!(map.get(&Symbol::of("a")), Some(&1));
    assert_eq!(map.get(&Symbol::of("b")), Some(&2));
    assert_eq!(map.get(&Symbol::of("c")), None);
}

#[test]
fn symbol_order_is_total_and_text_free() {
    let mut symbols = ["delta", "alpha", "charlie", "bravo"].map(Symbol::of);
    symbols.sort();
    for pair in symbols.windows(2) {
        assert!(pair[0] < pair[1]);
    }
}

/// The three classes partition the token space: every text lands in exactly one, so no two
/// classified symbols can wrap the same text.
#[test]
fn the_three_classes_partition_token_text() {
    for text in [
        "xs", "int_ord", "it", "IntOrd", "Carrier", "Ordered", "FN", "USING", "+", "<=", "+ *",
        "AND OR", "X",
    ] {
        let hits = [
            ValueSymbol::of(text).is_some(),
            TypeSymbol::of(text).is_some(),
            KeywordSymbol::of(text).is_some(),
        ]
        .into_iter()
        .filter(|hit| *hit)
        .count();
        assert_eq!(hits, 1, "{text} classified into {hits} classes");
    }
}

#[test]
fn value_symbols_accept_value_tokens_only() {
    assert!(ValueSymbol::of("xs").is_some());
    assert!(ValueSymbol::of("int_ord").is_some());
    // Nothing binds to a keyword.
    assert!(ValueSymbol::of("USING").is_none());
    assert!(ValueSymbol::of("+").is_none());
    assert!(ValueSymbol::of("IntOrd").is_none());
}

#[test]
fn type_symbols_accept_type_tokens_only() {
    assert!(TypeSymbol::of("IntOrd").is_some());
    assert!(TypeSymbol::of("Carrier").is_some());
    assert!(TypeSymbol::of("xs").is_none());
    assert!(TypeSymbol::of("FN").is_none());
}

/// A space-joined operator probe key gains no lowercase letter, so it stays keyword-class — which
/// is what lets the operator table key by `KeywordSymbol`.
#[test]
fn keyword_symbols_cover_joined_operator_probes() {
    assert!(KeywordSymbol::of("+").is_some());
    assert!(KeywordSymbol::of("<=").is_some());
    assert!(KeywordSymbol::of("+ *").is_some());
    assert!(KeywordSymbol::of("AND OR").is_some());
    assert!(KeywordSymbol::of("xs").is_none());
    assert!(KeywordSymbol::of("IntOrd").is_none());
}

#[test]
fn binder_symbols_carry_the_bind_kind_and_reject_keywords() {
    assert_eq!(
        BinderSymbol::of("xs").map(BinderSymbol::bind_kind),
        Some(BindKind::Value)
    );
    assert_eq!(
        BinderSymbol::of("IntOrd").map(BinderSymbol::bind_kind),
        Some(BindKind::Type)
    );
    assert!(BinderSymbol::of("USING").is_none());
}

/// `of` is a probe: it classifies without recording anything. `declared` is the declaration
/// constructor and interns, so a diagnostic can render the name later.
#[test]
fn only_declared_interns_the_text() {
    let interner = LabelInterner::new();
    assert!(ValueSymbol::of("probed").is_some());
    assert_eq!(interner.len(), 0);

    let declared = ValueSymbol::declared("bound", &interner).expect("value token");
    assert_eq!(
        interner.resolve(declared.symbol()).as_deref(),
        Some("bound")
    );

    let rejected = ValueSymbol::declared("USING", &interner);
    assert!(rejected.is_none());
    assert_eq!(interner.len(), 1, "a rejected name is not recorded");
}

/// The wrapper is the same bits as the bare digest — a classified key and a raw `Symbol` feed the
/// same identity hasher and compare the same way.
#[test]
fn classified_symbols_carry_the_bare_digest() {
    assert_eq!(
        ValueSymbol::of("xs").expect("value token").symbol(),
        Symbol::of("xs")
    );
    assert_eq!(
        TypeSymbol::of("IntOrd").expect("Type token").symbol(),
        Symbol::of("IntOrd")
    );
    assert_eq!(
        KeywordSymbol::of("+ *").expect("keyword token").symbol(),
        Symbol::of("+ *")
    );
    assert_eq!(
        BinderSymbol::of("xs").expect("value token").symbol(),
        Symbol::of("xs")
    );
}

/// Every classified key type must route its derived `Hash` through `write_u128`, which the
/// identity hasher is the only accepting path for.
#[test]
fn classified_symbols_hash_through_the_identity_hasher() {
    use std::collections::HashMap;
    let mut values: HashMap<ValueSymbol, u8, IdentityBuildHasher> = HashMap::default();
    values.insert(ValueSymbol::of("xs").expect("value token"), 1);
    assert_eq!(
        values.get(&ValueSymbol::of("xs").expect("value token")),
        Some(&1)
    );

    let mut types: HashMap<TypeSymbol, u8, IdentityBuildHasher> = HashMap::default();
    types.insert(TypeSymbol::of("IntOrd").expect("Type token"), 2);
    assert_eq!(
        types.get(&TypeSymbol::of("IntOrd").expect("Type token")),
        Some(&2)
    );

    let mut keywords: HashMap<KeywordSymbol, u8, IdentityBuildHasher> = HashMap::default();
    keywords.insert(KeywordSymbol::of("+ *").expect("keyword token"), 3);
    assert_eq!(
        keywords.get(&KeywordSymbol::of("+ *").expect("keyword token")),
        Some(&3)
    );
}
