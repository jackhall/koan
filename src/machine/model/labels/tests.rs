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
            ValueSymbol::classify(text).is_some(),
            TypeSymbol::classify(text).is_some(),
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
    assert!(ValueSymbol::classify("xs").is_some());
    assert!(ValueSymbol::classify("int_ord").is_some());
    // Nothing binds to a keyword.
    assert!(ValueSymbol::classify("USING").is_none());
    assert!(ValueSymbol::classify("+").is_none());
    assert!(ValueSymbol::classify("IntOrd").is_none());
}

#[test]
fn type_symbols_accept_type_tokens_only() {
    assert!(TypeSymbol::classify("IntOrd").is_some());
    assert!(TypeSymbol::classify("Carrier").is_some());
    assert!(TypeSymbol::classify("xs").is_none());
    assert!(TypeSymbol::classify("FN").is_none());
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

/// A run digest reads its members as a *set*: order does not matter and repeats collapse. This is
/// what lets a chain's probe (`operator_probe_for`, which streams operators in source order, with
/// repeats) hit the key its group's powerset registered (`powerset_probes`, which streams a subset)
/// — a divergence here would make every chain miss its own registration.
#[test]
fn a_run_digest_is_order_insensitive_and_dedupes() {
    let run = |glyphs: &[&str]| {
        let members: Vec<_> = glyphs
            .iter()
            .map(|glyph| KeywordSymbol::of(glyph).expect("a fixture glyph is keyword-class"))
            .collect();
        KeywordSymbol::of_run(&members)
    };
    assert_eq!(run(&["*", "+"]), run(&["+", "*"]));
    assert_eq!(run(&["+", "*", "+"]), run(&["+", "*"]));
    assert_eq!(run(&["+"]), run(&["+", "+"]));
    assert_ne!(run(&["+"]), run(&["+", "*"]));
}

/// A singleton run is its own key, distinct from the bare member symbol: a probe is a digest over
/// the run, so nothing can confuse the group registered for `{+}` with the token `+` itself.
#[test]
fn a_singleton_run_is_not_its_member() {
    let plus = KeywordSymbol::of("+").expect("`+` is keyword-class");
    assert_ne!(KeywordSymbol::of_run(&[plus]), plus);
}

/// The rendering `declared_run` records is what a diagnostic naming a probe key resolves, and it is
/// recorded under the very digest `of_run` mints, so registration and probe agree by construction.
#[test]
fn a_declared_run_records_its_join_under_the_probe_digest() {
    let labels = LabelInterner::new();
    let members: Vec<_> = ["+", "*"]
        .iter()
        .map(|glyph| {
            KeywordSymbol::declared(glyph, &labels).expect("a fixture glyph is keyword-class")
        })
        .collect();
    let declared = KeywordSymbol::declared_run(&members, &labels);
    assert_eq!(declared, KeywordSymbol::of_run(&members));
    let rendered = labels.render(declared.symbol());
    assert!(
        rendered
            .split(' ')
            .collect::<std::collections::HashSet<_>>()
            == ["+", "*"].into_iter().collect(),
        "the recorded rendering names both members: {rendered}",
    );
}

#[test]
fn binder_symbols_carry_the_bind_kind_and_reject_keywords() {
    assert_eq!(
        BinderSymbol::classify("xs").map(BinderSymbol::bind_kind),
        Some(BindKind::Value)
    );
    assert_eq!(
        BinderSymbol::classify("IntOrd").map(BinderSymbol::bind_kind),
        Some(BindKind::Type)
    );
    assert!(BinderSymbol::classify("USING").is_none());
}

/// `classify` is the pure funnel: it decides the class without recording anything. `declared` is
/// the declaration constructor and interns, so a diagnostic can render the name later.
#[test]
fn only_declared_interns_the_text() {
    let interner = LabelInterner::new();
    assert!(ValueSymbol::classify("probed").is_some());
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
        ValueSymbol::classify("xs").expect("value token").symbol(),
        Symbol::of("xs")
    );
    assert_eq!(
        TypeSymbol::classify("IntOrd").expect("Type token").symbol(),
        Symbol::of("IntOrd")
    );
    assert_eq!(
        KeywordSymbol::of("+ *").expect("keyword token").symbol(),
        Symbol::of("+ *")
    );
    assert_eq!(
        BinderSymbol::classify("xs").expect("value token").symbol(),
        Symbol::of("xs")
    );
}

/// Every classified key type must route its derived `Hash` through `write_u128`, which the
/// identity hasher is the only accepting path for.
#[test]
fn classified_symbols_hash_through_the_identity_hasher() {
    use std::collections::HashMap;
    let mut values: HashMap<ValueSymbol, u8, IdentityBuildHasher> = HashMap::default();
    values.insert(ValueSymbol::classify("xs").expect("value token"), 1);
    assert_eq!(
        values.get(&ValueSymbol::classify("xs").expect("value token")),
        Some(&1)
    );

    let mut types: HashMap<TypeSymbol, u8, IdentityBuildHasher> = HashMap::default();
    types.insert(TypeSymbol::classify("IntOrd").expect("Type token"), 2);
    assert_eq!(
        types.get(&TypeSymbol::classify("IntOrd").expect("Type token")),
        Some(&2)
    );

    let mut keywords: HashMap<KeywordSymbol, u8, IdentityBuildHasher> = HashMap::default();
    keywords.insert(KeywordSymbol::of("+ *").expect("keyword token"), 3);
    assert_eq!(
        keywords.get(&KeywordSymbol::of("+ *").expect("keyword token")),
        Some(&3)
    );
}

