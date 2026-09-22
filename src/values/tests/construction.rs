//! Each construction door: what it lays down, the type it memoizes and the weight it states.

use std::ptr;

use crate::parse::ExpressionPart;
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KKind, KType, TypeNode};
use crate::values::{Key, KeyRejected, TypeValue, Weight};

use super::{Dict, List, Record, Tagged, TypeSymbol, Value, pin, text, with_fixture};

const WORD: Weight = Weight::flat::<Value<'static, 'static>>();

#[test]
fn a_list_memoizes_the_join_of_its_cells_and_weighs_every_byte_it_lays_down() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let (types, scratch) = (fixture.types, fixture.scratch());
            let strings = [text(writer, "ab"), text(writer, "cde")];
            let list = List::new(writer, strings.into_iter(), types, scratch);
            assert_eq!(list.len(), 2);
            assert_eq!(list.get(1).and_then(Value::as_str), Some("cde"));
            assert_eq!(list.ktype(), types.list(KType::STR));
            assert_eq!(
                list.weight(),
                Weight::flat::<List<'static, 'static>>()
                    .plus(WORD)
                    .plus(WORD)
                    .plus(Weight::text(5))
            );
            let mixed = [Value::Number(1.0), text(writer, "a")];
            let mixed = List::new(writer, mixed.into_iter(), types, scratch);
            assert!(types.is_union(match types.node(mixed.ktype()) {
                TypeNode::List { element } => element,
                _ => panic!("a list memoizes a list type"),
            }));
            let empty = List::new(writer, [].into_iter(), types, scratch);
            assert_eq!(empty.ktype(), types.list(KType::NEVER));
        })
    });
}

#[test]
fn a_record_sorts_its_fields_and_memoizes_the_record_of_their_types() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
            let y = BinderSymbol::declared("y", symbols).unwrap();
            let x = BinderSymbol::declared("x", symbols).unwrap();
            let fields = [(y, text(writer, "b")), (x, Value::Number(1.0))];
            let record = Record::new(writer, &fields, types, scratch);
            assert!(record.names().is_sorted());
            assert!(matches!(record.field(x.symbol()), Some(Value::Number(1.0))));
            assert_eq!(record.field(y.symbol()).and_then(Value::as_str), Some("b"));
            assert_eq!(
                record.ktype(),
                types.record(scratch, &[(x, KType::NUMBER), (y, KType::STR)])
            );
            assert_eq!(
                record.weight(),
                Weight::flat::<Record<'static, 'static>>()
                    .plus(Weight::flat::<crate::symbols::Symbol>())
                    .plus(Weight::flat::<crate::symbols::Symbol>())
                    .plus(WORD)
                    .plus(WORD)
                    .plus(Weight::text(1))
            );
        })
    });
}

#[test]
fn a_dict_keeps_the_last_of_a_repeated_key_and_reads_in_key_order() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let (types, scratch) = (fixture.types, fixture.scratch());
            let entries = [
                (Key::str("b"), Value::Number(1.0)),
                (Key::number(2.0).unwrap(), Value::Number(2.0)),
                (Key::str("a"), Value::Number(3.0)),
                (Key::bool(true), Value::Number(4.0)),
                (Key::str("b"), Value::Number(5.0)),
            ];
            let dict = Dict::new(writer, &entries, types, scratch);
            let keys: Vec<Key<'_>> = dict.entries().map(|(key, _)| *key).collect();
            assert_eq!(
                keys,
                [
                    Key::bool(true),
                    Key::number(2.0).unwrap(),
                    Key::str("a"),
                    Key::str("b")
                ]
            );
            assert!(matches!(dict.get(&Key::str("b")), Some(Value::Number(5.0))));
            assert!(dict.get(&Key::str("c")).is_none());
            assert_eq!(dict.ktype(), {
                let key = crate::type_lattice::join_iter(
                    types,
                    scratch,
                    [KType::BOOL, KType::NUMBER, KType::STR],
                );
                types.dict(key, KType::NUMBER)
            });
        })
    });
}

#[test]
fn a_key_refuses_nan_and_non_scalars_and_folds_the_zeros() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let list = List::new(
                context.writer(),
                [].into_iter(),
                fixture.types,
                fixture.scratch(),
            );
            assert_eq!(Key::of(&Value::Number(f64::NAN)), Err(KeyRejected::NaN));
            assert_eq!(
                Key::of(&Value::List(list)),
                Err(KeyRejected::NotAScalar(list.ktype()))
            );
            assert_eq!(Key::number(-0.0), Ok(Key::number(0.0).unwrap()));
            assert_eq!(Key::number(f64::NAN), Err(KeyRejected::NaN));
            let Value::Number(zero) = Key::of(&Value::Number(-0.0)).unwrap().value() else {
                panic!("a number key is a number");
            };
            assert!(zero.is_sign_positive());
        })
    });
}

