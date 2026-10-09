//! The store's own contract, which no law states: how much a registry holds, that a walk reads
//! nodes through references, that a node's identity is its digest, and that a seal lays each
//! component's members before the types over them.

use crate::bump::Bump;
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner, TypeSymbol};

use crate::types::handle::{KType, Parametric, TypeHandle};
use crate::types::kind::KKind;
use crate::types::node::{NodeSchema, TypeNode};
use crate::types::registry::TypeRegistry;
use crate::types::shape::DispatchTokenElement;
use crate::types::typed::{fits, is_subtype_of, join, substitute_quantified};
use crate::types::window::{RecursiveGroupWindow, RelativeSchema};

/// A registry of 16k mixed types — lists, dicts, records, functions, unions and shapes over a
/// growing pool — holds under 5 MB of bump, dead tables included.
#[test]
fn sixteen_thousand_types_fit_in_five_megabytes() {
    let symbols = SymbolInterner::new();
    let names: Vec<BinderSymbol> = ["a", "b", "c", "d", "e", "f", "g", "h"]
        .iter()
        .map(|name| BinderSymbol::declared(name, &symbols).expect("a bindable token"))
        .collect();
    let keys: Vec<KeywordSymbol> = ["ONE", "TWO", "THREE", "FOUR"]
        .iter()
        .map(|key| KeywordSymbol::declared(key, &symbols).expect("a keyword token"))
        .collect();
    let (region, scratch) = (Bump::new(), Bump::new());
    let types = TypeRegistry::in_region(&region);
    let mut pool = vec![KType::NUMBER, KType::STR, KType::BOOL, KType::NULL];
    let mut step = 0usize;
    while types.node_count() < 16_384 {
        let n = pool.len();
        let pick = |k: usize| pool[(step * 7 + k * 13) % n];
        // Unions draw from the first 64 entries, so none grows past a handful of members.
        let early = |k: usize| pool[(step * 5 + k * 11) % n.min(64)];
        let kt = match step % 6 {
            0 => types.list(pick(0)),
            1 => types.dict(pick(0), pick(1)),
            2 => types.record(
                &scratch,
                &[(names[step % 8], pick(0)), (names[(step + 3) % 8], pick(1))],
            ),
            3 => types.function_type(&scratch, &[(names[step % 8], pick(0))], pick(1)),
            4 => types.union_of(&scratch, &[early(0), early(1)]),
            _ => types.shape_type(
                &[
                    DispatchTokenElement::Keyword(keys[step % 4]),
                    DispatchTokenElement::Slot(pick(0)),
                    DispatchTokenElement::Keyword(keys[(step + 1) % 4]),
                    DispatchTokenElement::Slot(pick(1)),
                ],
                &[],
                pick(2),
            ),
        };
        pool.push(kt);
        step += 1;
    }
    let bytes = region.allocated_bytes();
    println!("{} types hold {bytes} B", types.node_count());
    assert!(
        bytes < 5_000_000,
        "{} types hold {bytes} B",
        types.node_count()
    );
}

/// Walks and relations follow references: each looks up the handles it is handed and nothing
/// else, however deep the types.
#[test]
fn walks_and_relations_look_up_only_their_operands() {
    let symbols = SymbolInterner::new();
    let [x, f, y] = ["x", "f", "y"]
        .map(|name| BinderSymbol::declared(name, &symbols).expect("a bindable token"));
    let (region, scratch) = (Bump::new(), Bump::new());
    let types = TypeRegistry::in_region(&region);
    // Twelve lists over a record holding a function: `narrow` lies under `wide` field by field.
    let deep = |inner: KType| (0..12).fold(inner, |kt, _| types.list(kt));
    let record = |x_type: KType, ret: KType| {
        let function = types.function_type(&scratch, &[(y, KType::STR)], ret);
        types.record(&scratch, &[(x, x_type), (f, function)])
    };
    let number_or_str = types.union_of(&scratch, &[KType::NUMBER, KType::STR]);
    let number_or_null = types.union_of(&scratch, &[KType::NUMBER, KType::NULL]);
    let narrow = deep(record(KType::NUMBER, KType::NUMBER));
    let wide = deep(record(number_or_str, number_or_null));

    let before = types.lookups();
    assert!(is_subtype_of(&types, &scratch, narrow, wide));
    assert_eq!(
        types.lookups() - before,
        2,
        "the order looks up its two operands"
    );

    let before = types.lookups();
    assert!(fits(&types, &scratch, narrow, wide));
    assert_eq!(
        types.lookups() - before,
        2,
        "fits looks up its two operands"
    );

    let before = types.lookups();
    assert_eq!(join(&types, &scratch, narrow, wide), wide);
    assert_eq!(
        types.lookups() - before,
        2,
        "join looks up its two operands"
    );

    // A rebuild: twelve lists over a quantified position, substituted once.
    let variable: Parametric = types.quantified(0, KType::ANY);
    let deep_variable = (0..12).fold(variable, |kt, _| types.list(kt));
    let before = types.lookups();
    let solved = substitute_quantified(&types, &scratch, deep_variable, &[KType::NUMBER]);
    assert_eq!(types.lookups() - before, 2, "the root and its one binding");
    assert_eq!(solved, Parametric::from(deep(KType::NUMBER)));
}

