//! Hand-written cases: the toy domain itself, the design's examples, ownership and `BindsAfter`,
//! each refusal, and sharing one handle.

use bumpstorage::Bump;

use super::harness::Kept;
use super::toy::{Probe, Span, Step, Term, Toy, ToyAnswers, atom, list, name, pair, peel, union};
use crate::{
    Answers, Binding, BodyId, Domain, Doors, Edge, EdgeId, Facts, Graph, LayError, Malformed,
    NameId, Nesting, NodeId, OpenCaller, QuestionId, Site, Text,
};

/// A text under construction with its toy answer function alongside.
pub(super) struct Case {
    pub(super) text: Text<Toy>,
    pub(super) answers: ToyAnswers,
}

impl Case {
    pub(super) fn new() -> Self {
        Self {
            text: Text::new(),
            answers: ToyAnswers::default(),
        }
    }

    /// A question at `site` reading `reads`, whose plan probes each of `plan` in order, declaring
    /// each probe as an ask.
    pub(super) fn ask(
        &mut self,
        site: Site,
        reads: &[NameId],
        plan: &[(NameId, Probe)],
        arms: u32,
    ) -> QuestionId {
        let steps = plan.iter().map(|&(name, probe)| Step {
            names: vec![name],
            probe,
            guarded: false,
        });
        self.answers.plans.push((steps.collect(), arms));
        let question = self.text.question(site, reads, arms);
        for &(name, probe) in plan {
            self.text.may_ask(question, &[name], probe);
        }
        question
    }

    /// A call at `site` the text selects outright, into `callee`.
    pub(super) fn call(
        &mut self,
        site: Site,
        callee: BodyId,
        mapping: Vec<(NameId, Term)>,
    ) -> EdgeId {
        self.text.passes_at(site, callee, mapping)
    }

    pub(super) fn lay(self) -> (Graph<Toy>, ToyAnswers) {
        (self.text.lay(&Toy).expect("the case lays"), self.answers)
    }

    pub(super) fn refusal(self) -> LayError {
        self.text.lay(&Toy).err().expect("the case is refused")
    }
}

pub(super) fn bind(pairs: &[(NameId, Term)]) -> Binding<Toy> {
    let mut binding = Binding::new();
    for &(name, content) in pairs {
        binding.bind(name, content);
    }
    binding
}

fn span(lo: u32, hi: u32) -> Span {
    Span { lo, hi }
}

#[test]
fn toy_domain_probes_applies_and_mentions() {
    let x = NameId::new(0);
    let y = NameId::new(1);
    let ask = |probe: Probe, content: Option<Term>| Toy.ask(probe, &[content]);
    assert_eq!(ask(Probe::Depth, Some(list(list(atom(3))))), span(2, 2));
    assert_eq!(ask(Probe::Depth, Some(list(name(x)))), span(1, Span::MAX));
    assert_eq!(ask(Probe::Depth, None), span(0, Span::MAX));
    assert_eq!(ask(Probe::Leaf, Some(list(atom(3)))), span(3, 3));
    assert_eq!(ask(Probe::Leaf, Some(name(x))), span(0, 255));
    let same = |left, right| Toy.ask(Probe::Same, &[left, right]);
    assert_eq!(same(Some(atom(1)), Some(atom(1))), span(1, 1));
    assert_eq!(same(Some(atom(1)), Some(list(atom(1)))), span(0, 0));
    assert_eq!(same(Some(name(x)), Some(name(x))), span(0, 1));
    assert_eq!(same(None, Some(atom(1))), span(0, 1));

    let binding = bind(&[(x, list(atom(1)))]);
    assert_eq!(
        Toy.apply(list(name(x)), &binding),
        Some(list(list(atom(1))))
    );
    assert_eq!(Toy.apply(list(name(y)), &binding), Some(list(name(y))));
    assert_eq!(Toy.apply(peel(name(x)), &binding), Some(atom(1)));
    assert_eq!(Toy.apply(peel(name(y)), &binding), None);

    assert_eq!(Toy.mentions(atom(0)), vec![]);
    assert_eq!(Toy.mentions(name(x)), vec![(x, Nesting::Bare)]);
    assert_eq!(Toy.mentions(list(list(name(x)))), vec![(x, Nesting::Built)]);
    assert_eq!(
        Toy.mentions(pair(name(x), list(name(y)))),
        vec![(x, Nesting::Bare), (y, Nesting::Built)]
    );

    // A union is flat, without repeats or the empty union, and one of one is that one.
    let never = union(&[]);
    assert_eq!(Toy.union(&[]), never);
    assert_eq!(
        union(&[atom(1), atom(0), atom(1)]),
        union(&[atom(0), union(&[atom(1), never])])
    );
    assert_eq!(union(&[atom(2), never]), atom(2));
    let both = union(&[atom(0), list(atom(3))]);
    assert_eq!(ask(Probe::Leaf, Some(both)), span(0, 3));
    assert_eq!(ask(Probe::Depth, Some(both)), span(0, 1));
    // Applying leaves out an alternative bound to the empty union.
    let dropped = bind(&[(x, never), (y, never)]);
    assert_eq!(
        Toy.apply(union(&[name(x), atom(0)]), &dropped),
        Some(atom(0))
    );
    assert_eq!(Toy.apply(union(&[name(x), name(y)]), &dropped), Some(never));
    let mut mentioned = Toy.mentions(union(&[name(x), list(name(y))]));
    mentioned.sort_by_key(|&(name, _)| name);
    assert_eq!(
        mentioned,
        vec![(x, Nesting::Alternative), (y, Nesting::Built)]
    );

    // Every counted span round-trips through its index.
    for span in [span(0, 0), span(3, 255), span(255, Span::MAX), span(7, 7)] {
        let index = Toy.answer_index(Probe::Depth, span);
        assert!(index < Toy.answer_count(Probe::Depth));
        assert_eq!(Toy.nth_answer(Probe::Depth, index), span);
    }
}

