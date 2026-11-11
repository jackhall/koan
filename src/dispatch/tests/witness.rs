//! A witnessing walk: the first walk of a resumption that witnesses a binding, which leaves the
//! binding's type names rigid and asks each discrete door's question under the witness
//! ([build/README.md § Resumption](../../build/README.md#resumption)).

use crate::scope::{Answer, LoadFacts, Mark, OpenKind, Segment, UnitId, Unknown};
use crate::type_lattice::{DeclaredType, KType, Parametric};

use super::resumption::{COUNTER, Door, quantified, resumed, unknown};

/// `PEEK`, whose overloads tell a list of strings from any other list. A list's type node is no
/// variable, so a reader of `xs :(LIST OF Elt)` reaches only discrete questions on `Elt`.
const PEEK: &str = "EXPR #(PEEK xs :(LIST OF Any)) -> Str = #(\"a\")\n\
                    EXPR #(PEEK xs :(LIST OF Str)) -> Number = #(1)\n";

/// The answered marks on `unknown` among the records `facts` itself laid for `segment`'s units,
/// by kind beside the answer, sorted.
fn answered(facts: &LoadFacts<'_>, segment: Segment, unknown: Unknown) -> Vec<(OpenKind, String)> {
    let mut found: Vec<_> = (facts.own_units().iter())
        .filter(|(unit, _)| unit.segment == segment)
        .flat_map(|(_, (_, frontier))| frontier.marks())
        .filter(|noted| noted.unknown == unknown)
        .filter_map(|noted| match noted.mark {
            Mark::Open(open) => open.answered.map(|answered| (open.kind, answered.answer)),
            Mark::Named => None,
        })
        .map(|(kind, answer): (OpenKind, Answer)| (kind, format!("{answer:?}")))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Every type the records `facts` itself laid for `segment`'s units name.
fn named(facts: &LoadFacts<'_>, segment: Segment) -> Vec<DeclaredType<Parametric>> {
    let mut named = Vec::new();
    for (unit, (record, frontier)) in facts.own_units().iter() {
        if unit.segment == segment {
            record.each_type(&mut |ty| named.push(ty));
            frontier.each_type(&mut |ty| named.push(ty));
        }
    }
    named
}

/// A witnessing walk notes each discrete door's answer under the witness beside its question, and
/// lays no record naming the witness's type for the name it leaves rigid.
#[test]
fn a_witnessing_walk_notes_its_answers_and_names_no_witnessed_type() {
    let source =
        format!("{PEEK}EXPR FOR ALL #[Elt] #(SEE xs :(LIST OF Elt)) -> Any = #(PEEK xs)\n");
    let (number, str, named_number) = resumed(&source, |door: &Door<'_, '_>| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let witnessed = |closed| {
            (door.witness(door.binding(&[(elt, closed)]))).expect("the witness refuses nothing")
        };
        let (under_number, under_str) = (witnessed(KType::NUMBER), witnessed(KType::STR));
        let named_number = named(under_number, f.segment()).contains(&KType::NUMBER.into());
        (
            answered(under_number, f.segment(), elt),
            answered(under_str, f.segment(), elt),
            named_number,
        )
    });
    assert!(!number.is_empty() && !str.is_empty(), "{number:?} {str:?}");
    assert_ne!(
        number, str,
        "`PEEK :Str` is never under `Number` and always under `Str`"
    );
    assert!(
        number.contains(&(OpenKind::Verdict, "Verdict(Never)".into())),
        "{number:?}"
    );
    assert!(
        str.contains(&(OpenKind::Verdict, "Solved(Always)".into())),
        "{str:?}"
    );
    assert!(!named_number, "no record names the witness's type");
}

/// A read by content holds a witnessing walk's unit on each witnessed name: a path read through a
/// module binder's class gathers nothing where it misses, so the walk cannot tell what a binding
/// would find.
#[test]
fn a_read_by_content_holds_a_witnessing_walks_unit() {
    let source = format!(
        "{COUNTER}{PEEK}EXPR FOR ALL #[Elt] #(WRAP xs :(LIST OF Elt) AND m :Counter) -> Any = \
         #(\n  LET s = {{a = (PEEK xs), b = m.zero}}\n  s\n)"
    );
    let held = resumed(&source, |door| {
        let f = quantified(door);
        let elt = unknown(door.symbols(), f, "Elt");
        let witnessed = (door.witness(door.binding(&[(elt, KType::NUMBER)])))
            .expect("the witness refuses nothing");
        let s = super::resumption::unit_of(door, f, "s");
        holds_on(witnessed, s, elt)
    });
    assert!(held, "`s` reads `m.zero` by content, so it holds on `Elt`");
}

/// Whether `facts`' record of `unit` holds on `unknown`: an undecided module-content mark on it.
fn holds_on(facts: &LoadFacts<'_>, unit: UnitId, unknown: Unknown) -> bool {
    let (_, frontier) = facts.unit(unit).expect("the walk ran the unit");
    (frontier.marks().iter()).any(|noted| {
        noted.unknown == unknown
            && matches!(noted.mark, Mark::Open(open)
                if open.kind == OpenKind::ModuleContent && open.answered.is_none())
    })
}

