//! Label-vocabulary laws: what a symbol is, what the interner records, and how the three token
//! classes partition a spelling. The hasher-plumbing pins stay instances — they fix a routing
//! choice (`write_u128` and nothing else), not a statement over inputs.

use std::collections::HashSet;

use proptest::prelude::*;

use super::*;
use crate::parse::BindKind;

/// The placeholder every render path writes for a symbol this run never recorded.
const MISSING: &str = "<label>";

/// A name of this module's own, so the properties pin [`StaticName`] itself rather than whatever
/// spelling a builtin happens to declare.
static SLOT: StaticName<ValueSymbol> = crate::static_name!(ValueSymbol, "slot");

// A group of this module's own, pinning `slots!` the same way.
crate::slots! { GROUP { width, height } }

/// Token text drawn across the whole space the classifiers are defined over: value spellings, Type
/// spellings, keyword words, glyph runs, and free-form text that lands wherever it lands.
fn token_text() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z][a-z0-9_]{0,6}",
        "[A-Z][a-z][A-Za-z0-9]{0,5}",
        "[A-Z]{1,6}",
        "[-+*/<>=~^!?|]{1,3}",
        "[A-Za-z0-9_ +*]{0,8}",
    ]
}

/// Keyword-class text only, with no space in it, so a run's rendering splits back into its members.
fn keyword_text() -> impl Strategy<Value = String> {
    prop_oneof!["[A-Z]{2,5}", "[-+*/<>=~^!?|]{1,3}"]
}

/// The distinct entries of `texts`, in first-seen order.
fn distinct(texts: &[String]) -> Vec<&String> {
    let mut seen = HashSet::new();
    texts.iter().filter(|text| seen.insert(*text)).collect()
}

