//! A witnessing walk: the first walk of a resumption that witnesses a binding, which leaves the
//! binding's type names rigid and asks each discrete door's question under the witness
//! ([build/README.md § Resumption](../../build/README.md#resumption)).

use crate::scope::{Answer, LoadFacts, Mark, OpenKind, Segment, Unknown};
use crate::type_lattice::{DeclaredType, KType, Parametric};

use super::resumption::{Door, quantified, resumed, unknown};

/// `PEEK`, whose overloads tell a list of strings from any other list. A list's type node is no
/// variable, so a reader of `xs :(LIST OF Elt)` reaches only discrete questions on `Elt`.
const PEEK: &str = "EXPR #(PEEK xs :(LIST OF Any)) -> Str = #(\"a\")\n\
                    EXPR #(PEEK xs :(LIST OF Str)) -> Number = #(1)\n";

/// The answered marks on `unknown` among the records `facts` itself laid for `segment`'s units,
/// by kind beside the answer, sorted.
fn answered(facts: &LoadFacts<'_>, segment: Segment, unknown: Unknown) -> Vec<(OpenKind, String)> {
    let mut found: Vec<_> = (facts.own_units())
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
    for (unit, (record, frontier)) in facts.own_units() {
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
        str.contains(&(OpenKind::Verdict, "Verdict(Always)".into())),
        "{str:?}"
    );
    assert!(!named_number, "no record names the witness's type");
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
                .filter_map(|unit| layer.unit(unit)?.typed())
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

/// A quote's code's refusal reads its captures as a re-run does: a `$Elt` crossing into the code
/// is a name of the code's own, but under a binding of `SUBJECT`'s `Elt` a walk re-running the code
/// reads it as that binding, so the refusal is worded `Number` under `Elt := Number` — in a
/// resumption witnessed afresh, over a layer witnessed under another binding, and in a walk
/// re-running every unit alike. Over `SUBJECT`'s own levels and others' (`Alias` shifts `Elt` off
/// the code's level), and where only a binding makes the code refuse (`Elt` unbounded): a question
/// on the capture is one on its source, and the code's closing waits on it, refused or not.
#[test]
fn a_code_s_refusal_reads_its_captures_as_a_re_run_does() {
    let bounded = |prefix: &str| {
        format!(
            "{prefix}EXPR FOR ALL #{{Elt: :(Number | Bool)}} #(SUBJECT x :Elt) -> Any = #(\n  \
             LET c = #(LET a :($Elt) = \"a\")\n  \
             x\n\
             )\n"
        )
    };
    let sources = [
        bounded(""),
        bounded("LET Alias = Number\n"),
        subject_over("LET a = (PEEK xs)\n  LET c = #(LET q :($Elt) = \"a\")\n  a"),
    ];
    for source in &sources {
        let worded = resumed(source, |door| {
            let shape = super::resumption::subject(door.root());
            let code = (shape.nested_shapes().iter())
                .map(|(_, nested)| *nested)
                .find(|nested| nested.kind() == crate::scope::ShapeKind::Code)
                .expect("`SUBJECT` holds a quote's code");
            let elt = unknown(door.symbols(), shape, "Elt");
            let worded = |facts: crate::scope::Facts<'_>| {
                let refusal = facts.typing_refusal(door.types, door.bump, code, &mut |_| None);
                (refusal.map(|refusal| refusal.display(door.symbols(), door.types).to_string()))
                    .unwrap_or_default()
            };
            let layered = |layer: crate::scope::BuildLayer<'_>| {
                let layer = crate::memory::resident(door.bump, layer);
                worded(crate::scope::Facts::under(door.load(), Some(layer)))
            };
            let under = |closed| door.binding(&[(elt, closed)]);
            let first = door.resumption(under(KType::BOOL), None, false);
            let witnessed = (first.expect("`SUBJECT` loads").witnessed).expect("it witnesses");
            [(KType::NUMBER, "Number"), (KType::BOOL, "Bool")].map(|(closed, name)| {
                let rerun = door
                    .resume(None, under(closed), true)
                    .expect("`SUBJECT` loads");
                let fresh = door
                    .resumption(under(closed), None, false)
                    .expect("`SUBJECT` loads");
                let over = (door.resumption(under(closed), Some(witnessed), false))
                    .expect("`SUBJECT` loads");
                (
                    name,
                    worded(crate::scope::Facts::of(rerun)),
                    layered(fresh.layer),
                    layered(over.layer),
                )
            })
        });
        for (name, rerun, fresh, over) in &worded {
            assert!(
                rerun.ends_with(&format!("can never satisfy its annotation {name}")),
                "{rerun}\nin\n{source}"
            );
            assert_eq!(fresh, rerun, "witnessed afresh, in\n{source}");
            assert_eq!(over, rerun, "over a witnessed layer, in\n{source}");
        }
    }
}

