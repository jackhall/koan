//! The programs dispatch's property laws draw: types as plain [`Desc`]s, spelled as koan source
//! and inhabited by drawn values, and whole programs built over them. Every program is valid by
//! construction — a law that sees one refused or faulting outside what it states has found a
//! generator bug, not a finding — save a registration a union in which ties, which the load refuses
//! where it is declared ([`TIED`]). A registration draws no union of two bare variables, which
//! always tie; whether any other two members tie is the lattice's to answer, and a law discards
//! the rare draw that does.
//!
//! [`rules`](super::rules) interns [`Desc`]s and [`chain`]s for the type-rule law;
//! [`narrowing`](super::narrowing) runs [`dispatched`] programs, a keyworded use over drawn
//! registrations, narrowed and unnarrowed; [`spellings`](super::spellings) runs [`spelled_calls`],
//! one registration called by keyword and by name; [`lexical`](super::lexical) runs [`lexical`]
//! programs, a site naming a quantified variable, with and without the contexts drawn around it;
//! [`retypes`](super::retypes) retypes a value drawn under a [`retyped_type`] at each retype site;
//! [`held`](super::held) runs [`held()`] programs, a body reading modules through holders, beside
//! the same body with some of them written in place.

use proptest::prelude::*;

/// What the load's refusal of a registration whose union ties says.
pub(super) const TIED: &str = "tie in one union";

/// Why a law other than the retype law never meets a dict or a nominal type.
pub(super) const RETYPED_ONLY: &str = "only the retype law draws a dict or nominal type";

/// The field names a drawn record or name list picks from.
pub(super) const NAMES: [&str; 3] = ["x", "y", "z"];

/// The names a registration's quantified variables are spelled by, in order.
const VARIABLES: [&str; 2] = ["Elt", "Key"];

/// A type, drawn as a plain description and interned or spelled inside the test.
#[derive(Clone, Debug)]
pub(super) enum Desc {
    Never,
    Number,
    Str,
    Null,
    Any,
    /// The slot's declared type, so a draw can lie under a slot no plain type lies under.
    Declared,
    /// A quantified variable of a registration's group, by its index there.
    Variable(u8),
    List(Box<Desc>),
    Record(Vec<(u8, Desc)>),
    Union(Box<Desc>, Box<Desc>),
    /// A dict keyed by `Str`. This and the nominal types below are drawn by [`retyped_type`] alone,
    /// over the types [`NOMINALS`] declares.
    Dict(Box<Desc>),
    /// The newtype `Boxed`, over `Number`.
    Boxed,
    /// `Wrap`'s application to a type.
    Wrapped(Box<Desc>),
    /// The parameterized union `Option` applied to a type.
    Optional(Box<Desc>),
    /// The union `Maybe`.
    Maybe,
    /// `Maybe`'s variant `Some`.
    MaybeSome,
}

/// The nominal types [`retyped_type`] draws from, declared ahead of a program that names them.
pub(super) const NOMINALS: &str = "NEWTYPE Boxed = Number\n\
                                   NEWTYPE (Type -> Wrap)\n\
                                   UNION Maybe = #{Some: Number, None: Null}\n\
                                   UNION (Elem -> Option) = #{Some: Elem, None: Null}";

pub(super) fn desc() -> BoxedStrategy<Desc> {
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
pub(super) fn chain() -> BoxedStrategy<[Desc; 4]> {
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
pub(super) fn levels() -> impl Strategy<Value = [u8; 4]> {
    [0..4u8, 0..4u8, 0..4u8, 0..4u8].prop_map(|mut levels| {
        levels.sort_unstable();
        levels
    })
}

/// A type over the first `variables` of a group's variables: any `Desc` but `Declared`.
fn typed(variables: u8) -> BoxedStrategy<Desc> {
    let scalar = prop_oneof![
        Just(Desc::Never),
        Just(Desc::Number),
        Just(Desc::Str),
        Just(Desc::Null),
        Just(Desc::Any),
    ];
    let leaf = match variables {
        0 => scalar.boxed(),
        _ => prop_oneof![4 => scalar, 1 => (0..variables).prop_map(Desc::Variable)].boxed(),
    };
    leaf.prop_recursive(3, 12, 3, |inner| {
        prop_oneof![
            1 => inner.clone().prop_map(|each| Desc::List(Box::new(each))),
            2 => prop::collection::vec((0..3u8, inner.clone()), 0..3).prop_map(Desc::Record),
            1 => (inner.clone(), inner).prop_map(|(a, b)| Desc::Union(Box::new(a), Box::new(b))),
        ]
    })
    .boxed()
}

/// A closed type some value lies under, drawn so: no `Never` where it would empty the type.
fn inhabited_type() -> BoxedStrategy<Desc> {
    let leaf = prop_oneof![
        Just(Desc::Number),
        Just(Desc::Str),
        Just(Desc::Null),
        Just(Desc::Any),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        prop_oneof![
            1 => typed(0).prop_map(|each| Desc::List(Box::new(each))),
            2 => prop::collection::vec((0..3u8, inner.clone()), 0..3).prop_map(Desc::Record),
            1 => (inner, typed(0), any::<bool>()).prop_map(|(inhabited, other, first)| {
                let (a, b) = if first { (inhabited, other) } else { (other, inhabited) };
                Desc::Union(Box::new(a), Box::new(b))
            }),
        ]
    })
    .boxed()
}

/// A closed, inhabited type as [`inhabited_type`] draws one, and also a dict, a nominal type or a
/// family's or a parameterized union's application: every kind of type a retype re-stamps a value
/// at.
pub(super) fn retyped_type() -> BoxedStrategy<Desc> {
    let leaf = prop_oneof![
        Just(Desc::Number),
        Just(Desc::Str),
        Just(Desc::Null),
        Just(Desc::Any),
        Just(Desc::Boxed),
        Just(Desc::Maybe),
        Just(Desc::MaybeSome),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        prop_oneof![
            1 => typed(0).prop_map(|each| Desc::List(Box::new(each))),
            1 => inner.clone().prop_map(|each| Desc::List(Box::new(each))),
            2 => prop::collection::vec((0..3u8, inner.clone()), 0..3).prop_map(Desc::Record),
            1 => inner.clone().prop_map(|each| Desc::Dict(Box::new(each))),
            1 => inner.clone().prop_map(|each| Desc::Wrapped(Box::new(each))),
            1 => inner.clone().prop_map(|each| Desc::Optional(Box::new(each))),
            // Records naming different fields in one list, which the fill types against each other.
            1 => (inner.clone(), inner.clone()).prop_map(|(a, b)| {
                let (a, b) = (Desc::Record(vec![(0, a)]), Desc::Record(vec![(1, b)]));
                Desc::List(Box::new(Desc::Union(Box::new(a), Box::new(b))))
            }),
            1 => (inner, typed(0), any::<bool>()).prop_map(|(inhabited, other, first)| {
                let (a, b) = if first { (inhabited, other) } else { (other, inhabited) };
                Desc::Union(Box::new(a), Box::new(b))
            }),
        ]
    })
    .boxed()
}

/// A record's fields as a type keeps them: the first of each name.
fn kept(fields: &[(u8, Desc)]) -> impl Iterator<Item = &(u8, Desc)> {
    let mut seen = [false; NAMES.len()];
    (fields.iter()).filter(move |(name, _)| !std::mem::replace(&mut seen[*name as usize], true))
}

/// `desc` as koan spells it, its variables named by `variables`.
pub(super) fn spelled(desc: &Desc, variables: &[&str]) -> String {
    match desc {
        Desc::Never => "Never".to_string(),
        Desc::Number => "Number".to_string(),
        Desc::Str => "Str".to_string(),
        Desc::Null => "Null".to_string(),
        Desc::Any => "Any".to_string(),
        Desc::Declared => unreachable!("a spelled type declares nothing"),
        Desc::Variable(index) => variables[*index as usize].to_string(),
        Desc::List(element) => format!("(LIST OF {})", spelled(element, variables)),
        Desc::Record(fields) => {
            let fields: Vec<String> = kept(fields)
                .map(|(name, field)| {
                    format!("{} {}", NAMES[*name as usize], ascribed(field, variables))
                })
                .collect();
            format!(":{{{}}}", fields.join(", "))
        }
        Desc::Union(a, b) => format!("({} | {})", spelled(a, variables), spelled(b, variables)),
        Desc::Dict(value) => format!(":(MAP Str -> {})", spelled(value, variables)),
        Desc::Boxed => "Boxed".to_string(),
        Desc::Wrapped(inner) => format!(":({} -> Wrap)", spelled(inner, variables)),
        Desc::Optional(inner) => format!(":({} -> Option)", spelled(inner, variables)),
        Desc::Maybe => "Maybe".to_string(),
        Desc::MaybeSome => "Maybe.Some".to_string(),
    }
}

/// `desc` as written after a name it types: `:Number`, `:{x :Str}`.
pub(super) fn ascribed(desc: &Desc, variables: &[&str]) -> String {
    let spelled = spelled(desc, variables);
    match spelled.starts_with(':') {
        true => spelled,
        false => format!(":{spelled}"),
    }
}

/// Whether some value lies under the closed `desc`.
pub(super) fn inhabited(desc: &Desc) -> bool {
    match desc {
        Desc::Never => false,
        Desc::Number | Desc::Str | Desc::Null | Desc::Any | Desc::List(_) => true,
        Desc::Record(fields) => kept(fields).all(|(_, field)| inhabited(field)),
        Desc::Union(a, b) => inhabited(a) || inhabited(b),
        Desc::Dict(value) | Desc::Wrapped(value) => inhabited(value),
        // `None` lies under every application.
        Desc::Boxed | Desc::Optional(_) | Desc::Maybe | Desc::MaybeSome => true,
        Desc::Declared | Desc::Variable(_) => unreachable!("an inhabited type is closed"),
    }
}

/// A value under the closed, inhabited `desc`, written as koan source.
pub(super) fn value_under(desc: &Desc) -> BoxedStrategy<String> {
    let scalar = || {
        prop_oneof![
            value_under(&Desc::Number),
            value_under(&Desc::Str),
            value_under(&Desc::Null)
        ]
    };
    match desc {
        Desc::Number => prop_oneof![Just("1".to_string()), Just("2".to_string())].boxed(),
        Desc::Str => prop_oneof![Just("\"s\"".to_string()), Just("\"t\"".to_string())].boxed(),
        Desc::Null => Just("null".to_string()).boxed(),
        Desc::Any => prop_oneof![
            3 => scalar(),
            1 => scalar().prop_map(|value| format!("[{value}]")),
            1 => scalar().prop_map(|value| format!("{{x = {value}}}")),
        ]
        .boxed(),
        Desc::List(element) if inhabited(element) => prop_oneof![
            Just("[]".to_string()),
            (value_under(element), value_under(element)).prop_map(|(v, w)| format!("[{v}, {w}]")),
        ]
        .boxed(),
        Desc::List(_) => Just("[]".to_string()).boxed(),
        Desc::Record(fields) => {
            let values: Vec<BoxedStrategy<String>> = kept(fields)
                .map(|(name, field)| {
                    let name = NAMES[*name as usize];
                    value_under(field).prop_map(move |value| format!("{name} = {value}"))
                })
                .map(Strategy::boxed)
                .collect();
            values
                .prop_map(|fields| format!("{{{}}}", fields.join(", ")))
                .boxed()
        }
        Desc::Union(a, b) => match (inhabited(a), inhabited(b)) {
            (true, true) => prop_oneof![value_under(a), value_under(b)].boxed(),
            (true, false) => value_under(a),
            (false, _) => value_under(b),
        },
        // Koan spells no empty dict, so every drawn dict holds an entry.
        Desc::Dict(value) => prop_oneof![
            value_under(value).prop_map(|v| format!("{{\"a\": {v}}}")),
            (value_under(value), value_under(value))
                .prop_map(|(v, w)| format!("{{\"a\": {v}, \"b\": {w}}}")),
        ]
        .boxed(),
        Desc::Boxed => Just("(Boxed 1)".to_string()).boxed(),
        Desc::Wrapped(inner) => value_under(inner)
            .prop_map(|value| format!("(Wrap ({value}))"))
            .boxed(),
        Desc::Optional(inner) if inhabited(inner) => prop_oneof![
            value_under(inner).prop_map(|value| format!("(Option.Some ({value}))")),
            Just("(Option.None null)".to_string()),
        ]
        .boxed(),
        Desc::Optional(_) => Just("(Option.None null)".to_string()).boxed(),
        Desc::Maybe => prop_oneof![
            Just("(Maybe.Some 1)".to_string()),
            Just("(Maybe.None null)".to_string()),
        ]
        .boxed(),
        Desc::MaybeSome => Just("(Maybe.Some 1)".to_string()).boxed(),
        Desc::Never | Desc::Declared | Desc::Variable(_) => {
            unreachable!("a value lies under a closed, inhabited type")
        }
    }
}

/// The narrowing law's program: one keyworded use of `KAA _` (or `KAA _ AND _`) inside a function,
/// over one to four registrations each returning its own index, called once with values under its
/// parameters' types.
///
/// ```koan
/// EXPR #(KAA 2 AND 1)
/// EXPR #(KAA x :<S00> AND y :<S01>) -> Str = #("0")
/// EXPR FOR ALL #[Elt] #(KAA x :<S10> AND y :<S11>) -> Str = #("1")
/// LET g = (FN :{p :<D0>, q :<D1>} -> Any = #(KAA <a0> AND <a1>))
/// PRINT (g {p = <v0>, q = <v1>})
/// ```
#[derive(Clone, Debug)]
pub(super) struct Dispatched {
    /// Whether `KAA _ AND _` is ranked `#(KAA 2 AND 1)`.
    ranked: bool,
    registrations: Vec<Registration>,
    /// Each parameter's closed, inhabited type and the value it is called with.
    parameters: Vec<(Desc, String)>,
    /// Each argument: the parameter of its position where `None`, else a literal.
    arguments: Vec<Option<String>>,
}

