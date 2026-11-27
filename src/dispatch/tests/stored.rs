//! The **stored-question laws**: a rule the walk decides at a site, laid whole in its unit's
//! record ([`SiteQuestion`]), decides as the walk did at the load, and as a fresh load with a
//! binding declared does under it; and its asks are **monotone**, each only narrowing as more of
//! the binding is written in.
//!
//! A rule the walk does not yet lay is listed in [`UNCONVERTED`]; the laws skip it. Each rule the
//! walk comes to lay leaves the list, and its comparison with the fresh load joins
//! [`a_stored_question_decides_as_a_fresh_load_with_its_binding_declared`]: the load's question
//! decided with the binding's content written into each ask ([`ask`]), its taken arm's payload
//! applied ([`apply`]), and rendered by `SUBJECT`'s structure, which the restart shares, beside
//! what the restart's walk fixed at the same site. A question deciding a refusal words its
//! template with the binding written in ([`Applying`]); a restart refusing in a converted rule's
//! words refuses as one such does, unless a question the law does not compare may refuse first.
//!
//! A body's class (porous or sealed) and its mint counts are no stored question: they stay load
//! facts (decision D9 of the binding-graph build), which only the base's folds read, so the load's
//! conservative class stands under every binding. The laws check only that a body
//! sealed at the load stays sealed.

use std::collections::HashMap;

use proptest::prelude::*;

use crate::memory::Bump;
use crate::parse::ExpressionPart;
use crate::program::{CellSubstrate, LoadError};
use crate::scope::{
    Answer, AskedBy, Asks, BodyShape, CandidateRoute, CloseLaying, Closed, Closer,
    Content as ContentRecord, DerivedId, DerivedTerm, Fixed, InstanceRead, Laying, NameAddress,
    Passing, Payload, Point, Posed, Question, Run, Segment, Selectable, Selection, ShapeError,
    SiteAnswer, SiteQuestion, SiteRule, Slot, Spellings, Static, StaticInstance, StaticType,
    decide, each_part, free_variables, part_of,
};
use crate::symbols::SymbolInterner;
use crate::type_lattice::{
    DeclaredType, KKind, KType, Parametric, Variable, Verdict, display_name,
};
use crate::values::tests::reading;
use crate::values::{BuildBudget, Chain, Retype};

use super::super::graph::{Content, Named, Naming, apply, ask};
use super::generate::{Bound, Desc, inhabited, resumable};
use super::under::laws::{canonical, levels, narrowed, unlocated};
use super::under::{Door, opened, subject};
use super::{Koan, output};
use crate::build::lay::NameVariable;

/// What a decided line wording a refusal starts with.
const REFUSES: &str = "refuses ";

/// The decided line standing for a question the law does not compare, which may refuse.
const UNSEEN: &str = "unseen";

/// What a decided line marking a site whose question decides all the restart fixes there starts
/// with, then the restart's line's start: a member retype's, whose undecided outcome is that no
/// retype is recorded, a view's settlement, a decided hold's settlement and re-stamp, and which
/// arms of a `MATCH` run.
const WHOLE: &str = "whole ";

/// Words only the refusals a converted rule lays a template for use: until every rule is
/// converted, a restart may refuse first at a site no stored question decides.
const TEMPLATED: &[&str] = &[
    "has no instance under",
    "no overload of `",
    "ambiguous call of `",
    "has no field ",
    "has no member ",
    "nothing fixes ",
    "which can never satisfy the parameters ",
    " can never be read by ",
    " takes operands the builtin ",
    "`..` spreads a list, and this value is ",
    "`$..` splices a list of code, and this value is ",
    ", which can never be code for `EVAL` to run",
    "this `EVAL`'s code returns ",
    " is no module to ascribe",
    " is no signature",
    ", which can never satisfy its ascription ",
    ", which can never satisfy its annotation ",
    ", which can never be code: write the body as a quote",
    " guards two arms; first at ",
    ", which can never satisfy its declared return ",
    "no arm of `MATCH",
    "ambiguous `MATCH`: ",
];

/// The rules whose stored question no law compares yet. `SignatureValue` and `SignatureMeet` ask
/// only through a module the run builds (a carried type, a view, a path), which waits on module
/// substitution with `ViewTree`, `PartTree`, `Picks`, `IsRead`, `RootExact` and `ModuleBinder`.
const UNCONVERTED: &[SiteRule] = &[
    SiteRule::ViewTree,
    SiteRule::PartTree,
    SiteRule::Picks,
    SiteRule::IsRead,
    SiteRule::RootExact,
    SiteRule::ModuleBinder,
    SiteRule::SignatureValue,
    SiteRule::SignatureMeet,
];

/// Each ask answered by its door's rigid answer at the load.
struct Rigid<'q>(&'q [Posed<'q>]);

impl Asks for Rigid<'_> {
    fn ask(&mut self, index: u16) -> Option<Answer> {
        self.0[usize::from(index)].rigid
    }
}

/// Each ask answered with a binding's content written in, by the binding graph's fact domain.
struct Written<'q, 'n, 'x, 'tier, 'pair, 'reg, 'door> {
    types: &'x Chain<'tier, 'pair, 'reg, 'door>,
    scratch: &'x Bump,
    naming: Naming<'n>,
    asks: &'q [Posed<'q>],
    names: &'q [Named<'q>],
}

impl Asks for Written<'_, '_, '_, '_, '_, '_, '_> {
    fn ask(&mut self, index: u16) -> Option<Answer> {
        let question = &self.asks[usize::from(index)].question;
        ask(
            self.types,
            self.scratch,
            self.naming,
            None,
            question,
            self.names,
        )
    }
}

/// Each type with a binding's content written in, by the binding graph's fact domain: a refusal
/// template worded under the binding.
struct Applying<'q, 'n, 'x, 'tier, 'pair, 'reg, 'door> {
    types: &'x Chain<'tier, 'pair, 'reg, 'door>,
    scratch: &'x Bump,
    naming: Naming<'n>,
    names: &'q [Named<'q>],
}

impl Closer for Applying<'_, '_, '_, '_, '_, '_, '_> {
    fn close(&mut self, ty: DeclaredType<Parametric>) -> Closed {
        match apply(self.types, self.scratch, self.naming, ty, self.names) {
            Some(applied) if applied != ty => Closed::Partial(applied),
            _ => Closed::Same,
        }
    }

    fn concrete(&self, ty: Parametric) -> Option<KType> {
        self.types.concrete(ty)
    }

    fn rebuilt(
        &mut self,
        scheme: crate::type_lattice::Scheme,
        rebounds: crate::scope::Rebounds<'_>,
    ) -> Option<DeclaredType<KType>> {
        let (types, scratch) = (self.types, self.scratch);
        crate::scope::rebuilt_closed(types, scratch, scheme, rebounds, &mut |ty| self.close(ty))
    }
}

impl<'x> Laying<'x> for Applying<'_, '_, 'x, '_, '_, '_, '_> {
    type Lay = &'x Bump;

    fn lay(&self) -> &'x Bump {
        self.scratch
    }
}

/// What [`stored`] hands each question to: the unit laying it, rendered, the question, and the
/// route the walk recorded at its site.
type EachStored<'e> = dyn FnMut(&str, &SiteQuestion<'_>, Option<&[CandidateRoute]>) + 'e;

/// Each stored question `source`'s load lays, handed to `each`.
fn stored(source: &str, each: &mut EachStored<'_>) {
    let Ok(mut substrate) =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
    else {
        return;
    };
    substrate.with(|running| {
        let (facts, shapes) = (running.program().facts(), running.program().shapes());
        let bump = Bump::new();
        let types = reading(running.program().tiers(), &bump);
        let units =
            (facts.own_units()).map(|(unit, (own, _))| (format!("{unit:?}"), own.questions));
        let closings = (facts.own_closings())
            .map(|(body, (closing, _))| (format!("closing {body:?}"), closing.questions));
        for (record, questions) in units.chain(closings) {
            for question in questions.iter() {
                let scratch = Bump::new();
                let route = facts.route(
                    &types,
                    &scratch,
                    shapes.shape_at(question.segment),
                    question.site,
                );
                each(&record, question, route.map(|route| route.per));
            }
        }
    });
}

