//! The type rules: the law every native's [rule](super::super::rules) obeys, checked for every
//! builtin over drawn argument intervals and drawn names and code; and what `FROM`'s and `ATTR`'s
//! rules make the load type and refuse, and the run retype.
//!
//! A lower end the load computes is a type some value carries, or `Never`: never a union and never
//! `Any`. The draws keep to such lower ends. `ATTR`'s need turns on whether its argument's lower
//! end is a record, which is monotone only over them.

use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestCaseError, TestRunner};

use crate::memory::Bump;
use crate::program::Program;
use crate::scope::BuiltinIndex;
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    Interval, KType, TypeNode, TypeRegistry, display_name, is_subtype_of, shape_return, shape_slots,
};
use crate::values::record_type;

use super::super::builtins::Native;
use super::super::rules::{Given, typed};
use super::ascription::{WHICH, ends};
use super::run;
use super::statics::{body, loaded};

/// The field names a drawn record or name list picks from.
const NAMES: [&str; 3] = ["x", "y", "z"];

/// A type, drawn as a plain description and interned inside the test.
#[derive(Clone, Debug)]
enum Desc {
    Never,
    Number,
    Str,
    Null,
    Any,
    /// The slot's declared type, so a draw can lie under a slot no plain type lies under.
    Declared,
    List(Box<Desc>),
    Record(Vec<(u8, Desc)>),
    Union(Box<Desc>, Box<Desc>),
}

fn desc() -> BoxedStrategy<Desc> {
    let leaf = prop_oneof![
        Just(Desc::Never),
        Just(Desc::Number),
        Just(Desc::Str),
        Just(Desc::Null),
        Just(Desc::Any),
        Just(Desc::Declared),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        prop_oneof![
            1 => inner.clone().prop_map(|each| Desc::List(Box::new(each))),
            2 => prop::collection::vec((0..3u8, inner.clone()), 0..3).prop_map(Desc::Record),
            1 => (inner.clone(), inner).prop_map(|(a, b)| Desc::Union(Box::new(a), Box::new(b))),
        ]
    })
    .boxed()
}

/// Four types, each above the one before, built alike so they often differ only deep inside: a
/// scalar climbing through `Never`, itself, a union over it and `Any`; a list of a chain; or a
/// record of chained fields, its lower positions holding an extra field or split into a union of
/// two records each holding one. Some positions from the bottom may be `Never` and some from the
/// top `Any`.
fn chain() -> BoxedStrategy<[Desc; 4]> {
    let scalar = prop_oneof![Just(Desc::Number), Just(Desc::Str), Just(Desc::Null)];
    let leaf = (scalar.clone(), scalar, levels()).prop_map(|(own, other, levels)| {
        levels.map(|level| match level {
            0 => Desc::Never,
            1 => own.clone(),
            2 => Desc::Union(Box::new(own.clone()), Box::new(other.clone())),
            _ => Desc::Any,
        })
    });
    leaf.prop_recursive(3, 12, 3, |inner| {
        let list = inner
            .clone()
            .prop_map(|chain| chain.map(|each| Desc::List(Box::new(each))));
        let record = (
            prop::collection::vec((0..2u8, inner), 1..3),
            (0..3u8, desc()),
            prop::option::weighted(0.7, (0..3u8, desc())),
            prop_oneof![0..4usize, Just(3)],
        )
            .prop_map(|(fields, extra, split, below)| {
                std::array::from_fn(|position| {
                    let record: Vec<(u8, Desc)> = (fields.iter())
                        .map(|(name, chain)| (*name, chain[position].clone()))
                        .collect();
                    let with = |field: &(u8, Desc)| {
                        let mut record = record.clone();
                        record.push(field.clone());
                        Desc::Record(record)
                    };
                    match &split {
                        _ if position >= below => Desc::Record(record.clone()),
                        None => with(&extra),
                        Some(split) => Desc::Union(Box::new(with(&extra)), Box::new(with(split))),
                    }
                })
            });
        let clipped = (
            prop_oneof![list.clone(), record.clone()],
            0..2usize,
            3..5usize,
        )
            .prop_map(|(chain, never, any): ([Desc; 4], usize, usize)| {
                let mut chain = chain;
                chain[..never].fill(Desc::Never);
                chain[any..].fill(Desc::Any);
                chain
            });
        prop_oneof![1 => list, 3 => record, 1 => clipped]
    })
    .boxed()
}