/// One registration of the key: how many of [`VARIABLES`] it quantifies, and its slots' types,
/// each variable named by at least one.
#[derive(Clone, Debug)]
struct Registration {
    quantified: u8,
    slots: Vec<Desc>,
}

/// A [`Dispatched`] program as source, and where its keyworded use sits.
pub(super) struct Rendered {
    pub(super) source: String,
    /// The use's `<test>:line:column`, as a diagnostic names it.
    pub(super) use_at: String,
}

pub(super) fn dispatched() -> impl Strategy<Value = Dispatched> {
    (1..3usize).prop_flat_map(|arity| {
        (
            any::<bool>(),
            prop::collection::vec(registration(arity), 1..5),
            parameters(arity),
            arguments(arity),
        )
            .prop_map(
                move |(ranked, registrations, parameters, arguments)| Dispatched {
                    ranked: ranked && arity == 2,
                    registrations,
                    parameters,
                    arguments,
                },
            )
    })
}

/// `arity` parameters, each a closed, inhabited type beside a value under it.
fn parameters(arity: usize) -> impl Strategy<Value = Vec<(Desc, String)>> {
    let parameter = inhabited_type().prop_flat_map(|desc| {
        let value = value_under(&desc);
        (Just(desc), value)
    });
    prop::collection::vec(parameter, arity)
}

/// `arity` arguments, each its position's parameter where `None`, else a drawn literal.
fn arguments(arity: usize) -> impl Strategy<Value = Vec<Option<String>>> {
    let argument = prop_oneof![
        Just(None),
        inhabited_type().prop_flat_map(|desc| value_under(&desc).prop_map(Some)),
    ];
    prop::collection::vec(argument, arity)
}

/// A registration of `arity` slots, closed or quantifying one or two variables.
fn registration(arity: usize) -> BoxedStrategy<Registration> {
    (0..3u8)
        .prop_flat_map(move |quantified| {
            (
                Just(quantified),
                prop::collection::vec(typed(quantified), arity),
                prop::collection::vec((any::<usize>(), any::<usize>()), quantified as usize),
            )
        })
        .prop_map(|(quantified, mut slots, places)| {
            for (variable, (slot, leaf)) in places.into_iter().enumerate() {
                let variable = variable as u8;
                if !slots.iter().any(|each| names(each, variable)) {
                    let count = slots.len();
                    place(&mut slots[slot % count], variable, leaf);
                }
            }
            slots.iter_mut().for_each(untie);
            Registration { quantified, slots }
        })
        .boxed()
}

/// `desc` with every union of two distinct bare variables made a union of the first and a list of
/// the second: every variable is bounded by `Any`, so such members always tie, and the load refuses
/// the registration ([`TIED`]). Each variable is still named.
fn untie(desc: &mut Desc) {
    match desc {
        Desc::Union(a, b) => {
            if let (Desc::Variable(x), Desc::Variable(y)) = (&**a, &**b)
                && x != y
            {
                **b = Desc::List(Box::new(Desc::Variable(*y)));
            }
            untie(a);
            untie(b);
        }
        Desc::List(element) => untie(element),
        Desc::Record(fields) => fields.iter_mut().for_each(|(_, field)| untie(field)),
        Desc::Dict(_)
        | Desc::Boxed
        | Desc::Wrapped(_)
        | Desc::Optional(_)
        | Desc::Maybe
        | Desc::MaybeSome => {
            unreachable!("{RETYPED_ONLY}")
        }
        _ => {}
    }
}

/// Whether `desc`, as spelled, names the variable `variable`.
fn names(desc: &Desc, variable: u8) -> bool {
    match desc {
        Desc::Variable(index) => *index == variable,
        Desc::List(element) => names(element, variable),
        Desc::Record(fields) => kept(fields).any(|(_, field)| names(field, variable)),
        Desc::Union(a, b) => names(a, variable) || names(b, variable),
        Desc::Dict(_)
        | Desc::Boxed
        | Desc::Wrapped(_)
        | Desc::Optional(_)
        | Desc::Maybe
        | Desc::MaybeSome => {
            unreachable!("{RETYPED_ONLY}")
        }
        _ => false,
    }
}