/// A spread reads its list's element through the projection door, so a closed rule over a union of
/// lists reads the union of their elements, as a projection over the bound substitutes to: under
/// `Ll := (LIST OF Number) | (LIST OF Str)`, `[..xs 0]` types its elements `Number | Str` in a
/// resumption and in a fresh load with the binding declared alike.
#[test]
fn a_spread_over_a_union_binding_reads_the_union_of_its_elements() {
    let source = "EXPR FOR ALL #{Ll: :(LIST OF Any)} #(SUBJECT xs :Ll) -> Any = #(\n  \
                  LET ys = [..xs 0]\n  \
                  ys\n\
                  )\n";
    let (resumed, restart) = resumed(source, |door| {
        let shape = super::resumption::subject(door.root());
        let ll = unknown(door.symbols(), shape, "Ll");
        let lists = [KType::NUMBER, KType::STR].map(|element| door.types.list(element));
        let either = door.types.union_of(door.bump, &lists);
        let under = door.binding(&[(ll, either)]);
        let resumed = door
            .resumption(under, None, false)
            .expect("`SUBJECT` loads");
        let restart = door.resumption(under, None, true).expect("`SUBJECT` loads");
        (
            rendered_over(door, resumed.layer),
            rendered_over(door, restart.layer),
        )
    });
    assert_eq!(resumed, restart);
    assert!(
        restart
            .iter()
            .any(|fact| fact.contains("ys = [:(LIST OF Number), :(LIST OF :(Number | Str))]")),
        "{restart:#?}"
    );
}

/// A construction judges its payload through the doors: under `NEWTYPE Meters = Number`,
/// `(Meters x)` over `x :Elt` asks whether `Elt` fits `Number`, so under `Elt := Number` a
/// resumption types it `[Meters, Meters]`, as a fresh load with the binding declared does.
#[test]
fn a_construction_over_a_run_bound_payload_is_typed_as_a_fresh_load_types_it() {
    let source = "NEWTYPE Meters = Number\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET m = (Meters x)\n  \
                  m\n\
                  )\n";
    let (resumed, restart) = resumed(source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let under = door.binding(&[(elt, KType::NUMBER)]);
        let resumed = door
            .resumption(under, None, false)
            .expect("`SUBJECT` loads");
        let restart = door.resumption(under, None, true).expect("`SUBJECT` loads");
        (
            rendered_over(door, resumed.layer),
            rendered_over(door, restart.layer),
        )
    });
    assert_eq!(resumed, restart);
    assert!(
        restart.iter().any(|fact| fact == " m = [Meters, Meters]"),
        "{restart:#?}"
    );
}