/// Each site of `SUBJECT`'s body and the bodies nested in it that the restart under `bound`
/// shares, keyed as the laws' renderers key a use: by the path of nested bodies, the
/// statement's index among those the restart keeps, and the use's index in the statement.
fn site_keys(
    shape: &BodyShape<'_>,
    symbols: &SymbolInterner,
    bound: &[String],
    path: &str,
    keys: &mut HashMap<(Segment, crate::scope::Site), String>,
) {
    // A statement binding a bound name is the restart's own: what it holds is no shared site.
    let skipped: Vec<usize> = (0..shape.slots())
        .map(|at| crate::scope::Slot(at as u32))
        .filter(|at| {
            let spelled = symbols.display(shape.slot_name(*at).symbol()).to_string();
            path.is_empty() && bound.contains(&spelled)
        })
        .filter_map(|at| shape.position_of(at).statement_index())
        .collect();
    let statements =
        (shape.body().iter().enumerate()).filter(|(index, _)| !skipped.contains(index));
    for (written, (_, statement)) in statements.enumerate() {
        let (mut uses, mut parts) = (vec![shape.site_of_node(statement)], Vec::new());
        each_part(statement, |part| {
            if let ExpressionPart::Expression(node) = part {
                uses.push(shape.site_of_node(node));
            }
            parts.push(shape.site(part));
        });
        for (at, site) in uses.into_iter().enumerate() {
            keys.insert(
                (shape.segment(), site),
                format!("{path} #{written} use {at}"),
            );
        }
        // A part no use sits at, such as a call by name's argument, keyed by its order.
        for (at, site) in parts.into_iter().enumerate() {
            (keys.entry((shape.segment(), site)))
                .or_insert_with(|| format!("{path} #{written} part {at}"));
        }
    }
    let within = |site| {
        (skipped.iter()).any(|index| {
            let statement = &shape.body()[*index];
            shape.site_of_node(statement) == site || part_of(shape, statement, site).is_some()
        })
    };
    let nested = (shape.nested_shapes().iter()).filter(|(site, _)| !within(*site));
    for (index, (_, nested)) in nested.enumerate() {
        site_keys(nested, symbols, bound, &format!("{path}/{index}"), keys);
    }
}

/// The arms an arms question keeps, a bit each, beside those that always admit: as the walk
/// records them, each by its order beside its verdict.
fn kept_arms(kept: u64, always: u64) -> Vec<(u16, Verdict)> {
    (0..64u16)
        .filter(|at| kept & 1 << at != 0)
        .map(|at| match always & 1 << at != 0 {
            true => (at, Verdict::Always),
            false => (at, Verdict::Maybe),
        })
        .collect()
}

/// The names a binding binds, as the laws leave them out of a render.
fn bound_names(bound: &Bound) -> Vec<String> {
    let mut names: Vec<String> = (bound.entries().iter())
        .map(|(name, _)| name.to_string())
        .collect();
    if bound.held.is_some() {
        names.push("hb".to_string());
    }
    names
}

/// A contribution as a line renders it: by its type's name, `None` for the carried type.
fn contribution(
    types: &Chain<'_, '_, '_, '_>,
    symbols: &SymbolInterner,
    typed: StaticType<'_>,
) -> Option<String> {
    let named = |ty: DeclaredType<Parametric>| display_name(ty, types, symbols).to_string();
    match typed {
        Static::Unknown => None,
        Static::Closed(closed) => Some(named(Parametric::from(closed).into())),
        Static::Rigid { value, .. } => Some(named(value.into())),
    }
}

/// An instance as a line renders it: by its type's name where that is closed, else `open`.
fn made(
    types: &Chain<'_, '_, '_, '_>,
    symbols: &SymbolInterner,
    ktype: Option<Parametric>,
) -> String {
    match ktype.filter(|ktype| types.concrete(*ktype).is_some()) {
        Some(ktype) => display_name(ktype, types, symbols).to_string(),
        None => "open".to_string(),
    }
}

/// Whether `question` reads a name the binding graph cannot write a binding's content into yet: a
/// path class's variable, or the type a module binder carries, directly or through a derived name's
/// term, whose module the graph's class door does not read (ruling R6 lays it with the frames).
fn reads_a_module(
    types: &Chain<'_, '_, '_, '_>,
    scratch: &Bump,
    load: &crate::scope::LoadFacts<'_>,
    question: &SiteQuestion<'_>,
) -> bool {
    let mut seen = Vec::new();
    let mut pending = Vec::new();
    for posed in question.asks {
        posed.question.each_type(&mut |ty| pending.push(ty));
    }
    while let Some(ty) = pending.pop() {
        for variable in free_variables(types, scratch, ty).iter() {
            match variable {
                Variable::Path { .. } => return true,
                Variable::Derived { term, .. } if !seen.contains(term) => {
                    seen.push(*term);
                    let Some(name) = load.derived_name(*term) else {
                        continue;
                    };
                    if matches!(name.term, DerivedTerm::Carried { .. }) {
                        return true;
                    }
                    name.term.each_type(&mut |ty| pending.push(ty));
                }
                _ => {}
            }
        }
    }
    false
}

/// Each type `bound` gives, named by its level as a stored question reads it.
fn named(door: &Door<'_, '_>, bound: &Bound) -> Vec<Named<'static>> {
    named_at(&levels(door, bound))
}

/// Each type `levels` gives a level, named by it as a stored question reads it.
fn named_at(levels: &[(usize, KType)]) -> Vec<Named<'static>> {
    (levels.iter())
        .map(|(level, ty)| Named {
            variable: Some(NameVariable::Level(*level)),
            address: NameAddress::Answered(DerivedId([*level as u64 + 1, 0])),
            upper: KType::ANY,
            content: Some(Content::Type(Parametric::from(*ty).into())),
        })
        .collect()
}

/// What the stored-question law answers `question` with `levels`' types written in: `None` for a
/// question no law compares, of a rule [`UNCONVERTED`] lists or reading a module's content.
pub(super) fn decided_at(
    door: &Door<'_, '_>,
    levels: &[(usize, KType)],
    question: &SiteQuestion<'_>,
) -> Option<SiteAnswer> {
    let (types, load) = (door.types, door.load());
    let scratch = Bump::new();
    if UNCONVERTED.contains(&question.rule) || reads_a_module(types, &scratch, load, question) {
        return None;
    }
    let named = named_at(levels);
    let names = |id| load.derived_name(id);
    let naming = Naming {
        names: &names,
        spellings: Spellings::new(door.symbols()),
        laid: &|_| {},
    };
    let mut asks = Written {
        types,
        scratch: &scratch,
        naming,
        asks: question.asks,
        names: &named,
    };
    Some(decide(question, &mut asks))
}

/// What the stored-question law answers each of the load's own questions under `bound`, in their
/// order: decided with the binding's content written into each ask. `None` for a question no law
/// compares: of a rule [`UNCONVERTED`] lists, or reading a module's content.
pub(super) fn written(door: &Door<'_, '_>, bound: &Bound) -> Vec<Option<SiteAnswer>> {
    let (types, load) = (door.types, door.load());
    let scratch = Bump::new();
    let named = named(door, bound);
    let names = |id| load.derived_name(id);
    let naming = Naming {
        names: &names,
        spellings: Spellings::new(door.symbols()),
        laid: &|_| {},
    };
    (load.own_questions())
        .map(|question| {
            let unread = UNCONVERTED.contains(&question.rule)
                || reads_a_module(types, &scratch, load, question);
            (!unread).then(|| {
                let mut asks = Written {
                    types,
                    scratch: &scratch,
                    naming,
                    asks: question.asks,
                    names: &named,
                };
                decide(question, &mut asks)
            })
        })
        .collect()
}