/// Make `slot` name `variable`: replace the `leaf`-th of its spelled leaves that names no variable,
/// or, where every leaf names one, join it with the variable.
fn place(slot: &mut Desc, variable: u8, leaf: usize) {
    fn leaves<'d>(desc: &'d mut Desc, into: &mut Vec<&'d mut Desc>) {
        let leaf = match &*desc {
            Desc::Record(fields) => fields.is_empty(),
            Desc::Variable(_) | Desc::List(_) | Desc::Union(..) => false,
            Desc::Dict(_)
            | Desc::Boxed
            | Desc::Wrapped(_)
            | Desc::Optional(_)
            | Desc::Maybe
            | Desc::MaybeSome => {
                unreachable!("{RETYPED_ONLY}")
            }
            _ => true,
        };
        if leaf {
            return into.push(desc);
        }
        match desc {
            Desc::List(element) => leaves(element, into),
            Desc::Union(a, b) => {
                leaves(a, into);
                leaves(b, into);
            }
            Desc::Record(fields) => {
                let mut seen = [false; NAMES.len()];
                for (name, field) in fields {
                    if !std::mem::replace(&mut seen[*name as usize], true) {
                        leaves(field, into);
                    }
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    leaves(slot, &mut found);
    match found.len() {
        0 => {
            let joined = std::mem::replace(slot, Desc::Never);
            *slot = Desc::Union(Box::new(joined), Box::new(Desc::Variable(variable)));
        }
        count => *found.swap_remove(leaf % count) = Desc::Variable(variable),
    }
}

impl Dispatched {
    pub(super) fn render(&self) -> Rendered {
        let mut lines = Vec::new();
        if self.ranked {
            lines.push("EXPR #(KAA 2 AND 1)".to_string());
        }
        for (index, registration) in self.registrations.iter().enumerate() {
            let variables = &VARIABLES[..registration.quantified as usize];
            let quantified = match variables {
                [] => String::new(),
                _ => format!("FOR ALL #[{}] ", variables.join(" ")),
            };
            let slots = (registration.slots.iter().zip(["x", "y"]))
                .map(|(slot, name)| format!("{name} {}", ascribed(slot, variables)));
            lines.push(format!(
                "EXPR {quantified}#({}) -> Str = #(\"{index}\")",
                keyed(slots)
            ));
        }
        let parameters: Vec<String> = (self.parameters.iter().zip(["p", "q"]))
            .map(|((desc, _), name)| format!("{name} {}", ascribed(desc, &[])))
            .collect();
        let head = format!("LET g = (FN :{{{}}} -> Any = #(", parameters.join(", "));
        let use_at = format!("<test>:{}:{}", lines.len() + 1, head.len());
        let arguments = (self.arguments.iter().zip(["p", "q"]))
            .map(|(argument, name)| argument.clone().unwrap_or(name.to_string()));
        lines.push(format!("{head}{}))", keyed(arguments)));
        let values: Vec<String> = (self.parameters.iter().zip(["p", "q"]))
            .map(|((_, value), name)| format!("{name} = {value}"))
            .collect();
        lines.push(format!("PRINT (g {{{}}})", values.join(", ")));
        Rendered {
            source: lines.join("\n"),
            use_at,
        }
    }
}

/// `KAA <first>`, or `KAA <first> AND <second>`.
fn keyed(parts: impl Iterator<Item = String>) -> String {
    let parts: Vec<String> = parts.collect();
    format!("KAA {}", parts.join(" AND "))
}

#[test]
fn a_dispatched_program_renders_as_source_that_loads() {
    let program = Dispatched {
        ranked: true,
        registrations: vec![
            Registration {
                quantified: 0,
                slots: vec![
                    Desc::Record(vec![(0, Desc::Number), (1, Desc::Never), (0, Desc::Str)]),
                    Desc::Any,
                ],
            },
            Registration {
                quantified: 1,
                slots: vec![
                    Desc::List(Box::new(Desc::Variable(0))),
                    Desc::Union(Box::new(Desc::Null), Box::new(Desc::Variable(0))),
                ],
            },
        ],
        parameters: vec![
            (Desc::List(Box::new(Desc::Number)), "[1, 2]".to_string()),
            (Desc::Str, "\"s\"".to_string()),
        ],
        arguments: vec![None, Some("null".to_string())],
    };
    let rendered = program.render();
    assert_eq!(
        rendered.source,
        "EXPR #(KAA 2 AND 1)\n\
         EXPR #(KAA x :{x :Number, y :Never} AND y :Any) -> Str = #(\"0\")\n\
         EXPR FOR ALL #[Elt] #(KAA x :(LIST OF Elt) AND y :(Null | Elt)) -> Str = #(\"1\")\n\
         LET g = (FN :{p :(LIST OF Number), q :Str} -> Any = #(KAA p AND null))\n\
         PRINT (g {p = [1, 2], q = \"s\"})"
    );
    assert_eq!(rendered.use_at, "<test>:4:54");
    assert_eq!(super::run(&rendered.source), "1");

    let refused = Dispatched {
        ranked: false,
        registrations: vec![Registration {
            quantified: 0,
            slots: vec![Desc::Str],
        }],
        parameters: vec![(Desc::Number, "1".to_string())],
        arguments: vec![None],
    }
    .render();
    assert_eq!(refused.use_at, "<test>:2:36");
    assert!(
        super::run(&refused.source).starts_with(&format!("load: {}: ", refused.use_at)),
        "a refusal names the use where it was rendered"
    );
}

/// The spelling laws' program: one registration of `KAA _` (or `KAA _ AND _`, ranked as one
/// class), bound to `kaa`, returning its parameters and then the variables it quantifies, and
/// called inside a function by keyword or by name, with values under its parameters' types.
///
/// ```koan
/// EXPR #(KAA 1 AND 1)
/// LET kaa = FN EXPR FOR ALL #[Elt] #(KAA u :<S0> AND v :<S1>) -> Any = #([u, v, Elt])
/// LET g = (FN :{p :<D0>, q :<D1>} -> Any = #(kaa {u = <a0>, v = <a1>}))
/// PRINT (g {p = <v0>, q = <v1>})
/// ```
#[derive(Clone, Debug)]
pub(super) struct Spelled {
    registration: Registration,
    /// Each parameter's closed, inhabited type and the value it is called with.
    parameters: Vec<(Desc, String)>,
    /// Each argument: the parameter of its position where `None`, else a literal.
    arguments: Vec<Option<String>>,
}

/// How a [`Spelled`] program calls its registration.
#[derive(Clone, Copy, Debug)]
pub(super) enum Spelling {
    /// `KAA <a0> AND <a1>`.
    Keyworded,
    /// `kaa {u = <a0>, v = <a1>}`.
    ByName,
    /// `kaa {v = <a1>, u = <a0>}`.
    Reversed,
}

pub(super) fn spelled_calls() -> impl Strategy<Value = Spelled> {
    (1..3usize).prop_flat_map(spelled_calls_of)
}

/// A [`Spelled`] program whose registration has `arity` slots.
pub(super) fn spelled_calls_of(arity: usize) -> impl Strategy<Value = Spelled> {
    (registration(arity), parameters(arity), arguments(arity)).prop_map(
        |(registration, parameters, arguments)| Spelled {
            registration,
            parameters,
            arguments,
        },
    )
}

impl Spelled {
    /// How many slots the registration has.
    fn arity(&self) -> usize {
        self.registration.slots.len()
    }

    pub(super) fn render(&self, spelling: Spelling) -> String {
        let mut lines = Vec::new();
        if self.arity() == 2 {
            lines.push("EXPR #(KAA 1 AND 1)".to_string());
        }
        let variables = &VARIABLES[..self.registration.quantified as usize];
        let quantified = match variables {
            [] => String::new(),
            _ => format!("FOR ALL #[{}] ", variables.join(" ")),
        };
        let slots = (self.registration.slots.iter().zip(["u", "v"]))
            .map(|(slot, name)| format!("{name} {}", ascribed(slot, variables)));
        let listed: Vec<&str> = (["u", "v"].into_iter().take(self.arity()))
            .chain(variables.iter().copied())
            .collect();
        lines.push(format!(
            "LET kaa = FN EXPR {quantified}#({}) -> Any = #([{}])",
            keyed(slots),
            listed.join(", ")
        ));
        let parameters: Vec<String> = (self.parameters.iter().zip(["p", "q"]))
            .map(|((desc, _), name)| format!("{name} {}", ascribed(desc, &[])))
            .collect();
        let arguments: Vec<String> = (self.arguments.iter().zip(["p", "q"]))
            .map(|(argument, name)| argument.clone().unwrap_or(name.to_string()))
            .collect();
        let mut fields: Vec<String> = (["u", "v"].into_iter().zip(&arguments))
            .map(|(name, argument)| format!("{name} = {argument}"))
            .collect();
        let call = match spelling {
            Spelling::Keyworded => keyed(arguments.into_iter()),
            Spelling::ByName => format!("kaa {{{}}}", fields.join(", ")),
            Spelling::Reversed => {
                fields.reverse();
                format!("kaa {{{}}}", fields.join(", "))
            }
        };
        lines.push(format!(
            "LET g = (FN :{{{}}} -> Any = #({call}))",
            parameters.join(", ")
        ));
        let values: Vec<String> = (self.parameters.iter().zip(["p", "q"]))
            .map(|((_, value), name)| format!("{name} = {value}"))
            .collect();
        lines.push(format!("PRINT (g {{{}}})", values.join(", ")));
        lines.join("\n")
    }
}

#[test]
fn a_spelled_program_renders_as_source_that_loads() {
    let program = Spelled {
        registration: Registration {
            quantified: 1,
            slots: vec![
                Desc::List(Box::new(Desc::Variable(0))),
                Desc::Union(Box::new(Desc::Null), Box::new(Desc::Variable(0))),
            ],
        },
        parameters: vec![
            (Desc::List(Box::new(Desc::Number)), "[1, 2]".to_string()),
            (Desc::Str, "\"s\"".to_string()),
        ],
        arguments: vec![None, Some("null".to_string())],
    };
    let head = "EXPR #(KAA 1 AND 1)\n\
                LET kaa = FN EXPR FOR ALL #[Elt] #(KAA u :(LIST OF Elt) AND v :(Null | Elt)) -> \
                Any = #([u, v, Elt])\n";
    let tail = "PRINT (g {p = [1, 2], q = \"s\"})";
    let rendered = [
        (Spelling::Keyworded, "KAA p AND null"),
        (Spelling::ByName, "kaa {u = p, v = null}"),
        (Spelling::Reversed, "kaa {v = null, u = p}"),
    ];
    for (spelling, call) in rendered {
        let source = program.render(spelling);
        assert_eq!(
            source,
            format!(
                "{head}LET g = (FN :{{p :(LIST OF Number), q :Str}} -> Any = #({call}))\n{tail}"
            )
        );
        assert_eq!(super::run(&source), "[[1, 2], null, Number]", "{source}");
    }
}

/// The lexical-variable law's program: a site naming `WRAP`'s variable `Outer` by a type, inside a
/// stack of contexts that each put a scope, a block or a quote's code between it and `Outer`'s
/// home.
///
/// ```koan
/// EXPR FOR ALL #[Elt] #(SOLVE x :Elt) -> Type = #(Elt)
/// EXPR FOR ALL #[Outer] #(WRAP a :Outer) -> Any = #((<contexts around the site>) (null))
/// EXPR #(WIDE p :<D>) -> Any = #(WRAP p)
/// WRAP <v>                       — or —   WIDE <w>
/// ```
#[derive(Clone, Debug)]
pub(super) struct Lexical {
    /// `Outer` bound exactly, by `WRAP <v>`, or to `D`'s upper end through `WIDE <w>`.
    wide: bool,
    /// `D`, and the value `WRAP` or `WIDE` is called with.
    bound: (Desc, String),
    over: Over,
    /// Whether the site is an instance argument, else a contribution to `SOLVE`.
    instance: bool,
    /// The contexts around the site, outermost first: at most one `Code`, and no `Hoist` directly
    /// around a `Module`.
    pub(super) contexts: Vec<Context>,
}

/// A type over `Outer`.
#[derive(Clone, Debug)]
enum Over {
    Outer,
    List(Box<Over>),
    /// A record of one field, `x`.
    Record(Box<Over>),
    OrNull(Box<Over>),
}

/// What lies between a site and the body of `WRAP`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Context {
    /// A called `FN`'s body.
    Fn,
    /// The body of a closure a called `FN` returns, then called.
    Closure,
    /// An `EVAL`ed quote's code.
    Code,
    /// The middle operand of `"a" == … == "b"`, hoisted into a block.
    Hoist,
    /// A module's body.
    Module,
}

pub(super) fn lexical() -> impl Strategy<Value = Lexical> {
    let over = Just(Over::Outer).prop_recursive(2, 4, 1, |inner| {
        prop_oneof![
            inner.clone().prop_map(|over| Over::List(Box::new(over))),
            inner.clone().prop_map(|over| Over::Record(Box::new(over))),
            inner.prop_map(|over| Over::OrNull(Box::new(over))),
        ]
    });
    let context = prop_oneof![
        Just(Context::Fn),
        Just(Context::Closure),
        Just(Context::Code),
        Just(Context::Hoist),
        Just(Context::Module),
    ];
    let bound = inhabited_type().prop_flat_map(|desc| {
        let value = value_under(&desc);
        (Just(desc), value)
    });
    (
        any::<bool>(),
        bound,
        over,
        any::<bool>(),
        prop::collection::vec(context, 0..4),
    )
        .prop_map(|(wide, bound, over, instance, mut contexts)| {
            // A second quote's code would resolve its `$` names inside the first's.
            let mut quoted = false;
            for context in &mut contexts {
                if *context == Context::Code && std::mem::replace(&mut quoted, true) {
                    *context = Context::Closure;
                }
            }
            // A hoisted operand is one expression, and a module two statements.
            for index in 1..contexts.len() {
                if contexts[index] == Context::Module && contexts[index - 1] == Context::Hoist {
                    contexts[index - 1] = Context::Fn;
                }
            }
            Lexical {
                wide,
                bound,
                over,
                instance,
                contexts,
            }
        })
}

impl Lexical {
    /// The program with the site inside `contexts`, outermost first.
    pub(super) fn render(&self, contexts: &[Context]) -> String {
        let (desc, value) = &self.bound;
        let statements = self.statements(contexts, false, &mut 0);
        let call = match self.wide {
            true => format!("WIDE {value}"),
            false => format!("WRAP {value}"),
        };
        format!(
            "EXPR FOR ALL #[Elt] #(SOLVE x :Elt) -> Type = #(Elt)\n\
             EXPR FOR ALL #[Outer] #(WRAP a :Outer) -> Any = #(({}) (null))\n\
             EXPR #(WIDE p {}) -> Any = #(WRAP p)\n\
             {call}",
            statements.join(") ("),
            ascribed(desc, &[]),
        )
    }

    /// The site inside `contexts`, as the statements of the body holding it; `code` where a quote's
    /// code holds it, `modules` counting the modules already named.
    fn statements(&self, contexts: &[Context], code: bool, modules: &mut usize) -> Vec<String> {
        let Some((context, inside)) = contexts.split_first() else {
            return vec![self.site(code)];
        };
        let inner = self.statements(inside, code || *context == Context::Code, modules);
        let group = match &inner[..] {
            [one] => one.clone(),
            _ => format!("({})", inner.join(") (")),
        };
        match context {
            Context::Fn => vec![format!("((FN :{{}} -> Any = #({group})) {{}})")],
            Context::Closure => vec![format!(
                "(((FN :{{}} -> :(FN :{{}} -> Any) = #(FN :{{}} -> Any = #({group}))) {{}}) {{}})"
            )],
            Context::Code => vec![format!("(EVAL #({group}) -> Any)")],
            Context::Hoist => vec![format!("(\"a\" == ({group}) == \"b\")")],
            // A module lists what its body reads from outside: `WRAP`'s parameter and variable,
            // and — inside a quote's code, whose unmarked keys are holes — every key its body can
            // use: `PRINT`'s, and the comparison run's a hoisting context rewrites to.
            Context::Module => {
                *modules += 1;
                // The names the body's text spells, its string literals left out.
                let code_text: String = group.split('"').step_by(2).collect::<Vec<_>>().join(" ");
                let reads = |name: &str| {
                    code_text
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .any(|token| token == name)
                };
                let mut over: Vec<&str> = ["a", "Outer"]
                    .into_iter()
                    .filter(|name| reads(name))
                    .collect();
                if code {
                    over.extend(["(PRINT _)", "(_ == _)", "(_ AND _)"]);
                }
                vec![
                    format!("MODULE m{modules} OVER #[{}] = ({group})", over.join(" ")),
                    "null".to_string(),
                ]
            }
        }
    }

    /// The `PRINT` naming `Outer` by a type; `code` where a quote's code holds it.
    fn site(&self, code: bool) -> String {
        let spelled = self.over.spelled(code);
        if self.instance {
            let typed = match spelled.starts_with(':') {
                true => spelled.clone(),
                false => format!(":{spelled}"),
            };
            format!(
                "PRINT ((FN FOR ALL #[Elt] :{{x :Elt}} -> Elt = #(x)) :! :(FN :{{x {typed}}} -> {spelled}))"
            )
        } else {
            let solve = if code { "$(SOLVE" } else { "(SOLVE" };
            format!("PRINT {solve} ({} :! {spelled}))", self.over.value(code))
        }
    }
}

impl Over {
    /// The type as koan spells it: inside a quote's code, `Outer` is the quote's own, `($Outer)`.
    fn spelled(&self, code: bool) -> String {
        match self {
            Over::Outer if code => "($Outer)".to_string(),
            Over::Outer => "Outer".to_string(),
            Over::List(element) => format!("(LIST OF {})", element.spelled(code)),
            Over::Record(field) => {
                let field = field.spelled(code);
                match field.starts_with(':') {
                    true => format!(":{{x {field}}}"),
                    false => format!(":{{x :{field}}}"),
                }
            }
            Over::OrNull(inner) => format!("({} | Null)", inner.spelled(code)),
        }
    }

    /// A value of the type built over `a`.
    fn value(&self, code: bool) -> String {
        match self {
            Over::Outer if code => "$a".to_string(),
            Over::Outer => "a".to_string(),
            Over::List(element) => format!("[{}]", element.value(code)),
            Over::Record(field) => format!("{{x = {}}}", field.value(code)),
            Over::OrNull(inner) => inner.value(code),
        }
    }
}

/// What the stored and frame laws draw ([`stored`](super::stored), [`framed`](super::framed)): one keyworded registration
/// `SUBJECT`, quantified over `Elt` and `Key`, never called — so the load types its body and no
/// other fact reads it — beside the unknowns its body reads and a binding of some of them.
#[derive(Clone, Debug)]
pub(super) struct Resumable {
    /// Whether `SUBJECT` takes each drawn parameter: `k :Key`, `xs :(LIST OF Elt)`, `Item :Type`,
    /// `m :Counter` (or `:Module`, or `:(Counter & Zeroed)`), `mk :(FN :{} -> Counter)` and `p :Picks`. `x :Elt` it always
    /// takes.
    pub(super) key: bool,
    pub(super) list: bool,
    pub(super) item: bool,
    pub(super) module: bool,
    pub(super) maker: bool,
    pub(super) picks: bool,
    /// The type `m` is declared at.
    pub(super) module_at: ModuleAt,
    /// Whether the body binds the local module `n` and the type binder `Pair` over `Elt`, ahead
    /// of its drawn statements.
    pub(super) local: bool,
    pub(super) pair: Option<Desc>,
    /// Whether the body binds the type binder `Both = :{a :Elt b :(m.Carrier)}`, a path after a
    /// run-bound name in a type binder; only when it takes `m`.
    pub(super) both: bool,
    pub(super) statements: Vec<Statement>,
    /// What the binding gives each unknown it binds: `Elt`, `Key` and `Item` a closed type, and `m`
    /// and `n` a module over a drawn `Carrier`. `Pair` is rigid over `Elt`, so no binding binds it.
    pub(super) bound: Bound,
}

/// The unknowns a [`Resumable`] binding binds: each type name to a closed type, each module binder
/// to a module whose `Carrier` is the type, beside the value its `zero` holds.
#[derive(Clone, Debug, Default)]
pub(super) struct Bound {
    pub(super) elt: Option<Desc>,
    pub(super) key: Option<Desc>,
    pub(super) item: Option<Desc>,
    pub(super) module: Option<(Desc, String)>,
    pub(super) local: Option<(Desc, String)>,
    /// The module the first [`Statement::UndecidedModule`]'s `ha` binds: content the run builds,
    /// holding a call an answer selects among.
    pub(super) held: Option<Built>,
    /// The module the first [`Statement::BuiltModule`]'s `hq` binds: content the run builds whose
    /// tree holds a leaf no binder names.
    pub(super) built: Option<Built>,
}

/// A module a binding gives a binder holding content the run builds: `Zero` at `zero`'s type, and
/// `zero` where it holds one, which `Zeroed` wants.
#[derive(Clone, Debug)]
pub(super) struct Built {
    pub(super) zero: Desc,
    pub(super) value: Option<String>,
}

impl Built {
    /// The module, `Zero` and `zero` as drawn.
    fn module(&self) -> String {
        let zero = match &self.value {
            Some(value) => format!(" (LET zero = {value})"),
            None => String::new(),
        };
        format!("((LET Zero = {}){zero})", ascribed(&self.zero, &[]))
    }
}

/// The unknowns a [`Bound`] may bind, by the names the program spells them with.
pub(super) const UNKNOWNS: [&str; 7] = ["Elt", "Key", "Item", "m", "n", "ha", "hq"];

/// The [`UNKNOWNS`] a module binds: each declared as `bound<name>` and at its stamp as
/// `stamped<name>`.
pub(super) const MODULES: [&str; 4] = ["m", "n", "ha", "hq"];

impl Bound {
    /// Each unknown it binds, by name, beside the closed type it binds — a module's `Carrier`.
    pub(super) fn entries(&self) -> Vec<(&'static str, &Desc)> {
        let all = [
            self.elt.as_ref(),
            self.key.as_ref(),
            self.item.as_ref(),
            self.module.as_ref().map(|(carrier, _)| carrier),
            self.local.as_ref().map(|(carrier, _)| carrier),
            self.held.as_ref().map(|held| &held.zero),
            self.built.as_ref().map(|built| &built.zero),
        ];
        (UNKNOWNS.iter().zip(all))
            .filter_map(|(name, bound)| Some((*name, bound?)))
            .collect()
    }

    /// The module `name`, `m` or `n`, is bound to, declared at the top level.
    fn module_of(&self, name: &str) -> Option<&(Desc, String)> {
        match name {
            "m" => self.module.as_ref(),
            "n" => self.local.as_ref(),
            _ => None,
        }
    }

    /// The module the binder `name`, `ha` or `hq`, holding content the run builds, is bound to.
    fn built_of(&self, name: &str) -> Option<&Built> {
        match name {
            "ha" => self.held.as_ref(),
            "hq" => self.built.as_ref(),
            _ => None,
        }
    }
}

/// One drawn statement of `SUBJECT`'s body, each binding a fresh name.
#[derive(Clone, Debug)]
pub(super) enum Statement {
    /// `LET s = (OP <argument>)`.
    Call(Argument),
    /// `LET s = (FIRST x)` and `LET t = (OP s)`: a call over two candidates whose selection a
    /// binding of `Elt` decides, the record's returning its field `x`, a piece of the argument,
    /// and a later call reading its result.
    FirstCall,
    /// `LET s = (CHOOSE x)` and `LET t = (OP (s.zero))`: a holder of a callee's result, a module
    /// its last statement selects by `Elt`, and a later call reading a member of it.
    HeldResult,
    /// `LET s = (MATCH x WITH …)` and `LET t = (OP s)`: a `MATCH` over `x` whose arm a binding
    /// of `Elt` decides, and a later call reading it.
    Matched,
    /// `LET am = (MATCH x WITH #{:(LIST OF Elt): (it), _: (0)})`: an arm guarded by a type over
    /// `Elt`, whose `it` a binding re-stamps.
    ArmOverName,
    /// `LET rg = (MATCH x WITH #{:Elt: (1), :Number: (2), _: (0)})`: two guards a binding may make
    /// one type, refused where it does.
    RepeatedGuard,
    /// `LET w :(FN :{y :Elt} -> Number) = (FN FOR ALL #[Tee] :{y :Tee} -> Tee = #(y))`: an instance
    /// wanted at a type over `Elt`, whose solve a binding may make otherwise.
    InstanceOverName,
    /// A local registration whose first class solves `Tee` from `y :(LIST OF Tee)`, called on `x`
    /// beside an instance argument: a solve read through `Elt`'s bound, stored whole.
    StoredSolve,
    /// A local registration whose `f` ranks before `y :(LIST OF Tee)`, called on `[x, 1]` beside
    /// an instance argument: a slot a binding of `Elt` may make one type, so the load keeps it.
    InstanceBesideLiteral,
    /// `LET s = [x, 1]` and `LET t = {"k": x}`: container literals over `x`, typed through the
    /// fill.
    Literals,
    /// `LET s = m.zero`.
    Member,
    /// `LET s :(Elt & Number) = x`.
    Meet,
    /// `LET s :(Key & Number) = k`.
    MeetKey,
    /// `LET s :<type> = x`, the type over the body's type names.
    Annotated(Typed),
    /// `LET s = (x :! <type>)`.
    Ascribed(Typed),
    /// `LET g = (FN :{y :Elt} -> Any = #(LET p = (OP y) p))`: an inner callable over `Elt`.
    Inner,
    /// `LET l = x` and `LET g = (FN :{} -> Any = #(OP l))`: an inner callable over a local.
    InnerLocal,
    /// `LET s = (USING (m :! Counter) SCOPE (OP zero))`: a surfaced name.
    Using,
    /// `LET s = (MATCH x WITH #{:Counter: (it.zero), _: (0)})`: a member read through a guard
    /// over a variable a binding decides, the way a body reads a member its bound does not
    /// declare.
    VariableMember,
    /// `OP #(+) OVER Elt = #(0)`: a registration a binding may make overlap a builtin overload.
    Overlapping,
    /// `LET sq = #($..x)`: a splice of `x`, refused where a binding puts it outside a list of code.
    Splice,
    /// `LET ev = (EVAL x -> Any)` and `LET ew = (EVAL #($x) -> Str)`: an `EVAL` of `x`, refused
    /// where a binding makes it no code, and one whose code returns `x`, refused where a binding
    /// makes that no `Str`.
    Evaluated,
    /// `LET df = (FN :{} -> Any = x)`: a body written as a name, refused where a binding makes it
    /// no code.
    Deferred,
    /// `LET rd = ([1 2] x)`: a read whose argument a binding decides, refused where it puts `x`
    /// outside an index and a slice.
    Read,
    /// `LET ap = (x 0)` and `LET aq = (x "k")`: applications of a head a binding decides, each a
    /// read, a call, or refused.
    Applied,
    /// `LET s = (m :! Counter).zero`: a member read through a hold written in place.
    HeldMember,
    /// `LET s = ((m :! Counter) :! Counter).zero`: a member read through a hold of a hold.
    HeldTwice,
    /// `(m :! Counter)` and `LET s = [(m :! Counter)]`: holds written in place, as a statement and
    /// as a list's element.
    HeldInPlace,
    /// `LET s = (m :! (Counter WITH {Carrier = Elt}))`: a hold a binding may refuse.
    HeldPinned,
    /// `LET s = (USING (p :! (Picks WITH {Carrier = Elt})) SCOPE (PICK zero))`: a surfaced head,
    /// its ascription rigid over `Elt`.
    UsingHead,
    /// `LET s :{a :(Elt & Number) b :(m.Carrier)} = {a = x, b = m.zero}`: a path after a spelling
    /// the load leaves unknown over `Elt`.
    PathAfterMeet,
    /// `LET Mine = :(m.Carrier)`: a type binder over a path.
    PathBinder,
    /// A quote's code.
    Quote(Quoted),
    /// `LET v = (m :| Counter)`: a view over the module parameter.
    ViewParameter,
    /// `(m :| Counter)` and `LET s = [(m :| Counter)]`: views over the module parameter written in
    /// place, as a statement and as a list's element.
    ViewInPlace,
    /// A local `MODULE` body, and a view over it.
    ViewLocal,
    /// A local registration quantified over `Tee`, and a call selecting it at a closed solution.
    Unfolding,
    /// A local registration building a `MODULE` over its parameter, `e :Elt` or quantified
    /// `e :Tee` as drawn, called on `x`, and an annotation through the built module's member: a
    /// call the text selects, unfolded over `x`'s name.
    UnfoldModule(bool),
    /// Two local registrations building a `MODULE` over a `Number` or a `Str`, called twice on
    /// `x`, an annotation through one call's module of the other's member, and a view of the first
    /// at `Counter`, which every module they build refuses, where it `refuses`, else at `Zeroed`:
    /// a call only a binding of `Elt` selects among, two writes of which are one tree.
    UndecidedModule(bool),
    /// A local registration building a `MODULE` over a record, called on a record literal, whose
    /// tree no binder names, and an annotation through the module of its member: content whose
    /// tree holds a leaf no binder names.
    BuiltModule,
    /// A local callable over `Elt`, a call of it by name over a record literal of `x`, and one in
    /// the tail of a callable whose contract is `Number`; or, where it `solves`, a callable
    /// quantified over `Tee` called so at its head, and one whose other field is an instance of a
    /// quantified callable wanted at a type over `Tee`.
    ByName(bool),
    /// A local registration over `Elt`, and a call of it.
    LocalRegistration,
    /// A local `MODULE` body holding a callable over `Elt`.
    ModuleBody,
    /// A local `MODULE` body over `Elt` whose members never name it.
    ModuleOverName,
    /// A local `MODULE` body over a local alias of a type written over `Elt`.
    ModuleOverAlias,
    /// Two local callables over `Elt`, each calling the other: one component.
    Component,
    /// A quantified callable a local `MODULE` holds, read where it is wanted at a type over `Elt`.
    RigidInstance,
    /// A local `MODULE` holding `FN FOR ALL #[Tee] :{t :Tee, u :Elt, v :Key} -> Tee`: a scheme
    /// over both of `SUBJECT`'s names, read with one bound and the other kept.
    SchemeOverBoth,
    /// `LET s = px` and `LET t :(pw.Carrier) = pw.zero`: a top-level binder typed by a path of a
    /// class through one view, read beside a path of the class through another, both views of a
    /// module the load does not see, declared ahead of `SUBJECT` ([`PREFIX_CLASS`]).
    PrefixClass,
    /// `LET s = (FN :{q :(Paired WITH {Left = Elt})} -> Any = #(…))`: a callable reading a path
    /// through an application over `Elt` that leaves a parameter unpinned, so binding `Elt` alone
    /// moves the path's class ([`PAIRED`]), and the label it pins at `Elt`, which is that pin.
    PartialPath,
}

/// What a [`Statement::PrefixClass`] reads, declared at the top level ahead of `SUBJECT`.
const PREFIX_CLASS: &str = "MODULE pints = ((LET zero = 0) \
     (LET succ = (FN :{x :Number} -> Number = #(x))))\n\
     LET pm = (EVAL #($pints) -> Counter)\n\
     LET pv = (pm :| Counter)\nLET pw = (pm :| Counter)\nLET px :(pv.Carrier) = pv.zero\n";

/// What a [`Statement::PartialPath`] reads, declared at the top level ahead of `SUBJECT`.
const PAIRED: &str = "SIG Paired FOR ALL #[Left Right] = #[(VAL left :Left) (VAL right :Right)]\n";

/// The quote's code a [`Statement::Quote`] writes.
#[derive(Clone, Copy, Debug)]
pub(super) enum Quoted {
    /// `#(OP y)`: a hole a `USING` may fill.
    Hole,
    /// `#(OP \y)`: a mark the `EVAL` running it offers.
    Mark,
    /// `(EVAL #(OP $x) -> Any)`: a `$` name over `x`.
    Dollar,
    /// `(EVAL #(LET t :($Elt) = $x) -> Any)`: a `$` name over `Elt`.
    DollarType,
    /// `(EVAL #($m.zero) -> Any)`: a `$` name over the module binder `m`, whose capture holds the
    /// module the binding binds `m` to.
    DollarModule,
    /// `(EVAL #($(OP $x)) -> Any)`: a `$(…)` candidate over `x`, selected where the quote is born
    /// and typed in the code over `Elt`.
    DollarCall,
}

/// What a drawn `OP` call is applied to.
#[derive(Clone, Copy, Debug)]
pub(super) enum Argument {
    X,
    Xs,
    K,
    MZero,
    NZero,
    Number,
}

/// A type a statement writes: one of the body's type names, or `Number`.
#[derive(Clone, Copy, Debug)]
pub(super) enum Typed {
    Elt,
    Item,
    Pair,
    Number,
}

/// A closed type a binding gives an unknown: any [`desc`] but `Declared`, kept small.
fn closed() -> BoxedStrategy<Desc> {
    let leaf = prop_oneof![
        Just(Desc::Number),
        Just(Desc::Str),
        Just(Desc::Null),
        Just(Desc::Never),
        Just(Desc::Any),
    ];
    leaf.prop_recursive(2, 6, 2, |inner| {
        prop_oneof![
            inner.clone().prop_map(|each| Desc::List(Box::new(each))),
            prop::collection::vec((0..3u8, inner.clone()), 1..3).prop_map(Desc::Record),
            (inner.clone(), inner).prop_map(|(a, b)| Desc::Union(Box::new(a), Box::new(b))),
        ]
    })
    .boxed()
}

/// A module's `Carrier`, a closed inhabited type, beside a value under it for its `zero`.
fn carrier() -> BoxedStrategy<(Desc, String)> {
    closed()
        .prop_filter("a carrier some value inhabits", inhabited)
        .prop_flat_map(|carrier| {
            value_under(&carrier).prop_map(move |zero| (carrier.clone(), zero))
        })
        .boxed()
}

/// The type a [`Resumable`] program's module parameter `m` is declared at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModuleAt {
    /// `:Counter`, a signature with a head parameter its paths pin.
    Counter,
    /// `:Module`: exactly the type its argument carries, a stamp at `Module` keeping the
    /// module's own signature.
    Module,
    /// `:(Counter & Zeroed)`, a meet pinned member by member.
    Meet,
}

/// A [`Resumable`] program and a binding of its unknowns.
pub(super) fn resumable() -> impl Strategy<Value = Resumable> {
    let parameters = (
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        prop_oneof![
            2 => Just(ModuleAt::Counter),
            1 => Just(ModuleAt::Module),
            1 => Just(ModuleAt::Meet),
        ],
    );
    let bodies = (
        any::<bool>(),
        prop::option::of(prop_oneof![
            Just(Desc::Variable(0)),
            Just(Desc::List(Box::new(Desc::Variable(0)))),
            Just(Desc::Union(
                Box::new(Desc::Variable(0)),
                Box::new(Desc::Number)
            )),
        ]),
        prop::collection::vec(statement(), 1..5),
        any::<bool>(),
    );
    let bindings = (
        closed(),
        prop::option::weighted(0.6, Just(())),
        prop::option::weighted(0.4, closed()),
        prop::option::weighted(0.4, closed()),
        prop::option::weighted(0.5, carrier()),
        prop::option::weighted(0.5, carrier()),
    );
    // What `MKU` builds over a number or a string, and what `MKB` builds.
    let held = prop::option::weighted(0.6, any::<bool>());
    let built = prop::option::weighted(0.6, Just(()));
    (parameters, bodies, bindings, (held, built)).prop_map(
        |(
            (key, list, item, module, maker, picks, module_at),
            (local, pair, statements, both),
            bindings,
            (held, built),
        )| {
            let (solved, elt, bound_key, bound_item, bound_module, bound_local) = bindings;
            let elt = elt.map(|()| solved);
            let local = local && maker;
            let mut case = Resumable {
                key,
                list,
                item,
                module,
                maker,
                picks,
                module_at: match module {
                    true => module_at,
                    false => ModuleAt::Counter,
                },
                local,
                pair: pair.clone(),
                both: both && module && module_at != ModuleAt::Module,
                statements,
                bound: Bound {
                    elt,
                    key: bound_key.filter(|_| key),
                    item: bound_item.filter(|_| item),
                    module: bound_module.filter(|_| module),
                    local: bound_local.filter(|_| local),
                    held: None,
                    built: None,
                },
            };
            // A statement reads only what the program binds.
            let (module, local, pair, item) = (case.module, case.local, case.pair.is_some(), item);
            let counter = module && case.module_at != ModuleAt::Module;
            case.statements.retain(|statement| match statement {
                // A path reads a type member `Module` declares none of.
                Statement::PathAfterMeet | Statement::PathBinder => counter,
                Statement::Call(Argument::Xs) => list,
                Statement::Call(Argument::K) | Statement::MeetKey => key,
                Statement::Call(Argument::MZero)
                | Statement::Member
                | Statement::Using
                | Statement::HeldMember
                | Statement::HeldTwice
                | Statement::HeldInPlace
                | Statement::HeldPinned
                | Statement::ViewParameter
                | Statement::ViewInPlace
                | Statement::Quote(Quoted::DollarModule) => module,
                Statement::UsingHead => picks,
                Statement::Call(Argument::NZero) => local,
                Statement::Annotated(Typed::Item) | Statement::Ascribed(Typed::Item) => item,
                Statement::Annotated(Typed::Pair) | Statement::Ascribed(Typed::Pair) => pair,
                _ => true,
            });
            // A module binder the run builds is bound only where a statement writes it, to a module
            // a run builds there: over `Elt`'s binding where it binds one.
            let writes = |of: fn(&Statement) -> bool| case.statements.iter().any(of);
            let number = Built {
                zero: Desc::Number,
                value: Some("1".to_string()),
            };
            let string = Built {
                zero: Desc::Str,
                value: Some("\"s\"".to_string()),
            };
            let over = match (&case.bound.elt, held) {
                (Some(Desc::Number), Some(_)) | (None, Some(true)) => Some(number.clone()),
                (Some(Desc::Str), Some(_)) | (None, Some(false)) => Some(string),
                _ => None,
            };
            case.bound.held = over
                .filter(|_| writes(|statement| matches!(statement, Statement::UndecidedModule(_))));
            case.bound.built = built
                .map(|()| number)
                .filter(|_| writes(|statement| matches!(statement, Statement::BuiltModule)));
            case
        },
    )
}

/// The body of a callable whose own bound names an encloser's `Elt`, which reads its parameter
/// `y :Tee` under the bound: `y`, a call of `OP` on it, which its bound decides, or a list of it.
#[derive(Clone, Copy, Debug)]
pub(super) enum EnclosedBody {
    Itself,
    Op,
    Listed,
}

impl EnclosedBody {
    pub(super) fn text(self) -> &'static str {
        match self {
            EnclosedBody::Itself => "y",
            EnclosedBody::Op => "OP y",
            EnclosedBody::Listed => "[y]",
        }
    }
}