/// A type binder over a run-bound name is rigid, and its readers across a callable boundary read it
/// at its own level: `Pair` is `:{a :Elt}`, and `GET`'s body reads `p.a` through it at `Elt`.
/// Under `Elt := Number` a resumption types `GET`'s body as a fresh load does.
#[test]
fn a_type_binder_over_a_run_bound_name_is_read_rigid_across_a_callable_boundary() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET Pair = :{a :Elt}\n  \
                  EXPR #(GET p :Pair) -> Any = #(p.a)\n  \
                  GET {a = x}\n\
                  )\n";
    let (fresh, resumed, restart) = resumed(source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let under = door.binding(&[(elt, KType::NUMBER)]);
        let resumed = (door.resumption(under, None, false)).expect("`SUBJECT` loads");
        let restart = (door.resumption(under, None, true)).expect("`SUBJECT` loads");
        let facts = crate::scope::Facts::under(door.load(), None);
        (
            super::resumption::laws::render_over(&facts, door.types, door.symbols(), &[]),
            rendered_over(door, resumed.layer),
            rendered_over(door, restart.layer),
        )
    });
    for fact in [
        "Pair declares rigid :{a :Elt}",
        "/0 #0 = [(ONE OF Never, (RETYPED TO Elt)), Elt]",
    ] {
        assert!(fresh.iter().any(|held| held.trim() == fact), "{fresh:#?}");
    }
    assert!(
        resumed.iter().any(|fact| fact.contains(":{a :Number}")),
        "{resumed:#?}"
    );
    assert_eq!(resumed, restart);
}

/// A reader of a rigid type binder reads its declaration over the name, so a binding that keeps the
/// reader's questions re-runs nothing: the function written in place reads `p :Pair` as
/// `:{a :Elt}`, which asks the same questions under `Number` and `Bool`.
#[test]
fn a_rigid_type_binder_s_reader_keeps_its_answers_under_a_second_binding() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET Pair = :{a :Elt}\n  \
                  LET f = (FN :{p :Pair} -> Any = #(p.a))\n  \
                  f\n\
                  )\n";
    let (reruns, over, closed) = resumed(source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let under = |closed| door.binding(&[(elt, closed)]);
        let reruns = || crate::dispatch::walk::RERUNS.with(|reruns| reruns.get());
        let first = (door.resumption(under(KType::NUMBER), None, false)).expect("`SUBJECT` loads");
        let witnessed = first.witnessed.expect("the first resumption witnesses");
        crate::dispatch::walk::RERUNS
            .with(|reruns| reruns.set([0; crate::dispatch::walk::UNMARKED + 1]));
        let second = door.resumption(under(KType::BOOL), Some(witnessed), false);
        let second = second.expect("`SUBJECT` loads");
        let reruns = reruns().iter().sum::<usize>();
        let restart = (door.resumption(under(KType::BOOL), None, true)).expect("`SUBJECT` loads");
        (
            reruns,
            rendered_over(door, second.layer),
            rendered_over(door, restart.layer),
        )
    });
    assert_eq!(reruns, 0, "`Bool` keeps every answer");
    assert!(
        over.iter().any(|fact| fact.contains(":{a :Bool}")),
        "{over:#?}"
    );
    assert_eq!(over, closed);
}

/// `source`'s `SUBJECT` resumed under `Elt` bound to the type `first` declares, then, from that
/// witness, to the one `second` declares: each derived name the load lays, rendered, the facts the
/// first resumption gives beside a fresh load's under `first`, the re-runs the second takes, and
/// its facts beside a fresh load's under `second`.
fn spelled_under(source: &str, first: &str, second: &str) -> SpelledUnder {
    resumed(source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let under = |closed| door.binding(&[(elt, closed)]);
        let (first, second) = (door.declared(first), door.declared(second));
        let reruns = || crate::dispatch::walk::RERUNS.with(|reruns| reruns.get());
        let resumed = (door.resumption(under(first), None, false)).expect("`SUBJECT` loads");
        let restart = (door.resumption(under(first), None, true)).expect("`SUBJECT` loads");
        let fresh = (
            rendered_over(door, resumed.layer),
            rendered_over(door, restart.layer),
        );
        let witnessed = resumed.witnessed.expect("the first resumption witnesses");
        crate::dispatch::walk::RERUNS
            .with(|reruns| reruns.set([0; crate::dispatch::walk::UNMARKED + 1]));
        let kept = door.resumption(under(second), Some(witnessed), false);
        let kept = kept.expect("`SUBJECT` loads");
        let reruns = reruns().iter().sum::<usize>();
        let restart = (door.resumption(under(second), None, true)).expect("`SUBJECT` loads");
        let derived = door.load().base_layer().derived();
        let spelled = (derived.names().iter())
            .map(|name| {
                crate::type_lattice::display_name(name.ty, door.types, door.symbols()).to_string()
            })
            .collect();
        SpelledUnder {
            spelled,
            fresh,
            reruns,
            kept: (
                rendered_over(door, kept.layer),
                rendered_over(door, restart.layer),
            ),
        }
    })
}