/// Two registries interning one content hold two nodes that compare equal: a node's identity is
/// its digest, never its address.
#[test]
fn nodes_compare_by_digest_across_registries() {
    let symbols = SymbolInterner::new();
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let (first_region, second_region, scratch) = (Bump::new(), Bump::new(), Bump::new());
    let (first, second) = (
        TypeRegistry::in_region(&first_region),
        TypeRegistry::in_region(&second_region),
    );
    let record = |types: &TypeRegistry<'_>| types.record(&scratch, &[(x, KType::NUMBER)]);
    let (a, b) = (record(&first), record(&second));
    assert_eq!(a, b, "one content is one handle in every registry");
    let (a, b) = (first.enter(a.raw()), second.enter(b.raw()));
    assert!(!std::ptr::eq(a, b), "each registry lays its own node");
    assert_eq!(a, b);
}

/// A seal over two components joined by an edge — `Wrapper` over `List<Other>`, `Other` over
/// itself — lays `Other` before the type naming it, and each member keys the handle its part
/// sealed alone keys.
#[test]
fn a_seal_lays_each_component_before_the_types_over_it() {
    let symbols = SymbolInterner::new();
    let [wrapper, other] = ["Wrapper", "Other"]
        .map(|name| TypeSymbol::declared(name, &symbols).expect("a Type token"));
    let (host, region, scratch) = (Bump::new(), Bump::new(), Bump::new());
    let types = TypeRegistry::in_region(&region);
    let window =
        RecursiveGroupWindow::new(&host, &[(wrapper, KKind::NewType), (other, KKind::NewType)]);
    let list_of_other = types.list(window.sibling(other, KKind::NewType, &types));
    let schema = RelativeSchema::NewType(list_of_other);
    assert!(window.fill_member(0, schema, &types, &scratch).is_none());
    let sealed = window
        .fill_member(1, schema, &types, &scratch)
        .expect("the last fill seals");
    let (sealed_wrapper, sealed_other) = (
        sealed.member(0).expect("every member seals"),
        sealed.member(1).expect("every member seals"),
    );

    // `Other` alone reads itself as its own first sibling, and `Wrapper` alone the sealed `Other`.
    let self_list = RelativeSchema::NewType(types.list(types.sibling(0)));
    let alone_other =
        RecursiveGroupWindow::seal_singleton(&host, other, self_list, &types, &scratch);
    assert_eq!(alone_other, sealed_other);
    let over_other = RelativeSchema::NewType(types.list(sealed_other));
    let alone_wrapper =
        RecursiveGroupWindow::seal_singleton(&host, wrapper, over_other, &types, &scratch);
    assert_eq!(alone_wrapper, sealed_wrapper);

    // Each member's schema names the sealed members, cyclic edge included.
    for member in [sealed_wrapper, sealed_other] {
        let TypeNode::SetMember {
            schema: NodeSchema::NewType(representation),
            ..
        } = types.node(member)
        else {
            unreachable!("a sealed newtype is a member with a newtype's schema")
        };
        assert_eq!(representation, types.list(sealed_other));
    }
}