#[test]
fn peeling_replaces_one_tagged_layer_and_holding_keeps_every_layer() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let inner = Tagged::hold(writer, Value::Number(1.0), KType::NUMBER);
            let held = Tagged::hold(writer, Value::Tagged(inner), KType::STR);
            assert!(matches!(held.payload(), Value::Tagged(layer) if ptr::eq(*layer, inner)));
            let peeled = Tagged::peel(writer, Value::Tagged(inner), KType::STR);
            assert_eq!(peeled.ktype(), KType::STR);
            assert!(matches!(peeled.payload(), Value::Number(1.0)));
            let bare = Tagged::peel(writer, Value::Bool(true), KType::BOOL);
            assert!(matches!(bare.payload(), Value::Bool(true)));
        })
    });
}

#[test]
fn a_quote_weighs_its_pointer_and_a_type_value_its_kind() {
    with_fixture(|fixture| {
        let ExpressionPart::QuotedExpression(node) = fixture.part("#(a b c)") else {
            panic!("a quote parses to a quote part");
        };
        fixture.in_cell(pin, |context| {
            assert_eq!(Value::Expression(node).weight(), WORD);
            let value = TypeValue::new(context.writer(), KType::NUMBER, fixture.types);
            assert_eq!(value.handle(), KType::NUMBER);
            assert_eq!(value.ktype(), KType::of_kind(KKind::ProperType));
            assert_eq!(
                Value::Type(value).weight(),
                WORD.plus(Weight::flat::<TypeValue>())
            );
        })
    });
}

#[test]
fn retyping_swaps_the_handle_over_the_same_cells() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let (types, scratch) = (fixture.types, fixture.scratch());
            let numbers = [Value::Number(1.0), Value::Number(2.0)];
            let list = List::new(writer, numbers.into_iter(), types, scratch);
            let Value::List(retyped) = Value::List(list).retyped(writer, KType::LIST_OF_ANY, types)
            else {
                panic!("a list retypes to a list");
            };
            assert_eq!(retyped.ktype(), KType::LIST_OF_ANY);
            assert!(ptr::eq(retyped.cells(), list.cells()));
            assert_eq!(retyped.weight(), list.weight());
            // A declared node of another kind leaves the value alone.
            let Value::List(same) = Value::List(list).retyped(writer, KType::ANY, types) else {
                panic!("a list stays a list");
            };
            assert!(ptr::eq(same, list));
        })
    });
}

#[test]
fn a_tagged_value_retyped_against_a_union_takes_the_member_it_inhabits() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let types = fixture.types;
            let union = types.union_of(fixture.scratch(), &[KType::NUMBER, KType::STR]);
            let tagged = Tagged::hold(writer, Value::Number(1.0), KType::STR);
            let Value::Tagged(stamped) = Value::Tagged(tagged).retyped(writer, union, types) else {
                panic!("a tagged value stays tagged");
            };
            assert_eq!(stamped.ktype(), KType::STR);
            let outside = Tagged::hold(writer, Value::Number(1.0), KType::BOOL);
            let Value::Tagged(kept) = Value::Tagged(outside).retyped(writer, union, types) else {
                panic!("a tagged value stays tagged");
            };
            assert!(ptr::eq(kept, outside));
        })
    });
}

#[test]
fn lowering_builds_nested_literals_and_quotes_and_refuses_names() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let nested = fixture.part("[[1 2] [\"a\"]]");
        let dict = fixture.part("{\"k\": #(x), 2: {y = true}}");
        let named = fixture.part("[1 x]");
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let Some(Value::List(outer)) = Value::lower_part(writer, &nested, types, scratch)
            else {
                panic!("a nested list literal lowers");
            };
            assert_eq!(outer.len(), 2);
            let inner = outer.get(1).and_then(Value::as_list).unwrap();
            assert_eq!(inner.get(0).and_then(Value::as_str), Some("a"));
            let Some(Value::Dict(dict)) = Value::lower_part(writer, &dict, types, scratch) else {
                panic!("a dict literal of lowerable entries lowers");
            };
            assert!(
                dict.get(&Key::str("k"))
                    .and_then(Value::as_expression)
                    .is_some()
            );
            assert!(
                dict.get(&Key::number(2.0).unwrap())
                    .and_then(Value::as_record)
                    .is_some()
            );
            assert!(Value::lower_part(writer, &named, types, scratch).is_none());
        })
    });
}