/// What [`spelled_under`] hands back.
struct SpelledUnder {
    spelled: Vec<String>,
    fresh: (Vec<String>, Vec<String>),
    reruns: usize,
    kept: (Vec<String>, Vec<String>),
}

/// A member read off a run-bound owner is the name decided on whether the binding declares it:
/// `p :(Elt.Red)` under `Elt := Color` is `Color`'s `Red`, as a fresh load types it, and `Shade`,
/// which declares a `Red` too, keeps every answer.
#[test]
fn a_member_over_a_run_bound_owner_reads_its_binding_s_member() {
    let source = "UNION Color = #{Red: Null, Blue: Null}\n\
                  UNION Shade = #{Red: Null, Dark: Null}\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET f = (FN :{p :(Elt.Red)} -> Any = #(p))\n  \
                  f\n\
                  )\n";
    let under = spelled_under(source, "Color", "Shade");
    assert!(
        (under.spelled.iter()).any(|name| name == "(ONE OF Never, (MEMBER Red OF Elt))"),
        "{:?}",
        under.spelled
    );
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(under.reruns, 0, "`Shade` declares a `Red` too");
    assert_eq!(under.kept.0, under.kept.1);
}

/// An application whose head is a run-bound name is the name decided on whether the binding's head
/// takes its arguments: `y :(Number -> Elt)` under `Elt := Box`, a one-parameter family, is `Box`
/// at `Number`, as a fresh load spells it, and `Crate`, another family of one parameter, keeps the
/// answer.
#[test]
fn an_application_over_a_run_bound_head_applies_its_binding() {
    let source = "NEWTYPE (Item -> Box)\n\
                  NEWTYPE (Item -> Crate)\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET f = (FN :{y :(Number -> Elt)} -> Any = #(y))\n  \
                  f\n\
                  )\n";
    let under = spelled_under(source, "Box", "Crate");
    assert!(
        (under.spelled.iter()).any(|name| name == "(ONE OF Never, (Number -> Elt))"),
        "{:?}",
        under.spelled
    );
    assert!(
        under.fresh.0.iter().any(|fact| fact.contains("Box")),
        "{:#?}",
        under.fresh.0
    );
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(under.reruns, 0, "`Crate` takes one argument too");
    assert_eq!(under.kept.0, under.kept.1);
}

/// A code kind over a run-bound name needing a name is the name decided on whether the binding is a
/// code kind: `q :(Elt NEEDING #[y])` under `Elt := Expression` is that kind needing `y`, as a fresh
/// load spells it, and `Block` keeps the answer.
#[test]
fn a_code_kind_over_a_run_bound_name_needs_at_its_binding() {
    let source = "LET Ex = Expression\n\
                  LET Bl = Block\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET f = (FN :{q :(Elt NEEDING #[y])} -> Any = #(q))\n  \
                  f\n\
                  )\n";
    let under = spelled_under(source, "Ex", "Bl");
    assert!(
        (under.spelled.iter()).any(|name| name == "(ONE OF Never, (Elt NEEDING #[y]))"),
        "{:?}",
        under.spelled
    );
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(under.reruns, 0, "`Block` is a code kind too");
    assert_eq!(under.kept.0, under.kept.1);
}

