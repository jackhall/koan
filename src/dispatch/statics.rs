//! Static selection: every value expression and value binder given a **static type** where the
//! program loads, and every keyworded use's candidates narrowed by them.
//!
//! A static type is an [`Interval`]: every type the run carries at that expression lies within it.
//! An exact one is a point. The pass is local and bidirectional over what the
//! [type channel's load pass](crate::elaborate::type_channel) fixed — a parameter's declared type
//! is its reads', an ascription's type its own, a callee's declared return its calls', and a
//! literal's, a container's, a construction's and a selected candidate's own type flow up. A
//! parameter or an ascription is exactly its type where the retype makes it so — a list, dict or
//! record type or a nominal one — since the run retypes the value to it, and at most its type
//! otherwise. What the load cannot bound is `[Never, Any]`. A static type may hold the lexical
//! variables of its **chain** — the shapes from the program or a quote's code down, numbered as the
//! type channel numbers them — and a crossing into code, or an `EVAL` leaving it, is read through
//! [`bound_above`], so no variable leaks across.
//!
//! Each keyworded use **judges** each candidate class by class ([`judge_by_class`]): *never*
//! drops it, *always* means it admits whatever the run carries, and a *maybe* one the call admits.
//! An argument at a slot whose class solves a variable ([`solving_slots`]) **contributes** its
//! static upper end to the solve, unless that is `Any`: the judge reads it as exactly that type, and
//! the use records it for the call. A contribution naming a lexical variable records where the use
//! reads it — a hop per block, and a **type capture** per callable or module between the use and the
//! variable's home, which the shape lays down past its builder's captures.
//! A use left with none refuses the load. A *maybe* an *always* one outranks at class 0, both
//! closed, drops too. Where no *maybe* is left, a lone candidate, or the one closed candidates rank
//! first, is **selected** and runs without admitting; where they rank none first, the load refuses
//! the certain ambiguity. A builtin candidate is judged through its native's [type rule](super::rules)
//! too: an argument whose lower end lies outside the type the rule needs drops it, and a use whose
//! last candidate a need dropped is refused in the native's words. A builtin's call is the return
//! its rule gives — `FROM`'s the projection of its record and `ATTR`'s the named field's type. An
//! `EVAL` is its declared type, as a registration's call is, and code the load traces to a written
//! quote is checked against it as a body is checked against its return. A
//! quantified candidate's return is read through its group's intervals. A call whose callee and
//! solve the load knows exactly — a selected registration, or a call by name of an exact callee —
//! is exactly the return its frame retypes to; a call in tail position never finishes, and is
//! typed as any other. A callable body whose static type meets its declared return at `Never`
//! refuses the load too, as does an ascription whose operand's static type meets its type at
//! `Never`; one whose operand's static upper end lies under its type is **settled**, and the run
//! checks nothing. An annotated binder, `LET <name> <type> = <value>`, is held to its type as an
//! ascription holds its operand, settled by its type part's site. A call by name whose callee is
//! exactly an unquantified function refuses the load where its argument can never satisfy the
//! parameters. A call by name of a quantified function solves its group from its argument record's
//! fields, each contributing as a keyworded argument does, and records the contributions by its
//! argument's site for the frame; it refuses the load where a closed contribution misses its
//! parameter or closed ones leave the group unsolved. What the pass fixes rests in each shape's write-once [`Statics`] cell, which
//! [`evaluate`](super::evaluate) reads.
//!
//! A node is read here exactly as the evaluator reads it, through its [`Form`]. A shape's code is
//! typed before its statements, so an `EVAL` finds it typed. It is typed twice: once for its cell,
//! where an unmarked key's hole is a candidate the load cannot read, since a `USING` may fill it;
//! and once as a traced `EVAL` runs it, unfilled, the hole holding nothing — which fixes nothing.
//! Inside a quote's code a refusal is kept on the code shape, and the `EVAL` running it reports it.
//!
//! A static type is over [`Parametric`] types: it may hold the lexical variables of its chain. A
//! binder of a quantified callable is typed by its scheme — a module body's plain `LET` member, or a
//! keyworded form's name — and a call's head reads it as a [`DeclaredType`]. Anywhere else a
//! quantified function is an **instance site**: a part is typed with the type it is **wanted** at —
//! an annotation's, an ascription's, a callable body's declared return for its last statement, a
//! container's element type where that is the container's own type, a call by name's parameter
//! record, and a keyworded argument's slot at each candidate — and the site is instantiated at the
//! least instance under it ([`instance_under`]). A quantified callee's other arguments solve its
//! group first, and the slot is read through that solve; the candidates a use keeps must agree on
//! each instance. A name's solution is recorded by its site
//! and a literal's in its body's born-instance cell; a site the wanted type fixes nothing at, or
//! fixes to no closed instance, refuses the load. So a part's and a statement's static type is
//! never a scheme.
//!
//! See [README.md § Static types](README.md#static-types).

use crate::knot::{BuiltinFunction, KBuiltins};
use crate::memory::{BumpAllocator, BumpVec, Writer, collect, resident};
use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::scope::{
    BodyShape, Candidate, CandidateList, CaptureSlot, CaptureSource, Coordinate, Narrowing,
    Position, ShapeError, ShapeKind, Site, Slot, Static, StaticType, Statics, Target, UnitWork,
    Variable as Located, source_of,
};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, Symbol};
use crate::type_lattice::{
    Collector, DeclaredType, InstanceFailure, Interval, KType, Parametric, Record, Scheme, Side,
    TypeNode, TypeRegistry, Variable, Variance, Verdict, admits_with, bound_above, class_at_least,
    fits, instance_under, instantiate_quantified, intervals, judge_by_class, meet,
    quantifier_bounds, read_through, scheme_bound_above, scheme_return, scheme_slots,
    select_by_class, shape_return, shape_slots, solving_slots,
};
use crate::values::{ConstructionRefused, Value, construction, dict_type, list_type, record_type};

use super::builtins::Native;
use super::evaluate::{Form, Wanted, of_node, of_part, slots};
use super::one_name;
use super::rules::{self, Given};

/// Give every value expression and value binder of `root`, and of every shape nested in it, a
/// static type, and narrow every keyworded use's candidates; refuse a use no candidate can admit.
pub(super) fn statics<'graph>(
    root: &'graph BodyShape<'graph>,
    builtins: &KBuiltins<'graph, 'graph>,
    types: &TypeRegistry<'graph>,
    writer: Writer<'graph>,
    scratch: BumpAllocator<'_>,
) -> Result<(), ShapeError<'graph>> {
    let mut pass = Pass {
        builtins,
        types,
        writer,
        scratch,
        chain: BumpVec::new_in(scratch),
        unfilled: false,
        ran: BumpVec::new_in(scratch),
    };
    pass.push(root, true);
    let visited = pass.visit(0);
    pass.chain.pop();
    visited
}

/// A static type under `upper` and bounded below by nothing — exact where `upper` is `Number`,
/// `Str`, `Bool` or `Null`, since no value carries a type strictly under one.
pub(super) fn under(upper: Parametric) -> Interval {
    let leaf = [KType::NUMBER, KType::STR, KType::BOOL, KType::NULL]
        .into_iter()
        .any(|leaf| upper == leaf.into());
    if leaf {
        Interval::point(upper)
    } else {
        Interval {
            lower: KType::NEVER.into(),
            upper,
        }
    }
}

/// `[Never, Any]`: what the load cannot bound.
fn unknown() -> Interval {
    under(KType::ANY.into())
}

/// A binder's static type: a type's interval, or a quantified callable's scheme.
type Bound = DeclaredType<Interval>;

/// The static type of a value retyped to `declared` (an ascription's, a parameter's, a frame's
/// return): exactly `declared` where every value satisfying it is retyped to it — a list, dict or
/// record type, a family or its application, a newtype or a union's variant — and at most
/// `declared` otherwise; a union keeps each variant's own type.
pub(super) fn retyped_to(types: &TypeRegistry<'_>, declared: Parametric) -> Interval {
    match types.node(declared) {
        TypeNode::List { .. }
        | TypeNode::Dict { .. }
        | TypeNode::Record { .. }
        | TypeNode::ConstructorApply { .. }
        | TypeNode::SetMember { .. } => Interval::point(declared),
        _ => under(declared),
    }
}

/// One shape on the chain, innermost last, with what the pass has typed of it so far.
struct Level<'p, 'graph> {
    shape: &'graph BodyShape<'graph>,
    /// Whether the shape roots a chain: the program or a quote's code.
    roots_chain: bool,
    parts: BumpVec<'p, (Site, Interval)>,
    statements: BumpVec<'p, Interval>,
    binders: BumpVec<'p, Bound>,
    narrowings: BumpVec<'p, Narrowing<'graph>>,
    /// Each `:!` whose operand's static upper end lies under its type, and each annotated binder's
    /// type part whose value's does.
    settled: BumpVec<'p, Site>,
    /// Each name read at an instance site, beside the solution its function is instantiated at.
    instances: BumpVec<'p, (Site, &'graph [KType])>,
    /// Each keyworded use's contributions, parallel to the candidate lists.
    contributions: BumpVec<'p, &'graph [StaticType<'graph>]>,
    /// Each call by name's contributions, by its argument part's site.
    named: BumpVec<'p, (Site, &'graph [(Symbol, StaticType<'graph>)])>,
    /// Each type capture a contribution read in a shape nested here added, by the variable's level,
    /// beside the coordinate it reads in the enclosing activation.
    type_captures: BumpVec<'p, (usize, Coordinate)>,
}

/// The walk's state: the chain of enclosing shapes.
struct Pass<'p, 'x, 'graph> {
    builtins: &'x KBuiltins<'graph, 'graph>,
    types: &'x TypeRegistry<'graph>,
    writer: Writer<'graph>,
    scratch: BumpAllocator<'p>,
    chain: BumpVec<'p, Level<'p, 'graph>>,
    /// Whether the pass is typing a quote's code as a traced `EVAL` runs it, no hole filled: it
    /// fixes nothing.
    unfilled: bool,
    /// Each quote's code beside its last statement's static type as it runs unfilled.
    ran: BumpVec<'p, (&'graph BodyShape<'graph>, Interval)>,
}

/// What the load knows of one candidate's registered expression shape.
#[derive(Clone, Copy)]
enum Known {
    /// The same at every run.
    Closed(DeclaredType<KType>),
    /// Over lexical variables the run supplies.
    Rigid(DeclaredType<Parametric>),
    Unknown,
}

impl Known {
    /// The shape, where the load knows it.
    fn shape(self) -> Option<DeclaredType<Parametric>> {
        match self {
            Known::Closed(shape) => Some(shape.into()),
            Known::Rigid(shape) => Some(shape),
            Known::Unknown => None,
        }
    }
}

/// One candidate a keyworded use kept: what the load knows of it, its verdict, its group's
/// intervals where the static solve succeeded, and a builtin's return as its rule gives it.
#[derive(Clone, Copy)]
struct Judgement<'x, 'graph> {
    candidate: Candidate,
    known: Known,
    /// Whether the candidate is a `USING … SCOPE` block's surfaced head, whose return is at most.
    surfaced: bool,
    verdict: Verdict,
    intervals: Option<&'x [Interval]>,
    ruled: Option<Interval>,
    /// The instance each of the use's instance arguments takes at this candidate, in argument
    /// order.
    instances: &'x [Made<'graph>],
}