/// `KEEP`, `PASS` and `WRAP` from the design: `WRAP` gives `KEEP`'s `Elem` the type `LIST OF
/// Item`, so an instance is made at a type still open over the caller's name, and binding the
/// caller's name makes the narrower one.
#[test]
fn keep_wrap_and_pass() {
    let mut case = Case::new();
    let root = case.text.root();
    let keep = case.text.body(root);
    let pass = case.text.body(root);
    let wrap = case.text.body(root);
    let elem = case.text.name(Site::Entry(keep));
    let pass_item = case.text.name(Site::Entry(pass));
    let wrap_item = case.text.name(Site::Entry(wrap));
    case.ask(Site::Entry(keep), &[elem], &[(elem, Probe::Depth)], 0);
    case.call(Site::Entry(pass), keep, vec![(elem, name(pass_item))]);
    case.call(Site::Entry(wrap), keep, vec![(elem, list(name(wrap_item)))]);
    let (graph, mut answers) = case.lay();
    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    let keep_entry = graph.entry(keep);

    kept.instance(graph.entry(wrap), &Binding::new(), &mut answers);
    let open_view = bind(&[(elem, list(name(wrap_item)))]);
    let open = kept.found(keep_entry, &open_view, &mut answers);
    assert_eq!(open.expect("made")[0].span, span(1, Span::MAX));

    kept.instance(
        graph.entry(wrap),
        &bind(&[(wrap_item, atom(5))]),
        &mut answers,
    );
    let closed_view = bind(&[(elem, list(atom(5)))]);
    let closed = kept.found(keep_entry, &closed_view, &mut answers);
    assert_eq!(closed.expect("made")[0].span, span(1, 1));
    assert_ne!(
        kept.instance(keep_entry, &open_view, &mut answers),
        kept.instance(keep_entry, &closed_view, &mut answers)
    );

    kept.instance(
        graph.entry(pass),
        &bind(&[(pass_item, atom(5))]),
        &mut answers,
    );
    let passed = kept.found(keep_entry, &bind(&[(elem, atom(5))]), &mut answers);
    assert_eq!(passed.expect("made")[0].span, span(0, 0));
}

/// `DEEPEN` binds its name one level deeper on each turn.
#[test]
fn deepen_is_refused() {
    let mut case = Case::new();
    let deepen = case.text.body(case.text.root());
    let item = case.text.name(Site::Entry(deepen));
    let edge = case.call(Site::Entry(deepen), deepen, vec![(item, list(name(item)))]);
    assert_eq!(
        case.refusal(),
        LayError::Grows {
            edge: Edge::Passes(edge),
            name: item
        }
    );
}