/// Each ask of each of the load's own questions, in their order, answered with `bound`'s content
/// written in: none for a question no law compares, or of a body outside `SUBJECT`, whose names
/// the binding does not bind.
fn asked(door: &Door<'_, '_>, bound: &Bound) -> Vec<Vec<Option<Answer>>> {
    let (types, load) = (door.types, door.load());
    let mut keys = HashMap::new();
    site_keys(subject(door.root()), door.symbols(), &[], "", &mut keys);
    let scratch = Bump::new();
    let named = named(door, bound);
    let names = |id| load.derived_name(id);
    let naming = Naming {
        names: &names,
        spellings: Spellings::new(door.symbols()),
        laid: &|_| {},
    };
    (load.own_questions())
        .map(|question| {
            if !keys.contains_key(&(question.segment, question.site))
                || UNCONVERTED.contains(&question.rule)
                || reads_a_module(types, &scratch, load, question)
            {
                return Vec::new();
            }
            let mut asks = Written {
                types,
                scratch: &scratch,
                naming,
                asks: question.asks,
                names: &named,
            };
            (0..question.asks.len() as u16)
                .map(|index| asks.ask(index))
                .collect()
        })
        .collect()
}

/// The chain of bindings `bound`'s type names widen through, in the order `seed` draws: nothing
/// bound, then one more at each step, to all of them. Its modules stay bound throughout.
fn widening(bound: &Bound, seed: u64) -> Vec<Bound> {
    let mut order: Vec<usize> = (0..3).collect();
    let mut state = seed;
    for at in (1..order.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        order.swap(at, (state >> 33) as usize % (at + 1));
    }
    let mut step = Bound {
        elt: None,
        key: None,
        item: None,
        ..bound.clone()
    };
    let mut chain = vec![step.clone()];
    for at in order {
        match at {
            0 => step.elt = bound.elt.clone(),
            1 => step.key = bound.key.clone(),
            _ => step.item = bound.item.clone(),
        }
        chain.push(step.clone());
    }
    chain
}

/// Whether `ask` reads a lexical variable at one of `levels`, directly or through a derived name's
/// term.
fn reads_levels(door: &Door<'_, '_>, scratch: &Bump, ask: &Question<'_>, levels: &[usize]) -> bool {
    let (types, load) = (door.types, door.load());
    let (mut seen, mut pending) = (Vec::new(), Vec::new());
    ask.each_type(&mut |ty| pending.push(ty));
    while let Some(ty) = pending.pop() {
        for variable in free_variables(types, scratch, ty).iter() {
            match variable {
                Variable::Lexical { level, .. } if levels.contains(level) => return true,
                Variable::Derived { term, .. } if !seen.contains(term) => {
                    seen.push(*term);
                    if let Some(name) = load.derived_name(*term) {
                        name.term.each_type(&mut |ty| pending.push(ty));
                    }
                }
                _ => {}
            }
        }
    }
    false
}

/// Whether the ask answer `narrow` is `wide` or narrower: no answer is the widest, a *maybe*
/// verdict or a run that *may* happen narrows to any verdict or run, and a type some value *may*
/// satisfy narrows to one none ever does; any other answer narrows to itself alone.
fn answer_narrows(wide: Option<Answer>, narrow: Option<Answer>) -> bool {
    match (wide, narrow) {
        _ if wide == narrow => true,
        (None, _) => true,
        (Some(Answer::Verdict(Verdict::Maybe)), Some(Answer::Verdict(_))) => true,
        (Some(Answer::Run(Run::May)), Some(Answer::Run(_))) => true,
        (Some(Answer::Satisfiable), Some(Answer::Never)) => true,
        _ => false,
    }
}

