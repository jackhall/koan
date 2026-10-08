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
//! [`retypes`](super::retypes) retypes a value drawn under a [`retyped_type`] at each retype site.

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
    /// The union `Maybe`.
    Maybe,
    /// `Maybe`'s variant `Some`.
    MaybeSome,
}

/// The nominal types [`retyped_type`] draws from, declared ahead of a program that names them.
pub(super) const NOMINALS: &str = "NEWTYPE Boxed = Number\n\
                                   NEWTYPE (Type AS Wrap)\n\
                                   UNION Maybe = #{Some: Number, None: Null}";

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
/// family's application: every kind of type a retype re-stamps a value at.
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
        Desc::Wrapped(inner) => format!(":({} AS Wrap)", spelled(inner, variables)),
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
        Desc::Boxed | Desc::Maybe | Desc::MaybeSome => true,
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
        Desc::Dict(_) | Desc::Boxed | Desc::Wrapped(_) | Desc::Maybe | Desc::MaybeSome => {
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
        Desc::Dict(_) | Desc::Boxed | Desc::Wrapped(_) | Desc::Maybe | Desc::MaybeSome => {
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
            Desc::Dict(_) | Desc::Boxed | Desc::Wrapped(_) | Desc::Maybe | Desc::MaybeSome => {
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