/// `MIX`'s two calls each repeat alone; the swap followed by the wrap builds `Right` over itself.
#[test]
fn mix_is_refused_though_each_call_repeats() {
    let mix = |swap: bool, wrap: bool| {
        let mut case = Case::new();
        let body = case.text.body(case.text.root());
        let left = case.text.name(Site::Entry(body));
        let right = case.text.name(Site::Entry(body));
        let swapped = case.text.statement(body);
        let last = case.text.statement(body);
        let mut edges = Vec::new();
        if swap {
            let mapping = vec![(left, name(right)), (right, name(left))];
            edges.push(case.call(swapped, body, mapping));
        }
        if wrap {
            let mapping = vec![(left, name(left)), (right, list(name(left)))];
            edges.push(case.call(last, body, mapping));
        }
        (case.text.lay(&Toy).err(), edges, right)
    };
    assert_eq!(mix(true, false).0, None);
    assert_eq!(mix(false, true).0, None);
    let (refusal, edges, right) = mix(true, true);
    assert_eq!(
        refusal,
        Some(LayError::Grows {
            edge: Edge::Passes(edges[1]),
            name: right
        })
    );
}

/// A rename cycle repeats after two turns, and a cycle that passes fixed content at one call
/// repeats too. Bound, the rename cycle makes one instance per order of the pair.
#[test]
fn repeating_cycles_lay_and_bind_finitely() {
    let mut case = Case::new();
    let root = case.text.root();
    let body = case.text.body(root);
    let x = case.text.name(Site::Entry(body));
    let y = case.text.name(Site::Entry(body));
    case.call(Site::Entry(root), body, vec![(x, atom(1)), (y, atom(2))]);
    let probes = [(x, Probe::Leaf), (y, Probe::Leaf)];
    let question = case.ask(Site::Entry(body), &[x, y], &probes, 1);
    case.text
        .passes(question, 0, body, vec![(x, name(y)), (y, name(x))]);
    let (graph, mut answers) = case.lay();
    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    kept.instance(graph.root(), &Binding::new(), &mut answers);
    assert_eq!(kept.resting_at(graph.root()), 1);
    assert_eq!(kept.resting_at(graph.entry(body)), 2);

    let mut case = Case::new();
    let root = case.text.root();
    let first = case.text.body(root);
    let second = case.text.body(root);
    let x = case.text.name(Site::Entry(first));
    let y = case.text.name(Site::Entry(second));
    case.call(Site::Entry(first), second, vec![(y, list(name(x)))]);
    case.call(Site::Entry(second), first, vec![(x, atom(0))]);
    case.lay();
}

/// `GROW` at `Number` selects the first `HOLD`, whose call back to `GROW` selects the second, and
/// the chain stops. The check counts every laid arm, so it refuses `GROW` all the same.
#[test]
fn hold_and_grow_is_refused_over_every_laid_arm() {
    let mut case = Case::new();
    let root = case.text.root();
    let hold_number = case.text.body(root);
    let hold_list = case.text.body(root);
    let grow = case.text.body(root);
    let first = case.text.name(Site::Entry(hold_number));
    let second = case.text.name(Site::Entry(hold_list));
    let item = case.text.name(Site::Entry(grow));
    let select = case.ask(Site::Entry(grow), &[item], &[(item, Probe::Depth)], 2);
    let wraps = case
        .text
        .passes(select, 0, hold_number, vec![(first, list(name(item)))]);
    case.text
        .passes(select, 1, hold_list, vec![(second, list(name(item)))]);
    case.call(Site::Entry(hold_number), grow, vec![(item, name(first))]);
    // `Item` and the first `HOLD`'s `Elem` share a component; the call that wraps `Item` into it
    // is the first built arc inside it.
    assert_eq!(
        case.refusal(),
        LayError::Grows {
            edge: Edge::Passes(wraps),
            name: first
        }
    );
}