/// Four levels in `0..4`, each at least the one before.
fn levels() -> impl Strategy<Value = [u8; 4]> {
    [0..4u8, 0..4u8, 0..4u8, 0..4u8].prop_map(|mut levels| {
        levels.sort_unstable();
        levels
    })
}

/// One slot as drawn: pinned at most its declared type in both intervals, or a chain `J.lower ≤
/// I.lower ≤ I.upper ≤ J.upper`, a lower end the load never computes read as `Never`; and the
/// names and code it holds.
#[derive(Clone, Debug)]
struct Slot {
    pinned: bool,
    chain: [Desc; 4],
    names: Vec<u8>,
    code: Desc,
}

fn slot() -> impl Strategy<Value = Slot> {
    (
        prop_oneof![2 => Just(false), 1 => Just(true)],
        chain(),
        prop_oneof![
            2 => prop::collection::vec(0..2u8, 1..2),
            1 => prop::collection::vec(0..3u8, 1..4),
        ],
        desc(),
    )
        .prop_map(|(pinned, chain, names, code)| Slot {
            pinned,
            chain,
            names,
            code,
        })
}

/// What the test interns into.
struct World<'x, 'graph> {
    program: &'x Program<'graph>,
    types: &'graph TypeRegistry<'graph>,
    scratch: &'x Bump,
}

impl World<'_, '_> {
    fn name(&self, index: u8) -> BinderSymbol {
        BinderSymbol::declared(NAMES[index as usize], self.program.symbols()).expect("a name")
    }

    /// `desc` interned, where the slot it is drawn for declares `declared`.
    fn intern(&self, desc: &Desc, declared: KType) -> KType {
        let intern = |desc| self.intern(desc, declared);
        match desc {
            Desc::Never => KType::NEVER,
            Desc::Number => KType::NUMBER,
            Desc::Str => KType::STR,
            Desc::Null => KType::NULL,
            Desc::Any => KType::ANY,
            Desc::Declared => declared,
            Desc::List(element) => self.types.list(intern(element)),
            Desc::Record(fields) => {
                let mut distinct: Vec<(BinderSymbol, KType)> = Vec::new();
                for (index, field) in fields {
                    let name = self.name(*index);
                    if !distinct.iter().any(|(named, _)| *named == name) {
                        distinct.push((name, intern(field)));
                    }
                }
                record_type(self.types, self.scratch, distinct.into_iter())
            }
            Desc::Union(a, b) => self.types.union_of(self.scratch, &[intern(a), intern(b)]),
        }
    }

    fn below(&self, a: KType, b: KType) -> bool {
        is_subtype_of(self.types, self.scratch, a, b)
    }

    /// Whether `x` lies within `y`: its upper end `Never`, or both ends inside `y`'s.
    fn within(&self, x: Interval, y: Interval) -> bool {
        x.upper == KType::NEVER || (self.below(y.lower, x.lower) && self.below(x.upper, y.upper))
    }

    /// `lower` where the load can compute it as a lower end — no union and not `Any` — else
    /// `Never`.
    fn carried(&self, lower: KType) -> KType {
        match self.types.node(lower) {
            TypeNode::Union { .. } | TypeNode::Any => KType::NEVER,
            _ => lower,
        }
    }

    fn render(&self, interval: Interval) -> String {
        let (types, symbols) = (self.types, self.program.symbols());
        format!(
            "[{}, {}]",
            display_name(interval.lower, types, symbols),
            display_name(interval.upper, types, symbols)
        )
    }
}

/// One slot's interned intervals `I` within `J`, and what it holds.
struct Drawn<'x> {
    inner: Interval,
    outer: Interval,
    names: &'x [BinderSymbol],
    code: KType,
}

