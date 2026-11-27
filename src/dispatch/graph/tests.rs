//! The fact domain's laws: an ask over closed contents is the closed door with the binding written
//! in, a rigid answer is every binding's, two positions bound to one rigid are judged apart, a term applied to closed contents is the
//! term rebound, a name the binding leaves open is its own open variable, applying never grows past
//! what the mentions report, and the empty union is no alternative.

use std::cell::RefCell;

use bindgraph::Nesting;
use proptest::prelude::*;

use crate::memory::Bump;
use crate::scope::{
    Answer, DerivedName, DerivedTerm, Levels, NameAddress, Question, Rule, Spellings, answer,
    free_variables, rebound, rigid_answer,
};
use crate::symbols::{SymbolInterner, TypeSymbol};
use crate::type_lattice::{
    DeclaredType, DerivedId, Interval, KType, Parametric, Step, TypeNode, Variable,
};
use crate::values::Chain;
use crate::values::tests::TestTypes;

use super::{Content, Named, Naming, apply, ask, rigid, union};
use crate::build::lay::{NameVariable, mentions};

/// The derived names a test lays, in a bump that outlives it.
struct Table {
    names: RefCell<Vec<DerivedName<'static>>>,
    bump: &'static Bump,
}

impl Table {
    fn new() -> Self {
        Table {
            names: RefCell::new(Vec::new()),
            bump: Box::leak(Box::new(Bump::new())),
        }
    }

    fn get(&self, id: DerivedId) -> Option<DerivedName<'static>> {
        self.names
            .borrow()
            .iter()
            .find(|name| name.id == id)
            .copied()
    }

    fn lay(&self, name: DerivedName<'_>) {
        if self.get(name.id).is_none() {
            self.names.borrow_mut().push(name.laid(self.bump));
        }
    }
}

/// The naming door over `table`.
fn naming<'n>(
    table: &'n Table,
    symbols: &'n SymbolInterner,
    names: &'n dyn Fn(DerivedId) -> Option<DerivedName<'n>>,
    laid: &'n dyn Fn(DerivedName<'_>),
) -> Naming<'n> {
    let _ = table;
    Naming {
        names,
        spellings: Spellings::new(symbols),
        laid,
    }
}

/// A generated type over the levels 0 and 1.
#[derive(Clone, Debug)]
enum Drawn {
    Number,
    Str,
    Any,
    Level(usize),
    List(Box<Drawn>),
    Union(Box<Drawn>, Box<Drawn>),
}

fn drawn(open: bool) -> impl Strategy<Value = Drawn> {
    let mut leaves = vec![Just(Drawn::Number).boxed(), Just(Drawn::Str).boxed()];
    leaves.push(Just(Drawn::Any).boxed());
    if open {
        leaves.push((0..2usize).prop_map(Drawn::Level).boxed());
    }
    proptest::strategy::Union::new(leaves).prop_recursive(3, 12, 2, |inner| {
        prop_oneof![
            inner.clone().prop_map(|each| Drawn::List(Box::new(each))),
            (inner.clone(), inner).prop_map(|(a, b)| Drawn::Union(Box::new(a), Box::new(b))),
        ]
    })
}

fn built(
    types: &Chain<'_, '_, '_, '_>,
    scratch: &Bump,
    levels: &[Parametric; 2],
    drawn: &Drawn,
) -> Parametric {
    match drawn {
        Drawn::Number => KType::NUMBER.into(),
        Drawn::Str => KType::STR.into(),
        Drawn::Any => KType::ANY.into(),
        Drawn::Level(level) => levels[*level],
        Drawn::List(element) => types.list(built(types, scratch, levels, element)),
        Drawn::Union(a, b) => {
            let members = [
                built(types, scratch, levels, a),
                built(types, scratch, levels, b),
            ];
            types.union_of(scratch, &members)
        }
    }
}

/// How deep `ty` nests lists, a union as deep as its deepest member.
fn depth(types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> usize {
    match types.node(ty) {
        TypeNode::List { element } => 1 + depth(types, element),
        TypeNode::Union { members } => (members.iter())
            .map(|member| depth(types, member))
            .max()
            .unwrap_or(0),
        _ => 0,
    }
}

fn site(index: u64) -> NameAddress {
    NameAddress::Answered(DerivedId([index, 0]))
}

/// The two level names, each bound to `contents`' entry.
fn named<'c>(contents: [Option<Content<'c>>; 2]) -> [Named<'c>; 2] {
    [0, 1].map(|level| Named {
        variable: Some(NameVariable::Level(level)),
        address: site(level as u64 + 1),
        upper: KType::ANY,
        content: contents[level],
    })
}