#[test]
fn each_linked_door_stores_its_memo_and_weighs_its_links() {
    use super::{Link, Node};
    use crate::memory::KnotPlan;
    type LinkedList =
        crate::values::List<'static, 'static, Node<'static, 'static>, Link<'static, 'static>>;
    type LinkedTagged =
        crate::values::Tagged<'static, 'static, Node<'static, 'static>, Link<'static, 'static>>;
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
            let edge = Link::Edge(KnotPlan::new(1).edge(0).unwrap());
            let link = Weight::flat::<Link<'static, 'static>>();
            let word = Link::Value(crate::values::text(writer, "abc"));
            let memo = types.list(KType::STR);

            let list = crate::values::List::linked(writer, &[word, edge], memo);
            assert_eq!(list.ktype(), memo);
            assert_eq!(
                list.weight(),
                Weight::flat::<LinkedList>()
                    .plus(link)
                    .plus(link)
                    .plus(Weight::text(3))
            );

            let entries = [(Key::str("k"), edge), (Key::str("k"), word)];
            let dict = crate::values::Dict::linked(writer, &entries, memo, scratch);
            assert_eq!((dict.ktype(), dict.len()), (memo, 1));
            assert!(
                matches!(dict.get(&Key::str("k")), Some(Link::Value(_))),
                "the last key wins"
            );

            let (y, x) = (
                BinderSymbol::declared("y", symbols).unwrap(),
                BinderSymbol::declared("x", symbols).unwrap(),
            );
            let record =
                crate::values::Record::linked(writer, &[(y, edge), (x, word)], memo, scratch);
            assert!(record.names().is_sorted());
            assert!(matches!(record.field(y.symbol()), Some(Link::Edge(_))));

            let tagged = crate::values::Tagged::linked(writer, word, KType::STR);
            assert_eq!(tagged.ktype(), KType::STR);
            assert_eq!(
                tagged.weight(),
                Weight::flat::<LinkedTagged>().plus(Weight::text(3))
            );
        })
    });
}

#[test]
fn a_newtype_construction_is_checked_against_its_representation() {
    use crate::values::{ConstructionRefused, construction};
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let ring = fixture.ring_type("Ring", "next");
        let distance = fixture.newtype("Distance", KType::NUMBER);
        let next = BinderSymbol::declared("next", symbols).unwrap();
        let other = BinderSymbol::declared("other", symbols).unwrap();
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let head = |handle| TypeValue::new(writer, handle, types);
            let inner = Value::Tagged(Tagged::hold(writer, Value::Null, ring));
            let fitting = Value::Record(Record::new(writer, &[(next, inner)], types, scratch));
            let built = Tagged::construct(writer, head(ring), fitting, types, scratch).unwrap();
            assert_eq!(built.ktype(), ring);
            assert_eq!(
                construction(types, scratch, ring, fitting.ktype()),
                Ok(ring)
            );
            let three =
                Tagged::construct(writer, head(distance), Value::Number(3.0), types, scratch);
            assert_eq!(three.map(|tagged| tagged.ktype()), Ok(distance));

            assert_eq!(
                Tagged::construct(writer, head(KType::NUMBER), fitting, types, scratch).err(),
                Some(ConstructionRefused::NotNewType(KType::NUMBER))
            );
            let misfit = Value::Record(Record::new(writer, &[(other, inner)], types, scratch));
            let Err(ConstructionRefused::Misfit { identity, .. }) =
                Tagged::construct(writer, head(ring), misfit, types, scratch)
            else {
                panic!("a payload of another record type misfits");
            };
            assert_eq!(identity, ring);
            assert!(matches!(
                construction(types, scratch, distance, KType::STR),
                Err(ConstructionRefused::Misfit { .. })
            ));
        })
    });
}

#[test]
fn a_member_seals_under_a_mint_its_source_binding_admits() {
    use crate::memory::ScopeId;
    use crate::values::{SealRefused, sealing};
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let distance = fixture.newtype("Distance", KType::NUMBER);
        let carrier = TypeSymbol::declared("Carrier", symbols).unwrap();
        let nonce = ScopeId::next();
        let declared = types.abstract_type(scratch, nonce, carrier, &[], None, KType::ANY);
        let mint = types.abstract_type(scratch, nonce, carrier, &[], Some(nonce), KType::ANY);
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let sealed = Tagged::seal(
                writer,
                Value::Number(1.0),
                mint,
                KType::NUMBER,
                types,
                scratch,
            );
            assert_eq!(sealed.map(|tagged| tagged.ktype()), Ok(mint));

            // A tagged payload takes the mint as its one layer, not a second one.
            let tagged = Value::Tagged(Tagged::hold(writer, Value::Number(2.0), distance));
            let sealed = Tagged::seal(writer, tagged, mint, distance, types, scratch).unwrap();
            assert_eq!(sealed.ktype(), mint);
            assert!(matches!(sealed.payload(), Value::Number(2.0)));

            assert_eq!(
                Tagged::seal(
                    writer,
                    Value::Number(1.0),
                    declared,
                    KType::NUMBER,
                    types,
                    scratch
                )
                .err(),
                Some(SealRefused::NotAMint(declared)),
                "a SIG-body declaration is no per-application mint",
            );
            assert_eq!(
                sealing(types, scratch, mint, KType::STR, KType::NUMBER),
                Err(SealRefused::Misfit {
                    mint,
                    witness: KType::STR
                }),
            );
        })
    });
}