/// Taking a part of a name is bare, so a self-call that peels lays, and its recursion stops where
/// the part cannot be computed or the content stops changing.
#[test]
fn peel_lays_and_terminates() {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let item = case.text.name(Site::Entry(body));
    case.call(Site::Entry(body), body, vec![(item, peel(name(item)))]);
    case.ask(Site::Entry(body), &[item], &[(item, Probe::Depth)], 0);
    let (graph, mut answers) = case.lay();
    let entry = graph.entry(body);

    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    kept.instance(entry, &Binding::new(), &mut answers);
    assert_eq!(kept.resting(), 1);

    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    kept.instance(entry, &bind(&[(item, list(list(atom(1))))]), &mut answers);
    let depths: Vec<Span> = [list(list(atom(1))), list(atom(1)), atom(1)]
        .into_iter()
        .map(|item_is| {
            let found = kept.found(entry, &bind(&[(item, item_is)]), &mut answers);
            found.expect("filed")[0].span
        })
        .collect();
    assert_eq!(depths, vec![span(2, 2), span(1, 1), span(0, 0)]);
    assert_eq!(kept.resting_at(entry), 3);
}

/// A mapping that mentions two names draws an arc from each, bare and built; growth follows the
/// built one.
#[test]
fn pair_grows_through_its_built_arc_only() {
    let pair = |into_y: bool| {
        let mut case = Case::new();
        let body = case.text.body(case.text.root());
        let x = case.text.name(Site::Entry(body));
        let y = case.text.name(Site::Entry(body));
        let term = pair(name(x), list(name(y)));
        let mapped = if into_y { y } else { x };
        let edge = case.call(Site::Entry(body), body, vec![(mapped, term)]);
        (case.text.lay(&Toy).err(), edge, y)
    };
    // Into `x`: a bare self-arc on `x` and a built arc from `y`, which nothing maps back to.
    assert_eq!(pair(false).0, None);
    let (refusal, edge, y) = pair(true);
    let edge = Edge::Passes(edge);
    assert_eq!(refusal, Some(LayError::Grows { edge, name: y }));
}

/// A statement read only beside a later one is a node that owns nothing.
#[test]
fn a_statement_read_beside_a_later_one_owns_nothing() {
    let mut case = Case::new();
    let root = case.text.root();
    let sites: Vec<Site> = (0..4).map(|_| case.text.statement(root)).collect();
    let early = case.text.name(sites[1]);
    let late = case.text.name(sites[3]);
    case.ask(sites[3], &[early, late], &[], 0);
    let (graph, _) = case.lay();
    let early_node = graph.node_at(sites[1]).expect("a node");
    let late_node = graph.node_at(sites[3]).expect("a node");
    assert!(graph.node(early_node).owns().is_empty());
    assert_eq!(graph.node(late_node).owns(), &[QuestionId::new(0)]);
    assert_eq!(
        graph
            .node(late_node)
            .binds_after()
            .map(|edge| edge.earlier()),
        Some(early_node)
    );
    assert_eq!(graph.node_at(sites[2]), None);
}

/// The root's entry binds no site name, and every question reads one.
#[test]
fn the_root_binds_nothing_and_every_question_reads() {
    let mut case = Case::new();
    let root = case.text.root();
    let name = case.text.name(Site::Entry(root));
    assert_eq!(
        case.refusal(),
        LayError::Malformed(Malformed::RootBindsName { name })
    );

    let mut case = Case::new();
    let inner = case.text.body(case.text.root());
    let statement = case.text.statement(inner);
    let question = case.ask(statement, &[], &[], 0);
    assert_eq!(
        case.refusal(),
        LayError::Malformed(Malformed::ReadsNothing { question })
    );
}

/// `PACK`'s result is bound at its entry, and `packed`, taken through the call `PACK 5`, has an
/// empty base: the root binds it, owns the call with no input, and owns `SHOW (packed.sample)`.
#[test]
fn the_root_owns_what_reads_only_what_its_calls_return() {
    let mut case = Case::new();
    let root = case.text.root();
    let pack = case.text.body(root);
    let x = case.text.name(Site::Entry(pack));
    let result = case.text.answered_name();
    let boxed = case.ask(Site::Entry(pack), &[x], &[(x, Probe::Depth)], 2);
    case.text.answers(boxed, 0, result, atom(1));
    case.text.answers(boxed, 1, result, list(atom(1)));
    let first = case.text.statement(root);
    let call = case.call(first, pack, vec![(x, atom(5))]);
    let packed = case.text.answered_name();
    case.text.answers_through(call, packed, result);
    let second = case.text.statement(root);
    let show = case.ask(second, &[packed], &[(packed, Probe::Depth)], 0);
    let (graph, _) = case.lay();
    assert_eq!(graph.node(graph.entry(pack)).answered(), &[result]);
    assert_eq!(graph.node(graph.root()).answered(), &[packed]);
    assert_eq!(graph.edge(call).owner(), graph.root());
    assert_eq!(graph.question(show).owner(), graph.root());
    assert_eq!(graph.node(graph.root()).follows(), &[call]);
    assert_eq!(graph.node_at(first), None);
}