/// `SUBJECT` over `x :Elt` and `xs :(LIST OF Elt)`, running `body`, after `PEEK`.
fn subject_over(body: &str) -> String {
    format!(
        "{PEEK}EXPR #(ONLY ys :(LIST OF Number)) -> Any = #(ys)\n\
         EXPR FOR ALL #[Elt] #(SUBJECT x :Elt xs :(LIST OF Elt)) -> Any = #(\n  {body}\n)\n"
    )
}

/// Every fact of `SUBJECT`'s body a cell over `layer` reads, but of `Elt`'s binder.
fn rendered_over(door: &Door<'_, '_>, layer: crate::scope::BuildLayer<'_>) -> Vec<String> {
    let layer = crate::memory::resident(door.bump, layer);
    let facts = crate::scope::Facts::under(door.load(), Some(layer));
    super::resumption::laws::render_over(&facts, door.types, door.symbols(), &["Elt".to_string()])
}

/// A binding over a witnessed layer whose questions it keeps reads as its closed walk does, record
/// by record: the closing walk over the layer re-runs what holds — the entry unit, which seeds
/// `Elt` as a value, and a reader of `x`, whose node it reads — and shares the rest.
#[test]
fn a_binding_over_a_witnessed_layer_it_keeps_reads_as_its_closed_walk() {
    for body in [
        "LET a = (PEEK xs)\n  LET b = a\n  b",
        "LET a = (PEEK xs)\n  LET c = x\n  a",
    ] {
        let source = subject_over(body);
        let (over, closed, kept, shared) = resumed(&source, |door| {
            let shape = super::resumption::subject(door.root());
            let elt = unknown(door.symbols(), shape, "Elt");
            let under = |closed| door.binding(&[(elt, closed)]);
            let first = (door.resumption(under(KType::NUMBER), None, false)).expect("no refusal");
            let witnessed = first.witnessed.expect("the first resumption witnesses");
            let kept = witnessed.admission().conditions().len();
            let second = door.resumption(under(KType::BOOL), Some(witnessed), false);
            let second = second.expect("no refusal");
            let restart = (door.resumption(under(KType::BOOL), None, true)).expect("no refusal");
            let a = super::resumption::unit_of(door, shape, "a");
            let shared = second.layer.unit_of(a).is_none();
            (
                rendered_over(door, second.layer),
                rendered_over(door, restart.layer),
                kept,
                shared,
            )
        });
        assert!(kept > 0, "the witness answered `PEEK xs` in\n{source}");
        assert!(shared, "`a`'s verdict is kept, so it stands in\n{source}");
        assert!(!over.is_empty(), "the render reads `SUBJECT`'s facts");
        assert_eq!(over, closed, "in\n{source}");
    }
}

/// A binding over a witnessed layer that keeps none of the questions a unit reached re-runs it,
/// and reads as its closed walk does.
#[test]
fn a_binding_that_keeps_no_question_re_runs_its_unit() {
    let source = subject_over("LET a = (PEEK xs)\n  a");
    let (over, closed, rerun) = resumed(&source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let under = |closed| door.binding(&[(elt, closed)]);
        let first = (door.resumption(under(KType::NUMBER), None, false)).expect("no refusal");
        let witnessed = first.witnessed.expect("the first resumption witnesses");
        let second = door.resumption(under(KType::STR), Some(witnessed), false);
        let second = second.expect("no refusal");
        let restart = (door.resumption(under(KType::STR), None, true)).expect("no refusal");
        let a = super::resumption::unit_of(door, shape, "a");
        (
            rendered_over(door, second.layer),
            rendered_over(door, restart.layer),
            second.layer.unit_of(a).is_some(),
        )
    });
    assert!(
        rerun,
        "`PEEK :(LIST OF Str)` is never under `Number` and always under `Str`"
    );
    assert!(!over.is_empty(), "the render reads `SUBJECT`'s facts");
    assert_eq!(over, closed);
}

/// A first walk that refuses is dropped and the event walked closed: the refusal is worded as the
/// closed walk words it.
#[test]
fn a_witnessing_walk_that_refuses_is_worded_by_its_closed_walk() {
    let source = subject_over("LET a = (ONLY xs)\n  a");
    let (witnessed, closed) = resumed(&source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let under = door.binding(&[(elt, KType::STR)]);
        (
            door.resumption(under, None, false).map(|_| ()),
            door.resumption(under, None, true).map(|_| ()),
        )
    });
    assert!(closed.is_err(), "no `ONLY` admits a list of strings");
    assert_eq!(witnessed, closed);
}