/// The lines each converted stored question of the load gives under `bound`: decided with its
/// content written into each ask, and its taken arm's payload applied.
fn decided_lines(door: &Door<'_, '_>, bound: &Bound) -> Vec<String> {
    let (types, symbols) = (door.types, door.symbols());
    let load = door.load();
    let mut keys = HashMap::new();
    site_keys(
        subject(door.root()),
        symbols,
        &bound_names(bound),
        "",
        &mut keys,
    );
    let scratch = Bump::new();
    let named = named(door, bound);
    let names = |id| load.derived_name(id);
    let naming = Naming {
        names: &names,
        spellings: Spellings::new(symbols),
        laid: &|_| {},
    };
    let written = |question: &SiteQuestion<'_>| {
        let mut asks = Written {
            types,
            scratch: &scratch,
            naming,
            asks: question.asks,
            names: &named,
        };
        decide(question, &mut asks)
    };
    // A use the binding refuses contributes nothing: in a quote's code, the refusal is no load's.
    let refused: Vec<_> = (load.own_questions())
        .filter(|question| question.rule == SiteRule::Selection)
        .filter(|question| matches!(written(question), SiteAnswer::Refused(_)))
        .map(|question| (question.segment, question.site))
        .collect();
    let mut lines = Vec::new();
    for question in load.own_questions() {
        let Some(key) = keys.get(&(question.segment, question.site)) else {
            continue;
        };
        if UNCONVERTED.contains(&question.rule) {
            continue;
        }
        // A question over a module's content is not compared, so it may refuse unseen.
        if reads_a_module(types, &scratch, load, question) {
            lines.push(UNSEEN.to_string());
            continue;
        }
        let mut asks = Written {
            types,
            scratch: &scratch,
            naming,
            asks: question.asks,
            names: &named,
        };
        let decided = decide(question, &mut asks);
        // A refusal, worded from its template with the binding written in. One in a quote's
        // code refuses no load: the `EVAL` running the code reports it.
        if let SiteAnswer::Refused(at) = decided
            && !load.shapes().in_code(question.segment)
            && let Payload::Refusal(template) = question.arms[usize::from(at)]
        {
            let mut applying = Applying {
                types,
                scratch: &scratch,
                naming,
                names: &named,
            };
            let worded = template.closed_laying(&mut applying);
            let worded = worded.display(symbols, types).to_string();
            lines.push(format!("{REFUSES}{}", unlocated(&worded)));
        }
        match (question.rule, decided.arm()) {
            (SiteRule::Contribution | SiteRule::ByName, Some(arm)) => {
                // A call that never runs contributes nothing.
                let Payload::Contributions(laid) = question.arms[usize::from(arm)] else {
                    continue;
                };
                if refused.contains(&(question.segment, question.site)) {
                    continue;
                }
                for (position, typed) in laid.iter().enumerate() {
                    let applied = match *typed {
                        Static::Rigid { value, variables } => {
                            match apply(types, &scratch, naming, value.into(), &named) {
                                Some(DeclaredType::Type(value)) => {
                                    Static::Rigid { value, variables }
                                }
                                _ => continue,
                            }
                        }
                        typed => typed,
                    };
                    if let Some(line) = contribution(types, symbols, applied) {
                        lines.push(canonical(&format!("{key} contributes {position} {line}")));
                    }
                }
            }
            // An undecided contribution leaves the use's selection, or the solve, to the
            // call; a refusal fixes nothing.
            (SiteRule::Contribution | SiteRule::ByName, None) => {}
            (SiteRule::InstanceAtFunction, Some(arm)) => {
                let Payload::Instance(instance) = question.arms[usize::from(arm)] else {
                    panic!("an instance question's arm is its instance");
                };
                let ktype = match instance {
                    StaticInstance::Closed { ktype, .. } => Some(Parametric::from(ktype)),
                    StaticInstance::Rigid { ktype, .. } => {
                        match apply(types, &scratch, naming, ktype.into(), &named) {
                            Some(DeclaredType::Type(ty)) => Some(ty),
                            _ => None,
                        }
                    }
                };
                let made = made(types, symbols, ktype);
                lines.push(canonical(&format!("{key} instance {made}")));
            }
            // No instance lies under the wanted type: the binding refuses.
            (SiteRule::InstanceAtFunction, None) => {}
            (SiteRule::TailRoute, _) => {
                let [Payload::Routing(routing)] = question.arms else {
                    panic!("a tail route's one arm is its routing");
                };
                let kept = match decided {
                    SiteAnswer::Bool(kept) => Some(kept),
                    _ => None,
                };
                let per = [CandidateRoute {
                    kept,
                    hop: routing.hop,
                }];
                lines.push(format!("{key} routes {per:?}"));
            }
            // A view settles where its operand fits, else nothing; a refusal is worded above.
            (SiteRule::View, _) => {
                lines.push(format!("{WHOLE}{key} settled"));
                if decided == SiteAnswer::Bool(true) {
                    lines.push(format!("{key} settled"));
                }
            }
            // A hold settles where its value fits, else nothing; an exact module's hold is
            // undecided. A refusal is worded above.
            (SiteRule::Hold, _) => {
                if decided != SiteAnswer::Undecided {
                    lines.push(format!("{WHOLE}{key} settled"));
                }
                if decided == SiteAnswer::Bool(true) {
                    lines.push(format!("{key} settled"));
                }
            }
            // A member read or a hold re-stamps at its declared type, applied. An undecided
            // member read is read at its value, recording no retype; an undecided hold never
            // arrives, is refused, or holds an exact module, re-stamped at its own type.
            (SiteRule::MemberRetype | SiteRule::HoldRetype | SiteRule::ArmRestamp, arm) => {
                if question.rule != SiteRule::HoldRetype || arm.is_some() {
                    lines.push(format!("{WHOLE}{key} retypes"));
                }
                let Some(arm) = arm else {
                    continue;
                };
                let Payload::Restamp(restamp) = question.arms[usize::from(arm)] else {
                    panic!("a member retype's arms are its restamps");
                };
                let applied = apply(types, &scratch, naming, restamp.at.into(), &named);
                if let Some(DeclaredType::Type(at)) = applied
                    && let Some(at) = types.concrete(at)
                {
                    let line = retype_line(types, symbols, restamp.how.at(at));
                    lines.push(canonical(&format!("{key} retypes {line}")));
                }
            }
            // An application's type is its rule's decided names; a refusal is worded above.
            (
                SiteRule::Application
                | SiteRule::Overlap
                | SiteRule::Splice
                | SiteRule::Eval
                | SiteRule::NeverCode
                | SiteRule::ReadRefusal
                | SiteRule::ReturnCheck
                | SiteRule::RepeatedGuard,
                _,
            ) => {}
            // Which arms run: the chosen one, else those kept; none where the scrutinee never
            // arrives. A refusal is worded above.
            (SiteRule::ArmsRun, _) => {
                lines.push(format!("{WHOLE}{key} arms"));
                match decided {
                    SiteAnswer::Arm(arm) => lines.push(format!("{key} arms chosen {}", arm - 1)),
                    SiteAnswer::Kept { kept, always, .. } => {
                        let kept = kept_arms(kept, always);
                        lines.push(format!("{key} arms kept {kept:?}"));
                    }
                    _ => {}
                }
            }
            (SiteRule::Selection, _) => {
                if let Some(narrowed) = selected(question, decided) {
                    lines.push(canonical(&format!("{key} narrows {narrowed}")));
                }
                if let Some(per) = routes(question, decided, &mut asks) {
                    lines.push(format!("{key} routes {per:?}"));
                }
                // The selected candidate's points: each exact one its upper end applied.
                let (SiteAnswer::Arm(arm), [_, arms @ ..]) = (decided, question.arms) else {
                    continue;
                };
                let Payload::Candidate(chosen) = arms[usize::from(arm) - 1] else {
                    continue;
                };
                // What its call passes: each closed term, applied.
                let shapes = load.shapes();
                for (slot, passing) in chosen.passes.iter().flat_map(|passes| {
                    let callee = shapes.shape_at(passes.callee);
                    (passes.slots.iter()).map(move |(slot, passing)| {
                        (spelled_slot(symbols, callee, *slot), *passing)
                    })
                }) {
                    let term = match passing {
                        Passing::Fixed(fixed) => {
                            let owner = shapes.shape_at(shapes.owner(question.segment));
                            let line = fixed_line(types, symbols, owner, fixed);
                            lines.push(canonical(&format!("{key} passes {slot} {line}")));
                            continue;
                        }
                        Passing::Point(at) => (chosen.points)
                            .and_then(|points| points.get(usize::from(at)))
                            .filter(|point| asks.ask(point.exact) == Some(Answer::Bool(true)))
                            .map(|point| point.upper),
                        Passing::Type(ty) => Some(ty),
                        Passing::Carried(typed) => Some(typed.upper),
                    };
                    let closed =
                        |ty: Parametric| match apply(types, &scratch, naming, ty.into(), &named) {
                            Some(DeclaredType::Type(ty)) => types.concrete(ty),
                            _ => None,
                        };
                    // A carried type passes where the binding makes the argument's ends one type.
                    let exact = match passing {
                        Passing::Carried(typed) => closed(typed.lower)
                            .is_some_and(|lower| term.and_then(closed) == Some(lower)),
                        _ => true,
                    };
                    if exact && let Some(closed) = term.and_then(closed) {
                        let line = fixed_line(
                            types,
                            symbols,
                            shapes.shape_at(shapes.owner(question.segment)),
                            Fixed::Content(ContentRecord::Type(closed)),
                        );
                        lines.push(canonical(&format!("{key} passes {slot} {line}")));
                    }
                }
                let Some(points) = chosen.points else {
                    continue;
                };
                let spelled = |ty: Parametric| display_name(ty, types, symbols).to_string();
                let mut solved = Vec::new();
                for point in points {
                    let exact = asks.ask(point.exact) == Some(Answer::Bool(true));
                    let applied = exact
                        .then(|| apply(types, &scratch, naming, point.upper.into(), &named))
                        .flatten();
                    solved.push(match applied {
                        Some(DeclaredType::Type(ty)) if types.concrete(ty).is_some() => spelled(ty),
                        _ => "open".to_string(),
                    });
                }
                lines.push(canonical(&format!("{key} solves {solved:?}")));
            }
            (rule, _) => panic!("{rule:?} is converted but has no comparison"),
        }
    }
    lines
}

/// The narrowing a use's selection question gives on `decided`, as [`narrowed`] renders the
/// restart's: by each candidate's position on the use's list. `None` where the call decides.
fn selected(question: &SiteQuestion<'_>, decided: SiteAnswer) -> Option<String> {
    let [Payload::Selection(layout), arms @ ..] = question.arms else {
        panic!("a selection question's first arm is its layout");
    };
    let position = |at: usize| match arms[at] {
        Payload::Candidate(candidate) if candidate.one => usize::from(candidate.position),
        _ => usize::MAX,
    };
    Some(match decided {
        SiteAnswer::Verdict(Verdict::Never) if layout.single => "selected 0".to_string(),
        SiteAnswer::Verdict(Verdict::Never) => "full".to_string(),
        SiteAnswer::Arm(arm) => format!("selected {}", position(usize::from(arm) - 1)),
        SiteAnswer::Kept {
            kept,
            always,
            ranked,
        } => {
            if kept.count_ones() == u32::from(layout.listed) && always == 0 && !ranked {
                return Some("full".to_string());
            }
            (0..usize::from(layout.refusals) - 1)
                .filter(|at| kept & 1 << at != 0)
                .fold("kept".to_string(), |text, at| {
                    let verdict = match always & 1 << at != 0 {
                        true => Verdict::Always,
                        false => Verdict::Maybe,
                    };
                    format!("{text} {}:{verdict:?}", position(at))
                })
        }
        _ => return None,
    })
}