fn level_names(types: &Chain<'_, '_, '_, '_>, symbols: &SymbolInterner) -> [Parametric; 2] {
    let name = |text| TypeSymbol::declared(text, symbols).expect("a Type token");
    [
        types.lexical(0, name("Elt"), KType::ANY),
        types.lexical(1, name("Key"), KType::ANY),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// Over closed contents, an ask is the closed door asked with the binding written in.
    #[test]
    fn an_ask_over_closed_contents_is_the_closed_door(
        a in drawn(true),
        b in drawn(true),
        first in drawn(false),
        second in drawn(false),
        kind in 0..4usize,
    ) {
        let (registry, scratch) = (Bump::new(), Bump::new());
        let types = TestTypes::new(&registry).load();
        let symbols = SymbolInterner::new();
        let table = Table::new();
        let (names, laid) = (|id| table.get(id), |name: DerivedName<'_>| table.lay(name));
        let naming = naming(&table, &symbols, &names, &laid);
        let levels = level_names(&types, &symbols);
        let (a, b) = (built(&types, &scratch, &levels, &a), built(&types, &scratch, &levels, &b));
        let closed = |drawn| types.concrete(built(&types, &scratch, &levels, drawn)).unwrap();
        let (first, second) = (closed(&first), closed(&second));
        let question = match kind {
            0 => Question::Fits { a: a.into(), b: b.into() },
            1 => Question::Same { a: a.into(), b: b.into() },
            2 => Question::Under { a, b },
            _ => Question::Kind { ty: a },
        };
        let contents = [first, second].map(|closed| Some(Content::Type(Parametric::from(closed).into())));
        let asked = ask(&types, &scratch, naming, None, &question, &named(contents));
        let bound = Levels::from_bound(&scratch, &[(0, first), (1, second)]);
        let (written, _) = question.substituted(&types, &scratch, &bound);
        prop_assert_eq!(asked, answer(&types, &scratch, &written));
    }

    /// A rigid answer is every binding's: where the one rule per question kind answers over names,
    /// the closed door answers alike with any binding written in.
    #[test]
    fn a_rigid_answer_is_every_binding_s(
        a in drawn(true),
        b in drawn(true),
        first in drawn(false),
        second in drawn(false),
        kind in 0..7usize,
    ) {
        let (registry, scratch) = (Bump::new(), Bump::new());
        let types = TestTypes::new(&registry).load();
        let symbols = SymbolInterner::new();
        let levels = level_names(&types, &symbols);
        let (a, b) = (built(&types, &scratch, &levels, &a), built(&types, &scratch, &levels, &b));
        let closed = |drawn| types.concrete(built(&types, &scratch, &levels, drawn)).unwrap();
        let (first, second) = (closed(&first), closed(&second));
        let question = match kind {
            0 => Question::Fits { a: a.into(), b: b.into() },
            1 => Question::Same { a: a.into(), b: b.into() },
            2 => Question::Under { a, b },
            3 => Question::Kind { ty: a },
            4 => Question::Module { ty: a },
            5 => Question::LowerEndOutside { value: Interval::point(a), bound: b.into() },
            _ => Question::NeverSatisfies { value: Interval::point(a), wanted: b },
        };
        let Some(rigid) = rigid_answer(&types, &scratch, &|_| None, &question) else {
            return Ok(());
        };
        let bound = Levels::from_bound(&scratch, &[(0, first), (1, second)]);
        let (written, _) = question.substituted(&types, &scratch, &bound);
        prop_assert_eq!(Some(rigid), answer(&types, &scratch, &written), "{:?}", question);
    }

    /// A term applied to closed contents is the term rebound under them, and one whose names are
    /// left open reads neither level, each name its own open variable.
    #[test]
    fn a_term_applied_is_the_term_rebound(
        term in drawn(true),
        first in drawn(false),
        second in drawn(false),
    ) {
        let (registry, scratch) = (Bump::new(), Bump::new());
        let types = TestTypes::new(&registry).load();
        let symbols = SymbolInterner::new();
        let table = Table::new();
        let (names, laid) = (|id| table.get(id), |name: DerivedName<'_>| table.lay(name));
        let naming = naming(&table, &symbols, &names, &laid);
        let levels = level_names(&types, &symbols);
        let term: DeclaredType<Parametric> = built(&types, &scratch, &levels, &term).into();
        let closed = |drawn| types.concrete(built(&types, &scratch, &levels, drawn)).unwrap();
        let (first, second) = (closed(&first), closed(&second));
        let contents = [first, second].map(|closed| Some(Content::Type(Parametric::from(closed).into())));
        let applied = apply(&types, &scratch, naming, term, &named(contents));
        let bound = Levels::from_bound(&scratch, &[(0, first), (1, second)]);
        let expected = match rebound(&types, &scratch, term, &bound) {
            crate::scope::Closed::Same => term,
            crate::scope::Closed::Type(closed) => closed.into(),
            crate::scope::Closed::Partial(partial) => partial,
        };
        prop_assert_eq!(applied, Some(expected));

        let open = apply(&types, &scratch, naming, term, &named([None, None]))
            .expect("a term over levels applies with its names open");
        for variable in free_variables(&types, &scratch, open).iter() {
            let Variable::Derived { term, .. } = *variable else {
                prop_assert!(false, "{variable:?} is no open variable");
                continue;
            };
            let opened = table.get(term).map(|name| name.term);
            let is_open = matches!(opened, Some(DerivedTerm::Open { .. }));
            prop_assert!(is_open, "{:?} is no open variable", opened);
        }
    }

    /// Applying never grows past what the mentions report: the result is no deeper than the
    /// term's own depth, nor than each content's depth plus the term's where the term builds it.
    #[test]
    fn applying_never_grows_past_the_mentions(
        term in drawn(true),
        first in drawn(false),
        second in drawn(false),
    ) {
        let (registry, scratch) = (Bump::new(), Bump::new());
        let types = TestTypes::new(&registry).load();
        let symbols = SymbolInterner::new();
        let table = Table::new();
        let (names, laid) = (|id| table.get(id), |name: DerivedName<'_>| table.lay(name));
        let naming = naming(&table, &symbols, &names, &laid);
        let levels = level_names(&types, &symbols);
        let term = built(&types, &scratch, &levels, &term);
        let contents = [&first, &second].map(|drawn| built(&types, &scratch, &levels, drawn));
        let variables = [NameVariable::Level(0), NameVariable::Level(1)];
        let mentioned = mentions(&types, &scratch, naming.names, term.into(), &variables);
        let bound = contents.map(|ty| Some(Content::Type(ty.into())));
        let Some(DeclaredType::Type(applied)) = apply(&types, &scratch, naming, term.into(), &named(bound)) else {
            prop_assert!(false, "a term over levels applies to a type");
            return Ok(());
        };
        let mut limit = depth(&types, term);
        for (index, nesting) in &mentioned {
            let content = depth(&types, contents[*index]);
            limit = limit.max(match nesting {
                Nesting::Built => content + depth(&types, term),
                Nesting::Bare | Nesting::Alternative => content,
            });
        }
        prop_assert!(depth(&types, applied) <= limit, "{mentioned:?}");
    }
}

/// Two positions bound to one rigid are judged apart: a fits over them is undecided, though the
/// rigid fits itself.
#[test]
fn two_positions_bound_to_one_rigid_are_judged_apart() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let table = Table::new();
    let (names, laid) = (|id| table.get(id), |name: DerivedName<'_>| table.lay(name));
    let naming = naming(&table, &symbols, &names, &laid);
    let [elt, key] = level_names(&types, &symbols);
    let held = rigid(&types, &scratch, naming, site(9), KType::ANY);
    let Content::Type(DeclaredType::Type(rigid_ty)) = held else {
        panic!("a rigid is a type");
    };
    let itself = Question::Fits {
        a: rigid_ty.into(),
        b: rigid_ty.into(),
    };
    assert_eq!(
        answer(&types, &scratch, &itself),
        Some(Answer::Bool(true)),
        "a rigid fits itself"
    );
    let question = Question::Fits {
        a: elt.into(),
        b: key.into(),
    };
    let asked = ask(
        &types,
        &scratch,
        naming,
        None,
        &question,
        &named([Some(held); 2]),
    );
    assert_eq!(
        asked, None,
        "the two positions stand for unrelated unknowns"
    );
    let same = Question::Same {
        a: elt.into(),
        b: key.into(),
    };
    assert_eq!(
        ask(
            &types,
            &scratch,
            naming,
            None,
            &same,
            &named([Some(held); 2])
        ),
        None
    );
}

/// A piece of a name is a bare mention, a list of it a built one, and a union member of it an
/// alternative.
#[test]
fn a_piece_is_bare_a_list_built_and_a_member_an_alternative() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let symbols = SymbolInterner::new();
    let table = Table::new();
    let (names, laid) = (|id| table.get(id), |name: DerivedName<'_>| table.lay(name));
    let naming = naming(&table, &symbols, &names, &laid);
    let [elt, _] = level_names(&types, &symbols);
    let operands = [DeclaredType::Type(elt)];
    let piece = DerivedTerm::Output {
        rule: Rule::Piece(Step::Element),
        operands: &operands,
        output: 0,
    };
    let element = Spellings::new(&symbols).named((&types, &scratch), piece);
    table.lay(element);
    let variables = [NameVariable::Level(0)];
    let classed =
        |ty: Parametric| mentions(&types, &scratch, naming.names, ty.into(), &variables).to_vec();
    assert_eq!(classed(element.ty), [(0, Nesting::Bare)]);
    assert_eq!(classed(types.list(elt)), [(0, Nesting::Built)]);
    let member = types.union_of(&scratch, &[elt, KType::NUMBER.into()]);
    assert_eq!(classed(member), [(0, Nesting::Alternative)]);
    assert_eq!(classed(KType::NUMBER.into()), []);
}

/// The union of none is `Never`, and a union holding `Never` is the union without it.
#[test]
fn the_empty_union_is_no_alternative() {
    let (registry, scratch) = (Bump::new(), Bump::new());
    let types = TestTypes::new(&registry).load();
    let ty = |kt: KType| Content::Type(Parametric::from(kt).into());
    assert_eq!(union(&types, &scratch, &[]), ty(KType::NEVER));
    let never = union(&types, &scratch, &[]);
    assert_eq!(
        union(
            &types,
            &scratch,
            &[ty(KType::NUMBER), never, ty(KType::STR)]
        ),
        union(&types, &scratch, &[ty(KType::NUMBER), ty(KType::STR)]),
    );
}