#[test]
fn an_inner_question_on_an_outer_name_is_owned_by_the_outer_entry() {
    let mut case = Case::new();
    let outer = case.text.body(case.text.root());
    let inner = case.text.body(outer);
    let x = case.text.name(Site::Entry(outer));
    case.text.name(Site::Entry(inner));
    case.ask(Site::Entry(inner), &[x], &[], 0);
    let (graph, _) = case.lay();
    assert_eq!(
        graph.question(QuestionId::new(0)).owner(),
        graph.entry(outer)
    );
    assert!(graph.node(graph.entry(inner)).owns().is_empty());
}

/// A staged body's question and call on an outer name are owned at the staged body's entry, and
/// one in a body nested in it at that entry too; the outer entry owns neither.
#[test]
fn what_a_staged_body_asks_on_an_outer_name_is_owned_at_its_entry() {
    let mut case = Case::new();
    let outer = case.text.body(case.text.root());
    let staged = case.text.body(outer);
    let nested = case.text.body(staged);
    let callee = case.text.body(case.text.root());
    case.text.staged(staged);
    let x = case.text.name(Site::Entry(outer));
    let asked = case.ask(Site::Entry(staged), &[x], &[], 0);
    let deeper = case.ask(Site::Entry(nested), &[x], &[], 0);
    let at = case.text.statement(staged);
    let call = case.call(at, callee, vec![]);
    let (graph, _) = case.lay();
    let entry = graph.entry(staged);
    assert_eq!(graph.question(asked).owner(), entry);
    assert_eq!(graph.question(deeper).owner(), entry);
    assert_eq!(graph.edge(call).owner(), entry);
    assert!(graph.node(graph.entry(outer)).owns().is_empty());
}

#[test]
fn one_body_splits_its_questions_across_its_entry_and_statements() {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let entry = case.text.name(Site::Entry(body));
    let first = case.text.statement(body);
    let second = case.text.statement(body);
    let a = case.text.name(first);
    let b = case.text.name(second);
    case.ask(second, &[entry], &[], 0);
    case.ask(second, &[entry, a], &[], 0);
    case.ask(second, &[a, b], &[], 0);
    let (graph, _) = case.lay();
    let owners: Vec<NodeId> = graph
        .questions()
        .iter()
        .map(|question| question.owner())
        .collect();
    let expected =
        [Site::Entry(body), first, second].map(|site| graph.node_at(site).expect("a node"));
    assert_eq!(owners, expected);
}

#[test]
fn a_statement_no_question_reads_is_not_a_node() {
    let mut case = Case::new();
    let root = case.text.root();
    let statement = case.text.statement(root);
    case.text.name(statement);
    let (graph, _) = case.lay();
    assert_eq!(graph.node_at(statement), None);
    assert_eq!(graph.nodes().len(), 1);
}

/// An inner entry binds just after the encloser's latest statement it captures, including one a
/// body inside it reads; one capturing nothing binds just after the encloser's entry.
#[test]
fn an_inner_entry_binds_after_what_it_captures() {
    let mut case = Case::new();
    let outer = case.text.body(case.text.root());
    let statements: Vec<Site> = (0..4).map(|_| case.text.statement(outer)).collect();
    let names: Vec<NameId> = statements
        .iter()
        .map(|&site| case.text.name(site))
        .collect();
    case.ask(statements[3], &[names[1], names[3]], &[], 0);
    let capturing = case.text.body(outer);
    let deeper = case.text.body(capturing);
    case.ask(Site::Entry(deeper), &[names[2]], &[], 0);
    let free = case.text.body(outer);
    let (graph, _) = case.lay();
    let earlier = |body: BodyId| {
        graph
            .node(graph.entry(body))
            .binds_after()
            .map(|edge| edge.earlier())
    };
    assert_eq!(earlier(capturing), graph.node_at(statements[2]));
    assert_eq!(earlier(deeper), Some(graph.entry(capturing)));
    assert_eq!(earlier(free), Some(graph.entry(outer)));
    assert_eq!(earlier(outer), Some(graph.root()));
    let statement_3 = graph.node_at(statements[3]).expect("a node");
    assert_eq!(
        graph
            .node(statement_3)
            .binds_after()
            .map(|edge| edge.earlier()),
        graph.node_at(statements[2])
    );
    assert!(graph.site_le(statements[2], Site::Entry(deeper)));
    assert!(!graph.site_le(statements[3], Site::Entry(deeper)));
    assert!(!graph.site_le(statements[0], Site::Entry(free)));
}