/// A construction over a member read off a run-bound owner builds each end through the rule a
/// binding constructs by: `(Elt.Red null)` under `Elt := Color` is a `Red` at most the union it
/// reaches, as a fresh load types it, and `Shade`, which declares a `Red` too, keeps every answer.
#[test]
fn a_construction_over_a_run_bound_owner_s_member_is_typed_as_a_fresh_load_types_it() {
    let source = "UNION Color = #{Red: Null, Blue: Null}\n\
                  UNION Shade = #{Red: Null, Dark: Null}\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET r = (Elt.Red null)\n  \
                  r\n\
                  )\n";
    let under = spelled_under(source, "Color", "Shade");
    assert!(
        (under.fresh.0.iter()).any(|fact| fact == " r = [Red, :(Blue | Red)]"),
        "{:#?}",
        under.fresh.0
    );
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(under.reruns, 0, "`Shade` declares a `Red` too");
    assert_eq!(under.kept.0, under.kept.1);
}

/// A signature over a run-bound name with a head parameter pinned is the name decided on whether
/// the binding declares the parameter: `c :(Elt WITH {Set = Number})` under `Elt := Coll` is `Coll`
/// pinned, as a fresh load spells it, and `Bag`, which declares a `Set` too, keeps the answer. `x`
/// holds a list of modules, so no module binder reads `Elt`.
#[test]
fn a_pin_over_a_run_bound_signature_pins_its_binding() {
    let source = "SIG Coll FOR ALL #[Set] = #[(VAL empty :Set)]\n\
                  SIG Bag FOR ALL #[Set] = #[(VAL full :Set)]\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :(LIST OF Elt)) -> Any = #(\n  \
                  LET f = (FN :{c :(Elt WITH {Set = Number})} -> Any = #(c))\n  \
                  f\n\
                  )\n";
    let under = spelled_under(source, "Coll", "Bag");
    assert!(
        (under.spelled.iter()).any(|name| name == "(ONE OF Never, (Elt WITH {Set = Number}))"),
        "{:?}",
        under.spelled
    );
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(under.reruns, 0, "`Bag` declares a `Set` too");
    assert_eq!(under.kept.0, under.kept.1);
}

/// A `MATCH … UNDER` clause over a run-bound name reads each label's variant, its payload and the
/// payload `_` binds as derived names, whose terms read what the binding gives them: under
/// `Elt := Color` the arms are typed as a fresh load types them, and `Shade`, whose `Red` holds a
/// `Number` too and whose other variant a `Str`, keeps every answer.
#[test]
fn a_match_under_a_run_bound_clause_reads_its_binding_s_variants() {
    let source = "UNION Color = #{Red: Number, Blue: Str}\n\
                  UNION Shade = #{Red: Number, Dark: Str}\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET r = (MATCH x UNDER Elt WITH #{Red: (it), _: (it)})\n  \
                  r\n\
                  )\n";
    let under = spelled_under(source, "Color", "Shade");
    for name in ["(VARIANT Red OF Elt)", "(PAYLOAD Red OF Elt)"] {
        assert!(
            under.spelled.iter().any(|spelled| spelled == name),
            "{:?}",
            under.spelled
        );
    }
    assert!(
        (under.fresh.0.iter()).any(|fact| fact == " r = [Never, :(Number | Str)]"),
        "{:#?}",
        under.fresh.0
    );
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(under.reruns, 0, "`Shade` keeps every answer");
    assert_eq!(under.kept.0, under.kept.1);
}

/// A binding that gives a `MATCH … UNDER` clause over a run-bound name no variant a label names,
/// or no union, refuses the resumption as a fresh load refuses.
#[test]
fn a_binding_a_match_under_a_run_bound_clause_cannot_take_refuses() {
    let source = "UNION Color = #{Red: Number, Blue: Str}\n\
                  UNION Mood = #{Calm: Null, Wild: Null}\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  MATCH x UNDER Elt WITH #{Red: (it), _: (it)}\n\
                  )\n";
    let (lacks, number) = resumed(source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let under = |closed| door.binding(&[(elt, closed)]);
        let first =
            (door.resumption(under(door.declared("Color")), None, false)).expect("`SUBJECT` loads");
        let witnessed = first.witnessed.expect("the first resumption witnesses");
        let refused = |closed| {
            let over = door.resumption(under(closed), Some(witnessed), false);
            let fresh = door.resumption(under(closed), None, true);
            assert_eq!(over.as_ref().err(), fresh.as_ref().err());
            over.err().expect("the binding refuses")
        };
        (refused(door.declared("Mood")), refused(KType::NUMBER))
    });
    assert!(
        lacks.contains("Red names no variant of :(Calm | Wild)"),
        "{lacks}"
    );
    assert!(
        number.contains("takes a union, and Number is not one"),
        "{number}"
    );
}