fn enclosed_body() -> impl Strategy<Value = EnclosedBody> {
    prop_oneof![
        Just(EnclosedBody::Itself),
        Just(EnclosedBody::Op),
        Just(EnclosedBody::Listed)
    ]
}

/// A run of a callable whose own `FOR ALL` bound is written over the encloser's `Elt`, which its
/// type carries beside its scheme: `OUTER`'s `Elt` bound to `elt` by its argument `value`, which
/// carries exactly `elt`,
/// as a name a run binds is bound to an argument's carried type; `INNER`'s bound `bound` over
/// `Elt`, its body `body`, called at `x`, or at `other` where it is drawn, beside an `INNER` at
/// `Any` where `fallback`. Where `held`, the callable is a `FN` a `LET` holds, called by name: its
/// value carries the scheme born with `Elt`'s content in its bound, which the call solves.
#[derive(Clone, Debug)]
pub(super) struct EnclosedBound {
    pub(super) elt: Desc,
    pub(super) value: String,
    pub(super) bound: &'static str,
    pub(super) body: EnclosedBody,
    pub(super) fallback: bool,
    pub(super) held: bool,
    pub(super) other: Option<String>,
}

impl EnclosedBound {
    /// The program, `OUTER` over `FOR ALL #[Elt]`.
    pub(super) fn render(&self) -> String {
        self.with("EXPR FOR ALL #[Elt] ", "")
    }