/// Where the use's selection, read as a call's type reads it ([`Selection::runs`]), answers at
/// the load what a candidate does, the walk's outcome `decided` agrees: the one it selects runs,
/// one it drops never does, one it keeps may run or is selected, and one left to the call may
/// run.
fn runs_drift(question: &SiteQuestion<'_>, decided: SiteAnswer) -> Vec<String> {
    let [Payload::Selection(layout), arms @ ..] = question.arms else {
        return Vec::new();
    };
    let asks: Vec<Question<'_>> = question.asks.iter().map(|posed| posed.question).collect();
    let candidates: Vec<Selectable<'_>> = (arms.iter())
        .filter_map(|arm| match arm {
            Payload::Candidate(candidate) => Some(**candidate),
            _ => None,
        })
        .collect();
    let selection = Selection {
        asks: &asks,
        layout: *layout,
        candidates: &candidates,
    };
    let mut drifted = Vec::new();
    for at in 0..candidates.len() as u16 {
        let Some(run) = selection.runs(at, &mut Rigid(question.asks)) else {
            continue;
        };
        let agrees = match decided {
            SiteAnswer::Arm(arm) if arm == at + 1 => run == Run::Selected,
            SiteAnswer::Arm(_) => run == Run::Never,
            SiteAnswer::Kept { kept, .. } if kept & 1 << at == 0 => run == Run::Never,
            SiteAnswer::Kept { .. } => run != Run::Never,
            SiteAnswer::Undecided => run == Run::May,
            _ => true,
        };
        if !agrees {
            drifted.push(format!("candidate {at} runs {run:?}, the walk {decided:?}"));
        }
    }
    drifted
}

/// The route a use's selection question gives on `decided`, per candidate the call reads, in its
/// order, as the walk records it ([`Selectable::routed`](crate::scope::Selectable::routed)).
/// `None` where the use is no tail call, or the decision leaves the call nothing to route.
fn routes(
    question: &SiteQuestion<'_>,
    decided: SiteAnswer,
    asks: &mut dyn Asks,
) -> Option<Vec<CandidateRoute>> {
    let [Payload::Selection(layout), arms @ ..] = question.arms else {
        panic!("a selection question's first arm is its layout");
    };
    let candidate = |at: usize| match arms[at] {
        Payload::Candidate(candidate) => candidate,
        _ => panic!("a selection question's arms past its layout are its candidates"),
    };
    match decided {
        SiteAnswer::Arm(arm) => Some(vec![candidate(usize::from(arm) - 1).routed(true, asks)?]),
        SiteAnswer::Kept { kept, always, .. } => (0..usize::from(layout.refusals) - 1)
            .filter(|at| kept & 1 << at != 0)
            .map(|at| candidate(at).routed(always & 1 << at != 0, asks))
            .collect(),
        _ => None,
    }
}

/// A retype by its kind and the type it re-stamps at, displayed.
fn retype_line(types: &Chain<'_, '_, '_, '_>, symbols: &SymbolInterner, retype: Retype) -> String {
    let named = |ty: KType| display_name(Parametric::from(ty), types, symbols).to_string();
    match retype {
        Retype::Point(at) => format!("point {}", named(at)),
        Retype::Keeps => "keeps".to_string(),
        Retype::Computed(at) => format!("computed {}", named(at)),
        Retype::Fit(at) => format!("fit {}", named(at)),
    }
}

/// `slot` of `body`, by its name: a body the restart declares a binding in numbers its slots anew.
fn spelled_slot(symbols: &SymbolInterner, body: &BodyShape<'_>, slot: Slot) -> String {
    symbols.display(body.slot_name(slot).symbol()).to_string()
}

/// What a call passes at a slot, as the trace records it: a closed type, a module at the type it
/// carries in, or a slot of the caller `owner`, passed on, by its name.
fn fixed_line(
    types: &Chain<'_, '_, '_, '_>,
    symbols: &SymbolInterner,
    owner: &BodyShape<'_>,
    fixed: Fixed,
) -> String {
    let named = |ty: KType| display_name(Parametric::from(ty), types, symbols).to_string();
    match fixed {
        Fixed::Content(ContentRecord::Type(ty)) => format!("type {}", named(ty)),
        Fixed::Content(ContentRecord::Module { at, .. }) => format!("module at {}", named(at)),
        Fixed::Passed(from) => format!("on {}", spelled_slot(symbols, owner, from)),
    }
}

/// What the restart `source`'s walk fixed at each site a converted rule decides, keyed as
/// [`decided_lines`] keys them, or the restart's refusal.
fn restarted_lines(source: &str, bound: &Bound) -> Result<Vec<String>, String> {
    let loaded = CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output());
    let mut substrate = match loaded {
        Ok(substrate) => substrate,
        Err(LoadError::Shape { rendered }) => return Err(rendered),
        Err(error) => panic!("the restart parses: {error}\n{source}"),
    };
    substrate.with(|running| {
        let bump = Bump::new();
        let types = reading(running.program().tiers(), &bump);
        let symbols = running.symbols();
        let facts = running.program().facts();
        let mut keys = HashMap::new();
        let root = running.program().shapes().root();
        site_keys(subject(root), symbols, &bound_names(bound), "", &mut keys);
        let shapes = running.program().shapes();
        // Each keyworded use's trace, by its body and site.
        let traced: HashMap<_, _> = (facts.own_units())
            .flat_map(|(_, (unit, _))| unit.typed.iter())
            .flat_map(|(at, typed)| {
                typed
                    .traced
                    .iter()
                    .map(move |(site, traced)| ((*at, *site), *traced))
            })
            .filter(|(_, traced)| !traced.by_name)
            .collect();
        let mut lines = Vec::new();
        for ((segment, site), key) in &keys {
            let shape = shapes.shape_at(*segment);
            let closing = Bump::new();
            let named = facts.named_contributions(&types, &closing, shape, *site);
            for (position, typed) in named.iter().enumerate() {
                if let Some(line) = contribution(&types, symbols, *typed) {
                    lines.push(canonical(&format!("{key} contributes {position} {line}")));
                }
            }
            // An instance read here, or the one a callable body nested here is born at.
            let read = match facts.instance_at(&types, &closing, shape, *site) {
                Some(InstanceRead::Solved(instance)) => Some(instance),
                _ => (shape.nested(*site))
                    .and_then(|body| facts.born_instance(&types, &closing, body)),
            };
            if let Some(instance) = read {
                let ktype = match instance {
                    StaticInstance::Closed { ktype, .. } => Some(Parametric::from(ktype)),
                    StaticInstance::Rigid { .. } => None,
                };
                let made = made(&types, symbols, ktype);
                lines.push(canonical(&format!("{key} instance {made}")));
            }
            if facts.settled(shape, *site) {
                lines.push(format!("{key} settled"));
            }
            if let Some(arms) = facts.arms(&Bump::new(), shape, *site) {
                lines.push(match arms.chosen {
                    Some(chosen) => format!("{key} arms chosen {chosen}"),
                    None => format!("{key} arms kept {:?}", arms.kept),
                });
            }
            if let Some(retype) = facts.retype(&types, shape, *site) {
                let line = retype_line(&types, symbols, retype);
                lines.push(canonical(&format!("{key} retypes {line}")));
            }
            let Some((_, list)) = shape.candidate_lists().get(*site) else {
                // A call by name's route, at its node.
                if let Some(route) = facts.route(&types, &Bump::new(), shape, *site) {
                    lines.push(format!("{key} routes {:?}", route.per));
                }
                continue;
            };
            let fixed = facts.use_statics(&types, &closing, shape, *site);
            let narrowing = narrowed(fixed.narrowing, list);
            lines.push(canonical(&format!("{key} narrows {narrowing}")));
            if let Some(route) = facts.route(&types, &Bump::new(), shape, *site) {
                lines.push(format!("{key} routes {:?}", route.per));
            }
            // What the call its walk selected passes, each slot the trace follows at.
            if let Some(traced) = traced.get(&(*segment, *site)) {
                let callee = shapes.shape_at(traced.callee);
                let owner = shapes.shape_at(shapes.owner(*segment));
                for (slot, fixed) in traced.fixed {
                    let slot = spelled_slot(symbols, callee, *slot);
                    let line = fixed_line(&types, symbols, owner, *fixed);
                    lines.push(canonical(&format!("{key} passes {slot} {line}")));
                }
            }
            if let Some(points) = facts.selection(&types, &closing, shape, *site) {
                let named = |ty: Parametric| display_name(ty, &types, symbols).to_string();
                let solved: Vec<String> = (points.points.iter())
                    .map(|point| match point {
                        Point::Closed(point) => named(Parametric::from(*point)),
                        Point::Rigid(_) | Point::Carried(_) => "open".to_string(),
                    })
                    .collect();
                lines.push(canonical(&format!("{key} solves {solved:?}")));
            }
            for (position, typed) in fixed.contributions.iter().enumerate() {
                if let Some(line) = contribution(&types, symbols, *typed) {
                    lines.push(canonical(&format!("{key} contributes {position} {line}")));
                }
            }
        }
        Ok(lines)
    })
}