#[test]
fn each_malformed_text_is_refused() {
    // A read of the root's statement from its own entry, and of a sibling body's name.
    let mut case = Case::new();
    let root = case.text.root();
    let statement = case.text.statement(root);
    let late = case.text.name(statement);
    let question = case.ask(Site::Entry(root), &[late], &[], 0);
    assert_eq!(
        case.refusal(),
        LayError::Malformed(Malformed::ReadOffChain {
            question,
            name: late
        })
    );

    let mut case = Case::new();
    let root = case.text.root();
    let left = case.text.body(root);
    let right = case.text.body(root);
    let x = case.text.name(Site::Entry(left));
    let question = case.ask(Site::Entry(right), &[x], &[], 0);
    assert_eq!(
        case.refusal(),
        LayError::Malformed(Malformed::ReadOffChain { question, name: x })
    );

    // A call on an arm its question does not have.
    let mut case = Case::new();
    let root = case.text.root();
    let callee = case.text.body(root);
    let statement = case.text.statement(root);
    let read = case.text.name(statement);
    let question = case.ask(statement, &[read], &[], 1);
    let edge = case.text.passes(question, 1, callee, vec![]);
    let edge = Edge::Passes(edge);
    assert_eq!(
        case.refusal(),
        LayError::Malformed(Malformed::ArmOutOfRange { edge })
    );

    // Mapping a callee's statement name, mapping a name twice, mentioning an unread name.
    let mapped = |mapping: fn(NameId, NameId, NameId) -> Vec<(NameId, Term)>| {
        let mut case = Case::new();
        let root = case.text.root();
        let statement = case.text.statement(root);
        let caller = case.text.name(statement);
        let other = case.text.name(statement);
        let callee = case.text.body(root);
        let entry = case.text.name(Site::Entry(callee));
        let statement_inside = case.text.statement(callee);
        let local = case.text.name(statement_inside);
        let question = case.ask(statement, &[other], &[], 1);
        let edge = case
            .text
            .passes(question, 0, callee, mapping(caller, entry, local));
        (case.refusal(), edge, caller, entry, local)
    };
    let (refusal, edge, _, _, local) = mapped(|_, _, local| vec![(local, atom(0))]);
    assert_eq!(
        refusal,
        LayError::Malformed(Malformed::MapsUnboundName { edge, name: local })
    );
    let (refusal, edge, _, entry, _) =
        mapped(|_, entry, _| vec![(entry, atom(0)), (entry, atom(1))]);
    assert_eq!(
        refusal,
        LayError::Malformed(Malformed::MapsNameTwice { edge, name: entry })
    );
    let (refusal, edge, caller, _, _) = mapped(|caller, entry, _| vec![(entry, name(caller))]);
    let edge = Edge::Passes(edge);
    assert_eq!(
        refusal,
        LayError::Malformed(Malformed::MentionsUnread { edge, name: caller })
    );

    // A call on a site mentioning a name bound after it.
    let mut case = Case::new();
    let root = case.text.root();
    let callee = case.text.body(root);
    let entry = case.text.name(Site::Entry(callee));
    let early = case.text.statement(root);
    let late = case.text.statement(root);
    let later = case.text.name(late);
    let edge = Edge::Passes(case.call(early, callee, vec![(entry, name(later))]));
    assert_eq!(
        case.refusal(),
        LayError::Malformed(Malformed::MentionsOffChain { edge, name: later })
    );

    // A term on a site mentioning a name bound after it.
    let mut case = Case::new();
    let root = case.text.root();
    let early = case.text.statement(root);
    let late = case.text.statement(root);
    let later = case.text.name(late);
    let typed = case.text.answered_name();
    let edge = Edge::Answers(case.text.answers_at(early, typed, name(later)));
    assert_eq!(
        case.refusal(),
        LayError::Malformed(Malformed::MentionsOffChain { edge, name: later })
    );
}