    /// The program with `Elt` declared as the type the run binds it to.
    pub(super) fn restart(&self) -> String {
        self.with(
            "EXPR ",
            &format!("LET Elt = {}\n", ascribed(&self.elt, &[])),
        )
    }

    fn with(&self, head: &str, declared: &str) -> String {
        let fallback = match self.fallback {
            true => "EXPR #(INNER y :Any) -> Any = #(\"outer\")\n",
            false => "",
        };
        let (bound, body) = (self.bound, self.body.text());
        let argument = self.other.as_deref().unwrap_or("x");
        let inner = match self.held {
            true => format!(
                "LET inner = (FN FOR ALL #{{Tee: {bound}}} :{{y :Tee}} -> Any = #({body}))\n  \
                 inner {{y = {argument}}}"
            ),
            false => format!(
                "EXPR FOR ALL #{{Tee: {bound}}} #(INNER y :Tee) -> Any = #({body})\n  \
                 INNER {argument}"
            ),
        };
        format!(
            "EXPR #(OP n :Number) -> Str = #(\"n\")\n\
             EXPR #(OP s :Str) -> Number = #(1)\n\
             {fallback}{declared}{head}#(OUTER x :Elt) -> Any = #(\n  \
             {inner}\n\
             )\n\
             PRINT (OUTER {})\n",
            self.value
        )
    }
}

/// A [`Resumable`] program beside a call of `SUBJECT` that runs it: `x` a value carrying `Elt`'s
/// binding, `k` one carrying `Key`'s, `xs` a list of `x`, `Item` the type `Number`, and `m` and
/// `mk`'s result the module `m`'s binding declares, drawn where the case binds none. It takes no
/// `p`, so no statement reads one. Where `signature`, `x` is that module, so the run binds `Elt` to
/// a signature type, which makes `x` a module binder in a fresh load with the binding declared.
pub(super) fn called() -> impl Strategy<Value = (Resumable, String)> {
    let signature = prop::bool::weighted(0.3);
    (resumable(), carried(), carried(), carrier(), signature).prop_map(
        |(mut case, (elt, x), (key, k), module, signature)| {
            case.picks = false;
            (case.statements).retain(|statement| !matches!(statement, Statement::UsingHead));
            if (case.module || case.maker || signature) && case.bound.module.is_none() {
                case.bound.module = Some(module);
            }
            let x = match signature {
                true => Resumable::declared("m"),
                false => x,
            };
            case.bound.elt = (!signature).then_some(elt);
            case.bound.key = case.key.then_some(key);
            let mut arguments = vec![x.clone()];
            if case.key {
                arguments.push(k);
            }
            if case.list {
                arguments.push(format!("[{x}]"));
            }
            if case.item {
                arguments.push(":Number".to_string());
            }
            let declared = Resumable::declared("m");
            if case.module {
                arguments.push(declared.clone());
            }
            if case.maker {
                arguments.push(format!("(FN :{{}} -> Counter = #({declared}))"));
            }
            let call = format!("PRINT (SUBJECT {})\n", arguments.join(" AND "));
            (case, call)
        },
    )
}

/// A closed type beside a literal that carries exactly it: a scalar, a list of one element, or a
/// record of fields — no union or `Any`, which no value carries.
fn carried() -> BoxedStrategy<(Desc, String)> {
    let leaf = prop_oneof![
        Just((Desc::Number, "1".to_string())),
        Just((Desc::Str, "\"s\"".to_string())),
        Just((Desc::Null, "null".to_string())),
    ];
    leaf.prop_recursive(2, 6, 2, |inner| {
        prop_oneof![
            inner
                .clone()
                .prop_map(|(desc, value)| (Desc::List(Box::new(desc)), format!("[{value}]"))),
            (inner.clone(), inner).prop_map(|((a, x), (b, y))| {
                let fields = vec![(0, a), (1, b)];
                (Desc::Record(fields), format!("{{x = {x}, y = {y}}}"))
            }),
        ]
    })
    .boxed()
}

/// A [`EnclosedBound`] over a closed `Elt` its argument carries.
pub(super) fn enclosed_bound() -> impl Strategy<Value = EnclosedBound> {
    let bound = prop_oneof![Just("Elt"), Just(":(Elt | Null)"), Just(":(LIST OF Elt)")];
    let other = prop::option::weighted(0.4, carried().prop_map(|(_, value)| value));
    (
        carried(),
        bound,
        enclosed_body(),
        any::<bool>(),
        any::<bool>(),
        other,
    )
        .prop_map(
            |((elt, value), bound, body, fallback, held, other)| EnclosedBound {
                elt,
                value,
                bound,
                body,
                fallback,
                held,
                other,
            },
        )
}

fn statement() -> impl Strategy<Value = Statement> {
    let argument = prop_oneof![
        Just(Argument::X),
        Just(Argument::Xs),
        Just(Argument::K),
        Just(Argument::MZero),
        Just(Argument::NZero),
        Just(Argument::Number),
    ];
    let typed = prop_oneof![
        Just(Typed::Elt),
        Just(Typed::Item),
        Just(Typed::Pair),
        Just(Typed::Number),
    ];
    let quoted = prop_oneof![
        Just(Quoted::Hole),
        Just(Quoted::Mark),
        Just(Quoted::Dollar),
        Just(Quoted::DollarType),
        Just(Quoted::DollarModule),
        Just(Quoted::DollarCall),
    ];
    prop_oneof![
        3 => argument.prop_map(Statement::Call),
        1 => Just(Statement::FirstCall),
        1 => Just(Statement::HeldResult),
        1 => Just(Statement::Matched),
        1 => Just(Statement::InstanceOverName),
        1 => Just(Statement::StoredSolve),
        1 => Just(Statement::InstanceBesideLiteral),
        1 => Just(Statement::Literals),
        1 => Just(Statement::Member),
        1 => Just(Statement::Meet),
        1 => Just(Statement::MeetKey),
        1 => typed.clone().prop_map(Statement::Annotated),
        1 => typed.prop_map(Statement::Ascribed),
        1 => Just(Statement::Inner),
        1 => Just(Statement::InnerLocal),
        1 => Just(Statement::Using),
        1 => Just(Statement::VariableMember),
        1 => Just(Statement::Applied),
        1 => Just(Statement::Overlapping),
        1 => Just(Statement::Splice),
        1 => Just(Statement::Evaluated),
        1 => Just(Statement::Deferred),
        1 => Just(Statement::ArmOverName),
        1 => Just(Statement::RepeatedGuard),
        1 => Just(Statement::Read),
        1 => Just(Statement::HeldMember),
        1 => Just(Statement::HeldTwice),
        1 => Just(Statement::HeldInPlace),
        1 => Just(Statement::HeldPinned),
        1 => Just(Statement::UsingHead),
        1 => Just(Statement::PathAfterMeet),
        1 => Just(Statement::PathBinder),
        1 => quoted.prop_map(Statement::Quote),
        1 => Just(Statement::ViewParameter),
        1 => Just(Statement::ViewInPlace),
        1 => Just(Statement::ViewLocal),
        1 => Just(Statement::Unfolding),
        1 => any::<bool>().prop_map(Statement::UnfoldModule),
        1 => any::<bool>().prop_map(Statement::UndecidedModule),
        1 => Just(Statement::BuiltModule),
        1 => any::<bool>().prop_map(Statement::ByName),
        1 => Just(Statement::LocalRegistration),
        1 => Just(Statement::ModuleBody),
        1 => Just(Statement::ModuleOverName),
        1 => Just(Statement::ModuleOverAlias),
        1 => Just(Statement::Component),
        1 => Just(Statement::RigidInstance),
        1 => Just(Statement::SchemeOverBoth),
        1 => Just(Statement::PrefixClass),
        1 => Just(Statement::PartialPath),
    ]
}

/// The registrations and signatures every [`Resumable`] program declares ahead of `SUBJECT`.
const RESUMABLE_PRELUDE: &str = "SIG Counter FOR ALL #[Carrier] = \
     #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
     SIG Picks FOR ALL #[Carrier] = #[(VAL zero :Carrier) (EXPR #(PICK x :Carrier) -> Str)]\n\
     SIG Zeroed = #[(VAL zero :Any)]\n\
     EXPR #(OP x :Number) -> Str = #(\"n\")\n\
     EXPR #(OP x :Str) -> Number = #(1)\n\
     EXPR FOR ALL #[Elt] #(OP x :(LIST OF Elt)) -> :(LIST OF Elt) = #(x)\n\
     EXPR FOR ALL #[Elt] #(FIRST r :{x :Elt}) -> Elt = #(r.x)\n\
     EXPR #(FIRST n :Number) -> Number = #(n)\n\
     EXPR #(NZ) -> Module = #(MODULE made = (LET zero = 1))\n\
     EXPR #(SZ) -> Module = #(MODULE made = (LET zero = \"s\"))\n\
     EXPR FOR ALL #[Tee] #(CHOOSE y :Tee) -> Module = #(MATCH y WITH #{:Number: (NZ), _: (SZ)})\n\
     EXPR #(BOTH2 x :Number AND y :Number) -> Str = #(\"nn\")\n\
     EXPR #(BOTH2 x :Number AND y :Str) -> Str = #(\"ns\")\n\
     EXPR #(BOTH2 x :Str AND y :Any) -> Number = #(1)\n";

