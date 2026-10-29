//! The fill's laws over drawn types — order independence, that it only narrows, idempotence,
//! concatenation and an invisible memo — beside cases pinning what it gives, how deep it reaches
//! and what it costs.

use proptest::prelude::*;

use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, TypeNode, is_subtype_of};
use crate::values::{FILL_STEPS, filled, filled_union};

use super::{Fixture, with_fixture};

/// A concrete type as drawn: built over a fixture's load chain by [`Fixture::built`].
#[derive(Clone, Debug)]
enum Desc {
    Number,
    Str,
    Null,
    Record(Vec<(usize, Desc)>),
    List(Box<Desc>),
    Dict(Box<Desc>),
    Union(Vec<Desc>),
}

/// The field names a drawn record picks from: few, so drawn records share and differ by name.
const NAMES: [&str; 3] = ["x", "y", "z"];

fn desc() -> BoxedStrategy<Desc> {
    let leaf = prop_oneof![Just(Desc::Number), Just(Desc::Str), Just(Desc::Null)];
    leaf.prop_recursive(3, 24, 3, |inner| {
        prop_oneof![
            3 => prop::collection::vec((0..NAMES.len(), inner.clone()), 0..3).prop_map(Desc::Record),
            1 => inner.clone().prop_map(|desc| Desc::List(Box::new(desc))),
            1 => inner.clone().prop_map(|desc| Desc::Dict(Box::new(desc))),
            1 => prop::collection::vec(inner, 2..3).prop_map(Desc::Union),
        ]
    })
    .boxed()
}

impl Fixture<'_, '_> {
    fn name(&self, name: &str) -> BinderSymbol {
        BinderSymbol::declared(name, self.symbols).unwrap()
    }

    fn built(&self, desc: &Desc) -> KType {
        let (types, scratch) = (self.types, self.scratch());
        match desc {
            Desc::Number => KType::NUMBER,
            Desc::Str => KType::STR,
            Desc::Null => KType::NULL,
            Desc::Record(fields) => {
                let mut built: Vec<(BinderSymbol, KType)> = Vec::new();
                for (name, field) in fields {
                    let name = self.name(NAMES[*name]);
                    if built.iter().all(|(seen, _)| seen.symbol() != name.symbol()) {
                        built.push((name, self.built(field)));
                    }
                }
                types.record(scratch, &built)
            }
            Desc::List(element) => types.list(self.built(element)),
            Desc::Dict(value) => types.dict(KType::STR, self.built(value)),
            Desc::Union(members) => {
                let members: Vec<KType> = members.iter().map(|desc| self.built(desc)).collect();
                types.union_of(scratch, &members)
            }
        }
    }

    fn record(&self, fields: &[(&str, KType)]) -> KType {
        let fields: Vec<_> = (fields.iter())
            .map(|(name, ktype)| (self.name(name), *ktype))
            .collect();
        self.types.record(self.scratch(), &fields)
    }

    fn union(&self, members: &[KType]) -> KType {
        self.types.union_of(self.scratch(), members)
    }

    fn filled(&self, members: &[KType]) -> Vec<KType> {
        filled(self.types, self.scratch(), members).to_vec()
    }

    fn filled_union(&self, members: &[KType]) -> KType {
        filled_union(self.types, self.scratch(), members.iter().copied())
    }

    /// `ktype`'s members, or `ktype` alone where it is no union.
    fn members(&self, ktype: KType) -> Vec<KType> {
        match self.types.node(ktype) {
            TypeNode::Union { members } => members.iter().collect(),
            _ => vec![ktype],
        }
    }
}

fn members() -> impl Strategy<Value = Vec<Desc>> {
    prop::collection::vec(desc(), 1..5)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Each member's result does not depend on the order the members come in.
    #[test]
    fn the_fill_is_order_independent(
        (descs, order) in members().prop_flat_map(|descs| {
            let order = Just((0..descs.len()).collect::<Vec<_>>()).prop_shuffle();
            (Just(descs), order)
        })
    ) {
        with_fixture(|fixture| {
            let members: Vec<_> = descs.iter().map(|desc| fixture.built(desc)).collect();
            let shuffled: Vec<_> = order.iter().map(|at| members[*at]).collect();
            let (results, reordered) = (fixture.filled(&members), fixture.filled(&shuffled));
            for (at, from) in order.iter().enumerate() {
                assert_eq!(reordered[at], results[*from]);
            }
        });
    }

    /// Each result lies under its member: the fill only adds fields, at every depth.
    #[test]
    fn the_fill_only_narrows(descs in members()) {
        with_fixture(|fixture| {
            let members: Vec<_> = descs.iter().map(|desc| fixture.built(desc)).collect();
            for (member, result) in members.iter().zip(fixture.filled(&members)) {
                assert!(is_subtype_of(fixture.types, fixture.scratch(), result, *member));
            }
        });
    }

    /// Filling a fill's members against each other again changes nothing.
    #[test]
    fn the_fill_is_idempotent(descs in members()) {
        with_fixture(|fixture| {
            let members: Vec<_> = descs.iter().map(|desc| fixture.built(desc)).collect();
            let union = fixture.filled_union(&members);
            assert_eq!(fixture.filled_union(&fixture.members(union)), union);
        });
    }

    /// Filling two fills against each other is filling their members at once: what a container
    /// over two containers' cells is typed by.
    #[test]
    fn the_fill_of_two_fills_is_the_fill_of_their_members(
        (left, right) in (members(), members())
    ) {
        with_fixture(|fixture| {
            let left: Vec<_> = left.iter().map(|desc| fixture.built(desc)).collect();
            let right: Vec<_> = right.iter().map(|desc| fixture.built(desc)).collect();
            let both: Vec<_> = left.iter().chain(&right).copied().collect();
            let halves = [fixture.filled_union(&left), fixture.filled_union(&right)];
            assert_eq!(fixture.filled_union(&halves), fixture.filled_union(&both));
        });
    }

    /// A merge the memo answers gives what a merge over an empty memo does.
    #[test]
    fn the_fill_memo_is_invisible((earlier, descs) in (members(), members())) {
        let fresh = with_fixture(|fixture| {
            let members: Vec<_> = descs.iter().map(|desc| fixture.built(desc)).collect();
            fixture.filled(&members)
        });
        with_fixture(|fixture| {
            let earlier: Vec<_> = earlier.iter().map(|desc| fixture.built(desc)).collect();
            fixture.filled(&earlier);
            let members: Vec<_> = descs.iter().map(|desc| fixture.built(desc)).collect();
            fixture.filled(&members);
            assert_eq!(fixture.filled(&members), fresh);
        });
    }
}