/// A body `B` with an entry name `x`, and a question at its entry reading `x` with two arms.
struct Answering {
    case: Case,
    body: BodyId,
    x: NameId,
    question: QuestionId,
}

fn answering() -> Answering {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(body));
    let question = case.ask(Site::Entry(body), &[x], &[(x, Probe::Depth)], 2);
    Answering {
        case,
        body,
        x,
        question,
    }
}

#[test]
fn each_malformed_answers_name_edge_is_refused() {
    let refused = |malformed: Malformed, case: Case| {
        assert_eq!(case.refusal(), LayError::Malformed(malformed));
    };

    // Typing a site name; an arm out of range; content mentioning what the question does not read.
    let Answering {
        mut case,
        body,
        question,
        ..
    } = answering();
    let y = case.text.name(Site::Entry(body));
    let edge = case.text.answers(question, 0, y, atom(0));
    refused(Malformed::AnswersSiteName { edge, name: y }, case);

    let Answering {
        mut case, question, ..
    } = answering();
    let a = case.text.answered_name();
    let edge = Edge::Answers(case.text.answers(question, 2, a, atom(0)));
    refused(Malformed::ArmOutOfRange { edge }, case);

    let Answering {
        mut case,
        body,
        question,
        ..
    } = answering();
    let y = case.text.name(Site::Entry(body));
    let a = case.text.answered_name();
    let edge = Edge::Answers(case.text.answers(question, 0, a, name(y)));
    refused(Malformed::MentionsUnread { edge, name: y }, case);

    // Two edges on one arm, and an edge through a call on a site beside another.
    let Answering {
        mut case, question, ..
    } = answering();
    let a = case.text.answered_name();
    case.text.answers(question, 0, a, atom(0));
    let edge = case.text.answers(question, 0, a, atom(1));
    refused(Malformed::AnswersNameTwice { edge, name: a }, case);

    let Answering {
        mut case,
        body,
        x,
        question,
    } = answering();
    let a = case.text.answered_name();
    case.text.answers(question, 0, a, atom(0));
    let statement = case.text.statement(body);
    let call = case.call(statement, body, vec![(x, name(x))]);
    let edge = case.text.answers_through(call, a, x);
    refused(Malformed::AnswersNameTwice { edge, name: a }, case);

    // A term on a site beside an edge on an arm.
    let Answering {
        mut case,
        body,
        x,
        question,
    } = answering();
    let a = case.text.answered_name();
    case.text.answers(question, 0, a, atom(0));
    let edge = case.text.answers_at(Site::Entry(body), a, name(x));
    refused(Malformed::AnswersNameTwice { edge, name: a }, case);

    // An answered name no edge types.
    let Answering { mut case, .. } = answering();
    let a = case.text.answered_name();
    refused(Malformed::Unanswered { name: a }, case);

    // A deciding question that reads its own name, directly and through another.
    let Answering {
        mut case, body, x, ..
    } = answering();
    let a = case.text.answered_name();
    let question = case.ask(Site::Entry(body), &[x, a], &[], 1);
    case.text.answers(question, 0, a, atom(0));
    refused(Malformed::AnswersOwnRead { name: a }, case);

    let Answering {
        mut case, body, x, ..
    } = answering();
    let a = case.text.answered_name();
    let b = case.text.answered_name();
    let deciding_a = case.ask(Site::Entry(body), &[x, b], &[], 1);
    let deciding_b = case.ask(Site::Entry(body), &[x, a], &[], 1);
    case.text.answers(deciding_a, 0, a, atom(0));
    case.text.answers(deciding_b, 0, b, atom(0));
    refused(Malformed::AnswersOwnRead { name: a }, case);

    // A term on a site reading its own name through another.
    let Answering {
        mut case, body, x, ..
    } = answering();
    let a = case.text.answered_name();
    let b = case.text.answered_name();
    case.text
        .answers_at(Site::Entry(body), a, union(&[name(x), name(b)]));
    case.text.answers_at(Site::Entry(body), b, name(a));
    refused(Malformed::AnswersOwnRead { name: a }, case);

    // Taking a name bound in a sibling body, a site name and then an answered one.
    let Answering {
        mut case, body, x, ..
    } = answering();
    let root = case.text.root();
    let sibling = case.text.body(root);
    let z = case.text.name(Site::Entry(sibling));
    let statement = case.text.statement(root);
    let call = case.call(statement, body, vec![(x, atom(1))]);
    let taker = case.text.answered_name();
    let edge = case.text.answers_through(call, taker, z);
    refused(Malformed::TakesOffCallee { edge, name: z }, case);

    let Answering {
        mut case, body, x, ..
    } = answering();
    let root = case.text.root();
    let sibling = case.text.body(root);
    let z = case.text.name(Site::Entry(sibling));
    let deciding = case.ask(Site::Entry(sibling), &[z], &[(z, Probe::Depth)], 1);
    let answered_there = case.text.answered_name();
    case.text.answers(deciding, 0, answered_there, atom(0));
    let statement = case.text.statement(root);
    let call = case.call(statement, body, vec![(x, atom(1))]);
    let taker = case.text.answered_name();
    let edge = case.text.answers_through(call, taker, answered_there);
    refused(
        Malformed::TakesOffCallee {
            edge,
            name: answered_there,
        },
        case,
    );
}