#[test]
fn every_rule_is_converted_or_listed_once() {
    for rule in SiteRule::ALL {
        let listed = UNCONVERTED.iter().filter(|held| **held == rule).count();
        assert!(listed <= 1, "{rule:?} is listed twice");
    }
}

/// A use whose argument's type is a run-bound name lays its contribution question: the argument
/// `x :Elt` contributes `Elt`, unless a binding makes `Elt` `Any`.
///
/// ```text
/// EXPR FOR ALL #[Tee] #(PICK x :Tee) -> Tee = #(x)
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(PICK x)
/// ```
#[test]
fn a_use_over_a_run_bound_argument_lays_its_contribution_question() {
    let source = "EXPR FOR ALL #[Tee] #(PICK x :Tee) -> Tee = #(x)\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(PICK x)";
    let mut laid = Vec::new();
    stored(source, &mut |_, question, _| {
        if question.rule == SiteRule::Contribution {
            laid.push((question.asks.len(), question.taken));
        }
    });
    assert_eq!(laid, [(2, SiteAnswer::Arm(0))]);
}

/// A use whose argument a binding may make `Never` lays its selection question, which selects the
/// one candidate where the argument arrives.
///
/// ```text
/// EXPR FOR ALL #[Tee] #(PICK x :Tee) -> Tee = #(x)
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(PICK x)
/// ```
#[test]
fn a_use_over_a_run_bound_argument_lays_its_selection_question() {
    let source = "EXPR FOR ALL #[Tee] #(PICK x :Tee) -> Tee = #(x)\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(PICK x)";
    let mut laid = Vec::new();
    stored(source, &mut |_, question, _| {
        if question.rule == SiteRule::Selection {
            laid.push(question.taken);
        }
    });
    assert_eq!(laid, [SiteAnswer::Arm(1)]);
}

/// A use's selection question lays what each candidate's call passes its callee: `PICK`'s `Tee`
/// takes the candidate's point, `x`'s type, which a binding of `Elt` writes in.
///
/// ```text
/// EXPR FOR ALL #[Tee] #(PICK x :Tee) -> Tee = #(x)
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(PICK x)
/// ```
#[test]
fn a_use_over_a_run_bound_argument_lays_what_its_candidate_passes() {
    let source = "EXPR FOR ALL #[Tee] #(PICK x :Tee) -> Tee = #(x)\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(PICK x)";
    let mut laid = Vec::new();
    stored(source, &mut |_, question, _| {
        if question.rule == SiteRule::Selection {
            for arm in question.arms {
                if let Payload::Candidate(candidate) = arm {
                    let passes = candidate.passes.expect("PICK is a callable body");
                    laid.extend(passes.slots.iter().map(|(_, passing)| *passing));
                }
            }
        }
    });
    assert_eq!(laid, [Passing::Point(0)]);
}

/// Each ask answered rigidly where its door answers, and every `Same` left open answered yes where
/// it asks of the constant `of`, as a binding making a name that constant would.
struct Making<'q>(&'q [Posed<'q>], KType);

impl Asks for Making<'_> {
    fn ask(&mut self, index: u16) -> Option<Answer> {
        let posed = &self.0[usize::from(index)];
        match posed.question {
            Question::Same { b, .. } if posed.rigid.is_none() => Some(Answer::Bool(
                b == DeclaredType::Type(Parametric::from(self.1)),
            )),
            _ => posed.rigid,
        }
    }
}

/// A call by name of a quantified callable over a run-bound argument lays its question: `x :Elt`
/// contributes `Elt` to the callee's `Tee`, unless a binding makes `Elt` `Never`, where the call
/// never runs, or `Any`, which leaves the solve to the call.
///
/// ```text
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
///   LET c = ((FN FOR ALL #[Tee] :{y :Tee} -> Tee = #(y)) {y = x})
///   c)
/// ```
#[test]
fn a_call_by_name_over_a_run_bound_argument_lays_its_question() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET c = ((FN FOR ALL #[Tee] :{y :Tee} -> Tee = #(y)) {y = x})\n  \
                  c)";
    let mut laid = Vec::new();
    stored(source, &mut |_, question, _| {
        if question.rule == SiteRule::ByName {
            let at = |made| decide(question, &mut Making(question.asks, made));
            laid.push((question.taken, at(KType::NEVER), at(KType::ANY)));
        }
    });
    assert_eq!(
        laid,
        [(
            SiteAnswer::Arm(0),
            SiteAnswer::Arm(1),
            SiteAnswer::Undecided
        )]
    );
}