impl<'x> Drawn<'x> {
    /// What the slot gives a rule at `typed`: the names and code it holds where `named` and
    /// `traced`, and none where not.
    fn given(&self, typed: Interval, named: bool, traced: bool) -> Given<'x> {
        Given {
            typed,
            names: named.then_some(self.names),
            code: traced.then_some(self.code),
        }
    }
}

#[test]
fn every_rule_obeys_the_law() {
    loaded("LET n = 1", |program| {
        let builtins = program.builtins();
        for index in 0..builtins.len() {
            let value = builtins.get(BuiltinIndex(index as u32));
            let Some(builtin) = value.as_callable().and_then(|member| member.builtin()) else {
                continue;
            };
            let (native, declared) = (Native::of(builtin.id()), builtin.ktype());
            let config = ProptestConfig {
                cases: crate::tests::case_share(1, 2),
                ..ProptestConfig::default()
            };
            let slots =
                prop::collection::vec(slot(), shape_slots(declared, program.types()).count());
            TestRunner::new(config)
                .run(&slots, |slots| law(program, native, declared, &slots))
                .unwrap_or_else(|failure| panic!("{native:?}: {failure}"));
        }
    });
}

/// The law over the builtin `native` of the shape `declared`, its slots drawn as `slots`.
fn law(
    program: &Program<'_>,
    native: Native,
    declared: KType,
    slots: &[Slot],
) -> Result<(), TestCaseError> {
    let scratch = Bump::new();
    let world = World {
        program,
        types: program.types(),
        scratch: &scratch,
    };
    let declared_slots: Vec<KType> = shape_slots(declared, world.types).collect();
    let drawn = draw(&world, slots, &declared_slots);
    let over = |pick: &dyn Fn(usize, &Drawn<'_>) -> Interval, named, traced| {
        let given: Vec<Given<'_>> = (drawn.iter().enumerate())
            .map(|(index, slot)| slot.given(pick(index, slot), named, traced))
            .collect();
        typed(native, declared, &given, world.types, &scratch).returns
    };

    for named in [true, false] {
        let inner = over(&|_, slot| slot.inner, named, true);
        let outer = over(&|_, slot| slot.outer, named, true);
        prop_assert!(
            world.within(inner, outer),
            "over I, {} lies outside {} over J",
            world.render(inner),
            world.render(outer)
        );
    }

    let inner = over(&|_, slot| slot.inner, true, true);
    let unnamed = over(&|_, slot| slot.inner, false, true);
    prop_assert!(
        world.within(inner, unnamed),
        "{} over names lies outside {} over none",
        world.render(inner),
        world.render(unnamed)
    );
    let untraced = over(&|_, slot| slot.inner, true, false);
    prop_assert!(
        world.within(inner, untraced),
        "{} over code lies outside {} over none",
        world.render(inner),
        world.render(untraced)
    );

    let returns = over(
        &|index, _| Interval::within(declared_slots[index]),
        true,
        true,
    );
    let ret = shape_return(declared, world.types).expect("a builtin is a shape");
    prop_assert!(
        world.below(returns.upper, ret),
        "{} over its declared slots lies above its declared {}",
        world.render(returns),
        display_name(ret, world.types, program.symbols())
    );
    Ok(())
}

/// Each slot's intervals `I` within `J` and what it holds, interned where the slots declare
/// `declared`.
fn draw<'x>(world: &World<'x, '_>, slots: &[Slot], declared: &[KType]) -> Vec<Drawn<'x>> {
    let mut drawn = Vec::new();
    for (slot, declared) in slots.iter().zip(declared) {
        let names = &*world
            .scratch
            .alloc_slice_fill_iter(slot.names.iter().map(|index| world.name(*index)));
        let code = world.intern(&slot.code, KType::ANY);
        let (inner, outer) = if slot.pinned {
            (Interval::within(*declared), Interval::within(*declared))
        } else {
            let [lowest, lower, upper, uppest] =
                (slot.chain.each_ref()).map(|each| world.intern(each, *declared));
            // Read as `Never`, `I`'s lower end takes `J`'s along, which must lie under it.
            let (lowest, lower) = match world.carried(lower) {
                KType::NEVER => (KType::NEVER, KType::NEVER),
                lower => (world.carried(lowest), lower),
            };
            (
                Interval { lower, upper },
                Interval {
                    lower: lowest,
                    upper: uppest,
                },
            )
        };
        drawn.push(Drawn {
            inner,
            outer,
            names,
            code,
        });
    }
    drawn
}