/// A body whose every question its witness answers holds nothing: the entry seeds `Elt` at the
/// kind its witness gives, a reader of `x` asks whether `Elt` is a module type, and the closing
/// asks how a retype to each parameter's type re-stamps. So the closing walk under the witness
/// re-runs nothing, and a binding keeping every answer lays no unit.
#[test]
fn a_body_whose_questions_its_witness_answers_holds_nothing() {
    let source = subject_over("LET a = (PEEK xs)\n  LET c = x\n  a");
    let (holds, reruns, over, closed, laid, retyped) = resumed(&source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let under = |closed| door.binding(&[(elt, closed)]);
        let reruns = || crate::dispatch::walk::RERUNS.with(|reruns| reruns.get());
        crate::dispatch::walk::RERUNS.with(|reruns| reruns.set(Default::default()));
        let first = (door.resumption(under(KType::NUMBER), None, false)).expect("no refusal");
        let witnessed = first.witnessed.expect("the first resumption witnesses");
        let holds = witnessed.witnessing().expect("a witnessed layer").holds;
        let second = door.resumption(under(KType::BOOL), Some(witnessed), false);
        let second = second.expect("no refusal");
        let reruns = reruns().iter().sum::<usize>();
        let restart = (door.resumption(under(KType::BOOL), None, true)).expect("no refusal");
        let laid = ["x", "a", "c"].map(|name| {
            let unit = super::resumption::unit_of(door, shape, name);
            second.layer.unit_of(unit).is_some()
        });
        (
            holds,
            reruns,
            rendered_over(door, second.layer),
            rendered_over(door, restart.layer),
            laid,
            [second.layer, restart.layer].map(|layer| retypes_over(door, shape, layer)),
        )
    });
    assert!(!holds, "the closing walk under the witness re-ran nothing");
    assert_eq!(
        reruns, 0,
        "no closing walk over the witnessed layer re-runs a unit"
    );
    assert_eq!(laid, [false; 3], "`Bool` keeps every answer");
    assert_eq!(over, closed);
    let [retyped, restarted] = retyped;
    assert!(
        retyped.iter().any(|retype| retype.starts_with("Some")),
        "{retyped:?}"
    );
    assert_eq!(
        retyped, restarted,
        "the closing's retypes close through the table"
    );
}

/// What a frame of `shape` re-stamps its arguments and its value at, read over `layer`.
fn retypes_over(
    door: &Door<'_, '_>,
    shape: &crate::scope::BodyShape<'_>,
    layer: crate::scope::BuildLayer<'_>,
) -> Vec<String> {
    let layer = crate::memory::resident(door.bump, layer);
    let facts = crate::scope::Facts::under(door.load(), Some(layer));
    let scratch = crate::memory::Bump::new();
    let retypes = facts.retypes(door.types, &scratch, shape);
    let mut shown: Vec<String> = (retypes.parameters.iter())
        .map(|(name, _)| format!("{:?}", retypes.parameter(door.types, *name)))
        .collect();
    shown.push(format!("{:?}", retypes.returned(door.types)));
    shown
}

/// The answered marks on `unknown` each layer of the chain from `top` down holds on its own units'
/// frontiers, a list per layer beside whether it witnessed, innermost first.
fn chain_marks(
    top: &crate::scope::BuildLayer<'_>,
    unknown: Unknown,
) -> Vec<(bool, Vec<(OpenKind, String)>)> {
    std::iter::successors(Some(top), |layer| layer.parent())
        .map(|layer| {
            let mut marks: Vec<_> = (layer.own_units())
                .filter_map(|unit| layer.unit(unit))
                .flat_map(|(_, frontier)| frontier.marks())
                .filter(|noted| noted.unknown == unknown)
                .filter_map(|noted| match noted.mark {
                    Mark::Open(open) => open.answered.map(|answered| (open.kind, answered.answer)),
                    Mark::Named => None,
                })
                .map(|(kind, answer)| (kind, format!("{answer:?}")))
                .collect();
            marks.sort();
            (layer.witnessing().is_some(), marks)
        })
        .collect()
}

/// A closure's copy of its encloser's decided layers reads the witnessed layer apart from the
/// binding's: copied into another region, the chain keeps its spine's digest, its witnessed
/// layer at the same depth, and the answered marks on the name that layer witnessed.
#[test]
fn a_copied_chain_keeps_its_spine_and_its_witnessed_marks() {
    let source = subject_over("LET a = (PEEK xs)\n  a");
    let (before, after, same_spine) = resumed(&source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let first = door.resumption(door.binding(&[(elt, KType::NUMBER)]), None, false);
        let top = crate::memory::resident(door.bump, first.expect("no refusal").layer);
        let other = crate::memory::Bump::new();
        let copied = top.copied(&other);
        let same_spine =
            crate::scope::spine_digest(Some(top)) == crate::scope::spine_digest(Some(copied));
        (chain_marks(top, elt), chain_marks(copied, elt), same_spine)
    });
    assert!(same_spine, "the copy's spine digests as its source's");
    assert!(
        before
            .iter()
            .any(|(witnessed, marks)| *witnessed && !marks.is_empty()),
        "the witnessed layer holds answered marks on `Elt`: {before:?}"
    );
    assert_eq!(before, after);
}