/// A call by name whose instance field is wanted at a type its other field solves lays that
/// slot's check: a binding making `Elt` `Any` leaves `Tee` unfixed, which refuses the call in the
/// slot's words, as a fresh load refuses it.
///
/// ```text
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
///   ((FN FOR ALL #[Tee] :{y :Tee, f :(FN :{z :Tee} -> Tee)} -> Any = #(y)) {y = x, f = (FN FOR ALL #[Que] :{z :Que} -> Que = #(z))}))
/// ```
#[test]
fn a_call_by_name_leaving_its_instance_slot_unfixed_refuses_in_its_words() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  ((FN FOR ALL #[Tee] :{y :Tee, f :(FN :{z :Tee} -> Tee)} -> Any = #(y)) \
                  {y = x, f = (FN FOR ALL #[Que] :{z :Que} -> Que = #(z))}))";
    let mut laid = Vec::new();
    stored(source, &mut |_, question, _| {
        if question.rule == SiteRule::ByName {
            let SiteAnswer::Refused(at) = decide(question, &mut Making(question.asks, KType::ANY))
            else {
                panic!("a binding making `Elt` `Any` refuses the call");
            };
            let Payload::Refusal(refusal) = question.arms[usize::from(at)] else {
                panic!("a refusal's arm is its template");
            };
            laid.push((
                question.taken,
                matches!(refusal, ShapeError::Unfixed { .. }),
            ));
        }
    });
    assert_eq!(laid, [(SiteAnswer::Arm(0), true)]);
}

/// A view of a run-bound name lays its question: a binding making `x` no module refuses the view
/// in its words, and one making it a module that fits settles it.
///
/// ```text
/// SIG Zeroed = #[(VAL zero :Any)]
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
///   LET v = (x :| Zeroed)
///   x)
/// ```
#[test]
fn a_view_over_a_run_bound_name_lays_its_question() {
    let source = "SIG Zeroed = #[(VAL zero :Any)]\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET v = (x :| Zeroed)\n  \
                  x)";
    let mut laid = Vec::new();
    stored(source, &mut |_, question, _| {
        if question.rule == SiteRule::View {
            // Each open ask answered as a binding to a module, or to no module, would.
            let at = |module: bool| {
                decide(
                    question,
                    &mut AskedBy(|index| {
                        let posed = question.asks[usize::from(index)];
                        posed.rigid.or(Some(match posed.question {
                            Question::NeverSatisfies { .. } if module => Answer::Satisfiable,
                            Question::NeverSatisfies { .. } => Answer::Never,
                            Question::Kind { .. } => Answer::Kind(KKind::Signature),
                            // `x` arrives.
                            Question::Same { b, .. } if b == KType::NEVER.into() => {
                                Answer::Bool(false)
                            }
                            _ => Answer::Bool(true),
                        }))
                    }),
                )
            };
            let refused = match at(false) {
                SiteAnswer::Refused(arm) => matches!(
                    question.arms[usize::from(arm)],
                    Payload::Refusal(ShapeError::NotAModule { .. })
                ),
                _ => false,
            };
            laid.push((question.taken, refused, at(true)));
        }
    });
    assert_eq!(
        laid,
        [(SiteAnswer::Bool(false), true, SiteAnswer::Bool(true))]
    );
}

/// A spread of a run-bound name lays its question: a binding putting `x` outside every list
/// refuses the spread in its words.
///
/// ```text
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
///   LET sp = [..x 1]
///   x)
/// ```
#[test]
fn a_spread_over_a_run_bound_name_lays_its_refusal() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET sp = [..x 1]\n  \
                  x)";
    let mut laid = Vec::new();
    stored(source, &mut |_, question, _| {
        if question.rule == SiteRule::Splice {
            let outside = decide(question, &mut AskedBy(|_| Some(Answer::Bool(true))));
            let within = decide(question, &mut AskedBy(|_| Some(Answer::Bool(false))));
            let worded = match outside {
                SiteAnswer::Refused(at) => matches!(
                    question.arms[usize::from(at)],
                    Payload::Refusal(ShapeError::SpreadNeverAList { .. })
                ),
                _ => false,
            };
            laid.push((question.taken, worded, within));
        }
    });
    assert_eq!(laid, [(SiteAnswer::Undecided, true, SiteAnswer::Undecided)]);
}

/// Two guards of one arm set a binding may make one type lay their question: a binding making
/// `Elt` `Number` refuses the later guard in its words.
///
/// ```text
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
///   LET rg = (MATCH x WITH #{:Elt: (1), :Number: (2), _: (0)})
///   x)
/// ```
#[test]
fn guards_a_binding_may_make_alike_lay_their_refusal() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET rg = (MATCH x WITH #{:Elt: (1), :Number: (2), _: (0)})\n  \
                  x)";
    let mut laid = Vec::new();
    stored(source, &mut |_, question, _| {
        if question.rule == SiteRule::RepeatedGuard {
            let alike = decide(question, &mut AskedBy(|_| Some(Answer::Bool(true))));
            let apart = decide(question, &mut AskedBy(|_| Some(Answer::Bool(false))));
            let worded = match alike {
                SiteAnswer::Refused(at) => matches!(
                    question.arms[usize::from(at)],
                    Payload::Refusal(ShapeError::RepeatedGuard { .. })
                ),
                _ => false,
            };
            laid.push((question.taken, worded, apart));
        }
    });
    assert_eq!(laid, [(SiteAnswer::Undecided, true, SiteAnswer::Undecided)]);
}

/// A callable's return check over a run-bound name lays its question in the closing record of
/// the callable: where `Elt` is `Str`, the body's `Number` can never satisfy the declared return.
///
/// ```text
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
///   LET g = (FN :{} -> Elt = #(0))
///   x)
/// ```
#[test]
fn a_return_over_a_run_bound_name_lays_its_check_in_the_closing() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET g = (FN :{} -> Elt = #(0))\n  \
                  x)";
    let mut laid = Vec::new();
    stored(source, &mut |record, question, _| {
        if question.rule == SiteRule::ReturnCheck {
            let never = decide(question, &mut AskedBy(|_| Some(Answer::Never)));
            let worded = match never {
                SiteAnswer::Refused(at) => matches!(
                    question.arms[usize::from(at)],
                    Payload::Refusal(ShapeError::ReturnNeverSatisfied { .. })
                ),
                _ => false,
            };
            laid.push((record.starts_with("closing"), question.taken, worded));
        }
    });
    assert_eq!(laid, [(true, SiteAnswer::Undecided, true)]);
}

/// A `MATCH` whose guard is a run-bound name lays which of its arms run: at the load `:Elt` always
/// admits `x` and `:Number` may, unranked against it, so both are kept; a binding that puts `x`
/// outside `Number` leaves `:Elt` alone, which the `MATCH` chooses.
///
/// ```text
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
///   LET rg = (MATCH x WITH #{:Elt: (1), :Number: (2), _: (0)})
///   x)
/// ```
#[test]
fn a_match_over_a_run_bound_guard_lays_which_arms_run() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET rg = (MATCH x WITH #{:Elt: (1), :Number: (2), _: (0)})\n  \
                  x)";
    let number = DeclaredType::Type(Parametric::from(KType::NUMBER));
    let mut laid = Vec::new();
    stored(source, &mut |_, question, _| {
        if question.rule == SiteRule::ArmsRun {
            let outside = decide(
                question,
                &mut AskedBy(|at: u16| match question.asks[usize::from(at)].question {
                    Question::NeverSatisfies { wanted, .. }
                        if DeclaredType::Type(wanted) == number =>
                    {
                        Some(Answer::Never)
                    }
                    Question::NeverSatisfies { .. } => Some(Answer::Satisfiable),
                    Question::Fits { .. } => Some(Answer::Bool(true)),
                    _ => Some(Answer::Bool(false)),
                }),
            );
            laid.push((question.taken, outside));
        }
    });
    let kept = SiteAnswer::Kept {
        kept: 0b11,
        always: 0b01,
        ranked: false,
    };
    assert_eq!(laid, [(kept, SiteAnswer::Arm(1))]);
}

/// An instance wanted at a type over a run-bound name lays its question: whether an instance of
/// `FOR ALL #[Tee] :{y :Tee} -> Tee` lies under `FN :{y :Elt} -> Number` is the binding's, which
/// refuses where `Elt` is no `Number`.
///
/// ```text
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
///   LET w :(FN :{y :Elt} -> Number) = (FN FOR ALL #[Tee] :{y :Tee} -> Tee = #(y))
///   w)
/// ```
#[test]
fn an_instance_wanted_over_a_run_bound_name_lays_its_question() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
                  LET w :(FN :{y :Elt} -> Number) = (FN FOR ALL #[Tee] :{y :Tee} -> Tee = #(y))\n  \
                  w)";
    let mut laid = Vec::new();
    stored(source, &mut |_, question, _| {
        if question.rule == SiteRule::InstanceAtFunction {
            let at = |holds| decide(question, &mut AskedBy(|_| Some(Answer::Bool(holds))));
            laid.push((question.taken, at(true), at(false)));
        }
    });
    assert_eq!(
        laid,
        [(
            SiteAnswer::Arm(0),
            SiteAnswer::Arm(0),
            SiteAnswer::Refused(1)
        )]
    );
}

/// Each ask answered rigidly where its door answers, and every fit left open answered `holds`, as
/// a binding that decides it would.
struct Fitting<'q>(&'q [Posed<'q>], bool);

impl Asks for Fitting<'_> {
    fn ask(&mut self, index: u16) -> Option<Answer> {
        let posed = &self.0[usize::from(index)];
        match posed.question {
            Question::Fits { .. } if posed.rigid.is_none() => Some(Answer::Bool(self.1)),
            _ => posed.rigid,
        }
    }
}