/// A `NEWTYPE` over a run-bound name declares rigid, over a derived name whose term seals the
/// group a binding closes: under `Elt := Number` the body is typed as a fresh load types it, and
/// `Str` keeps every answer. The term's closed rule seals one identity per binding, and a
/// different one at `Str`.
#[test]
fn a_nominal_over_a_run_bound_name_is_sealed_by_its_binding() {
    let source = "LET Nb = Number\nLET St = Str\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  NEWTYPE Box = :{v :Elt}\n  \
                  LET b = (Box {v = x})\n  \
                  b\n\
                  )\n";
    let under = spelled_under(source, "Nb", "St");
    assert!(
        (under.spelled.iter()).any(|name| name == "(Box DECLARED OVER :{v :Elt})"),
        "{:?}",
        under.spelled
    );
    assert!(
        under.fresh.0.iter().any(|fact| fact == " b = [Box, Box]"),
        "{:#?}",
        under.fresh.0
    );
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(under.reruns, 0, "`Str` keeps every answer");
    assert_eq!(under.kept.0, under.kept.1);
    let sealed = resumed(source, |door| {
        let derived = door.load().base_layer().derived();
        let (group, output) = (derived.names().iter())
            .find_map(|name| match name.term {
                crate::scope::DerivedTerm::Nominal { group, output, .. } => Some((group, output)),
                _ => None,
            })
            .expect("`Box` is a derived name");
        let bump = crate::memory::Bump::new();
        let field = crate::symbols::BinderSymbol::declared("v", door.symbols()).expect("a name");
        let seal = |elt: KType| {
            let schema = door.types.record(&bump, &[(field, Parametric::from(elt))]);
            let sealed = crate::scope::sealed_nominals(door.types, &bump, group, &[schema]);
            sealed.expect("a closed group seals")[usize::from(output)]
        };
        [seal(KType::NUMBER), seal(KType::NUMBER), seal(KType::STR)]
    });
    let [number, again, str] = sealed;
    assert_eq!(number, again, "one binding's `Box` is one type");
    assert_ne!(
        number, str,
        "a `Box` at `Number` and one at `Str` are two types"
    );
}

/// A recursive `UNION` over a run-bound name is rigid at load the same way: the binder and its
/// variants are read off the derived name of its union, and a binding types the body as a fresh
/// load types it.
#[test]
fn a_recursive_nominal_over_a_run_bound_name_is_rigid_at_load() {
    let source = "LET Nb = Number\nLET St = Str\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  UNION Chain = #{Link: :{v :Elt, next :Chain}, End: Null}\n  \
                  LET c = (Chain.Link {v = x, next = (Chain.End null)})\n  \
                  c\n\
                  )\n";
    let under = spelled_under(source, "Nb", "St");
    assert!(
        (under.spelled.iter()).any(|name| name.starts_with("(Chain DECLARED OVER")),
        "{:?}",
        under.spelled
    );
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(under.kept.0, under.kept.1);
}

/// `(BOXIT 1)` built twice is one value of one `Box`: each run declares the group its binding
/// closes.
#[test]
fn a_nominal_over_a_run_bound_name_built_twice_is_one_type() {
    let source = "EXPR FOR ALL #[Elt] #(BOXIT x :Elt) -> Any = #(\n  \
                  NEWTYPE Box = :{v :Elt}\n  \
                  (Box {v = x})\n\
                  )\n\
                  PRINT ((BOXIT 1) == (BOXIT 1))\n\
                  PRINT (BOXIT 1)\n";
    assert_eq!(super::run(source), "true\nBox({v = 1})");
}

