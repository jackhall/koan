//! The store's own contract, which no law states: how much a registry holds, and that a walk reads
//! nodes through references.

use crate::bump::Bump;
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner};

use crate::types::handle::KType;
use crate::types::registry::TypeRegistry;
use crate::types::shape::DispatchTokenElement;

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