/// An instance: its exact static type beside the solution it is made at.
type Made<'graph> = (Interval, &'graph [KType]);

/// Where an instance site is written: a name read there, or a quantified `FN` written in place,
/// whose body the instance is born from.
#[derive(Clone, Copy)]
enum Instanced<'graph> {
    Name(&'graph ExpressionPart<'graph>),
    Literal(&'graph BodyShape<'graph>),
}

/// An instance argument of a keyworded use: its slot's position, its part as written, where its
/// instance is made, its scheme and where it is written.
#[derive(Clone, Copy)]
struct InstanceArgument<'graph> {
    position: usize,
    part: &'graph ExpressionPart<'graph>,
    instanced: Instanced<'graph>,
    scheme: Scheme,
    at: SourceRef,
}

/// Why a quantified callee's other arguments give an instance argument's slot no wanted type.
enum Unsolved<'p> {
    /// One can never fit its own slot: the call can never be admitted.
    Misfit,
    /// They leave these variables of the group the slot names open.
    Open(BumpVec<'p, usize>),
}

impl<'p, 'graph> Pass<'p, '_, 'graph> {
    fn push(&mut self, shape: &'graph BodyShape<'graph>, roots_chain: bool) {
        let scratch = self.scratch;
        let mut statements = BumpVec::with_capacity_in(shape.body().len(), scratch);
        statements.resize(shape.body().len(), unknown());
        let mut binders = BumpVec::with_capacity_in(shape.slots(), scratch);
        binders.resize(shape.slots(), DeclaredType::Type(unknown()));
        let mut narrowings = BumpVec::with_capacity_in(shape.candidate_lists().len(), scratch);
        narrowings.resize(shape.candidate_lists().len(), Narrowing::Full);
        let mut contributions = BumpVec::with_capacity_in(shape.candidate_lists().len(), scratch);
        contributions.resize(shape.candidate_lists().len(), &[][..]);
        self.chain.push(Level {
            shape,
            roots_chain,
            parts: BumpVec::new_in(scratch),
            statements,
            binders,
            narrowings,
            settled: BumpVec::new_in(scratch),
            instances: BumpVec::new_in(scratch),
            contributions,
            named: BumpVec::new_in(scratch),
            type_captures: BumpVec::new_in(scratch),
        });
    }

    /// Type the shape at chain level `level` — its code first — fix its cell, then type every
    /// shape nested in it.
    fn visit(&mut self, level: usize) -> Result<(), ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        for slot in 0..shape.slots() {
            let seeded = self.seeded(level, Slot(slot as u32));
            self.chain[level].binders[slot] = seeded;
        }
        for (_, nested) in shape.nested_shapes() {
            if nested.kind() == ShapeKind::Code && !done(nested) {
                self.nest(level, nested)?;
            }
        }
        for unit in shape.units() {
            match unit.work {
                UnitWork::Statement(index) => {
                    let wanted = self.tail(level, index as usize);
                    let typed = self.node(level, &shape.body()[index as usize], wanted)?;
                    self.chain[level].statements[index as usize] = typed;
                }
                UnitWork::Component(index) => self.component(level, index.index())?,
            }
        }
        self.returns(level)?;
        self.fix(level);

        for (_, nested) in shape.nested_shapes() {
            if !done(nested) {
                self.nest(level, nested)?;
            }
        }
        // A nested shape adds a type capture to each shape between it and the variable's home, so
        // the captures are laid down once every nested shape is typed.
        let at = &self.chain[level];
        if !self.unfilled && !at.type_captures.is_empty() {
            at.shape.fix_type_captures(collect(
                self.writer,
                at.type_captures.iter().map(|(_, source)| *source),
            ));
        }
        Ok(())
    }

    /// Type `nested`, a shape nested in the shape at `level`. A quote's code roots a chain and keeps
    /// its refusal, and is typed a second time as it runs unfilled.
    fn nest(
        &mut self,
        level: usize,
        nested: &'graph BodyShape<'graph>,
    ) -> Result<(), ShapeError<'graph>> {
        let code = nested.kind() == ShapeKind::Code;
        let visited = self.nested(level, nested, code);
        match visited {
            Err(error) if code => {
                nested.refuse_typing(resident(self.writer, error));
                Ok(())
            }
            Ok(_) if code => {
                let unfilled = std::mem::replace(&mut self.unfilled, true);
                // A use unfilled code can never run refuses nothing: its `EVAL` faults.
                if let Ok(Some(last)) = self.nested(level, nested, true) {
                    self.ran.push((nested, last));
                }
                self.unfilled = unfilled;
                Ok(())
            }
            visited => visited.map(|_| ()),
        }
    }

    /// Push `nested` above the shape at `level`, type it, and pop it: its last statement's static
    /// type.
    fn nested(
        &mut self,
        level: usize,
        nested: &'graph BodyShape<'graph>,
        roots_chain: bool,
    ) -> Result<Option<Interval>, ShapeError<'graph>> {
        self.push(nested, roots_chain);
        let visited = self.visit(level + 1);
        let last = self.chain[level + 1].statements.last().copied();
        self.chain.pop();
        visited.map(|_| last)
    }

    /// Lay what the pass typed of the shape at `level` down in its cell.
    fn fix(&mut self, level: usize) {
        if self.unfilled {
            return;
        }
        let writer = self.writer;
        let at = &mut self.chain[level];
        at.parts.sort_unstable_by_key(|(site, _)| *site);
        at.settled.sort_unstable();
        at.instances.sort_unstable_by_key(|(site, _)| *site);
        at.named.sort_unstable_by_key(|(site, _)| *site);
        at.shape.fix_statics(Statics {
            parts: collect(writer, at.parts.iter().copied()),
            statements: collect(writer, at.statements.iter().copied()),
            binders: collect(writer, at.binders.iter().copied()),
            narrowings: collect(writer, at.narrowings.iter().copied()),
            settled: collect(writer, at.settled.iter().copied()),
            instances: collect(writer, at.instances.iter().copied()),
            contributions: collect(writer, at.contributions.iter().copied()),
            named: collect(writer, at.named.iter().copied()),
        });
    }

    /// Where the lexical variable at `variable`'s level is read from the shape at chain level
    /// `level`: its home in the shape that declares it, stepped in through every shape between — a
    /// hop per block, a type capture per callable or module. `None` where no shape of this chain
    /// declares it.
    fn coordinate_of(&mut self, level: usize, variable: usize) -> Option<Coordinate> {
        let mut home = level;
        let target = loop {
            let at = &self.chain[home];
            let declared = at.shape.declared_variables().iter();
            if let Some((_, target)) = declared.into_iter().find(|(at, _)| *at == variable) {
                break *target;
            }
            if at.roots_chain || home == 0 {
                return None;
            }
            home -= 1;
        };
        let mut coordinate = Coordinate::Activation { hops: 0, target };
        for inner in home + 1..=level {
            coordinate = match self.chain[inner].shape.kind() {
                ShapeKind::Block => coordinate.through_block(),
                ShapeKind::Callable | ShapeKind::Module => Coordinate::Activation {
                    hops: 0,
                    target: Target::Capture(self.type_capture(inner, variable, coordinate)),
                },
                ShapeKind::Program | ShapeKind::Code => return None,
            };
        }
        Some(coordinate)
    }

    /// The closure slot of the shape at `level` that holds the lexical variable `variable`, read at
    /// `source` in the enclosing activation: the type capture already recorded for it, or a new one.
    fn type_capture(&mut self, level: usize, variable: usize, source: Coordinate) -> CaptureSlot {
        let at = &mut self.chain[level];
        let index = match at.type_captures.iter().position(|(v, _)| *v == variable) {
            Some(index) => index,
            None => {
                at.type_captures.push((variable, source));
                at.type_captures.len() - 1
            }
        };
        CaptureSlot((at.shape.captures().len() + index) as u32)
    }

    /// What an argument whose static upper end is `upper`, in the shape at `level`, contributes to
    /// a solve where its call runs: `Unknown` — the carried type — where `upper` is `Any`; `upper`
    /// where it is closed; and `upper` beside where each lexical variable it names is read.
    fn contribution(&mut self, level: usize, upper: Parametric) -> StaticType<'graph> {
        let (types, scratch) = (self.types, self.scratch);
        if upper == KType::ANY.into() {
            return Static::Unknown;
        }
        let mut levels: BumpVec<'_, usize> = BumpVec::new_in(scratch);
        read_through(types, scratch, upper, Side::Above, &mut |variable| {
            if let Variable::Lexical { level, .. } = variable
                && !levels.contains(&level)
            {
                levels.push(level);
            }
            None
        });
        if levels.is_empty() {
            return types
                .concrete(upper)
                .map_or(Static::Unknown, Static::Closed);
        }
        let mut located = BumpVec::with_capacity_in(levels.len(), scratch);
        for variable in levels.iter().copied() {
            let Some(at) = self.coordinate_of(level, variable) else {
                return Static::Unknown;
            };
            located.push(Located {
                level: variable,
                at,
            });
        }
        Static::Rigid {
            value: upper,
            variables: collect(self.writer, located.iter().copied()),
        }
    }

    /// A slot's static type before any unit runs: a registration's function type, a type name's
    /// type value's, a parameter's declared type as its body reads it, and `[Never, Any]` for a
    /// local its unit sets.
    fn seeded(&self, level: usize, slot: Slot) -> Bound {
        let shape = self.chain[level].shape;
        if shape.registration(slot).is_some() {
            return shape
                .births(slot)
                .map_or(DeclaredType::Type(unknown()), callable);
        }
        let name = shape.slot_name(slot);
        if let BinderSymbol::Type(_) = name {
            return DeclaredType::Type(match shape.declared_type(slot) {
                Static::Closed(handle) if shape.declarations(slot).is_some() => {
                    Interval::point(KType::of_kind(handle.kind_of(self.types)).into())
                }
                _ => under(KType::ANY_TYPE.into()),
            });
        }
        let is_parameter = shape
            .slot(name)
            .is_some_and(|(_, position)| position == Position::PARAMETER);
        if shape.kind() != ShapeKind::Callable || !is_parameter {
            return DeclaredType::Type(unknown());
        }
        let declared = self
            .in_body(shape)
            .and_then(|ktype| match self.types.node(ktype) {
                TypeNode::KFunction { params, .. } => params.get(name.symbol()),
                _ => None,
            });
        DeclaredType::Type(retyped_to(
            self.types,
            declared.unwrap_or_else(|| KType::ANY.into()),
        ))
    }

    /// A callable body's function type as its body reads it: its own group instantiated at its
    /// group levels. `None` where the load did not type the callable.
    fn in_body(&self, shape: &BodyShape<'graph>) -> Option<Parametric> {
        let ktype = match shape.callable_type() {
            Static::Closed(callable) => callable.ktype.into(),
            Static::Rigid {
                value: callable, ..
            } => callable.ktype,
            Static::Unknown => return None,
        };
        match ktype {
            DeclaredType::Type(ktype) => Some(ktype),
            DeclaredType::Scheme(scheme) => {
                debug_assert_eq!(
                    crate::type_lattice::quantifier_bounds(self.types, scheme).len(),
                    shape.group_levels().len(),
                    "the type channel numbered the callable's own group"
                );
                Some(instantiate_quantified(
                    self.types,
                    self.scratch,
                    scheme,
                    shape.group_levels(),
                ))
            }
        }
    }

    /// Type a component's value binders: each callable or module it births first, then each data
    /// member from its right-hand side, in member order. A component of type binders is the type
    /// channel's.
    fn component(&mut self, level: usize, index: usize) -> Result<(), ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        let component = &shape.components()[index];
        if shape.declarations(component.members[0]).is_some() {
            return Ok(());
        }
        for member in component.members {
            if let Some(body) = shape.births(*member) {
                let typed = match body.form() {
                    Some(_) => callable(body),
                    None => DeclaredType::Type(unknown()),
                };
                let typed = match typed {
                    DeclaredType::Scheme(scheme) => self.bound(level, *member, body, scheme)?,
                    typed => typed,
                };
                self.chain[level].binders[member.index()] = typed;
                // A function's retype is the identity, so it keeps its own exact type.
                if let (DeclaredType::Type(typed), Some(annotation)) =
                    (typed, shape.annotation(*member))
                {
                    self.annotated(level, *member, annotation, typed)?;
                }
            }
        }
        for member in component.members {
            if shape.births(*member).is_some() {
                continue;
            }
            if let Some(rhs) = shape.rhs(*member) {
                let wanted = match shape.annotation(*member) {
                    Some(annotation) => self.declared(level, annotation),
                    None => self
                        .statement_of(level, *member)
                        .and_then(|index| self.tail(level, index)),
                };
                let mut typed = self.part_at(level, rhs, wanted)?;
                if let Some(annotation) = shape.annotation(*member) {
                    typed = self.annotated(level, *member, annotation, typed)?;
                }
                self.chain[level].binders[member.index()] = DeclaredType::Type(typed);
            }
        }
        // A binding statement's value is the one binder it writes, where it writes only one.
        let mut values = component
            .members
            .iter()
            .filter(|member| shape.registration(**member).is_none());
        if let (Some(only), None) = (values.next(), values.next()) {
            // A statement binding a quantified callable is typed by no scheme: its value is read
            // only by name, at a call's head.
            if let Some(statement) = self.statement_of(level, *only) {
                let typed = self.chain[level].binders[only.index()];
                self.chain[level].statements[statement] = typed.as_type().unwrap_or_else(unknown);
            }
        }
        Ok(())
    }

    /// The index of the statement that binds `member` in the shape at `level`, where one does.
    fn statement_of(&self, level: usize, member: Slot) -> Option<usize> {
        let shape = self.chain[level].shape;
        let written = shape.slot(shape.slot_name(member)).map(|(_, at)| at);
        written.and_then(Position::statement_index)
    }

    /// The type the statement at `index` of the shape at `level` is wanted at: a callable body's
    /// declared return for its last statement, whose value is the body's.
    fn tail(&self, level: usize, index: usize) -> Option<Parametric> {
        let shape = self.chain[level].shape;
        if shape.kind() != ShapeKind::Callable || index + 1 != shape.body().len() {
            return None;
        }
        match self.in_body(shape).map(|ktype| self.types.node(ktype)) {
            Some(TypeNode::KFunction { ret, .. }) => Some(ret),
            _ => None,
        }
    }

    /// The static type of `member`, a binder of the shape at `level` whose statement births the
    /// quantified function `body` typed by `scheme`. A module body's plain `LET` keeps the scheme —
    /// a member, read at a call's head or instantiated where it is read. Any other `LET` is an
    /// instance site wanted at its annotation, or at the declared return for a callable body's last
    /// statement, and is refused where that fixes nothing. A keyworded form keeps its scheme.
    fn bound(
        &mut self,
        level: usize,
        member: Slot,
        body: &'graph BodyShape<'graph>,
        scheme: Scheme,
    ) -> Result<Bound, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        let Some(index) = self.statement_of(level, member) else {
            return Ok(DeclaredType::Scheme(scheme));
        };
        let statement = &shape.body()[index];
        let wanted = match statement.cache().builtin_shape().map(|form| form.id) {
            Some(BuiltinShapeId::LetAnnotated) => shape
                .annotation(member)
                .and_then(|annotation| self.declared(level, annotation)),
            Some(BuiltinShapeId::LetValue) if shape.kind() == ShapeKind::Module => {
                return Ok(DeclaredType::Scheme(scheme));
            }
            Some(BuiltinShapeId::LetValue) => self.tail(level, index),
            _ => return Ok(DeclaredType::Scheme(scheme)),
        };
        let (typed, solution) = self.instantiate(scheme, wanted, statement.source)?;
        if !self.unfilled {
            body.fix_born_instance(solution);
        }
        Ok(DeclaredType::Type(typed))
    }

    /// The instance of the quantified function typed by `scheme` at an instance site `at` wanted at
    /// `wanted`: its exact static type beside the solution it is made at. Refused where no function
    /// type is wanted, where the wanted type reaches some variable of the group with nothing, where
    /// no instance lies under it, and where the solution is not closed.
    fn instantiate(
        &self,
        scheme: Scheme,
        wanted: Option<Parametric>,
        at: SourceRef,
    ) -> Result<(Interval, &'graph [KType]), ShapeError<'graph>> {
        let (types, scratch) = (self.types, self.scratch);
        let TypeNode::KFunction { quantifiers, .. } = types.scheme_node(scheme) else {
            unreachable!("a scheme is a function type over a group")
        };
        let Some(wanted) =
            wanted.filter(|wanted| matches!(types.node(*wanted), TypeNode::KFunction { .. }))
        else {
            return Err(self.unfixed(scheme, wanted, at));
        };
        let solution = match instance_under(types, scratch, scheme, wanted) {
            Ok(solution) => solution,
            Err(InstanceFailure::NoInstance) => {
                return Err(ShapeError::NoInstance {
                    scheme,
                    wanted: bound_above(types, scratch, wanted),
                    at,
                });
            }
            Err(InstanceFailure::Unfixed(unfixed)) => {
                return Err(ShapeError::Unfixed {
                    variables: collect(
                        self.writer,
                        unfixed.iter().map(|index| quantifiers[*index]),
                    ),
                    wanted: Some(bound_above(types, scratch, wanted)),
                    at,
                });
            }
        };
        let mut closed = BumpVec::with_capacity_in(solution.len(), scratch);
        for (index, solved) in solution.iter().enumerate() {
            match types.concrete(*solved) {
                Some(solved) => closed.push(solved),
                None => {
                    return Err(ShapeError::OpenInstance {
                        variable: quantifiers[index],
                        at,
                    });
                }
            }
        }
        let solution = collect(self.writer, closed.iter().copied());
        let instance = instantiate_quantified(types, scratch, scheme, solution);
        Ok((Interval::point(instance), solution))
    }

    /// Where the part at `site` of the shape at `level` is written.
    fn source(&self, level: usize, site: Site) -> SourceRef {
        let shape = self.chain[level].shape;
        shape
            .body()
            .iter()
            .find_map(|node| source_of(node, site))
            .expect("a part lies in its shape's statements")
    }

    /// The instance site `part` of the shape at `level` is, through one-part parentheses: a name
    /// bound to a quantified function, or a quantified `FN` written in place; beside its scheme and
    /// where it is written. `None` for any other part.
    fn instance_site(
        &self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Option<(Instanced<'graph>, Scheme, SourceRef)> {
        let shape = self.chain[level].shape;
        match of_part(shape, part) {
            Form::Leaf(
                leaf @ (ExpressionPart::Identifier(_)
                | ExpressionPart::Type(_)
                | ExpressionPart::MarkedName(..)),
            ) => {
                let mention = shape.mention(Site::of(leaf))?;
                match self.read_declared(level, mention.coordinate) {
                    DeclaredType::Scheme(scheme) => Some((
                        Instanced::Name(leaf),
                        scheme,
                        self.source(level, Site::of(leaf)),
                    )),
                    DeclaredType::Type(_) => None,
                }
            }
            Form::Lambda(node) => {
                let body = Site::of_body(node).and_then(|site| shape.nested(site))?;
                match callable(body) {
                    DeclaredType::Scheme(scheme) => {
                        Some((Instanced::Literal(body), scheme, node.source))
                    }
                    DeclaredType::Type(_) => None,
                }
            }
            _ => None,
        }
    }

    /// The type a quantified function argument at the slot `slot` of a callee whose group is
    /// bounded by `bounds` is wanted at: the group solved from the `others` first — each other slot
    /// beside its argument's static type — then `slot` read from above through it. A variable the
    /// slot shares with them must come out one closed type; one only the slot names is read through
    /// `[Never, bound]`. Refused where the others admit no solve, or leave a shared variable open.
    fn slot_wanted(
        &self,
        bounds: &[KType],
        slot: Parametric,
        others: &[(Parametric, Interval)],
    ) -> Result<Parametric, Unsolved<'p>> {
        let (types, scratch) = (self.types, self.scratch);
        let names = |declared: Parametric, variable: usize| {
            types.references_quantifier(scratch, declared, variable)
        };
        let named = || {
            let mut named = BumpVec::new_in(scratch);
            named.extend((0..bounds.len()).filter(|variable| names(slot, *variable)));
            Unsolved::Open(named)
        };
        let mut collector = Collector::<Parametric>::new(scratch, bounds);
        let mut declared = BumpVec::with_capacity_in(others.len(), scratch);
        let mut exact = true;
        for (other, argument) in others.iter() {
            if !(0..bounds.len()).any(|variable| names(*other, variable)) {
                continue;
            }
            let admitted = admits_with(
                types,
                scratch,
                *other,
                argument.upper,
                Variance::Co,
                &mut collector,
            );
            if admitted.is_err() {
                // Only an argument meeting its slot at `Never` never fits; an imprecise one fixes
                // nothing.
                let (other, argument) = (
                    bound_above(types, scratch, *other),
                    bound_above(types, scratch, argument.upper),
                );
                return Err(
                    match meet(types, scratch, other, argument) == KType::NEVER {
                        true => Unsolved::Misfit,
                        false => named(),
                    },
                );
            }
            exact &= argument.is_exact() && types.concrete(argument.upper).is_some();
            declared.push(*other);
        }
        let Ok(solution) = collector.solve(types) else {
            return Err(named());
        };
        let solved = intervals(types, scratch, &declared, bounds, &solution, exact);
        let mut chosen = BumpVec::with_capacity_in(bounds.len(), scratch);
        let mut open = BumpVec::new_in(scratch);
        for (variable, bound) in bounds.iter().enumerate() {
            // Never `intervals`' answer for a variable no other slot names: it reads as the bound.
            if !declared.iter().any(|other| names(*other, variable)) {
                chosen.push(Interval::within(*bound));
                continue;
            }
            let interval = solved[variable];
            if names(slot, variable)
                && !(interval.is_exact() && types.concrete(interval.upper).is_some())
            {
                open.push(variable);
            }
            chosen.push(interval);
        }
        if !open.is_empty() {
            return Err(Unsolved::Open(open));
        }
        Ok(read_through(
            types,
            scratch,
            slot,
            Side::Above,
            &mut |variable| match variable {
                Variable::Quantified { index, .. } => chosen.get(index).copied(),
                _ => None,
            },
        ))
    }

    /// The instance each of `sites`, the instance arguments of a keyworded use whose arguments'
    /// static types are `arguments`, takes at a candidate the load knows as `known`: wanted at its
    /// slot's type, read through the candidate's group solved from its other arguments where it
    /// has one. Refused where the load does not know the candidate's shape or makes no instance;
    /// `None` where the other arguments do not fit their slots, which the judge refuses.
    fn instances(
        &self,
        known: Known,
        sites: &[InstanceArgument<'graph>],
        arguments: &[Interval],
    ) -> Result<&'p [Made<'graph>], Option<ShapeError<'graph>>> {
        let (types, scratch) = (self.types, self.scratch);
        let mut slots = BumpVec::new_in(scratch);
        let (bounds, quantifiers) = match known.shape() {
            Some(DeclaredType::Type(shape)) => {
                slots.extend(shape_slots(shape, types));
                (&[][..], &[][..])
            }
            Some(DeclaredType::Scheme(scheme)) => {
                slots.extend(scheme_slots(scheme, types));
                let quantifiers = match types.scheme_node(scheme) {
                    TypeNode::ExpressionShape { quantifiers, .. } => quantifiers,
                    _ => &[],
                };
                (quantifier_bounds(types, scheme), quantifiers)
            }
            None => return Err(Some(self.unfixed(sites[0].scheme, None, sites[0].at))),
        };
        let mut others = BumpVec::with_capacity_in(slots.len(), scratch);
        for (position, slot) in slots.iter().enumerate() {
            if !sites.iter().any(|site| site.position == position) {
                others.push((*slot, arguments[position]));
            }
        }
        let mut made = BumpVec::with_capacity_in(sites.len(), scratch);
        for site in sites.iter() {
            let slot = slots[site.position];
            let wanted = if bounds.is_empty() {
                slot
            } else {
                match self.slot_wanted(bounds, slot, &others) {
                    Ok(wanted) => wanted,
                    Err(Unsolved::Misfit) => return Err(None),
                    Err(Unsolved::Open(open)) => {
                        return Err(Some(ShapeError::Unfixed {
                            variables: collect(
                                self.writer,
                                open.iter().map(|index| quantifiers[*index]),
                            ),
                            wanted: None,
                            at: site.at,
                        }));
                    }
                }
            };
            made.push(self.instantiate(site.scheme, Some(wanted), site.at)?);
        }
        Ok(made.leak())
    }

    /// `Unfixed` for the quantified function typed by `scheme` at `at`, naming its whole group.
    fn unfixed(
        &self,
        scheme: Scheme,
        wanted: Option<Parametric>,
        at: SourceRef,
    ) -> ShapeError<'graph> {
        let (types, scratch) = (self.types, self.scratch);
        let TypeNode::KFunction { quantifiers, .. } = types.scheme_node(scheme) else {
            unreachable!("a scheme is a function type over a group")
        };
        ShapeError::Unfixed {
            variables: quantifiers,
            wanted: wanted.map(|wanted| bound_above(types, scratch, wanted)),
            at,
        }
    }

    /// Refuse a callable body whose static type can never satisfy its declared return — every call
    /// of it would fault on its contract.
    fn returns(&self, level: usize) -> Result<(), ShapeError<'graph>> {
        let at = &self.chain[level];
        let shape = at.shape;
        if shape.kind() != ShapeKind::Callable || shape.body().is_empty() {
            return Ok(());
        }
        let Some(TypeNode::KFunction { ret, .. }) =
            self.in_body(shape).map(|ktype| self.types.node(ktype))
        else {
            return Ok(());
        };
        let last = shape.body().len() - 1;
        let body = at.statements[last].upper;
        let (types, scratch) = (self.types, self.scratch);
        // A body that never arrives returns nothing to check.
        if body == KType::NEVER.into() {
            return Ok(());
        }
        // Compared, and named, through their bounds: a variable's name is its binder's.
        let (body, returns) = (
            bound_above(types, scratch, body),
            bound_above(types, scratch, ret),
        );
        if meet(types, scratch, body, returns) == KType::NEVER {
            return Err(ShapeError::ReturnNeverSatisfied {
                body,
                returns,
                at: shape.body()[last].source,
            });
        }
        Ok(())
    }

    /// The static type of the part `part` of the shape at `level`, wanted at no type.
    fn part(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        self.part_at(level, part, None)
    }

    /// The static type of the part `part` of the shape at `level`, wanted at `wanted`, recorded by
    /// its site.
    fn part_at(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
        wanted: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        let typed = self.form(level, of_part(shape, part), wanted)?;
        self.chain[level].parts.push((Site::of(part), typed));
        Ok(typed)
    }

    /// The static type of the statement `node` of the shape at `level`, wanted at `wanted`.
    fn node(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        wanted: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        self.form(level, of_node(shape, node), wanted)
    }

    /// The static type of a node or part the evaluator reads as `form`, wanted at `wanted`. A
    /// quantified `FN` written here is an instance site, born as the instance `wanted` fixes; at a
    /// call's head it is read by [`head`](Self::head).
    fn form(
        &mut self,
        level: usize,
        form: Form<'graph>,
        wanted: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        Ok(match form {
            Form::Leaf(part) => self.leaf(level, part, wanted)?,
            Form::Block(nested) => self.nested(level, nested, false)?.unwrap_or_else(unknown),
            Form::Lambda(node) => match Site::of_body(node).and_then(|site| shape.nested(site)) {
                Some(body) => match callable(body) {
                    DeclaredType::Type(typed) => typed,
                    DeclaredType::Scheme(scheme) => {
                        let (typed, solution) = self.instantiate(scheme, wanted, node.source)?;
                        if !self.unfilled {
                            body.fix_born_instance(solution);
                        }
                        typed
                    }
                },
                None => unknown(),
            },
            Form::Declaration => Interval::point(KType::NULL.into()),
            Form::Ascribe(node) => self.ascribe(level, node)?,
            Form::Eval(node) => self.eval(level, node)?,
            Form::Call(node, list) => self.narrow(level, node, list)?,
            Form::Apply(node) => self.apply(level, node)?,
            Form::Unevaluable(_) => unknown(),
        })
    }

    /// The static type of a call's head `head`: a quantified callable's scheme where it reads one —
    /// by name, or a quantified `FN` written in place — and otherwise its part's type, recorded by
    /// its site.
    fn head(
        &mut self,
        level: usize,
        head: &'graph ExpressionPart<'graph>,
    ) -> Result<Bound, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        let read = match of_part(shape, head) {
            Form::Leaf(
                ExpressionPart::Identifier(_)
                | ExpressionPart::Type(_)
                | ExpressionPart::MarkedName(..),
            ) => shape
                .mention(Site::of(head))
                .map(|mention| self.read_declared(level, mention.coordinate)),
            Form::Lambda(node) => Site::of_body(node)
                .and_then(|site| shape.nested(site))
                .map(callable),
            _ => None,
        };
        match read {
            Some(DeclaredType::Scheme(scheme)) => Ok(DeclaredType::Scheme(scheme)),
            _ => self.part(level, head).map(DeclaredType::Type),
        }
    }

    /// `<value> :! <Type>`: its type, exactly where the retype makes it so ([`retyped_to`]). Where
    /// the operand's static upper end lies under the type the ascription is settled, and the run
    /// checks nothing; where the two meet at `Never` the load is refused.
    fn ascribe(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let [operand, _, ascribed] = node.parts else {
            unreachable!("an ascription has an operand, its keyword and a type")
        };
        let declared = self.declared(level, &ascribed.value);
        let typed = self.part_at(level, &operand.value, declared)?;
        let held = self.held(level, typed, &ascribed.value, Site::of_node(node));
        held.map_err(|(value, ascribed)| ShapeError::AscriptionNeverSatisfied {
            value,
            ascribed,
            at: node.source,
        })
    }

    /// `LET <name> <type> = <value>`, the binder at `member` of the shape at `level`, whose value
    /// is `typed`: held to `annotation` as `:!` holds its operand, settled by the annotation's site.
    fn annotated(
        &mut self,
        level: usize,
        member: Slot,
        annotation: &'graph ExpressionPart<'graph>,
        typed: Interval,
    ) -> Result<Interval, ShapeError<'graph>> {
        let held = self.held(level, typed, annotation, Site::of(annotation));
        held.map_err(|(value, annotated)| {
            let shape = self.chain[level].shape;
            let written = shape.slot(shape.slot_name(member)).map(|(_, at)| at);
            let statement = written.and_then(Position::statement_index);
            ShapeError::AnnotationNeverSatisfied {
                value,
                annotated,
                at: shape.body()[statement.expect("an annotated binder is a statement's")].source,
            }
        })
    }

    /// A value of the static type `typed` held to the type part `part`: that type, exactly where
    /// the retype makes it so ([`retyped_to`]), and settled at `site` where `typed`'s upper end
    /// lies under it — where an exact function's, that function's own type. Where the two meet at
    /// `Never`, both read through their bounds.
    fn held(
        &mut self,
        level: usize,
        typed: Interval,
        part: &'graph ExpressionPart<'graph>,
        site: Site,
    ) -> Result<Interval, (KType, KType)> {
        // A value that never arrives has nothing to check.
        if typed.upper == KType::NEVER.into() {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        let Some(declared) = self.declared(level, part) else {
            return Ok(unknown());
        };
        let (types, scratch) = (self.types, self.scratch);
        // Compared, and named, through their bounds: a variable's name is its binder's.
        let (value, bounded) = (
            bound_above(types, scratch, typed.upper),
            bound_above(types, scratch, declared),
        );
        if meet(types, scratch, value, bounded) == KType::NEVER {
            return Err((value, bounded));
        }
        if fits(types, scratch, typed.upper, declared) {
            self.chain[level].settled.push(site);
            // A function's retype is the identity, so an exact one keeps its own type.
            if typed.is_exact() && matches!(types.node(declared), TypeNode::KFunction { .. }) {
                return Ok(typed);
            }
        }
        Ok(retyped_to(types, declared))
    }

    /// `EVAL <code> -> <Type>`: its declared type, exactly where the retype makes it so
    /// ([`retyped_to`]), as a registration's call is. An operand that can never be code refuses the
    /// load, and so does traced code whose type meets the declared one at `Never`.
    fn eval(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let [_, operand, _, declared] = node.parts else {
            unreachable!("an `EVAL` has its keyword, its code, `->` and a type")
        };
        let typed = self.part(level, &operand.value)?;
        // An operand that never arrives runs nothing.
        if typed.upper == KType::NEVER.into() {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        let (types, scratch) = (self.types, self.scratch);
        let value = bound_above(types, scratch, typed.upper);
        if meet(types, scratch, value, KType::ANY_CODE) == KType::NEVER {
            return Err(ShapeError::NotCode {
                value,
                at: node.source,
            });
        }
        let Some(declared) = self.declared(level, &declared.value) else {
            return Ok(unknown());
        };
        // Code that never arrives returns nothing to check.
        if let Some(code) = self.traced(level, &operand.value)
            && code != KType::NEVER
        {
            let returns = bound_above(types, scratch, declared);
            if meet(types, scratch, code, returns) == KType::NEVER {
                return Err(ShapeError::EvalNeverSatisfied {
                    code,
                    returns,
                    at: node.source,
                });
            }
        }
        Ok(retyped_to(types, declared))
    }

    /// The type the type part `part` of the shape at `level` denotes, where the load knows it.
    fn declared(&self, level: usize, part: &'graph ExpressionPart<'graph>) -> Option<Parametric> {
        match self.chain[level].shape.typed_expression(Site::of(part)) {
            Static::Closed(handle) => Some(handle.into()),
            Static::Rigid { value: handle, .. } => Some(handle),
            Static::Unknown => None,
        }
    }

    /// A leaf part wanted at `wanted`: a literal's type, a name's binder's, a quote's code type, a
    /// type value's kind, or a container's over its parts, end by end. A name bound to a quantified
    /// function is an instance site, recorded by its site; a container passes `wanted` on to its
    /// parts only where it is that container's own type.
    fn leaf(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
        wanted: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        let (types, scratch) = (self.types, self.scratch);
        Ok(match part {
            ExpressionPart::Literal(KLiteral::Number(_)) => Interval::point(KType::NUMBER.into()),
            ExpressionPart::Literal(KLiteral::String(_)) => Interval::point(KType::STR.into()),
            ExpressionPart::Literal(KLiteral::Boolean(_)) => Interval::point(KType::BOOL.into()),
            ExpressionPart::Literal(KLiteral::Null) => Interval::point(KType::NULL.into()),
            ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::MarkedName(..) => match shape.mention(Site::of(part)) {
                Some(mention) => match self.read_declared(level, mention.coordinate) {
                    DeclaredType::Type(typed) => typed,
                    DeclaredType::Scheme(scheme) => {
                        let site = Site::of(part);
                        let at = self.source(level, site);
                        let (typed, solution) = self.instantiate(scheme, wanted, at)?;
                        if !self.unfilled {
                            self.chain[level].instances.push((site, solution));
                        }
                        typed
                    }
                },
                None => unknown(),
            },
            ExpressionPart::QuotedExpression(_) => shape
                .nested(Site::of(part))
                .map_or_else(unknown, |code| Interval::point(code.code_type().into())),
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                match shape.typed_expression(Site::of(part)) {
                    Static::Closed(handle) => {
                        Interval::point(KType::of_kind(handle.kind_of(types)).into())
                    }
                    _ => under(KType::ANY_TYPE.into()),
                }
            }
            ExpressionPart::ListLiteral(items) => {
                let element = wanted.and_then(|wanted| match types.node(wanted) {
                    TypeNode::List { element } => Some(element),
                    _ => None,
                });
                let mut typed = BumpVec::with_capacity_in(items.len(), scratch);
                for item in items.iter() {
                    typed.push(self.part_at(level, item, element)?);
                }
                Interval {
                    lower: list_type(types, scratch, typed.iter().map(|each| each.lower)),
                    upper: list_type(types, scratch, typed.iter().map(|each| each.upper)),
                }
            }
            ExpressionPart::DictLiteral(pairs) => {
                let values = wanted.and_then(|wanted| match types.node(wanted) {
                    TypeNode::Dict { value, .. } => Some(value),
                    _ => None,
                });
                let mut typed = BumpVec::with_capacity_in(pairs.len(), scratch);
                for (key, value) in pairs.iter() {
                    typed.push((self.part(level, key)?, self.part_at(level, value, values)?));
                }
                Interval {
                    lower: dict_type(
                        types,
                        scratch,
                        typed.iter().map(|(k, v)| (k.lower, v.lower)),
                    ),
                    upper: dict_type(
                        types,
                        scratch,
                        typed.iter().map(|(k, v)| (k.upper, v.upper)),
                    ),
                }
            }
            ExpressionPart::RecordLiteral(fields) => {
                let declared = wanted.and_then(|wanted| match types.node(wanted) {
                    TypeNode::Record { fields } => Some(fields),
                    _ => None,
                });
                let mut typed = BumpVec::with_capacity_in(fields.len(), scratch);
                for (name, value) in fields.iter() {
                    let field = declared.and_then(|declared| declared.get(name.symbol()));
                    typed.push((*name, self.part_at(level, value, field)?));
                }
                Interval {
                    lower: record_type(types, scratch, typed.iter().map(|(n, v)| (*n, v.lower))),
                    upper: record_type(types, scratch, typed.iter().map(|(n, v)| (*n, v.upper))),
                }
            }
            ExpressionPart::Keyword(_)
            | ExpressionPart::Expression(_)
            | ExpressionPart::MarkedUse(..) => unknown(),
        })
    }

    /// The static type of what `coordinate`, read in the shape at `level`, holds, a quantified
    /// callable's its scheme. A capture along the chain reads its source's; one crossing into a
    /// quote's code reads it through its bounds.
    fn read_declared(&self, level: usize, coordinate: Coordinate) -> Bound {
        let (hops, target) = match coordinate {
            Coordinate::Builtin(index) => {
                return self
                    .builtins
                    .get(index)
                    .ktype()
                    .map(|ktype| Interval::point(ktype.into()));
            }
            Coordinate::Activation { hops, target } => (hops, target),
        };
        let at = level - hops as usize;
        let capture = match target {
            Target::Local(slot) => return self.chain[at].binders[slot.index()],
            Target::Capture(capture) => capture,
        };
        let outer = at.checked_sub(1);
        let read = match (
            self.chain[at].shape.captures()[capture.index()].source,
            outer,
        ) {
            (CaptureSource::Read(inner), Some(outer)) => self.read_declared(outer, inner),
            (CaptureSource::Member { component, index }, Some(outer)) => {
                let member =
                    self.chain[outer].shape.components()[component.index()].members[index as usize];
                self.chain[outer].binders[member.index()]
            }
            _ => return DeclaredType::Type(unknown()),
        };
        if !self.chain[at].roots_chain {
            return read;
        }
        let (types, scratch) = (self.types, self.scratch);
        match read {
            DeclaredType::Type(read) => {
                DeclaredType::Type(under(bound_above(types, scratch, read.upper).into()))
            }
            DeclaredType::Scheme(scheme) => {
                DeclaredType::Scheme(scheme_bound_above(types, scratch, scheme))
            }
        }
    }

    /// Where `coordinate`, read in the shape at `level`, lands: a slot of the shape at some level,
    /// followed through the captures it reads.
    fn slot_of(&self, level: usize, coordinate: Coordinate) -> Option<(usize, Slot)> {
        let Coordinate::Activation { hops, target } = coordinate else {
            return None;
        };
        let at = level.checked_sub(hops as usize)?;
        match target {
            Target::Local(slot) => Some((at, slot)),
            Target::Capture(capture) => {
                let outer = at.checked_sub(1)?;
                match self.chain[at].shape.captures()[capture.index()].source {
                    CaptureSource::Read(inner) => self.slot_of(outer, inner),
                    CaptureSource::Member { component, index } => {
                        let members =
                            self.chain[outer].shape.components()[component.index()].members;
                        Some((outer, members[index as usize]))
                    }
                    CaptureSource::Hole | CaptureSource::Offered => None,
                }
            }
        }
    }

    /// `(head argument)`: a construction's identity when the head is a type the load knows, the
    /// callee's declared return read through the group the argument solves when the head is a
    /// function, else `[Never, Any]`. The call is exactly a return the retype makes exact where the
    /// callee is exact and its solve is the load's.
    fn apply(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let [head, argument] = node.parts else {
            unreachable!("an application is a head and its argument")
        };
        let (head, argument) = (&head.value, &argument.value);
        let callee = self.head(level, head)?;
        let payload = self.payload(level, callee, argument)?;
        let (types, scratch) = (self.types, self.scratch);
        if let Some(identity) = self.head_handle(level, head) {
            let upper = match construction(types, scratch, identity, payload.upper) {
                Ok(constructed) => constructed,
                Err(ConstructionRefused::NotConstructible(_)) => KType::ANY.into(),
                // A misfit faults; an unsolved family lies under the bare family.
                Err(_) => identity.into(),
            };
            let lower = construction(types, scratch, identity, payload.lower)
                .unwrap_or_else(|_| KType::NEVER.into());
            return Ok(Interval { lower, upper });
        }
        if let DeclaredType::Type(callee) = callee {
            self.admissible(callee, payload, node)?;
        }
        // A scheme is one callee, so it is exact.
        let (function, exact) = match callee {
            DeclaredType::Type(callee) => (types.node(callee.upper), callee.is_exact()),
            DeclaredType::Scheme(scheme) => (types.scheme_node(scheme), true),
        };
        Ok(match function {
            TypeNode::KFunction {
                bounds,
                params,
                ret,
                ..
            } => {
                let scheme = match callee {
                    DeclaredType::Scheme(scheme) => Some(scheme),
                    DeclaredType::Type(_) => None,
                };
                let (returned, solved) = self.called(
                    level,
                    scheme,
                    argument,
                    node,
                    (bounds, params, ret),
                    payload,
                )?;
                // Only an exact callee: a function is never retyped, so one at most its type may
                // return less.
                if exact && solved {
                    retyped_to(types, returned)
                } else {
                    under(returned)
                }
            }
            _ => unknown(),
        })
    }

    /// The static type of a call by name's `argument`, its callee's static type `callee`. An
    /// unquantified function's argument is wanted at its parameter record. A quantified callee's
    /// record literal types its other fields first, and each field that is an instance site at its
    /// parameter read through the group they solve.
    fn payload(
        &mut self,
        level: usize,
        callee: Bound,
        argument: &'graph ExpressionPart<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let (types, scratch) = (self.types, self.scratch);
        let scheme = match callee {
            DeclaredType::Type(callee) => {
                let wanted = match types.node(callee.upper) {
                    TypeNode::KFunction { params, .. } => {
                        Some(record_type(types, scratch, params.iter()))
                    }
                    _ => None,
                };
                return self.part_at(level, argument, wanted);
            }
            DeclaredType::Scheme(scheme) => scheme,
        };
        let ExpressionPart::RecordLiteral(fields) = argument else {
            return self.part(level, argument);
        };
        let TypeNode::KFunction {
            quantifiers,
            bounds,
            params,
            ..
        } = types.scheme_node(scheme)
        else {
            unreachable!("a call by name's scheme is a function type")
        };
        let mut typed = BumpVec::with_capacity_in(fields.len(), scratch);
        for (_, value) in fields.iter() {
            typed.push(match self.instance_site(level, value) {
                Some(_) => None,
                None => Some(self.part(level, value)?),
            });
        }
        let mut others = BumpVec::with_capacity_in(fields.len(), scratch);
        for ((name, _), each) in fields.iter().zip(typed.iter()) {
            if let (Some(each), Some(param)) = (each, params.get(name.symbol())) {
                others.push((param, *each));
            }
        }
        for (index, (name, value)) in fields.iter().enumerate() {
            if typed[index].is_some() {
                continue;
            }
            let wanted = match params.get(name.symbol()) {
                Some(slot) => match self.slot_wanted(bounds, slot, &others) {
                    Ok(wanted) => Some(wanted),
                    // A call by name is one callee: nothing else could take the argument.
                    Err(unsolved) => {
                        let open = match unsolved {
                            Unsolved::Open(open) => open,
                            Unsolved::Misfit => {
                                let mut named = BumpVec::new_in(scratch);
                                named.extend((0..bounds.len()).filter(|variable| {
                                    types.references_quantifier(scratch, slot, *variable)
                                }));
                                named
                            }
                        };
                        return Err(ShapeError::Unfixed {
                            variables: collect(
                                self.writer,
                                open.iter().map(|index| quantifiers[*index]),
                            ),
                            wanted: None,
                            at: self.source(level, Site::of(value)),
                        });
                    }
                },
                None => None,
            };
            typed[index] = Some(self.part_at(level, value, wanted)?);
        }
        let field = |index: usize| typed[index].expect("every field is typed");
        let ends = |end: fn(Interval) -> Parametric| {
            let each = fields.iter().enumerate();
            record_type(
                types,
                scratch,
                each.map(|(index, (name, _))| (*name, end(field(index)))),
            )
        };
        let interval = Interval {
            lower: ends(|each| each.lower),
            upper: ends(|each| each.upper),
        };
        self.chain[level].parts.push((Site::of(argument), interval));
        Ok(interval)
    }

    /// Refuse a call by name, `node`, whose callee is exactly an unquantified function the argument
    /// of the static type `payload` can never satisfy the parameters of: a parameter the payload
    /// lacks, a field meeting its parameter at `Never`, or a field no parameter declares where the
    /// payload is exact. Any other callee is the call's to admit.
    fn admissible(
        &self,
        callee: Interval,
        payload: Interval,
        node: &'graph KExpression<'graph>,
    ) -> Result<(), ShapeError<'graph>> {
        let (types, scratch) = (self.types, self.scratch);
        let TypeNode::KFunction { bounds, params, .. } = types.node(callee.upper) else {
            return Ok(());
        };
        let TypeNode::Record { fields } = types.node(payload.upper) else {
            return Ok(());
        };
        if !callee.is_exact() || !bounds.is_empty() {
            return Ok(());
        }
        let met = |param: Parametric, field: Parametric| {
            let (param, field) = (
                bound_above(types, scratch, param),
                bound_above(types, scratch, field),
            );
            meet(types, scratch, param, field) != KType::NEVER
        };
        let missed = params.iter().any(|(name, param)| {
            fields
                .get(name.symbol())
                .is_none_or(|field| !met(param, field))
        });
        let extra = payload.is_exact()
            && fields
                .keys()
                .any(|name| params.get(name.symbol()).is_none());
        if missed || extra {
            return Err(ShapeError::CallNeverSatisfied {
                callee: DeclaredType::Type(bound_above(types, scratch, callee.upper)),
                arguments: bound_above(types, scratch, payload.upper),
                at: node.source,
            });
        }
        Ok(())
    }

    /// What a call by name of a function over `params` returning `ret`, its group bounded by
    /// `bounds`, returns for an argument of the static type `payload`: `ret` read through the
    /// intervals the argument's fields solve the group to, or its bounds where they do not; beside
    /// whether that solve is the call's — the group empty, or every interval a point.
    ///
    /// Where the callee is the quantified function typed by `scheme`, each parameter naming its
    /// group solves from its field's contribution, which the call records by `argument`'s site; the
    /// load refuses the call where a closed contribution does not fit its parameter, or where every
    /// solving parameter's contribution is closed and the group has no solution.
    fn called(
        &mut self,
        level: usize,
        scheme: Option<Scheme>,
        argument: &'graph ExpressionPart<'graph>,
        node: &'graph KExpression<'graph>,
        (bounds, params, ret): (&[KType], Record<'_, Parametric>, Parametric),
        payload: Interval,
    ) -> Result<(Parametric, bool), ShapeError<'graph>> {
        let (types, scratch) = (self.types, self.scratch);
        if bounds.is_empty() {
            return Ok((ret, true));
        }
        let fields = |typed| match types.node(typed) {
            TypeNode::Record { fields } => Some(fields),
            _ => None,
        };
        let Some(upper) = fields(payload.upper) else {
            return Ok((self.through(ret, None), false));
        };
        let lower = fields(payload.lower);
        let never = || ShapeError::CallNeverSatisfied {
            callee: DeclaredType::Scheme(scheme_bound_above(
                types,
                scratch,
                scheme.expect("only a scheme refuses"),
            )),
            arguments: bound_above(types, scratch, payload.upper),
            at: node.source,
        };
        let mut collector = Collector::<Parametric>::new(scratch, bounds);
        let mut declared = BumpVec::with_capacity_in(params.len(), scratch);
        let mut contributed = BumpVec::new_in(scratch);
        let (mut exact, mut closed) = (true, true);
        for (name, param) in params.iter() {
            let Some(field) = upper.get(name.symbol()) else {
                return Ok((self.through(ret, None), false));
            };
            let solving = (0..bounds.len())
                .any(|variable| types.references_quantifier(scratch, param, variable));
            let contribution = match scheme {
                Some(_) if solving => self.contribution(level, field),
                _ => Static::Unknown,
            };
            if admits_with(types, scratch, param, field, Variance::Co, &mut collector).is_err() {
                if let Static::Closed(_) = contribution {
                    return Err(never());
                }
                return Ok((self.through(ret, None), false));
            }
            if solving {
                // As a keyworded use judges it: a variable makes no solve the call's, save where
                // the call solves from this closed type itself.
                let contributes = matches!(contribution, Static::Closed(_));
                closed &= contributes;
                exact &= contributes
                    || lower.and_then(|lower| lower.get(name.symbol())) == Some(field)
                        && types.concrete(field).is_some();
            }
            if !matches!(contribution, Static::Unknown) {
                contributed.push((name.symbol(), contribution));
            }
            declared.push(param);
        }
        match collector.solve(types) {
            Ok(solution) => {
                if !self.unfilled && !contributed.is_empty() {
                    let contributed = collect(self.writer, contributed.iter().copied());
                    self.chain[level]
                        .named
                        .push((Site::of(argument), contributed));
                }
                let solved = intervals(types, scratch, &declared, bounds, &solution, exact);
                Ok((self.through(ret, Some(&solved)), exact))
            }
            Err(_) if scheme.is_some() && closed => Err(never()),
            Err(_) => Ok((self.through(ret, None), false)),
        }
    }

    /// `ret`, a return under its own group, read through that group's `intervals` — each variable
    /// at `[Never, bound]` where there are none — keeping every lexical variable.
    fn through(&self, ret: Parametric, intervals: Option<&[Interval]>) -> Parametric {
        read_through(
            self.types,
            self.scratch,
            ret,
            Side::Above,
            &mut |variable| match variable {
                Variable::Quantified { index, bound } => Some(
                    intervals
                        .and_then(|all| all.get(index).copied())
                        .unwrap_or(Interval::within(bound)),
                ),
                _ => None,
            },
        )
    }

    /// `shape`'s declared return read through its own group's `intervals`.
    fn returned(
        &self,
        shape: DeclaredType<Parametric>,
        intervals: Option<&[Interval]>,
    ) -> Parametric {
        let ret = match shape {
            DeclaredType::Type(shape) => shape_return(shape, self.types),
            DeclaredType::Scheme(scheme) => scheme_return(scheme, self.types),
        };
        self.through(ret.expect("a registered shape returns"), intervals)
    }

    /// The type an application's head denotes, where the load knows it.
    fn head_handle(&self, level: usize, head: &'graph ExpressionPart<'graph>) -> Option<KType> {
        let shape = self.chain[level].shape;
        match head {
            ExpressionPart::Type(_) => {
                let coordinate = shape.mention(Site::of(head))?.coordinate;
                if let Coordinate::Builtin(index) = coordinate {
                    return match self.builtins.get(index) {
                        Value::Type(value) => Some(value.handle()),
                        _ => None,
                    };
                }
                let (at, slot) = self.slot_of(level, coordinate)?;
                let holder = self.chain[at].shape;
                holder.declarations(slot)?;
                match holder.declared_type(slot) {
                    Static::Closed(handle) => Some(handle),
                    _ => None,
                }
            }
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                match shape.typed_expression(Site::of(head)) {
                    Static::Closed(handle) => Some(handle),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// A keyworded use: its arguments typed, each candidate judged against them — a builtin through
    /// its rule too, over what each slot holds as written — and the use narrowed, or a candidate
    /// selected, where no *maybe* one is left and one ranks first. Its static type is the selected
    /// candidate's [return](Self::candidate_return), or at most the join of what is left's returns.
    fn narrow(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        list: &'graph CandidateList<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let (types, scratch) = (self.types, self.scratch);
        let wanted = slots(node, scratch);
        let mut arguments = BumpVec::with_capacity_in(wanted.len(), scratch);
        let mut given = BumpVec::with_capacity_in(wanted.len(), scratch);
        // Each argument that is an instance site is typed per candidate, at the slot it fills.
        let mut sites = BumpVec::new_in(scratch);
        for (position, wanted) in wanted.iter().enumerate() {
            let site = match wanted {
                Wanted::Evaluated(part) => self.instance_site(level, part).map(|site| (part, site)),
                Wanted::Label(_) => None,
            };
            if let Some((part, (instanced, scheme, at))) = site {
                sites.push(InstanceArgument {
                    position,
                    part,
                    instanced,
                    scheme,
                    at,
                });
                arguments.push(unknown());
                given.push(Given {
                    typed: unknown(),
                    names: None,
                });
                continue;
            }
            let each = match wanted {
                Wanted::Label(name) => Given {
                    typed: Interval::point(
                        match name {
                            BinderSymbol::Type(_) => KType::TYPE_NAME_TOKEN,
                            _ => KType::IDENTIFIER,
                        }
                        .into(),
                    ),
                    names: Some(std::slice::from_ref(scratch.alloc(*name))),
                },
                Wanted::Evaluated(part) => Given {
                    typed: self.part(level, part)?,
                    names: written_names(part, scratch),
                },
            };
            arguments.push(each.typed);
            given.push(each);
        }
        let shape = self.chain[level].shape;
        let index = shape
            .candidate_lists()
            .binary_search_by_key(&Site::of_node(node), |(site, _)| *site)
            .expect("a keyworded use's candidate list is recorded at its node");
        // A use one of whose arguments never arrives never runs its call.
        if arguments
            .iter()
            .any(|argument| argument.upper == KType::NEVER.into())
        {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        // What each argument contributes where some candidate's slot solves from it: an instance
        // argument and a label their carried types. No other argument is read, so a type capture
        // is added only where a solve reads it.
        let mut solved = BumpVec::with_capacity_in(wanted.len(), scratch);
        solved.resize(wanted.len(), false);
        for candidate in list.candidates {
            if let Some(registered) = self.candidate(level, *candidate).shape() {
                let solving = solving_slots(types, scratch, registered);
                for (each, solves) in solved.iter_mut().zip(solving) {
                    *each |= *solves;
                }
            }
        }
        let mut contributions = BumpVec::with_capacity_in(wanted.len(), scratch);
        for (position, wanted) in wanted.iter().enumerate() {
            let instance = sites.iter().any(|site| site.position == position);
            contributions.push(match wanted {
                Wanted::Evaluated(_) if solved[position] && !instance => {
                    self.contribution(level, arguments[position].upper)
                }
                _ => Static::Unknown,
            });
        }
        let contributes = contributions
            .iter()
            .any(|contribution| !matches!(contribution, Static::Unknown));

        let uppers = || collect(self.writer, arguments.iter().map(|argument| argument.upper));

        let mut judged = BumpVec::with_capacity_in(list.candidates.len(), scratch);
        // The first argument a builtin's need dropped a candidate over, beside that need.
        let mut dropped = None;
        // The last refusal of an instance argument a candidate was dropped over.
        let mut refused = None;
        for candidate in list.candidates {
            // Unfilled code holds no function at an unmarked key's hole.
            if self.unfilled && self.hole(level, *candidate) {
                continue;
            }
            let known = self.candidate(level, *candidate);
            // At this candidate, each instance argument is exactly the instance its slot makes.
            let (mut here, mut given_here) = (&arguments[..], &given[..]);
            let mut instances: &[Made<'graph>] = &[];
            if !sites.is_empty() {
                match self.instances(known, &sites, &arguments) {
                    Ok(made) => instances = made,
                    Err(error) => {
                        refused = error.or(refused);
                        continue;
                    }
                }
                let mut typed = BumpVec::with_capacity_in(arguments.len(), scratch);
                typed.extend(arguments.iter().copied());
                let mut read = BumpVec::with_capacity_in(given.len(), scratch);
                read.extend(given.iter().copied());
                for (site, (instance, _)) in sites.iter().zip(instances.iter()) {
                    typed[site.position] = *instance;
                    read[site.position].typed = *instance;
                }
                (here, given_here) = (typed.leak(), read.leak());
            }
            let (mut verdict, intervals) = match known.shape() {
                Some(registered) => {
                    // A solving slot whose argument contributes its static type is solved from
                    // exactly that type's upper end.
                    if contributes {
                        let solving = solving_slots(types, scratch, registered);
                        let mut read = BumpVec::with_capacity_in(here.len(), scratch);
                        read.extend(here.iter().enumerate().map(|(position, argument)| {
                            let contributed = solving.get(position) == Some(&true)
                                && !matches!(contributions[position], Static::Unknown);
                            match contributed {
                                true => Interval::point(argument.upper),
                                false => *argument,
                            }
                        }));
                        here = read.leak();
                    }
                    let judged = judge_by_class(types, scratch, registered, here);
                    (judged.verdict, judged.intervals)
                }
                None => (Verdict::Maybe, None),
            };
            let builtin = self.builtin(*candidate);
            let mut ruled = None;
            if let Some(builtin) = builtin.filter(|_| verdict != Verdict::Never) {
                let native = Native::of(builtin.id());
                let typed = rules::typed(native, builtin.ktype(), given_here, types, scratch);
                if let Some(slot) = typed.dropped {
                    verdict = Verdict::Never;
                    dropped = dropped.or(Some((arguments[slot].lower, typed.needs[slot])));
                }
                ruled = Some(typed.returns);
            }
            if verdict != Verdict::Never {
                judged.push(Judgement {
                    candidate: *candidate,
                    known,
                    surfaced: self.surfaced(level, *candidate),
                    verdict,
                    intervals,
                    ruled,
                    instances,
                });
            }
        }
        if contributes && !self.unfilled {
            self.chain[level].contributions[index] =
                collect(self.writer, contributions.iter().copied());
        }
        if judged.is_empty() {
            // One candidate's own instance refusal, or every candidate's at once.
            if let Some(refused) = refused {
                return Err(match list.candidates {
                    [_] => refused,
                    _ => ShapeError::NoInstanceAtCandidates {
                        key: list.elements,
                        at: node.source,
                    },
                });
            }
            let missing = dropped.and_then(|(lower, need)| self.missing(lower, need));
            return Err(match missing {
                Some((of, field)) => ShapeError::NoField {
                    of,
                    field,
                    at: node.source,
                },
                None => ShapeError::NoAdmittingCandidate {
                    key: list.elements,
                    arguments: uppers(),
                    at: node.source,
                },
            });
        }
        // A *maybe* an *always* one strictly outranks at the first class never runs: wherever it
        // admits, the *always* one does too, and beats it.
        let beats = |a: DeclaredType<KType>, m: DeclaredType<KType>| {
            let (a, m) = (a.into(), m.into());
            class_at_least(types, scratch, a, m, 0) && !class_at_least(types, scratch, m, a, 0)
        };
        let outranked = |judgement: &Judgement<'_, 'graph>| match judgement.known {
            Known::Closed(m) if judgement.verdict == Verdict::Maybe => {
                judged.iter().any(|other| match other.known {
                    Known::Closed(a) if other.verdict == Verdict::Always => beats(a, m),
                    _ => false,
                })
            }
            _ => false,
        };
        let mut left = BumpVec::with_capacity_in(judged.len(), scratch);
        left.extend(
            judged
                .iter()
                .copied()
                .filter(|judgement| !outranked(judgement)),
        );
        let judged = left;

        if judged
            .iter()
            .all(|judgement| judgement.verdict == Verdict::Always)
        {
            let chosen = match judged[..] {
                [only] => Some(only),
                _ if judged
                    .iter()
                    .all(|judgement| matches!(judgement.known, Known::Closed(_))) =>
                {
                    let mut shapes: BumpVec<'_, DeclaredType<Parametric>> =
                        BumpVec::with_capacity_in(judged.len(), scratch);
                    shapes.extend(judged.iter().map(|judgement| match judgement.known {
                        Known::Closed(registered) => DeclaredType::<Parametric>::from(registered),
                        _ => unreachable!("every candidate is closed"),
                    }));
                    let survivors = select_by_class(types, scratch, &shapes);
                    let first = match survivors[..] {
                        [only] => Some(only),
                        _ => survivors
                            .iter()
                            .copied()
                            .find(|survivor| self.builtin(judged[*survivor].candidate).is_some()),
                    };
                    match first {
                        Some(survivor) => Some(judged[survivor]),
                        None => {
                            return Err(ShapeError::Ambiguous {
                                key: list.elements,
                                arguments: uppers(),
                                count: survivors.len(),
                                at: node.source,
                            });
                        }
                    }
                }
                // Several, a rigid one among them: ranked at the call.
                _ => None,
            };
            if let Some(chosen) = chosen {
                let Candidate::One(coordinate) = chosen.candidate else {
                    unreachable!("an always candidate is no spread");
                };
                self.made(level, node, list, &sites, &[chosen])?;
                self.chain[level].narrowings[index] = Narrowing::Selected(coordinate);
                return Ok(self.candidate_return(chosen));
            }
        }
        if judged.len() < list.candidates.len()
            || judged
                .iter()
                .any(|judgement| judgement.verdict == Verdict::Always)
        {
            let kept = collect(
                self.writer,
                judged
                    .iter()
                    .map(|judgement| (judgement.candidate, judgement.verdict)),
            );
            self.chain[level].narrowings[index] = Narrowing::Kept(kept);
        }
        self.made(level, node, list, &sites, &judged)?;
        let mut returns = BumpVec::with_capacity_in(judged.len(), scratch);
        for judgement in judged.iter() {
            if let Known::Unknown = judgement.known {
                return Ok(unknown());
            }
            returns.push(self.candidate_return(*judgement).upper);
        }
        Ok(under(types.union_of(scratch, &returns)))
    }

    /// Record the instance each of `sites`, the instance arguments of the keyworded use `node`,
    /// takes at the candidates `kept`, which must agree on it.
    fn made(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        list: &'graph CandidateList<'graph>,
        sites: &[InstanceArgument<'graph>],
        kept: &[Judgement<'_, 'graph>],
    ) -> Result<(), ShapeError<'graph>> {
        for (index, site) in sites.iter().enumerate() {
            let (typed, solution) = kept[0].instances[index];
            if kept
                .iter()
                .any(|judgement| judgement.instances[index].1 != solution)
            {
                return Err(ShapeError::AmbiguousInstance {
                    key: list.elements,
                    at: node.source,
                });
            }
            self.chain[level].parts.push((Site::of(site.part), typed));
            if self.unfilled {
                continue;
            }
            match site.instanced {
                Instanced::Name(leaf) => {
                    self.chain[level].instances.push((Site::of(leaf), solution))
                }
                Instanced::Literal(body) => body.fix_born_instance(solution),
            }
        }
        Ok(())
    }

    /// What a judged candidate returns: a builtin's return as its [rule](super::rules) gives it; a
    /// registration's shape's return read through its group's intervals — exactly that return where
    /// the retype makes it so and the solve is the call's, the group empty or every interval a
    /// point, since its frame retypes its value to it, and at most it otherwise. A surfaced head's
    /// return is at most whatever the solve: the module's own definition answers the call, and the
    /// signature states only a bound on what it returns.
    fn candidate_return(&self, judgement: Judgement<'_, '_>) -> Interval {
        if let Some(ruled) = judgement.ruled {
            return ruled;
        }
        let Some(registered) = judgement.known.shape() else {
            return unknown();
        };
        let returned = self.returned(registered, judgement.intervals);
        let solved = !judgement.surfaced
            && (judgement.intervals).is_some_and(|all| all.iter().all(|each| each.is_exact()));
        if solved {
            retyped_to(self.types, returned)
        } else {
            under(returned)
        }
    }

    /// Where a builtin's need `need` dropped a candidate over an argument whose lower end is
    /// `lower`, both records: that end, read through its bounds, and the first field the need names
    /// that it lacks.
    fn missing(&self, lower: Parametric, need: KType) -> Option<(KType, BinderSymbol)> {
        let (types, scratch) = (self.types, self.scratch);
        let (TypeNode::Record { fields: has }, TypeNode::Record { fields: needs }) =
            (types.node(lower), types.node(need))
        else {
            return None;
        };
        let field = needs.keys().find(|name| has.get(name.symbol()).is_none())?;
        Some((bound_above(types, scratch, lower), field))
    }

    /// The type of the code the part `operand` runs, where the load traces it to a written quote:
    /// its last statement's upper end as it runs unfilled, read through its bounds — traced code is
    /// written, so nothing composed or `USING` filled it.
    fn traced(&self, level: usize, operand: &'graph ExpressionPart<'graph>) -> Option<KType> {
        let code = self.traced_code(level, operand)?;
        let (_, last) = self.ran.iter().find(|(ran, _)| std::ptr::eq(*ran, code))?;
        Some(bound_above(self.types, self.scratch, last.upper))
    }

    /// The code shape the part `operand` runs, where the load traces it to a written quote: the
    /// quote itself, or a name a `LET` binds to one. Code names extend this; composed code has no
    /// shape at load.
    fn traced_code(
        &self,
        level: usize,
        operand: &'graph ExpressionPart<'graph>,
    ) -> Option<&'graph BodyShape<'graph>> {
        let shape = self.chain[level].shape;
        match operand {
            ExpressionPart::QuotedExpression(_) => shape.nested(Site::of(operand)),
            ExpressionPart::Identifier(_) => {
                let coordinate = shape.mention(Site::of(operand))?.coordinate;
                let (at, slot) = self.slot_of(level, coordinate)?;
                let holder = self.chain[at].shape;
                match holder.rhs(slot)? {
                    rhs @ ExpressionPart::QuotedExpression(_) => holder.nested(Site::of(rhs)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Whether `candidate`, read in the shape at `level`, is an unmarked key's hole: a spread a
    /// `USING` fills.
    fn hole(&self, level: usize, candidate: Candidate) -> bool {
        let Candidate::Spread(Coordinate::Activation {
            hops,
            target: Target::Capture(capture),
        }) = candidate
        else {
            return false;
        };
        let at = level - hops as usize;
        match self.chain[at].shape.captures()[capture.index()].source {
            CaptureSource::Hole => true,
            CaptureSource::Read(inner) if !self.chain[at].roots_chain => {
                self.hole(at - 1, Candidate::Spread(inner))
            }
            _ => false,
        }
    }

    /// The builtin `candidate` names, where it names one.
    fn builtin(&self, candidate: Candidate) -> Option<&'graph BuiltinFunction> {
        match candidate {
            Candidate::One(Coordinate::Builtin(index)) => self
                .builtins
                .get(index)
                .as_callable()
                .and_then(|member| member.builtin()),
            _ => None,
        }
    }

    /// Whether `candidate`, read from the shape at `level`, is a `USING … SCOPE` block's surfaced
    /// head.
    fn surfaced(&self, level: usize, candidate: Candidate) -> bool {
        let Candidate::One(coordinate) = candidate else {
            return false;
        };
        self.slot_of(level, coordinate).is_some_and(|(at, slot)| {
            self.chain[at]
                .shape
                .registration(slot)
                .is_some_and(|registration| registration.surfaced.is_some())
        })
    }

    /// What the load knows of `candidate`'s registered shape, read from the shape at `level`.
    fn candidate(&self, level: usize, candidate: Candidate) -> Known {
        let coordinate = match candidate {
            Candidate::Spread(_) => return Known::Unknown,
            Candidate::One(Coordinate::Builtin(_)) => {
                return match self.builtin(candidate) {
                    Some(builtin) => Known::Closed(builtin.ktype().into()),
                    None => Known::Unknown,
                };
            }
            Candidate::One(coordinate) => coordinate,
        };
        let Some((at, slot)) = self.slot_of(level, coordinate) else {
            return Known::Unknown;
        };
        let holder = self.chain[at].shape;
        if holder.registration(slot).is_none() {
            return Known::Unknown;
        }
        match holder.registered_type(slot) {
            Static::Closed(registered) => Known::Closed(registered.shape),
            Static::Rigid { value, .. } => Known::Rigid(value.shape),
            Static::Unknown => Known::Unknown,
        }
    }
}

/// The names the part `part` holds as written: a one-name quote's name, or a list literal of
/// one-name quotes' names. `None` for anything else, which the load cannot read.
fn written_names<'x>(
    part: &ExpressionPart<'_>,
    scratch: BumpAllocator<'x>,
) -> Option<&'x [BinderSymbol]> {
    let quoted = |part: &ExpressionPart<'_>| match part {
        ExpressionPart::QuotedExpression(node) => one_name(node.reference()),
        _ => None,
    };
    let mut names = BumpVec::new_in(scratch);
    match part {
        ExpressionPart::ListLiteral(items) => {
            for item in items.iter() {
                names.push(quoted(item)?);
            }
        }
        part => names.push(quoted(part)?),
    }
    Some(names.leak())
}

/// Whether the pass has typed `nested`, or need not: a quote's code the load refused is left.
fn done(nested: &BodyShape<'_>) -> bool {
    nested.statics().is_some() || (nested.kind() == ShapeKind::Code && nested.refusal().is_some())
}

/// The static type of the callable body `body`: a point at its function type, or its scheme, where
/// the load typed it, `[Never, Any]` elsewhere.
fn callable(body: &BodyShape<'_>) -> Bound {
    let ktype = match body.callable_type() {
        Static::Closed(callable) => callable.ktype.into(),
        Static::Rigid {
            value: callable, ..
        } => callable.ktype,
        Static::Unknown => return DeclaredType::Type(unknown()),
    };
    ktype.map(Interval::point)
}
