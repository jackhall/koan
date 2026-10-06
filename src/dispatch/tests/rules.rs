//! The type rules: the law every native's [rule](super::super::rules) obeys, checked for every
//! builtin over drawn argument intervals and drawn names; and what `FROM`'s and `ATTR`'s
//! rules make the load type and refuse, and the run carry.
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
    Interval, KType, Parametric, SchemaDraft, TypeNode, TypeRegistry, display_name, is_subtype_of,
    shape_return, shape_slots,
};
use crate::values::record_type;

use super::super::builtins::Native;
use super::super::rules::{Given, typed};
use super::ascription::{WHICH, ends};
use super::generate::{Desc, NAMES, chain};
use super::run;
use super::statics::{body, loaded};

/// One slot as drawn: pinned at most its declared type in both intervals, or a chain `J.lower ≤
/// I.lower ≤ I.upper ≤ J.upper`, a lower end the load never computes read as `Never`; and the
/// names it holds. Drawn as modules, each record the chain holds at its top is a module's
/// signature over the record's fields, which orders as the records do.
#[derive(Clone, Debug)]
struct Slot {
    pinned: bool,
    chain: [Desc; 4],
    modules: bool,
    names: Vec<u8>,
}

fn slot() -> impl Strategy<Value = Slot> {
    (
        prop_oneof![2 => Just(false), 1 => Just(true)],
        chain(),
        prop_oneof![3 => Just(false), 1 => Just(true)],
        prop_oneof![
            2 => prop::collection::vec(0..2u8, 1..2),
            1 => prop::collection::vec(0..3u8, 1..4),
        ],
    )
        .prop_map(|(pinned, chain, modules, names)| Slot {
            pinned,
            chain,
            modules,
            names,
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
            Desc::Variable(_) => unreachable!("the rules law draws no variable"),
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

    /// `desc` interned as [`intern`](Self::intern) does, each record at its top — itself, or a
    /// union's member — a module's signature over the record's fields, or `Never` where the record
    /// is.
    fn module(&self, desc: &Desc, declared: KType) -> KType {
        match desc {
            // A record no value can hold is `Never`, and so is its module.
            Desc::Record(_) if self.intern(desc, declared) == KType::NEVER => KType::NEVER,
            Desc::Record(fields) => {
                let mut draft = SchemaDraft::new(self.scratch);
                // A name drawn twice keeps its first field, as a record's does.
                for (index, field) in fields.iter().rev() {
                    let BinderSymbol::Value(name) = self.name(*index) else {
                        unreachable!("a drawn name is a value name")
                    };
                    draft.insert_value_slot(name, self.intern(field, declared));
                }
                self.types.signature(self.scratch, draft)
            }
            Desc::Union(a, b) => self.types.union_of(
                self.scratch,
                &[self.module(a, declared), self.module(b, declared)],
            ),
            desc => self.intern(desc, declared),
        }
    }

    /// The order over two ends. Every draw is concrete, and a rule over concrete intervals answers
    /// concrete ones.
    fn below(&self, a: Parametric, b: Parametric) -> bool {
        let concrete = |end| {
            self.types
                .concrete(end)
                .expect("a rule over concrete intervals answers concrete ends")
        };
        is_subtype_of(self.types, self.scratch, concrete(a), concrete(b))
    }

    /// Whether `x` lies within `y`: its upper end `Never`, or both ends inside `y`'s.
    fn within(&self, x: Interval, y: Interval) -> bool {
        x.upper == KType::NEVER.into()
            || (self.below(y.lower, x.lower) && self.below(x.upper, y.upper))
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
}

impl<'x> Drawn<'x> {
    /// What the slot gives a rule at `typed`: the names it holds where `named`, and none where not.
    fn given(&self, typed: Interval, named: bool) -> Given<'x> {
        Given {
            typed,
            names: named.then_some(self.names),
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
    let over = |pick: &dyn Fn(usize, &Drawn<'_>) -> Interval, named| {
        let given: Vec<Given<'_>> = (drawn.iter().enumerate())
            .map(|(index, slot)| slot.given(pick(index, slot), named))
            .collect();
        typed(native, declared, &given, world.types, &scratch).returns
    };

    for named in [true, false] {
        let inner = over(&|_, slot| slot.inner, named);
        let outer = over(&|_, slot| slot.outer, named);
        prop_assert!(
            world.within(inner, outer),
            "over I, {} lies outside {} over J",
            world.render(inner),
            world.render(outer)
        );
    }

    let inner = over(&|_, slot| slot.inner, true);
    let unnamed = over(&|_, slot| slot.inner, false);
    prop_assert!(
        world.within(inner, unnamed),
        "{} over names lies outside {} over none",
        world.render(inner),
        world.render(unnamed)
    );

    let returns = over(&|index, _| Interval::within(declared_slots[index]), true);
    let ret = shape_return(declared, world.types).expect("a builtin is a shape");
    prop_assert!(
        world.below(returns.upper, ret.into()),
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
        let (inner, outer) = if slot.pinned {
            (Interval::within(*declared), Interval::within(*declared))
        } else {
            let [lowest, lower, upper, uppest] =
                (slot.chain.each_ref()).map(|each| match slot.modules {
                    true => world.module(each, *declared),
                    false => world.intern(each, *declared),
                });
            // Read as `Never`, `I`'s lower end takes `J`'s along, which must lie under it.
            let (lowest, lower) = match world.carried(lower) {
                KType::NEVER => (KType::NEVER, KType::NEVER),
                lower => (world.carried(lowest), lower),
            };
            (
                Interval { lower, upper }.into(),
                Interval {
                    lower: lowest,
                    upper: uppest,
                }
                .into(),
            )
        };
        drawn.push(Drawn {
            inner,
            outer,
            names,
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