impl Resumable {
    /// The program, `SUBJECT` written over every unknown — and, beside it, each type the binding
    /// gives an unknown declared as `Bound<name>`, and each module it binds as `bound<name>` beside
    /// `stamped<name>`, that module at the type a frame's retype stamps on it, so the binding reads
    /// its types and trees off the load.
    pub(super) fn render(&self) -> String {
        self.source(&Bound::default())
    }

    /// The program's **restart** under `bound`: each bound `FOR ALL` name dropped from the group
    /// and the `:Type` parameter from the shape, each declared at the top level as an alias of the
    /// type it binds; a bound module parameter dropped from the shape and a bound local rebound,
    /// each to the module it binds at the type its retype stamps, at the body's head; and a bound
    /// `Pair` declared as its type.
    pub(super) fn restart(&self, bound: &Bound) -> String {
        self.source(bound)
    }

    /// The name the type or module `bound` gives the unknown `name` is declared under.
    pub(super) fn declared(name: &str) -> String {
        match name {
            name if MODULES.contains(&name) => format!("bound{name}"),
            _ => format!("Bound{name}"),
        }
    }

    /// The name the module `bound` gives the module binder `name` is declared under at the type
    /// the binder's retype stamps on it: what a frame binds the binder to.
    pub(super) fn stamped(name: &str) -> String {
        format!("stamped{name}")
    }

    /// The type the module binder `name` is declared at: `m`'s as drawn, else `Counter`.
    fn module_declared(&self, name: &str) -> &'static str {
        match (name, self.module_at) {
            ("m", ModuleAt::Module) => "Module",
            ("m", ModuleAt::Meet) => "(Counter & Zeroed)",
            _ => "Counter",
        }
    }

    fn source(&self, restart: &Bound) -> String {
        let mut out = String::from(RESUMABLE_PRELUDE);
        for (name, desc) in self.bound.entries() {
            if let Some(built) = self.bound.built_of(name) {
                out += &format!(
                    "MODULE {} = {}\nLET {} :Module = {}\n",
                    Self::declared(name),
                    built.module(),
                    Self::stamped(name),
                    Self::declared(name),
                );
                continue;
            }
            out += &match self.bound.module_of(name) {
                Some((carrier, zero)) => format!(
                    "MODULE {} = ((LET Carrier = {}) (LET zero = {zero}) \
                     (LET succ = (FN :{{x :Carrier}} -> Carrier = #(x))))\n\
                     LET {} :{} = {}\n",
                    Self::declared(name),
                    ascribed(carrier, &[]),
                    Self::stamped(name),
                    self.module_declared(name),
                    Self::declared(name),
                ),
                None => format!("LET {} = {}\n", Self::declared(name), ascribed(desc, &[])),
            };
        }
        for (name, desc) in restart.entries() {
            if matches!(name, "Elt" | "Key" | "Item") {
                out += &format!("LET {name} = {}\n", ascribed(desc, &[]));
            }
        }
        let group: Vec<&str> = ["Elt", "Key"]
            .into_iter()
            .filter(|name| restart.entries().iter().all(|(bound, _)| bound != name))
            .collect();
        let quantifier = match group.is_empty() {
            true => String::new(),
            false => format!("FOR ALL #[{}] ", group.join(" ")),
        };
        let mut parameters = vec!["x :Elt".to_string()];
        if self.key {
            parameters.push("k :Key".to_string());
        }
        if self.list {
            parameters.push("xs :(LIST OF Elt)".to_string());
        }
        if self.item && restart.item.is_none() {
            parameters.push("Item :Type".to_string());
        }
        if self.module && restart.module.is_none() {
            parameters.push(format!("m :{}", self.module_declared("m")));
        }
        if self.maker {
            parameters.push("mk :(FN :{} -> Counter)".to_string());
        }
        if self.picks {
            parameters.push("p :Picks".to_string());
        }
        if (self.statements.iter()).any(|statement| matches!(statement, Statement::PartialPath)) {
            out += PAIRED;
        }
        if (self.statements.iter()).any(|statement| matches!(statement, Statement::PrefixClass)) {
            out += PREFIX_CLASS;
        }
        out += &format!(
            "EXPR {quantifier}#(SUBJECT {}) -> Any = #(\n",
            parameters.join(" AND ")
        );
        if self.module && restart.module.is_some() {
            out += &format!("  LET m = {}\n", Self::stamped("m"));
        }
        if self.local {
            match &restart.local {
                Some(_) => out += &format!("  LET n = {}\n", Self::stamped("n")),
                None => out += "  LET n = (mk {})\n",
            }
        }
        if let Some(pair) = &self.pair {
            out += &format!("  LET Pair = {}\n", ascribed(pair, &VARIABLES));
        }
        if self.both {
            out += "  LET Both = :{a :Elt b :(m.Carrier)}\n";
        }
        // The first `UndecidedModule` and `BuiltModule` name their binders as the binding does.
        let first = |of: fn(&Statement) -> bool| self.statements.iter().position(of);
        let held = first(|statement| matches!(statement, Statement::UndecidedModule(_)));
        let built = first(|statement| matches!(statement, Statement::BuiltModule));
        for (index, statement) in self.statements.iter().enumerate() {
            let named = Some(index) == held || Some(index) == built;
            out += &format!("  {}\n", statement.render(index, named, restart));
        }
        out += "  x\n)\n";
        out
    }
}

impl Statement {
    /// The statement as `SUBJECT`'s body writes it, its binders named by `index`, in the restart
    /// under `restart`.
    fn render(&self, index: usize, named: bool, restart: &Bound) -> String {
        let typed = |typed: &Typed| match typed {
            Typed::Elt => "Elt",
            Typed::Item => "Item",
            Typed::Pair => "Pair",
            Typed::Number => "Number",
        };
        // A keyword or type name the statement declares, unique among the body's statements.
        let letter = ["A", "B", "C", "D", "E", "F"][index];
        match self {
            Statement::Call(argument) => {
                let argument = match argument {
                    Argument::X => "x",
                    Argument::Xs => "xs",
                    Argument::K => "k",
                    Argument::MZero => "m.zero",
                    Argument::NZero => "n.zero",
                    Argument::Number => "1",
                };
                format!("LET s{index} = (OP {argument})")
            }
            Statement::HeldResult => {
                format!("LET s{index} = (CHOOSE x)\n  LET t{index} = (OP (s{index}.zero))")
            }
            Statement::FirstCall => {
                format!("LET s{index} = (FIRST x)\n  LET t{index} = (OP s{index})")
            }
            Statement::Matched => format!(
                "LET s{index} = (MATCH x WITH #{{:Number: (1), _: (\"s\")}})\n  \
                 LET t{index} = (OP s{index})"
            ),
            Statement::RepeatedGuard => {
                format!("LET rg{index} = (MATCH x WITH #{{:Elt: (1), :Number: (2), _: (0)}})")
            }
            Statement::ArmOverName => {
                format!("LET am{index} = (MATCH x WITH #{{:(LIST OF Elt): (it), _: (0)}})")
            }
            Statement::InstanceOverName => format!(
                "LET w{index} :(FN :{{y :Elt}} -> Number) = \
                 (FN FOR ALL #[Tee] :{{y :Tee}} -> Tee = #(y))"
            ),
            Statement::StoredSolve => format!(
                "EXPR FOR ALL #[Tee] #(APPLY{letter} y :(LIST OF Tee) WITH f :(FN :{{x :Tee}} -> Tee)) \
                 -> Any = #(y)\n  \
                 LET s{index} = (APPLY{letter} x WITH (FN FOR ALL #[Pick] :{{x :Pick}} -> Pick = #(x)))"
            ),
            Statement::InstanceBesideLiteral => format!(
                "EXPR FOR ALL #[Tee] #(TAKE{letter} f :(FN :{{x :Tee}} -> Tee) TO y :(LIST OF Tee)) \
                 -> Any = #(y)\n  \
                 LET s{index} = (TAKE{letter} (FN FOR ALL #[Pick] :{{x :Pick}} -> Pick = #(x)) TO [x, 1])"
            ),
            Statement::Literals => {
                format!("LET s{index} = [x, 1]\n  LET t{index} = {{\"k\": x}}")
            }
            Statement::Member => format!("LET s{index} = m.zero"),
            Statement::Meet => format!("LET s{index} :(Elt & Number) = x"),
            Statement::MeetKey => format!("LET s{index} :(Key & Number) = k"),
            Statement::Annotated(written) => format!("LET s{index} :{} = x", typed(written)),
            Statement::Ascribed(written) => format!("LET s{index} = (x :! {})", typed(written)),
            Statement::Inner => {
                format!(
                    "LET g{index} = (FN :{{y :Elt}} -> Any = #(\n    LET p = (OP y)\n    p\n  ))"
                )
            }
            Statement::InnerLocal => {
                format!("LET l{index} = x\n  LET g{index} = (FN :{{}} -> Any = #(OP l{index}))")
            }
            Statement::Using => format!("LET s{index} = (USING (m :! Counter) SCOPE (OP zero))"),
            Statement::Overlapping => "OP #(+) OVER Elt = #(0)".to_string(),
            Statement::Splice => format!("LET sq{index} = #($..x)"),
            Statement::Evaluated => {
                format!("LET ev{index} = (EVAL x -> Any)\n  LET ew{index} = (EVAL #($x) -> Str)")
            }
            Statement::Read => format!("LET rd{index} = ([1 2] x)"),
            Statement::Deferred => format!("LET df{index} = (FN :{{}} -> Any = x)"),
            Statement::Applied => format!("LET ap{index} = (x 0)\n  LET aq{index} = (x \"k\")"),
            Statement::VariableMember => {
                format!("LET s{index} = (MATCH x WITH #{{:Counter: (it.zero), _: (0)}})")
            }
            Statement::HeldMember => format!("LET s{index} = (m :! Counter).zero"),
            Statement::HeldTwice => format!("LET s{index} = ((m :! Counter) :! Counter).zero"),
            Statement::HeldInPlace => format!("(m :! Counter)\n  LET s{index} = [(m :! Counter)]"),
            Statement::HeldPinned => {
                format!("LET s{index} = (m :! (Counter WITH {{Carrier = Elt}}))")
            }
            Statement::UsingHead => format!(
                "LET s{index} = (USING (p :! (Picks WITH {{Carrier = Elt}})) SCOPE (PICK zero))"
            ),
            Statement::PathAfterMeet => format!(
                "LET s{index} :{{a :(Elt & Number) b :(m.Carrier)}} = {{a = x, b = m.zero}}"
            ),
            Statement::PathBinder => format!("LET Mine{letter} = :(m.Carrier)"),
            Statement::Quote(Quoted::Hole) => format!("LET q{index} = #(OP y)"),
            Statement::Quote(Quoted::Mark) => format!("LET q{index} = #(OP \\y)"),
            Statement::Quote(Quoted::Dollar) => format!("LET s{index} = (EVAL #(OP $x) -> Any)"),
            Statement::Quote(Quoted::DollarType) => {
                format!("LET s{index} = (EVAL #(LET t :($Elt) = $x) -> Any)")
            }
            Statement::Quote(Quoted::DollarModule) => {
                format!("LET s{index} = (EVAL #($m.zero) -> Any)")
            }
            Statement::Quote(Quoted::DollarCall) => {
                format!("LET s{index} = (EVAL #($(OP $x)) -> Any)")
            }
            Statement::ViewParameter => format!("LET v{index} = (m :| Counter)"),
            Statement::ViewInPlace => format!("(m :| Counter)\n  LET s{index} = [(m :| Counter)]"),
            Statement::ViewLocal => format!(
                "MODULE lm{index} = ((LET Carrier = Number) (LET zero = 0) \
                 (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n  \
                 LET v{index} = (lm{index} :| Counter)"
            ),
            Statement::Unfolding => format!(
                "EXPR FOR ALL #[Tee] #(UNF{letter} y :Tee) -> Tee = #(y)\n  \
                 LET u{index} = (UNF{letter} 1)"
            ),
            Statement::UnfoldModule(quantified) => {
                let (group, declared) = match quantified {
                    true => ("FOR ALL #[Tee] ", "Tee"),
                    false => ("", "Elt"),
                };
                format!(
                    "EXPR {group}#(MKM{letter} e :{declared}) -> Module = #(\n    \
                     (MODULE built OVER #[e] = (LET Zero = Number))\n    built\n  )\n  \
                     LET a{index} = (MKM{letter} x)\n  LET z{index} :(a{index}.Zero) = 5"
                )
            }
            Statement::UndecidedModule(refuses) => {
                let built = |name: &str, typed: &str| {
                    format!(
                        "EXPR #(MKU{letter} {name} :{typed}) -> Module = #(\n    \
                         (MODULE built OVER #[{name}] = ((LET Zero = {typed}) \
                         (LET zero = {name})))\n    built\n  )\n  "
                    )
                };
                let (a, b) = match named {
                    true => ("ha".to_string(), "hb".to_string()),
                    false => (format!("a{index}"), format!("b{index}")),
                };
                // A binding of `ha` gives `hb` its content too: the restart declares both.
                let holders = match (named, &restart.held) {
                    (true, Some(_)) => {
                        format!(
                            "LET {a} = {}\n  LET {b} = {}",
                            Resumable::stamped("ha"),
                            Resumable::stamped("ha")
                        )
                    }
                    _ => format!("LET {a} = (MKU{letter} x)\n  LET {b} = (MKU{letter} x)"),
                };
                let viewed = match refuses {
                    true => "Counter",
                    false => "Zeroed",
                };
                format!(
                    "{}{}{holders}\n  LET z{index} :({a}.Zero) = {b}.zero\n  \
                     LET v{index} = ({a} :| {viewed})",
                    built("n", "Number"),
                    built("s", "Str")
                )
            }
            Statement::BuiltModule => {
                let q = match named {
                    true => "hq".to_string(),
                    false => format!("q{index}"),
                };
                let holder = match (named, &restart.built) {
                    (true, Some(_)) => format!("LET {q} = {}", Resumable::stamped("hq")),
                    _ => format!("LET {q} = (MKB{letter} {{x = 1}})"),
                };
                format!(
                    "EXPR #(MKB{letter} r :{{x :Number}}) -> Module = #(\n    \
                     (MODULE built OVER #[r] = ((LET Zero = Number) (LET zero = r.x)))\n    built\n  )\n  \
                     {holder}\n  LET zq{index} :({q}.Zero) = {q}.zero"
                )
            }
            // A quantified callable is read only at a call's head.
            Statement::ByName(true) => format!(
                "LET c{index} = ((FN FOR ALL #[Tee] :{{y :Tee}} -> Tee = #(y)) {{y = x}})\n  \
                 LET d{index} = ((FN FOR ALL #[Tee] :{{y :Tee, f :(FN :{{z :Tee}} -> Tee)}} -> Tee \
                 = #(y)) {{y = x, f = (FN FOR ALL #[Que] :{{z :Que}} -> Que = #(z))}})"
            ),
            Statement::ByName(false) => format!(
                "LET h{index} = (FN :{{y :Elt}} -> Elt = #(y))\n  \
                 LET t{index} = (FN :{{z :Elt}} -> Number = #(h{index} {{y = z}}))\n  \
                 LET c{index} = (h{index} {{y = x}})"
            ),
            Statement::LocalRegistration => format!(
                "EXPR #(LOC{letter} y :Elt) -> Any = #(y)\n  LET r{index} = (LOC{letter} x)"
            ),
            Statement::ModuleBody => {
                format!("MODULE md{index} OVER #[Elt] = (LET f = (FN :{{y :Elt}} -> Elt = #(y)))")
            }
            Statement::ModuleOverName => format!("MODULE mn{index} OVER #[Elt] = (LET f = 7)"),
            Statement::ModuleOverAlias => format!(
                "LET Row{index} = :(LIST OF Elt)\n  \
                 MODULE ma{index} OVER #[Row{index}] = \
                 (LET f = (FN :{{y :Row{index}}} -> Row{index} = #(y)))"
            ),
            Statement::Component => format!(
                "LET ev{index} = (FN :{{y :Elt}} -> Any = #(od{index} {{y = y}}))\n  \
                 LET od{index} = (FN :{{y :Elt}} -> Any = #(ev{index} {{y = y}}))"
            ),
            Statement::SchemeOverBoth => format!(
                "MODULE sb{index} OVER #[Elt Key] = \
                 (LET h = (FN FOR ALL #[Tee] :{{t :Tee, u :Elt, v :Key}} -> Tee = #(t)))"
            ),
            Statement::RigidInstance => format!(
                "MODULE ids{index} = (LET id = (FN FOR ALL #[Tee] :{{y :Tee}} -> Tee = #(y)))\n  \
                 LET w{index} :(FN :{{y :Elt}} -> Elt) = ids{index}.id"
            ),
            Statement::PrefixClass => {
                format!("LET s{index} = px\n  LET t{index} :(pw.Carrier) = pw.zero")
            }
            Statement::PartialPath => format!(
                "LET s{index} = (FN :{{q :(Paired WITH {{Left = Elt}})}} -> Any = #(\n    \
                 LET r = q.right\n    LET t :(q.Right) = r\n    LET l :(q.Left) = q.left\n    t\n  ))"
            ),
        }
    }
}