/// `INNER`'s `FOR ALL` bound names `OUTER`'s `Elt`, so its registration is left for the run, which
/// registers it: `(OUTER 5)` prints `5`.
#[test]
fn a_registration_whose_bound_names_a_run_bound_name_is_registered_by_the_run() {
    let source = "EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Any = #(\n  \
                  EXPR FOR ALL #{Tee: Elt} #(INNER y :Tee) -> Tee = #(y)\n  \
                  INNER x\n\
                  )\n\
                  PRINT (OUTER 5)\n";
    assert_eq!(super::run(source), "5");
}

/// `OUTER` resumed under `Elt := Number` gives the facts a fresh load with the binding declared
/// gives: the registration it leaves for the run is typed where the binding closes its bound.
#[test]
fn a_registration_left_for_the_run_resumes_as_a_fresh_load_types_it() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  EXPR FOR ALL #{Tee: Elt} #(INNER y :Tee) -> Tee = #(y)\n  \
                  INNER x\n\
                  )\n";
    let (resumed, restart) = resumed(source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let under = door.binding(&[(elt, KType::NUMBER)]);
        let first = (door.resumption(under, None, false)).expect("`SUBJECT` loads");
        let again = (door.resumption(under, None, true)).expect("`SUBJECT` loads");
        (
            rendered_over(door, first.layer),
            rendered_over(door, again.layer),
        )
    });
    assert_eq!(resumed, restart);
}

/// A registration whose parameter reads a meet over a run-bound name is rigid over its derived
/// name, and a call of it resumed under `Elt := Number` types as a fresh load does.
#[test]
fn a_registration_over_a_meet_resumes_as_a_fresh_load_types_it() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  EXPR #(SPIN e :(Elt & Number) n :Number) -> Number = #(n)\n  \
                  LET s = (SPIN x 1)\n  \
                  s\n\
                  )\n";
    let (fresh, restart) = resumed(source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let under = |closed| door.binding(&[(elt, closed)]);
        let first = (door.resumption(under(KType::NUMBER), None, false)).expect("`SUBJECT` loads");
        let again = (door.resumption(under(KType::NUMBER), None, true)).expect("`SUBJECT` loads");
        (
            rendered_over(door, first.layer),
            rendered_over(door, again.layer),
        )
    });
    assert!(
        fresh.iter().any(|fact| fact == " s = [Number, Number]"),
        "{fresh:#?}"
    );
    assert_eq!(fresh, restart);
}

/// A call through a head whose type is a run-bound name is the derived name of the application a
/// binding gives it, its argument typed at the record the binding's function wants: under
/// `Elt := (FN :{x :Number} -> Number)` it is the call's return, as a fresh load types it, and
/// `(FN :{x :Any} -> Str)`, which takes the argument too, keeps every answer.
#[test]
fn a_call_through_a_run_bound_head_is_typed_by_its_binding() {
    let source = "LET Nb = :(FN :{x :Number} -> Number)\nLET St = :(FN :{x :Any} -> Str)\n\
                  EXPR FOR ALL #{Elt: :(FN :{x :Number} -> Any)} #(SUBJECT f :Elt) -> Any = #(\n  \
                  LET s = (f {x = 1})\n  \
                  s\n\
                  )\n";
    let under = spelled_under(source, "Nb", "St");
    assert!(
        (under.spelled.iter()).any(|name| name == "(UPPER Elt APPLIED TO :{x :Number})"),
        "{:?}",
        under.spelled
    );
    assert!(
        under
            .fresh
            .0
            .iter()
            .any(|fact| fact == " s = [Number, Number]"),
        "{:#?}",
        under.fresh.0
    );
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(
        under.reruns, 0,
        "`(FN :{{x :Any}} -> Str)` keeps every answer"
    );
    assert_eq!(under.kept.0, under.kept.1);
}