/// A name of this module's own, so the tests below pin [`StaticName`] itself rather than whatever
/// spelling a builtin happens to declare.
static SLOT: StaticName<ValueSymbol> = crate::static_name!(ValueSymbol, "slot");

// A group of this module's own, pinning `slots!` the same way.
crate::slots! { GROUP { width, height } }

/// A [`StaticName`]'s memo is exactly what its class's `classify` mints — the whole basis for
/// reading a slot by static instead of by spelling.
#[test]
fn a_static_name_mints_what_classify_mints() {
    assert_eq!(
        SLOT.symbol(),
        ValueSymbol::classify("slot").expect("`slot` is a value token")
    );
    assert_eq!(SLOT.text(), "slot");
}

#[test]
fn record_interns_the_spelling_under_the_memoized_symbol() {
    let labels = LabelInterner::new();
    let before = labels.len();
    let classified = labels.record(&SLOT);
    assert_eq!(classified, SLOT.symbol());
    assert_eq!(
        labels.resolve(classified.symbol()),
        Some("slot".to_string())
    );
    assert_eq!(labels.len(), before + 1);
    labels.record(&SLOT);
    assert_eq!(labels.len(), before + 1);
}

/// A grouped slot is the same declaration a lone [`StaticName`] is: the ident supplies the
/// spelling, and each field carries its own memo rather than sharing one.
#[test]
fn a_slot_group_declares_each_field_independently() {
    assert_eq!(GROUP.width.text(), "width");
    assert_eq!(GROUP.height.text(), "height");
    assert_eq!(
        GROUP.width.symbol(),
        ValueSymbol::classify("width").expect("`width` is a value token")
    );
    assert_eq!(
        GROUP.height.symbol(),
        ValueSymbol::classify("height").expect("`height` is a value token")
    );
    assert_ne!(GROUP.width.symbol(), GROUP.height.symbol());
}

/// Each field records on its own, so a group interns one entry per slot — the count a diagnostic
/// resolving any one of them depends on.
#[test]
fn record_interns_each_grouped_slot_separately() {
    let labels = LabelInterner::new();
    labels.record(&GROUP.width);
    labels.record(&GROUP.height);
    assert_eq!(labels.len(), 2);
    assert_eq!(
        labels.resolve(GROUP.width.symbol().symbol()),
        Some("width".to_string())
    );
    assert_eq!(
        labels.resolve(GROUP.height.symbol().symbol()),
        Some("height".to_string())
    );
}

/// The `Display` view prints what `render` returns, on both the hit and the miss, so a message
/// that names a label reads the same whichever door built it.
#[test]
fn display_matches_render_on_a_hit_and_on_a_miss() {
    let labels = LabelInterner::new();
    let recorded = labels.intern("Ordered");
    let never_seen = Symbol::of("Absent");
    assert_eq!(
        labels.display(recorded).to_string(),
        labels.render(recorded)
    );
    assert_eq!(labels.display(recorded).to_string(), "Ordered");
    assert_eq!(
        labels.display(never_seen).to_string(),
        labels.render(never_seen)
    );
    assert_eq!(labels.display(never_seen).to_string(), "<label>");
}

/// `declared` classifies and records with one hash: the symbol it hands back is the one the
/// interner keyed the text under, so a later `resolve` of that symbol finds the spelling.
#[test]
fn declared_records_the_text_under_the_symbol_it_returns() {
    let labels = LabelInterner::new();
    let name = TypeSymbol::declared("Ordered", &labels).expect("`Ordered` is a Type token");
    assert_eq!(labels.resolve(name.symbol()), Some("Ordered".to_string()));
    assert_eq!(labels.len(), 1);
}

/// Sorted rendering arms order by the name a reader sees, and an unrecorded symbol takes the
/// placeholder's position rather than a digest-dependent one.
#[test]
fn compare_texts_orders_by_recorded_text() {
    use std::cmp::Ordering;
    let interner = LabelInterner::new();
    let alpha = interner.intern("alpha");
    let beta = interner.intern("beta");
    assert_eq!(interner.compare_texts(alpha, beta), Ordering::Less);
    assert_eq!(interner.compare_texts(beta, alpha), Ordering::Greater);
    assert_eq!(interner.compare_texts(alpha, alpha), Ordering::Equal);

    // "<label>" sorts below every alphabetic name, and two missing symbols tie.
    let missing = Symbol::of("never-interned");
    let other_missing = Symbol::of("also-never-interned");
    assert_eq!(interner.compare_texts(missing, alpha), Ordering::Less);
    assert_eq!(interner.compare_texts(alpha, missing), Ordering::Greater);
    assert_eq!(
        interner.compare_texts(missing, other_missing),
        Ordering::Equal
    );
}
