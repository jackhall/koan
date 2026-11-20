//! Each rule's closed function by hand, and what it writes over names at every binding.

use crate::memory::Bump;
use crate::symbols::{SymbolInterner, TypeSymbol};
use crate::type_lattice::{DeclaredType, KType, Step};
use crate::values::tests::TestTypes;

use super::Rule;

fn name(text: &str, symbols: &SymbolInterner) -> TypeSymbol {
    TypeSymbol::declared(text, symbols).expect("a Type token")
}

#[test]
fn a_piece_is_the_projection_door_s_piece_of_its_closed_base() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let piece = |base: KType| {
        let ruled =
            Rule::Piece(Step::Element).closed(&types, &scratch, &[DeclaredType::Type(base)]);
        ruled.map(|outputs| outputs[0])
    };
    let numbers = types.list(KType::NUMBER);
    assert_eq!(piece(numbers), Some(KType::NUMBER));
    // A union of lists answers the union of their elements.
    let strings = types.list(KType::STR);
    let either = types.union_of(&scratch, &[numbers, strings]);
    let pieces = types.union_of(&scratch, &[KType::NUMBER, KType::STR]);
    assert_eq!(piece(either), Some(pieces));
    // A type with no such piece gives none.
    assert_eq!(piece(KType::NUMBER), None);
}

#[test]
fn a_meet_is_the_meet_of_its_closed_operands() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let either = types.union_of(&scratch, &[KType::NUMBER, KType::STR]);
    let operands = [
        DeclaredType::Type(either),
        DeclaredType::Type(KType::NUMBER),
    ];
    let outputs = Rule::Meet
        .closed(&types, &scratch, &operands)
        .expect("a meet");
    assert_eq!(outputs, &[KType::NUMBER]);
}

/// Over names, a piece the bound guarantees is its projection, and one it does not is left to a
/// derived name; a meet over a variable is always left to one.
#[test]
fn a_rule_writes_over_names_only_what_every_binding_gives() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let listed = types.lexical(0, name("Elt", &symbols), types.list(KType::ANY));
    let any = types.lexical(0, name("Elt", &symbols), KType::ANY);
    let element = Rule::Piece(Step::Element);
    let guaranteed = element.spelled(&types, &scratch, &[DeclaredType::Type(listed)]);
    assert!(guaranteed[0].is_some());
    let open = element.spelled(&types, &scratch, &[DeclaredType::Type(any)]);
    assert_eq!(open, &[None]);
    let operands = [
        DeclaredType::Type(any),
        DeclaredType::Type(KType::NUMBER.into()),
    ];
    assert_eq!(Rule::Meet.spelled(&types, &scratch, &operands), &[None]);
}

/// A fill over exact elements is the run's fill of their points, and over inexact ones the ends:
/// the uppers' union, and the lowers' with a record read as `Never`.
#[test]
fn a_fill_is_the_runs_where_every_element_is_exact_and_the_ends_elsewhere() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let fill = |operands: &[KType]| {
        let operands: Vec<_> = operands.iter().map(|ty| DeclaredType::Type(*ty)).collect();
        let filled = (operands.len() / 2) as u32;
        let outputs = (Rule::Fill { filled })
            .closed(&types, &scratch, &operands)
            .expect("a fill always answers");
        (outputs[0], outputs[1])
    };
    let number = KType::NUMBER;
    assert_eq!(fill(&[number, number]), (number, number));
    let record = types.record(&scratch, &[]);
    let either = types.union_of(&scratch, &[number, KType::STR]);
    assert_eq!(fill(&[KType::NEVER, either, record, record]), {
        let upper = types.union_of(&scratch, &[either, record]);
        (KType::NEVER, upper)
    });
}

/// Over names, a fill whose ends hold no record is the plain unions at every binding.
#[test]
fn a_fill_over_names_holding_no_record_is_its_unions() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let numbers = types.lexical(0, name("Elt", &symbols), types.list(KType::NUMBER));
    let operands = [DeclaredType::Type(numbers), DeclaredType::Type(numbers)];
    let spelled = (Rule::Fill { filled: 1 }).spelled(&types, &scratch, &operands);
    assert_eq!(
        spelled,
        &[None, None],
        "a variable may be bound to a record"
    );
    let listed = types.list(KType::NUMBER).into();
    let operands = [DeclaredType::Type(listed), DeclaredType::Type(listed)];
    let spelled = (Rule::Fill { filled: 1 }).spelled(&types, &scratch, &operands);
    assert_eq!(spelled, &[Some(listed), Some(listed)]);
}

/// Over names, a type meets itself and `Any` at itself at every binding.
#[test]
fn a_meet_over_names_with_itself_or_any_is_that_type() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let elt = types.lexical(0, name("Elt", &symbols), KType::ANY);
    let any = KType::ANY.into();
    for operands in [[elt, elt], [elt, any], [any, elt]] {
        let operands = operands.map(DeclaredType::Type);
        assert_eq!(
            Rule::Meet.spelled(&types, &scratch, &operands),
            &[Some(elt)]
        );
    }
}