proptest! {
    /// A symbol is a pure function of its text, injective on distinct spellings, and totally
    /// ordered — the three facts every digest-keyed table in the tree rests on.
    #[test]
    fn a_symbol_is_pure_injective_and_totally_ordered(
        texts in prop::collection::vec(token_text(), 1..8),
    ) {
        for text in &texts {
            prop_assert_eq!(Symbol::of(text), Symbol::of(text));
        }
        for (index, left) in texts.iter().enumerate() {
            for right in &texts[index + 1..] {
                prop_assert_eq!(left == right, Symbol::of(left) == Symbol::of(right));
            }
        }
        let mut symbols: Vec<Symbol> = distinct(&texts).into_iter().map(|t| Symbol::of(t)).collect();
        symbols.sort();
        for pair in symbols.windows(2) {
            prop_assert!(pair[0] < pair[1]);
        }
    }

    /// Interning round-trips through `resolve`, records one entry per distinct spelling however
    /// often it is offered, and leaves a symbol nothing recorded a miss. `declared` and `record`
    /// are the same write door: each hands back the symbol its text was keyed under.
    #[test]
    fn interning_round_trips_and_records_each_spelling_once(
        texts in prop::collection::vec(token_text(), 1..8),
        absent in token_text(),
    ) {
        let interner = LabelInterner::new();
        for text in &texts {
            prop_assert_eq!(interner.intern(text), Symbol::of(text));
        }
        for text in &texts {
            interner.intern(text);
        }
        let distinct_texts = distinct(&texts);
        prop_assert_eq!(interner.len(), distinct_texts.len());
        for text in &texts {
            let resolved = interner.resolve(Symbol::of(text));
            prop_assert_eq!(resolved.as_deref(), Some(text.as_str()));
        }
        prop_assume!(!texts.contains(&absent));
        prop_assert_eq!(interner.resolve(Symbol::of(&absent)), None);

        // `declared` records under the symbol it returns, for whichever class the text lands in.
        if let Some(name) = TypeSymbol::declared(&absent, &interner) {
            let resolved = interner.resolve(name.symbol());
            prop_assert_eq!(resolved.as_deref(), Some(absent.as_str()));
        }

        // A `StaticName` records under its memo, and a second recording adds nothing. Each field of
        // a `slots!` group carries its own memo, so a group records one entry per slot.
        let before = interner.len();
        for (name, spelling) in [(&SLOT, "slot"), (&GROUP.width, "width"), (&GROUP.height, "height")] {
            let classified = interner.record(name);
            prop_assert_eq!(classified, name.symbol());
            let resolved = interner.resolve(classified.symbol());
            prop_assert_eq!(resolved.as_deref(), Some(spelling));
            interner.record(name);
        }
        prop_assert_eq!(interner.len(), before + 3 - distinct_texts.iter()
            .filter(|text| ["slot", "width", "height"].contains(&text.as_str()))
            .count());
    }

    /// The `Display` view prints what `render` returns on a hit and the placeholder on a miss, and
    /// `compare_texts` is the order of those very renderings — so a sorted diagnostic reads in the
    /// order a reader sees rather than in digest order.
    #[test]
    fn display_is_render_and_compare_texts_is_render_order(
        recorded in prop::collection::vec(token_text(), 1..5),
        missing in prop::collection::vec(token_text(), 1..3),
    ) {
        let interner = LabelInterner::new();
        for text in &recorded {
            interner.intern(text);
        }
        let mut symbols: Vec<Symbol> = recorded.iter().map(|t| Symbol::of(t)).collect();
        symbols.extend(missing.iter().map(|t| Symbol::of(t)));

        for symbol in &symbols {
            prop_assert_eq!(interner.display(*symbol).to_string(), interner.render(*symbol));
        }
        for text in &missing {
            prop_assume!(!recorded.contains(text));
            prop_assert_eq!(interner.render(Symbol::of(text)), MISSING);
        }
        for left in &symbols {
            for right in &symbols {
                prop_assert_eq!(
                    interner.compare_texts(*left, *right),
                    interner.render(*left).cmp(&interner.render(*right)),
                );
            }
        }
    }

    /// The three classes partition token text: every spelling lands in exactly one, a classified
    /// symbol is the bare digest of its own text, and `BinderSymbol` is the union of the two
    /// binding classes tagged with the channel each binds in. A `static_name!` memo is what its
    /// class's `classify` mints.
    #[test]
    fn the_three_classes_partition_token_text(text in token_text()) {
        let value = ValueSymbol::classify(&text);
        let ktype = TypeSymbol::classify(&text);
        let keyword = KeywordSymbol::of(&text);
        let hits = [value.is_some(), ktype.is_some(), keyword.is_some()]
            .into_iter()
            .filter(|hit| *hit)
            .count();
        prop_assert_eq!(hits, 1, "{} classified into {} classes", text, hits);

        let bare = Symbol::of(&text);
        if let Some(name) = value {
            prop_assert_eq!(name.symbol(), bare);
        }
        if let Some(name) = ktype {
            prop_assert_eq!(name.symbol(), bare);
        }
        if let Some(name) = keyword {
            prop_assert_eq!(name.symbol(), bare);
        }

        let binder = BinderSymbol::classify(&text);
        prop_assert_eq!(binder.is_some(), keyword.is_none());
        match binder {
            Some(name) => {
                prop_assert_eq!(name.symbol(), bare);
                prop_assert_eq!(
                    name.bind_kind(),
                    if value.is_some() { BindKind::Value } else { BindKind::Type },
                );
            }
            None => prop_assert!(keyword.is_some()),
        }

        prop_assert_eq!(SLOT.symbol(), ValueSymbol::classify("slot").expect("`slot` is a value token"));
        prop_assert_eq!(SLOT.text(), "slot");
    }

    /// `classify` is the pure funnel — it decides the class and records nothing. `declared` is the
    /// declaration constructor: it interns exactly when the text classifies, so a rejected name
    /// leaves no entry behind for a diagnostic to resolve.
    #[test]
    fn classify_records_nothing_and_declared_interns_iff_it_classifies(text in token_text()) {
        let interner = LabelInterner::new();
        ValueSymbol::classify(&text);
        TypeSymbol::classify(&text);
        KeywordSymbol::of(&text);
        BinderSymbol::classify(&text);
        prop_assert_eq!(interner.len(), 0, "classification records nothing");

        prop_assert_eq!(
            ValueSymbol::declared(&text, &interner).is_some(),
            ValueSymbol::classify(&text).is_some(),
        );
        prop_assert_eq!(
            TypeSymbol::declared(&text, &interner).is_some(),
            TypeSymbol::classify(&text).is_some(),
        );
        prop_assert_eq!(
            KeywordSymbol::declared(&text, &interner).is_some(),
            KeywordSymbol::of(&text).is_some(),
        );
        // Exactly one class accepts, so exactly one of the three recorded.
        prop_assert_eq!(interner.len(), 1);
        let resolved = interner.resolve(Symbol::of(&text));
        prop_assert_eq!(resolved.as_deref(), Some(text.as_str()));
    }

    /// A run digest reads its members as a set: order does not matter, repeats collapse, a
    /// singleton is not its own member, and a strictly larger set mints a different key. This is
    /// what lets a chain's probe hit the key its group's powerset registered. `declared_run` mints
    /// the same digest and records the set's spellings under it.
    #[test]
    fn a_run_digest_is_a_set_digest_disjoint_from_its_members(
        glyphs in prop::collection::vec(keyword_text(), 1..5),
        extra in keyword_text(),
    ) {
        let symbol = |text: &String| KeywordSymbol::of(text).expect("keyword-class by construction");
        let members: Vec<KeywordSymbol> = glyphs.iter().map(symbol).collect();
        let digest = KeywordSymbol::of_run(&members);

        let mut reversed = members.clone();
        reversed.reverse();
        prop_assert_eq!(KeywordSymbol::of_run(&reversed), digest, "order does not matter");

        let mut doubled = members.clone();
        doubled.extend(members.iter().copied());
        prop_assert_eq!(KeywordSymbol::of_run(&doubled), digest, "repeats collapse");

        if let [only] = members[..] {
            prop_assert_ne!(digest, only, "a singleton run is not its member");
        }
        if !glyphs.contains(&extra) {
            let mut grown = members.clone();
            grown.push(symbol(&extra));
            prop_assert_ne!(KeywordSymbol::of_run(&grown), digest, "a larger set is a new key");
        }

        let labels = LabelInterner::new();
        let declared: Vec<KeywordSymbol> = glyphs
            .iter()
            .map(|text| KeywordSymbol::declared(text, &labels).expect("keyword-class by construction"))
            .collect();
        let run = KeywordSymbol::declared_run(&declared, &labels);
        prop_assert_eq!(run, digest);
        let rendering = labels.render(run.symbol());
        prop_assert_eq!(
            rendering.split(' ').collect::<HashSet<_>>(),
            glyphs.iter().map(String::as_str).collect::<HashSet<_>>(),
        );
    }
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