/// One answered name typed by a fixed term on one arm and through a call on the other lays: both
/// edges hang on different arms of its deciding question.
#[test]
fn a_fixed_arm_beside_a_call_arm_lays() {
    let Answering {
        mut case,
        body,
        x,
        question,
    } = answering();
    let callee = case.text.body(case.text.root());
    let entry = case.text.name(Site::Entry(callee));
    let result = case.text.answered_name();
    let deciding = case.ask(Site::Entry(callee), &[entry], &[(entry, Probe::Leaf)], 1);
    case.text.answers(deciding, 0, result, list(name(entry)));
    let a = case.text.answered_name();
    case.text.answers(question, 0, a, atom(0));
    let call = case
        .text
        .passes(question, 1, callee, vec![(entry, name(x))]);
    case.text.answers_through(call, a, result);
    let (graph, _) = case.lay();
    assert_eq!(graph.node(graph.entry(body)).answered(), &[a]);
    assert_eq!(graph.node(graph.entry(callee)).answered(), &[result]);
}

/// An answer function that asks about a name its question does not read.
struct Overreaching(NameId);

impl Answers<Toy> for Overreaching {
    fn answer(&mut self, _: QuestionId, doors: &mut Doors<'_, Toy>) -> super::toy::Verdict {
        super::toy::Verdict {
            span: doors.ask(&[self.0], Probe::Depth),
            arm: None,
        }
    }
}

#[test]
#[should_panic(expected = "which it does not read")]
fn a_read_the_question_did_not_declare_panics() {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(body));
    let y = case.text.name(Site::Entry(body));
    case.ask(Site::Entry(body), &[y], &[], 0);
    let (graph, _) = case.lay();
    graph.afresh(
        &Toy,
        graph.entry(body),
        &Binding::new(),
        &mut Overreaching(x),
    );
}

/// Two bindings share an instance where they differ only in a name no question reads, or where
/// they answer every `NameQuestion` alike.
#[test]
fn bindings_that_answer_alike_share_one_instance() {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(body));
    let unread = case.text.name(Site::Entry(body));
    case.ask(Site::Entry(body), &[x], &[(x, Probe::Leaf)], 0);
    let (graph, mut answers) = case.lay();
    let entry = graph.entry(body);
    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    let first = kept.instance(
        entry,
        &bind(&[(x, atom(1)), (unread, atom(0))]),
        &mut answers,
    );
    let second = kept.instance(
        entry,
        &bind(&[(x, atom(1)), (unread, atom(3))]),
        &mut answers,
    );
    let deeper = kept.instance(entry, &bind(&[(x, list(list(atom(1))))]), &mut answers);
    assert_eq!([&second, &deeper], [&first, &first]);
    let other = kept.instance(entry, &bind(&[(x, atom(2))]), &mut answers);
    assert_ne!(other, first);
    assert_eq!(kept.resting(), 2);
}

#[test]
fn the_open_caller_mark_reads_back_from_its_entry() {
    let mut text: Text<Toy> = Text::new();
    let root = text.root();
    let open = text.body(root);
    let closed = text.body(root);
    text.open_caller(open);
    let graph = text.lay(&Toy).expect("lays");
    assert_eq!(
        graph.node(graph.entry(open)).open_caller(),
        Some(OpenCaller)
    );
    assert_eq!(graph.node(graph.entry(closed)).open_caller(), None);
    assert_eq!(graph.node(graph.root()).open_caller(), None);
}
