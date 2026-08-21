use super::*;

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