/// One declaration of an [`Independent`] program: it reads only what the prelude declares, or a
/// registration another declares.
#[derive(Clone, Copy, Debug)]
pub(super) enum Declaration {
    /// `LET v = [1, 2]`.
    Value,
    /// `LET u = (OP 1)`: a use of the prelude's registration.
    Call,
    /// `LET Tee = :(LIST OF Number)`.
    Type,
    /// `LET f = (FN :{y :Number} -> Number = #(y + 1))`.
    Callable,
    /// `EXPR #(REG y :Number) -> Number = #(y)`.
    Registration,
    /// `EXPR FOR ALL #[Elt] #(QUANT y :Elt) -> Elt = #(y)`.
    Quantified,
    /// `MODULE m = ((LET Carrier = Number) (LET zero = 0))`.
    Module,
    /// `LET c = (FN :{} -> Number = #(REG 1))`: a callable whose body calls the registration the
    /// declaration at this index makes, which the load unfolds wherever either is written.
    Caller(usize),
    /// `LET y :(w.Carrier) = w.zero`: a path through the prelude's view `w`.
    PathRead,
    /// `LET r = (FN :{k :Any} -> Any = #((LET q = w) (LET z = x0) (z)))`: a callable capturing the
    /// view `w` and reading `x0`, typed by a path of `w`'s class through the view `v`.
    Reader,
}

/// A program of [`Declaration`]s, beside two of them to swap whose units read nothing of each
/// other.
#[derive(Clone, Debug)]
pub(super) struct Independent {
    pub(super) declarations: Vec<Declaration>,
    pub(super) swap: (usize, usize),
}