/// The same application under a binding that gives the head a list is a read: `(f 0)` under
/// `Elt := (LIST OF Number)` reads its element, as a fresh load types it, and `(LIST OF Str)`
/// keeps every answer.
#[test]
fn a_read_through_a_run_bound_head_is_typed_by_its_binding() {
    let source = "LET Nb = :(LIST OF Number)\nLET St = :(LIST OF Str)\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT f :Elt) -> Any = #(\n  \
                  LET s = (f 0)\n  \
                  s\n\
                  )\n";
    let under = spelled_under(source, "Nb", "St");
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(under.reruns, 0, "`(LIST OF Str)` keeps every answer");
    assert_eq!(under.kept.0, under.kept.1);
}

/// A `SIG` whose member reads a run-bound name is the derived name of the signature a binding
/// declares: a reader of it under `Elt := Number` is typed as a fresh load types it, and `Str`
/// keeps every answer.
#[test]
fn a_signature_over_a_run_bound_name_is_declared_by_its_binding() {
    let source = "LET Nb = Number\nLET St = Str\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  SIG Holds = #[(VAL v :Elt)]\n  \
                  LET f = (FN :{m :Holds} -> Any = #(m))\n  \
                  f\n\
                  )\n";
    let under = spelled_under(source, "Nb", "St");
    assert!(
        (under.spelled.iter()).any(|name| name == "(SIG DECLARED OVER Elt)"),
        "{:?}",
        under.spelled
    );
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(under.reruns, 0, "`Str` keeps every answer");
    assert_eq!(under.kept.0, under.kept.1);
}

/// A registration born in a body whose parameter reads a run-bound name keys its memo by the
/// declared bound, which asks nothing a binding answers: `INNER`'s birth in `SUBJECT`'s body
/// re-runs nothing under a second binding.
#[test]
fn a_birth_over_a_run_bound_parameter_keeps_its_answers_under_a_second_binding() {
    let source = "LET Nb = Number\nLET St = Str\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  EXPR #(INNER y :Elt) -> Any = #(y)\n  \
                  INNER x\n\
                  )\n";
    let under = spelled_under(source, "Nb", "St");
    assert_eq!(under.fresh.0, under.fresh.1);
    assert_eq!(under.reruns, 0, "`Str` keeps every answer");
    assert_eq!(under.kept.0, under.kept.1);
}

/// A quote's code over `Elt` lies on `SUBJECT`'s chain, so a binding substitutes into the code's
/// records as into the body's: the witness walks no body for the code, and under `Elt := Str` the
/// `EVAL`'s refusal comes from the code's records closed, as a restart words it.
#[test]
fn a_binding_refuses_a_quote_s_code_by_its_records_closed() {
    let source = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                  EXPR #(PICK x :Str) -> Number = #(1)\n\
                  EXPR FOR ALL #{Elt: :(Number | Str)} #(SUBJECT x :Elt) -> Any = #(\n  \
                  EVAL #($(PICK $x)) -> Str\n\
                  )\n";
    let (holds, over, restart) = resumed(source, |door| {
        let shape = super::resumption::subject(door.root());
        let elt = unknown(door.symbols(), shape, "Elt");
        let under = |closed| door.binding(&[(elt, closed)]);
        let first = (door.resumption(under(KType::NUMBER), None, false)).expect("no refusal");
        let witnessed = first.witnessed.expect("the first resumption witnesses");
        let holds = witnessed.witnessing().expect("a witnessed layer").holds;
        let over = door.resumption(under(KType::STR), Some(witnessed), false);
        let restart = door.resumption(under(KType::STR), None, true);
        (holds, over.err(), restart.err())
    });
    assert!(!holds, "the witness walks no body for the code");
    let over = over.expect("`Elt := Str` refuses");
    assert!(
        over.contains("can never satisfy its declared return Str"),
        "{over}"
    );
    assert_eq!(Some(over), restart);
}