/// A record exactly `{x :(Number | Str), y :Str}` whose `x` carries a number.
const RETYPED: &str = "LET r = ({x = 1, y = \"a\"} :! :{x :(Number | Str), y :Str})\n";

/// The `KIND` overloads a projection of `RETYPED` dispatches through.
const KIND: &str = "EXPR #(KIND p :{x :Number}) -> Str = #(\"number\")\n\
                    EXPR #(KIND p :{x :(Number | Str)}) -> Str = #(\"number or str\")\n";

#[test]
fn a_projection_carries_its_record_s_carried_type() {
    let source = format!("{RETYPED}{KIND}LET p = (#[x] FROM r)\n");
    loaded(&source, |program| {
        let exact = ":{x :(Number | Str)}".to_string();
        assert_eq!(ends(program, program.shape(), "p"), (exact.clone(), exact));
    });
    assert_eq!(
        run(&format!("{source}PRINT (KIND p)")),
        "number or str",
        "the built record is retyped, though its `x` carries a number"
    );
}

#[test]
fn a_projection_over_names_the_load_cannot_read_is_retyped_at_run() {
    let source = format!("{RETYPED}{KIND}LET names = #[x]\nLET p = (names FROM r)\n");
    loaded(&source, |program| {
        assert_eq!(
            ends(program, program.shape(), "p"),
            ("Never".to_string(), ":{}".to_string())
        );
    });
    assert_eq!(run(&format!("{source}PRINT (KIND p)")), "number or str");
}

#[test]
fn a_projection_naming_a_field_no_record_holds_refuses_the_load() {
    assert_eq!(
        run("EXPR #(GET r :{a :Number}) -> Any = #(#[b] FROM r)"),
        "load: <test>:1:38: :{a :Number} has no field b"
    );
}

#[test]
fn a_field_of_an_exact_record_is_its_type_there() {
    let source = format!("{WHICH}LET r = ({{a = [1]}} :! :{{a :(LIST OF Any)}})\nLET v = r.a\n");
    loaded(&source, |program| {
        let exact = ":(LIST OF Any)".to_string();
        assert_eq!(ends(program, program.shape(), "v"), (exact.clone(), exact));
    });
    assert_eq!(
        run(&format!(
            "{source}EXPR #(HIDE x :Any) -> Any = #(x)\n\
             PRINT (WHICH r.a)\n\
             PRINT (WHICH (ATTR (HIDE r) a))"
        )),
        "any\nany",
        "the field's value is retyped, whether or not the load knows the record"
    );
}

#[test]
fn a_union_field_is_at_most_its_type() {
    let source = "LET r = ({a = 1} :! :{a :(Number | Str)})\n\
                  LET v = r.a\n\
                  LET read = FN EXPR #(READ s :(:{x :Number} | :{x :Str})) -> Any = #(\n  \
                  LET x = s.x\n  \
                  x\n\
                  )\n";
    loaded(source, |program| {
        let at_most = ("Never".to_string(), ":(Number | Str)".to_string());
        assert_eq!(
            ends(program, program.shape(), "v"),
            at_most,
            "a union keeps its variant"
        );
        let read = body(program, program.shape(), "read");
        assert_eq!(
            ends(program, read, "x"),
            at_most,
            "read over each record's field"
        );
    });
}

#[test]
fn a_field_no_record_holds_refuses_the_load() {
    assert_eq!(
        run("EXPR #(GET r :{a :Number}) -> Any = #(r.b)"),
        "load: <test>:1:38: :{a :Number} has no field b"
    );
}