/// The prelude every [`Independent`] program declares ahead of its declarations: `OP`, a
/// signature and a module fitting it.
const INDEPENDENT_PRELUDE: &str = "EXPR #(OP x :Number) -> Str = #(\"n\")\n\
     SIG Counter FOR ALL #[Carrier] = \
     #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
     MODULE ints = ((LET zero = 0) (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n";

/// Two views of the module `m`, which the load does not see, and a binder typed by a path through
/// the first: what an [`Independent`] program's declarations read, ahead of them.
const INDEPENDENT_VIEWS: &str =
    "LET v = (m :| Counter)\nLET w = (m :| Counter)\nLET x0 :(v.Carrier) = v.zero\n";

/// An [`Independent`] program and a pair of its declarations to swap.
pub(super) fn independent() -> impl Strategy<Value = Independent> {
    let declaration = prop_oneof![
        Just(Declaration::Value),
        Just(Declaration::Call),
        Just(Declaration::Type),
        Just(Declaration::Callable),
        Just(Declaration::Registration),
        Just(Declaration::Quantified),
        Just(Declaration::Module),
    ];
    let drawn = (
        prop::collection::vec(declaration, 2..6),
        prop::collection::vec(any::<u8>(), 6),
    );
    drawn.prop_flat_map(|(mut declarations, callers)| {
        // A path read through `w` and a reader capturing `w`, each read through one class.
        declarations.extend([Declaration::PathRead, Declaration::Reader]);
        // A caller of some registration the drawing makes, in place of a value.
        let registrations: Vec<usize> = (declarations.iter().enumerate())
            .filter(|(_, declaration)| matches!(declaration, Declaration::Registration))
            .map(|(index, _)| index)
            .collect();
        for (index, choice) in callers.iter().enumerate().take(declarations.len()) {
            if matches!(declarations[index], Declaration::Value) && !registrations.is_empty() {
                let target = registrations[*choice as usize % registrations.len()];
                declarations[index] = Declaration::Caller(target);
            }
        }
        let reads = |a: usize, b: usize, declarations: &[Declaration]| {
            matches!(declarations[a], Declaration::Caller(target) if target == b)
        };
        let pairs = |declarations: &[Declaration]| -> Vec<(usize, usize)> {
            let count = declarations.len();
            (0..count)
                .flat_map(|a| (a + 1..count).map(move |b| (a, b)))
                .filter(|(a, b)| !reads(*a, *b, declarations) && !reads(*b, *a, declarations))
                .collect()
        };
        // A type binder reads nothing, so it makes a pair with any declaration.
        if pairs(&declarations).is_empty() {
            declarations.push(Declaration::Type);
        }
        let swaps = pairs(&declarations);
        (0..swaps.len()).prop_map(move |at| Independent {
            declarations: declarations.clone(),
            swap: swaps[at],
        })
    })
}

impl Independent {
    /// The program, its two drawn declarations swapped where `swapped`: at the top level over a
    /// module the load does not see, or, where `nested`, as statements of a callable's body over
    /// its module parameter. Each declaration's names are its own, whatever its place.
    pub(super) fn render(&self, swapped: bool, nested: bool) -> String {
        let mut order: Vec<usize> = (0..self.declarations.len()).collect();
        if swapped {
            order.swap(self.swap.0, self.swap.1);
        }
        let mut out = String::from(INDEPENDENT_VIEWS);
        for index in order {
            let letter = ["A", "B", "C", "D", "E", "F", "G", "H"][index];
            out += &match self.declarations[index] {
                Declaration::Value => format!("LET v{index} = [1, 2]"),
                Declaration::Call => format!("LET u{index} = (OP 1)"),
                Declaration::Type => format!("LET Tee{letter} = :(LIST OF Number)"),
                Declaration::Callable => {
                    format!("LET f{index} = (FN :{{y :Number}} -> Number = #(y + 1))")
                }
                Declaration::Registration => {
                    format!("EXPR #(REG{letter} y :Number) -> Number = #(y)")
                }
                Declaration::Quantified => {
                    format!("EXPR FOR ALL #[Elt] #(QUANT{letter} y :Elt) -> Elt = #(y)")
                }
                Declaration::Module => {
                    format!("MODULE m{index} = ((LET Carrier = Number) (LET zero = 0))")
                }
                Declaration::Caller(target) => {
                    let callee = ["A", "B", "C", "D", "E", "F", "G", "H"][target];
                    format!("LET c{index} = (FN :{{}} -> Number = #(REG{callee} 1))")
                }
                Declaration::PathRead => format!("LET y{index} :(w.Carrier) = w.zero"),
                Declaration::Reader => format!(
                    "LET r{index} = (FN :{{k :Any}} -> Any = #((LET q = w) (LET z = x0) (z)))"
                ),
            };
            out += "\n";
        }
        match nested {
            false => format!("{INDEPENDENT_PRELUDE}LET m = (EVAL #($ints) -> Counter)\n{out}"),
            true => {
                let body: String = out.lines().map(|line| format!("  {line}\n")).collect();
                format!(
                    "{INDEPENDENT_PRELUDE}LET outer = (FN :{{m :Counter}} -> Any = #(\n{body}  \"done\"\n))\n"
                )
            }
        }
    }
}

/// The overloads an [`UnseenMember`] program's argument sites call: overloads wanting a function
/// beside one wanting a number, two at one function type beside a label at `Number` or `Str`, two
/// that rank, and two at function types neither ranks over.
const UNSEEN_PRELUDE: &str = "\
EXPR #(APPLY f :(FN :{x :Number} -> Number)) -> Number = #(f {x = 4})
EXPR #(APPLY n :Number) -> Number = #(n)
EXPR #(PAIR k :Number f :(FN :{x :Number} -> Number)) -> Number = #(f {x = k})
EXPR #(PAIR k :Str f :(FN :{x :Str} -> Str)) -> Str = #(f {x = k})
EXPR #(RANK f :(FN :{x :Number} -> Number)) -> Number = #(f {x = 4})
EXPR #(RANK f :(FN :{x :Number} -> Any)) -> Any = #(f {x = 5})
EXPR #(BOTH f :(FN :{x :Number} -> Number)) -> Number = #(f {x = 4})
EXPR #(BOTH f :(FN :{x :Str} -> Str)) -> Str = #(f {x = \"s\"})
";

/// A program reading the value member `identity` off a module, and the same program with the
/// module declared: `member` is the member's value, and `body` reads it, `MOD` standing for the
/// module, in a body whose module binder `EVAL` binds, or in one under the declared module.
#[derive(Clone, Debug)]
pub(super) struct UnseenMember {
    pub(super) member: &'static str,
    pub(super) body: String,
}

impl UnseenMember {
    /// The program reading the member off a module the load cannot see: an `EVAL`'s, which the
    /// run builds.
    pub(super) fn render(&self) -> String {
        let body = self.body.replace("MOD.", "built.");
        format!(
            "{UNSEEN_PRELUDE}EXPR #(USE code :Any) -> Any = #(\n  \
             (LET built = (EVAL code -> Module))\n  {body}\n)\n\
             PRINT (USE #(MODULE made = (LET identity = {})))\n",
            self.member
        )
    }

    /// The program with the module declared, read in a body of its own.
    pub(super) fn restart(&self) -> String {
        let body = self.body.replace("MOD.", "made.");
        format!(
            "{UNSEEN_PRELUDE}MODULE made = (LET identity = {})\n\
             EXPR #(USE) -> Any = #(\n  {body}\n)\nPRINT (USE)\n",
            self.member
        )
    }
}

/// An [`UnseenMember`]: a quantified member, one bounded or returning over its variable, a plain
/// function or a number, read at an annotated `LET`, in a list, held then called, called, or
/// passed to each overload set of [`UNSEEN_PRELUDE`], at a label or an argument of either scalar;
/// or a member the module does not declare, read at any of those.
pub(super) fn unseen_member() -> impl Strategy<Value = UnseenMember> {
    let member = prop_oneof![
        3 => Just("(FN FOR ALL #[Item] :{x :Item} -> Item = #(x))"),
        1 => Just("(FN FOR ALL #{Item: Number} :{x :Item} -> Item = #(x))"),
        1 => Just("(FN FOR ALL #[Item] :{x :Item} -> (LIST OF Item) = #([x]))"),
        1 => Just("(FN :{x :Number} -> Number = #(x))"),
        1 => Just("(FN :{x :Str} -> Str = #(x))"),
        1 => Just("5"),
    ];
    let scalar = prop_oneof![Just("1"), Just("\"s\"")];
    let annotation = prop_oneof![
        Just(":(FN :{x :Number} -> Number)"),
        Just(":(FN :{x :Str} -> Str)"),
        Just(":(FN :{x :Number} -> Any)"),
        Just(":(FN :{x :Str} -> Number)"),
        Just(":Any"),
    ];
    let body =
        (annotation, scalar.clone(), scalar, 0..9usize).prop_map(|(annotation, a, b, site)| {
            match site {
                0 => format!("(LET go {annotation} = MOD.identity)\n  (PRINT (go {{x = {a}}}))"),
                1 => "(PRINT [MOD.identity])".to_string(),
                2 => format!(
                    "(LET f = MOD.identity)\n  (PRINT (f {{x = {a}}}))\n  (PRINT (f {{x = {b}}}))"
                ),
                3 => format!("(PRINT (MOD.identity {{x = {a}}}))"),
                4 => "(PRINT (APPLY MOD.identity))".to_string(),
                5 => "(LET f = MOD.identity)\n  (PRINT (APPLY f))".to_string(),
                6 => format!("(PRINT (PAIR {a} MOD.identity))"),
                7 => format!("(LET v :Any = {a})\n  (PRINT (PAIR v MOD.identity))"),
                _ => match a {
                    "1" => "(PRINT (RANK MOD.identity))".to_string(),
                    _ => "(PRINT (BOTH MOD.identity))".to_string(),
                },
            }
        });
    // A read of a member the module does not declare is refused as the fresh load refuses it.
    let read = prop_oneof![5 => Just("identity"), 1 => Just("missing")];
    (member, body, read).prop_map(|(member, body, read)| UnseenMember {
        member,
        body: body.replace("MOD.identity", &format!("MOD.{read}")),
    })
}

#[test]
fn a_resumable_program_renders_as_source_that_loads() {
    use proptest::strategy::ValueTree;
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    for _ in 0..64 {
        let case = resumable()
            .new_tree(&mut runner)
            .expect("a case draws")
            .current();
        let source = case.render();
        let loaded = crate::program::CellSubstrate::load::<super::Koan>(
            &source,
            "<test>",
            8,
            crate::values::BuildBudget::default(),
            super::output(),
        );
        assert!(
            loaded.is_ok(),
            "{source}\n{:?}",
            loaded.err().map(|e| e.to_string())
        );
    }
}

/// Where a [`Holding`] program's statements run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Holds {
    /// The top level, over two modules bound there.
    Top,
    /// A called frame over two module arguments.
    Frame,
    /// An arm of a `MATCH` in a called frame, run on each call but a last one.
    Arm,
    /// A `TRY` body in a called frame, which a later statement raises out of.
    Try,
    /// A `MODULE` body in a called frame.
    Module,
    /// A callable nested in a called frame, reading the frame's arguments.
    Nested,
}

/// What a holder holds: an `EVAL` of the module argument it names, one of `left` and `right`,
/// which the run types; `BOX` over a number or a string, which the text types; or `BOX` over the
/// `sample` of a second holder's `EVAL`, which holds that holder. None prints or refuses, so a
/// holder's place among the others its statement holds is not seen.
#[derive(Clone, Copy, Debug)]
pub(super) enum Holding {
    Eval(bool),
    Boxed(bool),
    Reboxed(bool),
}

/// A statement of a [`Held`] body: a print, or a print of `SHOW` or `BOTH` over the `sample` of
/// the modules it holds. `BOTH` over two strings has no overload, so it is refused, at the load
/// where the text types both and at the run otherwise.
#[derive(Clone, Copy, Debug)]
pub(super) enum HeldUnit {
    Print,
    Show(Holding),
    Both(Holding, Holding),
}

/// A body whose statements read modules through holders, in the context `holds` drawn around it,
/// called once per entry of `calls`, each a `(left, right)` pair saying which argument's `sample`
/// is a string. [`Held::render`] writes some holders in place, beside the program's `LET`
/// spelling: the two run alike.
#[derive(Clone, Debug)]
pub(super) struct Held {
    pub(super) holds: Holds,
    units: Vec<HeldUnit>,
    calls: Vec<(bool, bool)>,
}

/// The [`Held`] programs: up to four statements holding at least one part, in each context.
pub(super) fn held() -> impl Strategy<Value = Held> {
    let holding = prop_oneof![
        3 => any::<bool>().prop_map(Holding::Eval),
        1 => any::<bool>().prop_map(Holding::Boxed),
        1 => any::<bool>().prop_map(Holding::Reboxed),
    ];
    let unit = prop_oneof![
        1 => Just(HeldUnit::Print),
        2 => holding.clone().prop_map(HeldUnit::Show),
        2 => (holding.clone(), holding).prop_map(|(a, b)| HeldUnit::Both(a, b)),
    ];
    let holds = prop_oneof![
        Just(Holds::Top),
        Just(Holds::Frame),
        Just(Holds::Arm),
        Just(Holds::Try),
        Just(Holds::Module),
        Just(Holds::Nested),
    ];
    let calls = prop::collection::vec(any::<(bool, bool)>(), 1..=2);
    (holds, prop::collection::vec(unit, 1..=4), calls)
        .prop_map(|(holds, units, calls)| Held {
            holds,
            units,
            calls,
        })
        .prop_filter("holds a part", |held| held.holders() > 0)
}

impl Held {
    /// How many holders the body writes.
    pub(super) fn holders(&self) -> usize {
        let holders = |part: &Holding| match part {
            Holding::Reboxed(_) => 2,
            _ => 1,
        };
        (self.units.iter())
            .map(|unit| match unit {
                HeldUnit::Print => 0,
                HeldUnit::Show(a) => holders(a),
                HeldUnit::Both(a, b) => holders(a) + holders(b),
            })
            .sum()
    }

    /// The program with holder `k` written in place where `inline(k)`, and its `LET` spelling:
    /// the holders `inline` keeps as `LET`s immediately before the statement holding them, then
    /// a `LET` for each it writes in place, left to right, as the run binds them.
    pub(super) fn render(&self, inline: impl Fn(usize) -> bool) -> (String, String) {
        (
            self.spelled(|holder| inline(holder).then_some(false)),
            self.spelled(|holder| inline(holder).then_some(true)),
        )
    }

    /// The program with holder `k` kept as a `LET` where `place(k)` is `None`, written in place
    /// where it is `Some(true)`, and bound by a `LET` after the kept ones where it is
    /// `Some(false)`.
    fn spelled(&self, place: impl Fn(usize) -> Option<bool>) -> String {
        let block = !matches!(self.holds, Holds::Top | Holds::Frame | Holds::Nested);
        let mut lines = Vec::new();
        let mut holder = 0;
        for (at, unit) in self.units.iter().enumerate() {
            let parts: &[Holding] = match unit {
                HeldUnit::Print => &[],
                HeldUnit::Show(a) => &[*a],
                HeldUnit::Both(a, b) => &[*a, *b],
            };
            let (mut kept, mut after) = (Vec::new(), Vec::new());
            let read: Vec<_> = (parts.iter())
                .map(|part| Self::part(*part, &mut holder, &place, &mut kept, &mut after))
                .map(|module| format!("({module}.sample)"))
                .collect();
            lines.append(&mut kept);
            lines.append(&mut after);
            lines.push(match unit {
                HeldUnit::Print => format!("PRINT \"p{at}\""),
                HeldUnit::Show(_) => format!("PRINT (SHOW {})", read[0]),
                HeldUnit::Both(..) => format!("PRINT (BOTH {} {})", read[0], read[1]),
            });
        }
        let indent = |depth: usize, lines: &[String]| {
            let pad = "  ".repeat(depth);
            (lines.iter())
                .map(|line| match block {
                    true => format!("{pad}({line})\n"),
                    false => format!("{pad}{line}\n"),
                })
                .collect::<String>()
        };
        let sample = |string: bool| match string {
            false => "#(MODULE made = (LET sample = 1))",
            true => "#(MODULE made = (LET sample = \"hi\"))",
        };
        let mut source = String::from(
            "EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR #(BOTH a :Number b :Number) -> Str = #(\"two numbers\")
EXPR #(BOTH a :Number b :Str) -> Str = #(\"number, string\")
EXPR #(BOTH a :Str b :Number) -> Str = #(\"string, number\")
EXPR #(BOX n :Number) -> Module = #(MODULE made = (LET sample = 1))
EXPR #(BOX s :Str) -> Module = #(MODULE made = (LET sample = \"hi\"))
EXPR #(GRAB box :{p :Any}) -> Any = #(box.p.z)
",
        );
        let params = match self.holds {
            Holds::Arm => "n :Number left :Any right :Any",
            _ => "left :Any right :Any",
        };
        let body = match self.holds {
            Holds::Top => {
                let (left, right) = self.calls[0];
                source += &format!(
                    "LET left = {}\nLET right = {}\n",
                    sample(left),
                    sample(right)
                );
                source += &indent(0, &lines);
                return source;
            }
            Holds::Frame => indent(1, &lines),
            Holds::Arm => format!(
                "  MATCH n WITH #{{\n    0: (\n{}    ),\n    Number: (\"other\")\n  }}\n",
                indent(3, &lines)
            ),
            Holds::Try => format!(
                "  TRY (\n{}    (GRAB {{p = {{x = 1}}}})\n  ) WITH #{{NoField: (\"caught\")}}\n",
                indent(2, &lines)
            ),
            Holds::Module => format!(
                "  MODULE pair OVER #[left right] = (\n{}  )\n  \"done\"\n",
                indent(2, &lines)
            ),
            Holds::Nested => format!(
                "  EXPR #(INNER) -> Any = #(\n{}  )\n  INNER\n",
                indent(2, &lines)
            ),
        };
        source += &format!("EXPR #(USE {params}) -> Any = #(\n{body})\n");
        for (left, right) in self.calls.iter() {
            let (left, right) = (sample(*left), sample(*right));
            source += &match self.holds {
                Holds::Arm => format!("PRINT (USE 0 {left} {right})\n"),
                _ => format!("PRINT (USE {left} {right})\n"),
            };
        }
        if self.holds == Holds::Arm {
            let (left, right) = self.calls[0];
            source += &format!("PRINT (USE 1 {} {})\n", sample(left), sample(right));
        }
        source
    }

    /// The text reading the module `part` holds, its holders numbered from `holder` on, inner
    /// first: each holder kept as a `LET`, after the `LET`s of the parts it holds, pushed to
    /// `kept`, and each bound after the kept ones pushed to `after`.
    fn part(
        part: Holding,
        holder: &mut usize,
        place: &impl Fn(usize) -> Option<bool>,
        kept: &mut Vec<String>,
        after: &mut Vec<String>,
    ) -> String {
        let eval = |right: bool| match right {
            false => "(EVAL left -> Module)",
            true => "(EVAL right -> Module)",
        };
        let source = match part {
            Holding::Eval(right) => eval(right).to_string(),
            Holding::Boxed(false) => "(BOX 5)".to_string(),
            Holding::Boxed(true) => "(BOX \"five\")".to_string(),
            Holding::Reboxed(right) => {
                // The inner holder's `LET` is before the statement holding it: the outer
                // holder's own where it is kept, else the one this part is written in.
                let outer = *holder + 1;
                let (mut inner_kept, mut inner_after) = (Vec::new(), Vec::new());
                let inner = Self::part(
                    Holding::Eval(right),
                    holder,
                    place,
                    &mut inner_kept,
                    &mut inner_after,
                );
                kept.append(&mut inner_kept);
                match place(outer) {
                    None => kept.append(&mut inner_after),
                    Some(_) => after.append(&mut inner_after),
                }
                format!("(BOX ({inner}.sample))")
            }
        };
        let at = *holder;
        *holder += 1;
        let bound = format!("LET h{at} = {source}");
        match place(at) {
            Some(true) => return source,
            Some(false) => after.push(bound),
            None => kept.push(bound),
        }
        format!("h{at}")
    }
}

#[test]
fn a_held_program_renders_as_source_that_loads() {
    use proptest::strategy::ValueTree;
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    for _ in 0..64 {
        let case = held()
            .new_tree(&mut runner)
            .expect("a case draws")
            .current();
        let (held, inline) = case.render(|holder| holder % 2 == 0);
        for source in [held, inline] {
            let loaded = crate::program::CellSubstrate::load::<super::Koan>(
                &source,
                "<test>",
                8,
                crate::values::BuildBudget::default(),
                super::output(),
            );
            assert!(
                loaded.is_ok()
                    || loaded
                        .as_ref()
                        .err()
                        .is_some_and(|e| e.to_string().contains("BOTH")),
                "{source}\n{:?}",
                loaded.err().map(|e| e.to_string())
            );
        }
    }
}