/// A tail call whose frame's contract is over a run-bound name routes as the call decides at the
/// load, and as a binding decides the fit of its return to the contract: `PICK x` returns `Elt`,
/// which keeps `SUBJECT`'s contract `Number` where `Elt` is `Number`.
///
/// ```text
/// EXPR FOR ALL #[Tee] #(PICK x :Tee) -> Tee = #(x)
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Number = #(PICK x)
/// ```
#[test]
fn a_tail_call_over_a_run_bound_name_routes_as_its_binding_decides() {
    let source = "EXPR FOR ALL #[Tee] #(PICK x :Tee) -> Tee = #(x)\n\
                  EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Number = #(PICK x)";
    let mut routed = Vec::new();
    stored(source, &mut |_, question, recorded| {
        if question.rule == SiteRule::Selection {
            let at = |holds| {
                let asks = &mut Fitting(question.asks, holds);
                routes(question, decide(question, asks), asks)
            };
            routed.push((recorded.map(<[_]>::to_vec), at(true), at(false)));
        }
    });
    let route = |kept| Some(vec![CandidateRoute { kept, hop: false }]);
    assert_eq!(
        routed,
        [(route(None), route(Some(true)), route(Some(false)))]
    );
}

/// A call by name in the tail of a frame whose contract is over a run-bound name lays its route's
/// question: the callee returns `Elt`, which keeps `SUBJECT`'s contract `Number` where `Elt` is
/// `Number`.
///
/// ```text
/// EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Number = #(
///   (FN :{y :Elt} -> Elt = #(y)) {y = x})
/// ```
#[test]
fn a_call_by_name_in_a_tail_over_a_run_bound_name_routes_as_its_binding_decides() {
    let source = "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Number = #(\n  \
                  (FN :{y :Elt} -> Elt = #(y)) {y = x})";
    let mut routed = Vec::new();
    stored(source, &mut |_, question, _| {
        if question.rule == SiteRule::TailRoute {
            let at = |holds| decide(question, &mut Fitting(question.asks, holds));
            routed.push((question.taken, at(true), at(false)));
        }
    });
    assert_eq!(
        routed,
        [(
            SiteAnswer::Undecided,
            SiteAnswer::Bool(true),
            SiteAnswer::Bool(false)
        )]
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(crate::tests::case_share(1, 4)))]

    /// The walk's outcome at a site is the decision function's under each ask's rigid answer:
    /// the load and a binding decide by one function. A use's route is the one its selection
    /// question gives there.
    #[test]
    fn a_stored_question_decides_as_the_walk_did(case in resumable()) {
        let mut drifted = Vec::new();
        stored(&case.render(), &mut |unit, question, recorded| {
            let decided = decide(question, &mut Rigid(question.asks));
            if decided != question.taken {
                drifted.push(format!("{unit} {:?}: {decided:?}, the walk {:?}", question.rule, question.taken));
            } else if question.rule == SiteRule::Selection {
                let routed = routes(question, decided, &mut Rigid(question.asks));
                if routed.as_deref() != recorded {
                    drifted.push(format!("{unit} routes {routed:?}, the walk {recorded:?}"));
                }
                drifted.extend(runs_drift(question, decided).into_iter().map(|line| format!("{unit} {line}")));
            }
        });
        prop_assert!(drifted.is_empty(), "{:#?} in\n{}", drifted, case.render());
    }

    /// Under the case's binding, a converted stored question decides as the fresh load with the
    /// binding declared does at its site, and its taken arm's payload, applied, is what that load
    /// fixes there.
    #[test]
    fn a_stored_question_decides_as_a_fresh_load_with_its_binding_declared(case in resumable()) {
        let decided = opened(&case.render(), |door| decided_lines(door, &case.bound));
        let unseen = decided.iter().any(|line| line == UNSEEN);
        let (refusals, decided): (Vec<_>, Vec<_>) = (decided.into_iter())
            .filter(|line| line != UNSEEN)
            .partition(|line| line.starts_with(REFUSES));
        let (whole, decided): (Vec<_>, Vec<_>) =
            decided.into_iter().partition(|line| line.starts_with(WHOLE));
        let restart = case.restart(&case.bound);
        match restarted_lines(&restart, &case.bound) {
            Ok(restarted) => {
                // A binding a stored question refuses, outside a quote's code, refuses the load.
                prop_assert!(refusals.is_empty(), "{:?} but the restart loads\n{}\nrestarted as\n{}", refusals, case.render(), restart);
                for line in &decided {
                    prop_assert!(restarted.contains(line), "{} not in the restart\n{}\nrestarted as\n{}\n{:#?}", line, case.render(), restart, restarted);
                }
                // Where a question decides all the restart fixes at its site, it fixes no less.
                for line in &restarted {
                    let decides = whole.iter().any(|marked| line.starts_with(&marked[WHOLE.len()..]));
                    prop_assert!(!decides || decided.contains(line), "{} not decided\n{}\nrestarted as\n{}\n{:#?}", line, case.render(), restart, decided);
                }
            }
            // A restart that refuses fixes no site to compare; where a stored question refuses,
            // a restart refusing in a converted rule's words refuses as one does.
            Err(refused) => {
                let refused = format!("{REFUSES}{}", unlocated(&refused));
                let converted = TEMPLATED.iter().any(|words| refused.contains(words));
                let seen = refusals.is_empty() || unseen || !converted;
                prop_assert!(seen || refusals.contains(&refused), "the restart {} is no stored question's\n{}\nrestarted as\n{}\n{:#?}", refused, case.render(), restart, refusals);
            }
        }
    }

    /// **Monotone**: binding more of the case's type names, one more at a time in any order, only
    /// narrows each ask of each compared stored question: an ask answered over a name left open
    /// answers for every binding of it. Once a name is bound to a type no value carries, a call
    /// over a value of it never runs, and a verdict reading the name is vacuous: `Never` stands for
    /// any.
    #[test]
    fn a_stored_question_s_asks_narrow_as_its_binding_grows(case in resumable(), seed in any::<u64>()) {
        let chain = widening(&case.bound, seed);
        let reverted = opened(&case.render(), |door| {
            let steps: Vec<Vec<Vec<Option<Answer>>>> =
                chain.iter().map(|bound| asked(door, bound)).collect();
            let scratch = Bump::new();
            let mut reverted = Vec::new();
            for (step, pair) in steps.windows(2).enumerate() {
                // The levels of the names the step binds to a type no value carries.
                let bound = &chain[step + 1];
                let uninhabited = |desc: &Option<Desc>| desc.clone().filter(|desc| !inhabited(desc));
                let empty = Bound {
                    elt: uninhabited(&bound.elt),
                    key: uninhabited(&bound.key),
                    item: uninhabited(&bound.item),
                    ..Bound::default()
                };
                let empty: Vec<usize> = levels(door, &empty).iter().map(|(level, _)| *level).collect();
                let questions = door.load().own_questions();
                for ((wide, narrow), question) in pair[0].iter().zip(&pair[1]).zip(questions) {
                    for (index, (wide, narrow)) in wide.iter().zip(narrow).enumerate() {
                        let vacuous = matches!(
                            (wide, narrow),
                            (Some(Answer::Verdict(_)), Some(Answer::Verdict(Verdict::Never)))
                        ) && reads_levels(door, &scratch, &question.asks[index].question, &empty);
                        if !(answer_narrows(*wide, *narrow) || vacuous) {
                            reverted.push(format!(
                                "step {step}: ask {index} of {:?} at {:?} of {:?}: {wide:?}, then {narrow:?}",
                                question.rule, question.site, question.segment
                            ));
                        }
                    }
                }
            }
            reverted
        });
        prop_assert!(reverted.is_empty(), "{:#?} in\n{}\nunder {:?}", reverted, case.render(), case.bound);
    }
}

/// The monotone law bites: on a fixed seed, one of 128 cases narrows an ask as its binding grows.
#[test]
fn the_monotone_law_narrows_an_ask() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{RngAlgorithm, TestRng, TestRunner};
    let mut runner = TestRunner::new_with_rng(
        ProptestConfig::default(),
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let strategy = resumable();
    let narrowed = (0..128).any(|_| {
        let case = strategy.new_tree(&mut runner).expect("a case").current();
        opened(&case.render(), |door| {
            let chain = widening(&case.bound, 0);
            asked(door, &chain[0]) != asked(door, &chain[chain.len() - 1])
        })
    });
    assert!(narrowed, "no case narrows an ask");
}