#[test]
fn a_record_lacking_a_field_another_names_gains_it_at_null() {
    with_fixture(|fixture| {
        let n = KType::NUMBER;
        let (wide, narrow) = (
            fixture.record(&[("x", n), ("y", n)]),
            fixture.record(&[("x", n)]),
        );
        let filled = fixture.record(&[("x", n), ("y", KType::NULL)]);
        assert_eq!(fixture.filled(&[wide, narrow]), [wide, filled]);
        // Nested, through a field.
        let (outer_wide, outer_narrow) = (
            fixture.record(&[("p", wide)]),
            fixture.record(&[("p", narrow)]),
        );
        assert_eq!(
            fixture.filled(&[outer_wide, outer_narrow]),
            [outer_wide, fixture.record(&[("p", filled)])]
        );
        // Through a list's element and a dict's value.
        let types = fixture.types;
        assert_eq!(
            fixture.filled(&[types.list(wide), types.list(narrow)]),
            [types.list(wide), types.list(filled)]
        );
        let dict = |value| types.dict(KType::STR, value);
        assert_eq!(
            fixture.filled(&[dict(wide), dict(narrow)]),
            [dict(wide), dict(filled)]
        );
        // A union's members at the top fill against each other; a scalar beside them is untouched.
        let other = fixture.record(&[("z", n)]);
        let [union, scalar] = fixture.filled(&[fixture.union(&[narrow, other]), n])[..] else {
            unreachable!()
        };
        let crossed = fixture.union(&[
            fixture.record(&[("x", n), ("z", KType::NULL)]),
            fixture.record(&[("x", KType::NULL), ("z", n)]),
        ]);
        assert_eq!((union, scalar), (crossed, n));
        // An empty record beside one naming `x`.
        let empty = fixture.record(&[]);
        assert_eq!(
            fixture.filled(&[empty, narrow])[0],
            fixture.record(&[("x", KType::NULL)])
        );
    });
}

#[test]
fn a_type_reaching_two_positions_gets_each_positions_result() {
    with_fixture(|fixture| {
        let n = KType::NUMBER;
        let narrow = fixture.record(&[("x", n)]);
        let wide = fixture.record(&[("x", n), ("y", n)]);
        // `narrow` sits at the field `p` beside `wide`, and at `q` alone.
        let left = fixture.record(&[("p", narrow), ("q", narrow)]);
        let right = fixture.record(&[("p", wide), ("q", narrow)]);
        let filled = fixture.record(&[("x", n), ("y", KType::NULL)]);
        assert_eq!(
            fixture.filled(&[left, right]),
            [fixture.record(&[("p", filled), ("q", narrow)]), right]
        );
    });
}

#[test]
fn the_fill_reaches_as_deep_as_a_type_nests() {
    with_fixture(|fixture| {
        let types = fixture.types;
        let n = KType::NUMBER;
        let (mut left, mut right) = (fixture.record(&[("x", n)]), fixture.record(&[("y", n)]));
        for _ in 0..10_000 {
            (left, right) = (types.list(left), types.list(right));
        }
        let results = fixture.filled(&[left, right]);
        assert_ne!(results, [left, right]);
    });
}

/// How many groups a loop nesting `acc = [{x = 1}, acc]` `depth` deep merges, over a fresh memo.
fn nesting_steps(depth: usize) -> usize {
    with_fixture(|fixture| {
        let types = fixture.types;
        let element = fixture.record(&[("x", KType::NUMBER)]);
        FILL_STEPS.with(|steps| steps.set(0));
        let mut acc = types.list(KType::NEVER);
        for _ in 0..depth {
            acc = types.list(fixture.filled_union(&[element, acc]));
        }
        FILL_STEPS.with(|steps| steps.get())
    })
}

#[test]
fn a_nesting_loop_merges_in_constant_steps_each_hop() {
    let (short, long) = (nesting_steps(100), nesting_steps(200));
    assert!(
        long <= 2 * short.max(1),
        "{short} steps for 100, {long} for 200"
    );
    with_fixture(|fixture| {
        // The same merge again is a memo hit.
        let pair = [
            fixture.record(&[("x", KType::NUMBER)]),
            fixture.record(&[("y", KType::NUMBER)]),
        ];
        fixture.filled(&pair);
        FILL_STEPS.with(|steps| steps.set(0));
        fixture.filled(&pair);
        assert_eq!(FILL_STEPS.with(|steps| steps.get()), 0);
    });
}
