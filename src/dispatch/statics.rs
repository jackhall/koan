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
//! [`bound_above`], so no variable leaks across. A capture is followed to its source through one
//! walk, which reports a crossing into code; a candidate it reaches across a code root whose
//! registered shape names a variable of the chain outside is unknown.
//!
//! Each keyworded use **judges** each candidate class by class ([`judge_by_class`]): *never*
//! drops it, *always* means it admits whatever the run carries, and a *maybe* one the call admits.
//! An argument at a slot whose class solves a variable ([`solving_slots`]) **contributes** its
//! static upper end to the solve, unless that is `Any`: the judge reads it as exactly that type, and
//! the use records it for the call. A contribution naming a lexical variable records where the use
//! reads it — a hop per block, and a **type capture** per callable or module between the use and the
//! variable's home, which the shape lays down past its builder's captures; a module body's `OVER`
//! list must name each one it holds, as it names a capture the builder made.
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
//! ascription holds its operand, settled by its type part's site.
//!
//! A call by name of an exact callee is judged as a keyworded use of its lone candidate: the
//! callee's parameters laid out as one class, in symbol order, and its argument's static type laid
//! onto them end by end. It contributes, records its contributions by its argument's site for the
//! frame — one per parameter, as a keyworded use records one per argument — and is refused where
//! the judge finds it *never*, or where its argument can never name the parameters exactly. What
//! the pass fixes rests in each shape's write-once [`Statics`] cell, which
//! [`evaluate`](super::evaluate) reads.
//!
//! A node is read here exactly as the evaluator reads it, through its [`Form`]. A shape's code is
//! typed before its statements, so an `EVAL` finds it typed. It is typed twice: once for its cell,
//! where an unmarked key's hole is a candidate the load cannot read, since a `USING` may fill it;
//! and once as a traced `EVAL` runs it, unfilled, the hole holding nothing — which fixes nothing.
//! Inside a quote's code a refusal is kept on the code shape, and the `EVAL` running it reports it.
//!
//! A `MODULE` or `GROUP` binder's body is typed where its binder is, since it runs inline, and the
//! binder is exactly the signature the run ties where every member is exact
//! ([`module_signature`]), else at most `Module`. An ascription of a module to a signature is a
//! view: refused where an exact operand can never fit it, exactly the view's signature under `:!`
//! over an exact operand, and at most the signature otherwise. A member read `m.f` is typed by its
//! [rule](super::rules), which reads the member off the operand's signature, and refused where a
//! signature its lower end is lacks the member. A `USING … SCOPE` body is typed as a block, each
//! surfaced name at the member read it names; a key it surfaces is a spread typed by the one head
//! its operand declares there, at most *maybe*, since a view carries every overload its member
//! admits.
//!
//! A static type is over [`Parametric`] types: it may hold the lexical variables of its chain. A
//! binder of a quantified callable is typed by its scheme — a module body's plain `LET` member, or a
//! keyworded form's name — and a call's head reads it as a [`DeclaredType`]. Anywhere else a
//! quantified function is an **instance site**: a part is typed with the type it is **wanted** at —
//! an annotation's, an ascription's, a callable body's declared return for its last statement, a
//! container's element type where that is the container's own type, a call by name's parameter
//! record, and an argument's slot at each candidate — and the site is instantiated at the least
//! instance under it ([`instance_under`]). A member read of a quantified member is a site too,
//! where its operand is a name or a chain of member reads rooted at one; one over a head parameter
//! its signature leaves unpinned is refused there, and read only at a call's head, whose site the
//! cell records with no solution. A quantified callee's other arguments solve its group
//! first, each read as the solve reads it — a contributing one exactly at its upper end — and the
//! slot is read through that solve: a variable a class before the slot's solves is taken from that
//! class's solving slots at their contributions, as the call solves it — class by class, each
//! pinned to what the classes before it solved — where a run reproduces the solve.
//! The candidates a use keeps must agree on each instance. A name's solution is recorded by its
//! site and a literal's in its body's born-instance cell; a site the wanted type fixes nothing at
//! refuses the load. A solution naming a lexical variable records where the site reads it, as a
//! contribution does, and the run reads the type it binds there. So a part's and a statement's
//! static type is never a scheme.
//!
//! Under test, `unnarrowed` loads a program with every keyworded use left whole and none refused,
//! each contribution still recorded: the narrowing law compares a run so loaded with the narrowed
//! one.
//!
//! See [README.md § Static types](README.md#static-types).

use crate::elaborate::module_signature;
use crate::knot::module::view::transparent_view_type;
use crate::knot::{BuiltinFunction, KBuiltins};
use crate::memory::{BumpAllocator, BumpVec, Writer, collect, resident};
use crate::parse::BuiltinShapeId;
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::scope::{
    BodyShape, BuiltinIndex, Candidate, CandidateList, CaptureSlot, CaptureSource, Coordinate,
    Listed, Narrowing, Position, ShapeError, ShapeKind, Site, Slot, Static, StaticSolution,
    StaticType, Statics, SurfacedHead, Target, UnitWork, Variable as Located, source_of,
};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::type_lattice::{
    Collector, DeclaredType, DispatchTokenElement, InstanceFailure, Interval, KType, Parametric,
    Scheme, Side, TypeNode, TypeRegistry, Variable, Variance, Verdict, admits_with, bound_above,
    class_of, fits, instance_under, instantiate_quantified, intervals, judge_by_class, meet,
    quantifier_bounds, read_through, scheme_bound_above, scheme_return, scheme_slots,
    select_by_class, shape_return, shape_slots, solving_slots,
};
use crate::values::{
    ConstructionRefused, Value, construction, dict_type, list_type, record_type, retyped_to, under,
    unknown,
};

use super::builtins::Native;
use super::evaluate::{Form, Wanted, of_node, of_part, slots};
use super::one_name;
use super::rules::{self, Given};
use super::select;

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
    let visited = pass.visit(0, None);
    pass.chain.pop();
    visited
}

#[cfg(test)]
thread_local! {
    /// Whether the pass leaves every keyworded use whole and refuses none: what the narrowing law
    /// loads a program under to see the call a narrowing or a refusal stands for.
    static UNNARROWED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `load` with every keyworded use left whole — no narrowing, no refusal — and each argument's
/// contribution still recorded.
#[cfg(test)]
pub(super) fn unnarrowed<R>(load: impl FnOnce() -> R) -> R {
    /// Clears the switch however `load` ends, so a panicking case leaves it off for the next.
    struct Cleared;
    impl Drop for Cleared {
        fn drop(&mut self) {
            UNNARROWED.with(|switch| switch.set(false));
        }
    }
    UNNARROWED.with(|switch| switch.set(true));
    let _cleared = Cleared;
    load()
}

/// A binder's static type: a type's interval, or a quantified callable's scheme.
type Bound = DeclaredType<Interval>;

/// `value` and `wanted` read through their bounds, where they meet at `Never`: no value of the
/// static type `value` can ever satisfy `wanted`. `None` where one might. Each end is named
/// through its bound, since a variable's name is its binder's.
fn never_satisfies(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    value: Parametric,
    wanted: Parametric,
) -> Option<(KType, KType)> {
    let (value, wanted) = (
        bound_above(types, scratch, value),
        bound_above(types, scratch, wanted),
    );
    (meet(types, scratch, value, wanted) == KType::NEVER).then_some((value, wanted))
}

/// Where a coordinate lands, its captures followed to their sources.
#[derive(Clone, Copy)]
enum Landing {
    Builtin(BuiltinIndex),
    /// A slot of the shape at `level`: a local, or the knot member a capture names.
    Slot {
        level: usize,
        slot: Slot,
    },
    /// A capture whose source the chain does not hold: a hole, a name offered, or a read past the
    /// chain's first shape.
    Open {
        hole: bool,
    },
}

/// A landing, and whether the walk to it crossed into a quote's code: took a capture of a shape
/// that roots a chain.
#[derive(Clone, Copy)]
struct Followed {
    landing: Landing,
    crossed: bool,
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
    instances: BumpVec<'p, (Site, StaticSolution<'graph>)>,
    /// Each keyworded use's contributions, parallel to the candidate lists.
    contributions: BumpVec<'p, &'graph [StaticType<'graph>]>,
    /// Each call by name's contributions, by its argument part's site: per parameter in symbol
    /// order.
    named: BumpVec<'p, (Site, &'graph [StaticType<'graph>])>,
    /// Each type capture a contribution read in a shape nested here added, by the variable's level,
    /// beside the coordinate it reads in the enclosing activation.
    /// Each type capture's variable's name, and the use that added it, ride beside it: a module
    /// body's `OVER` list must name it.
    type_captures: BumpVec<'p, (usize, Coordinate, (TypeSymbol, SourceRef))>,
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
struct Judgement<'x> {
    candidate: Candidate,
    known: Known,
    /// Whether the candidate is a `USING … SCOPE` block's surfaced head, whose return is at most.
    surfaced: bool,
    verdict: Verdict,
    intervals: Option<&'x [Interval]>,
    ruled: Option<Interval>,
    /// The instance each of the use's instance arguments takes at this candidate, in argument
    /// order.
    instances: &'x [Made<'x>],
}

/// An instance: its exact static type beside the solution it is made at, scratch-lived until its
/// site is recorded.
type Made<'x> = (Interval, &'x [Parametric]);

/// Where an instance site is written: a name read there, a quantified `FN` written in place, whose
/// body the instance is born from, or a member read of a quantified member.
#[derive(Clone, Copy)]
enum Instanced<'graph> {
    Name(&'graph ExpressionPart<'graph>),
    Literal(&'graph BodyShape<'graph>),
    /// A member read, `m.f`, of a quantified member.
    Member(&'graph KExpression<'graph>),
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

/// A call by name's argument laid onto its callee's slots: the argument's static upper end, each
/// slot's static type, and each slot's contribution.
type Laid<'p, 'graph> = (Parametric, &'p [Interval], &'p [StaticType<'graph>]);

/// A call by name's callee, read as a keyworded use's lone candidate.
#[derive(Clone, Copy)]
struct Callee<'p> {
    /// The callee's type as the load reads it — its function type or scheme — which a refusal
    /// names.
    function: DeclaredType<Parametric>,
    /// An expression shape over the callee's parameters, one slot each in symbol order, every slot
    /// in class 0, returning its return: a scheme over the callee's group where it has one.
    shape: DeclaredType<Parametric>,
    /// The parameters' names in symbol order: slot `i` is `names[i]`'s.
    names: &'p [BinderSymbol],
    /// The type the argument is wanted at: an unquantified callee's parameter record.
    wanted: Option<Parametric>,
    /// Whether the load knows the callee exactly — a scheme, or a point at an unquantified
    /// function type. Only an exact callee is judged and records contributions: one known at most
    /// may bind a function admitting more, or naming other parameters.
    exact: bool,
}

impl<'p, 'graph: 'p> Pass<'p, '_, 'graph> {
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
    /// shape nested in it. A `USING … SCOPE` body's `surfacing` is the static upper end of the
    /// module it surfaces, which types its surfaced names.
    fn visit(
        &mut self,
        level: usize,
        surfacing: Option<Parametric>,
    ) -> Result<(), ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        for slot in 0..shape.slots() {
            let seeded = self.seeded(level, Slot(slot as u32), surfacing);
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
                at.type_captures.iter().map(|(_, source, _)| *source),
            ));
        }
        // A module body reads a type its enclosing callable binds only where its `OVER` list
        // names it.
        if at.shape.kind() == ShapeKind::Module {
            let listed = at.shape.over().unwrap_or(&[]);
            for (_, _, (name, used)) in at.type_captures.iter() {
                let name = BinderSymbol::Type(*name);
                if !listed.contains(&Listed::Name(name)) {
                    return Err(ShapeError::Unlisted { name, at: *used });
                }
            }
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
        let visited = self.nested(level, nested, code, None);
        match visited {
            Err(error) if code => {
                nested.refuse_typing(resident(self.writer, error));
                Ok(())
            }
            Ok(_) if code => {
                let unfilled = std::mem::replace(&mut self.unfilled, true);
                // A use unfilled code can never run refuses nothing: its `EVAL` faults.
                if let Ok(Some(last)) = self.nested(level, nested, true, None) {
                    self.ran.push((nested, last));
                }
                self.unfilled = unfilled;
                Ok(())
            }
            visited => visited.map(|_| ()),
        }
    }

    /// Push `nested` above the shape at `level`, type it — a `USING … SCOPE` body over what it
    /// is `surfacing` — and pop it: its last statement's static type.
    fn nested(
        &mut self,
        level: usize,
        nested: &'graph BodyShape<'graph>,
        roots_chain: bool,
        surfacing: Option<Parametric>,
    ) -> Result<Option<Interval>, ShapeError<'graph>> {
        self.push(nested, roots_chain);
        let visited = self.visit(level + 1, surfacing);
        let last = self.chain[level + 1].statements.last().copied();
        self.chain.pop();
        visited.map(|_| last)
    }

    /// The static type of a `MODULE` or `GROUP` binder whose body is `body`, nested in the shape at
    /// `level`. The body runs inline before its binder is tied, so it is typed here, ahead of the
    /// shapes nested after it. Exactly the signature the run ties ([`module_signature`]) where the
    /// load knows every member exactly — each value binder at a closed point or a closed scheme,
    /// each type binder and each registration closed — and at most `Module` otherwise.
    fn module(
        &mut self,
        level: usize,
        body: &'graph BodyShape<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let (types, scratch) = (self.types, self.scratch);
        self.push(body, false);
        let visited = self.visit(level + 1, None);
        let binders = self.chain.pop().expect("the body was pushed").binders;
        visited?;
        let mut members = BumpVec::with_capacity_in(binders.len(), scratch);
        let mut keyworded = BumpVec::new_in(scratch);
        for (index, bound) in binders.iter().enumerate() {
            let slot = Slot(index as u32);
            let exact = match body.slot_name(slot) {
                BinderSymbol::Value(_) => match bound {
                    DeclaredType::Type(typed) if typed.is_exact() => types
                        .concrete(typed.upper)
                        .map(|closed| DeclaredType::Type(closed.into())),
                    DeclaredType::Scheme(scheme) => body
                        .births(slot)
                        .filter(|born| matches!(born.callable_type(), Static::Closed(_)))
                        .map(|_| DeclaredType::Scheme(*scheme)),
                    DeclaredType::Type(_) => None,
                },
                BinderSymbol::Type(_) => match body.declared_type(slot) {
                    Static::Closed(held) => Some(DeclaredType::Type(held.into())),
                    _ => None,
                },
                BinderSymbol::Registration(_) => {
                    let Static::Closed(registered) = body.registered_type(slot) else {
                        return Ok(under(KType::EMPTY_SIGNATURE.into()));
                    };
                    keyworded.push(registered.shape);
                    continue;
                }
                BinderSymbol::Key(_) => unreachable!("no binder declares a key"),
            };
            let Some(exact) = exact else {
                return Ok(under(KType::EMPTY_SIGNATURE.into()));
            };
            members.push((slot, exact));
        }
        let signature = module_signature(body, members, &keyworded, types, scratch);
        Ok(Interval::point(signature.into()))
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
    /// hop per block, a type capture per callable or module. The type channel numbers each lexical
    /// variable of a chain in the shape that declares it, so a static type names none without a
    /// home on its own chain.
    fn coordinate_of(
        &mut self,
        level: usize,
        variable: usize,
        named: (TypeSymbol, SourceRef),
    ) -> Coordinate {
        const HOMED: &str = "a lexical variable a static type names has its home on its chain";
        let mut home = level;
        let target = loop {
            let at = &self.chain[home];
            let declared = at.shape.declared_variables().iter();
            if let Some((_, target)) = declared.into_iter().find(|(at, _)| *at == variable) {
                break *target;
            }
            assert!(!at.roots_chain && home > 0, "{HOMED}");
            home -= 1;
        };
        let mut coordinate = Coordinate::Activation { hops: 0, target };
        for inner in home + 1..=level {
            coordinate = match self.chain[inner].shape.kind() {
                ShapeKind::Block => coordinate.through_block(),
                ShapeKind::Callable | ShapeKind::Module => Coordinate::Activation {
                    hops: 0,
                    target: Target::Capture(self.type_capture(inner, variable, named, coordinate)),
                },
                ShapeKind::Program | ShapeKind::Code => unreachable!("{HOMED}"),
            };
        }
        coordinate
    }

    /// The closure slot of the shape at `level` that holds the lexical variable `variable`, read at
    /// `source` in the enclosing activation: the type capture already recorded for it, or a new one
    /// beside the variable's name and the use adding it.
    fn type_capture(
        &mut self,
        level: usize,
        variable: usize,
        named: (TypeSymbol, SourceRef),
        source: Coordinate,
    ) -> CaptureSlot {
        let at = &mut self.chain[level];
        let index = match at
            .type_captures
            .iter()
            .position(|capture| capture.0 == variable)
        {
            Some(index) => index,
            None => {
                at.type_captures.push((variable, source, named));
                at.type_captures.len() - 1
            }
        };
        CaptureSlot((at.shape.captures().len() + index) as u32)
    }

    /// What an argument whose static upper end is `upper`, in the shape at `level`, contributes to
    /// a solve where its call runs: `Unknown` — the carried type — where `upper` is `Any`; `upper`
    /// where it is closed; and `upper` beside where each lexical variable it names is read.
    fn contribution(
        &mut self,
        level: usize,
        upper: Parametric,
        at: SourceRef,
    ) -> StaticType<'graph> {
        if upper == KType::ANY.into() {
            return Static::Unknown;
        }
        let variables = self.located(level, &[upper], at);
        if variables.is_empty() {
            return self
                .types
                .concrete(upper)
                .map_or(Static::Unknown, Static::Closed);
        }
        Static::Rigid {
            value: upper,
            variables,
        }
    }

    /// The solution `solution` of an instance site in the shape at `level`, as its cell records it:
    /// closed where every entry is concrete, and otherwise beside where the site reads each lexical
    /// variable an entry names.
    fn solution(
        &mut self,
        level: usize,
        solution: &[Parametric],
        at: SourceRef,
    ) -> StaticSolution<'graph> {
        let types = self.types;
        let mut closed = BumpVec::with_capacity_in(solution.len(), self.scratch);
        closed.extend(solution.iter().map_while(|each| types.concrete(*each)));
        if closed.len() == solution.len() {
            return Static::Closed(collect(self.writer, closed.iter().copied()));
        }
        let variables = self.located(level, solution, at);
        assert!(
            !variables.is_empty(),
            "an instance's solution names no variable but its chain's lexical ones"
        );
        Static::Rigid {
            value: collect(self.writer, solution.iter().copied()),
            variables,
        }
    }

    /// Where the shape at `level` reads each lexical variable `values` name, by level: a hop per
    /// block and a type capture per callable or module between the read and the variable's home,
    /// each added for the use at `used`. Empty where they name none.
    fn located(
        &mut self,
        level: usize,
        values: &[Parametric],
        used: SourceRef,
    ) -> &'graph [Located] {
        let (types, scratch) = (self.types, self.scratch);
        let mut levels: BumpVec<'_, (usize, TypeSymbol)> = BumpVec::new_in(scratch);
        for value in values {
            read_through(types, scratch, *value, Side::Above, &mut |variable| {
                if let Variable::Lexical { level, name, .. } = variable
                    && !levels.iter().any(|(at, _)| *at == level)
                {
                    levels.push((level, name));
                }
                None
            });
        }
        let mut located = BumpVec::with_capacity_in(levels.len(), scratch);
        for (variable, name) in levels.iter().copied() {
            let at = self.coordinate_of(level, variable, (name, used));
            located.push(Located {
                level: variable,
                at,
            });
        }
        collect(self.writer, located.iter().copied())
    }

    /// A slot's static type before any unit runs: a registration's function type, a type name's
    /// type value's, a parameter's declared type as its body reads it, a name a `USING … SCOPE`
    /// body surfaces from a module at most `surfacing` the member it names, and `[Never, Any]` for
    /// a local its unit sets.
    fn seeded(&self, level: usize, slot: Slot, surfacing: Option<Parametric>) -> Bound {
        let shape = self.chain[level].shape;
        if let (Some(upper), BinderSymbol::Value(_)) = (surfacing, shape.slot_name(slot))
            && shape.slot(shape.slot_name(slot)).map(|(_, at)| at) == Some(Position::PARAMETER)
        {
            let name = shape.slot_name(slot);
            return match rules::member_of(self.types, self.scratch, upper, name) {
                Some(rules::Member {
                    declared: DeclaredType::Type(declared),
                    unpinned: None,
                }) => DeclaredType::Type(under(declared)),
                Some(rules::Member {
                    declared: DeclaredType::Scheme(scheme),
                    unpinned: None,
                }) => DeclaredType::Scheme(scheme),
                _ => DeclaredType::Type(unknown()),
            };
        }
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
                    None if body.kind() == ShapeKind::Module => {
                        DeclaredType::Type(self.module(level, body)?)
                    }
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
            body.fix_born_instance(self.solution(level, solution, statement.source));
        }
        Ok(DeclaredType::Type(typed))
    }

    /// The instance of the quantified function typed by `scheme` at an instance site `at` wanted at
    /// `wanted`: its exact static type beside the solution it is made at. Refused where no function
    /// type is wanted, where the wanted type reaches some variable of the group with nothing, and
    /// where no instance lies under it.
    fn instantiate(
        &self,
        scheme: Scheme,
        wanted: Option<Parametric>,
        at: SourceRef,
    ) -> Result<Made<'p>, ShapeError<'graph>> {
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
        let solution = solution.leak();
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
            // One naming a parameter the load cannot name is refused where it is typed.
            Form::Call(node, _) => match self.member_scheme(level, node) {
                Some((scheme, None)) => Some((Instanced::Member(node), scheme, node.source)),
                _ => None,
            },
            _ => None,
        }
    }

    /// The member read `node`, `ATTR <operand> <label>` with its label written, where it reads a
    /// quantified member: the member's scheme, beside the unpinned head parameter it names, if it
    /// names one. The operand's static type is read without typing it — a name, or a chain of
    /// member reads rooted at one ([`peeked`](Self::peeked)) — so a read the load cannot see into
    /// is no instance site, and faults at run if it reads a scheme there.
    fn member_scheme(
        &self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Option<(Scheme, Option<BinderSymbol>)> {
        let (operand, name) = member_read(node)?;
        let upper = self.peeked(level, operand)?.upper;
        let member = rules::member_of(self.types, self.scratch, upper, name)?;
        let DeclaredType::Scheme(scheme) = member.declared else {
            return None;
        };
        Some((scheme, member.unpinned.map(BinderSymbol::Type)))
    }

    /// The static type of `part`, a name or a chain of member reads rooted at one, read without
    /// typing it: a name's binder's, and a member read's as its rule gives it. `None` for any other
    /// part.
    fn peeked(&self, level: usize, part: &'graph ExpressionPart<'graph>) -> Option<Interval> {
        let shape = self.chain[level].shape;
        match of_part(shape, part) {
            Form::Leaf(
                leaf @ (ExpressionPart::Identifier(_)
                | ExpressionPart::Type(_)
                | ExpressionPart::MarkedName(..)),
            ) => {
                let mention = shape.mention(Site::of(leaf))?;
                self.read_declared(level, mention.coordinate).as_type()
            }
            Form::Call(node, _) => {
                let (operand, name) = member_read(node)?;
                let upper = self.peeked(level, operand)?.upper;
                match rules::member_of(self.types, self.scratch, upper, name)? {
                    rules::Member {
                        declared: DeclaredType::Type(declared),
                        unpinned: None,
                    } => Some(under(declared)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The type a quantified function argument at the slot `slot` of a callee whose group is
    /// bounded by `bounds` is wanted at: the group solved from the `others` first — each other slot
    /// beside the type the solve reads there ([`solved_from`](Self::solved_from)) — then `slot`
    /// read from above through it. A variable an
    /// earlier class than the slot's solves is taken as `fixed` gives it, which is empty where none
    /// is. Any other variable the slot shares with the others must come out one point the call
    /// reproduces — a closed one, or one of a reproducible solve ([`Collector::reproducible`]); one
    /// only the slot names is read through `[Never, bound]`. Refused where the others admit no
    /// solve, or leave a shared variable open.
    fn slot_wanted(
        &self,
        bounds: &[KType],
        slot: Parametric,
        others: &[(Parametric, Interval)],
        fixed: &[Option<Parametric>],
    ) -> Result<Parametric, Unsolved<'p>> {
        let (types, scratch) = (self.types, self.scratch);
        let names = |declared: Parametric, variable: usize| {
            types.references_quantifier(scratch, declared, variable)
        };
        let fixed = |variable: usize| fixed.get(variable).copied().flatten();
        let named = || {
            let mut named = BumpVec::new_in(scratch);
            named.extend((0..bounds.len()).filter(|variable| names(slot, *variable)));
            Unsolved::Open(named)
        };
        let mut collector = Collector::<Parametric>::new(scratch, bounds);
        for (variable, bound) in bounds.iter().enumerate() {
            if let Some(to) = fixed(variable) {
                collector.pin(variable, *bound, to);
            }
        }
        let mut declared = BumpVec::with_capacity_in(others.len(), scratch);
        let mut exact = true;
        for (other, argument) in others.iter() {
            // One naming only fixed variables is admitted against them by the call, and says
            // nothing of the rest.
            if !(0..bounds.len())
                .any(|variable| names(*other, variable) && fixed(variable).is_none())
            {
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
                return Err(
                    match never_satisfies(types, scratch, argument.upper, *other) {
                        Some(_) => Unsolved::Misfit,
                        None => named(),
                    },
                );
            }
            exact &= argument.is_exact();
            declared.push(*other);
        }
        let Ok(solution) = collector.solve(types) else {
            return Err(named());
        };
        let reproducible = collector.reproducible(types);
        let solved = intervals(
            types,
            scratch,
            &declared,
            bounds,
            &solution,
            exact && reproducible,
        );
        let mut chosen = BumpVec::with_capacity_in(bounds.len(), scratch);
        let mut open = BumpVec::new_in(scratch);
        for (variable, bound) in bounds.iter().enumerate() {
            if let Some(to) = fixed(variable) {
                chosen.push(Interval::point(to));
                continue;
            }
            // Never `intervals`' answer for a variable no other slot names: it reads as the bound.
            if !declared.iter().any(|other| names(*other, variable)) {
                chosen.push(Interval::within(*bound));
                continue;
            }
            let interval = solved[variable];
            // A concrete point of a solve a run may not reproduce is still the run's: with `exact`
            // false, only a meet of concrete upper contributions comes out one.
            if names(slot, variable)
                && !(interval.is_exact()
                    && (reproducible || types.concrete(interval.upper).is_some()))
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

    /// The variables of the slot at `position` of the candidate `shape` that a class before that
    /// slot's solves, each solved as the call solves it: class by class, from each earlier class's
    /// solving slots at their contributions the load knows among `contributions`, pinned to what
    /// the classes before it solved. A variable is fixed only where that solve is reproducible
    /// ([`Collector::reproducible`]): the call binds each lexical variable as the load did. `None`
    /// for every other variable, and empty where there is none. `Misfit` where those contributions
    /// never admit at any binding, since the call then never runs; `Open` naming a variable a slot
    /// that solves it reads no contribution at — an instance argument's, or unknown — or one a run
    /// may solve otherwise.
    fn solved_earlier(
        &self,
        shape: DeclaredType<Parametric>,
        position: usize,
        contributions: &[StaticType<'graph>],
    ) -> Result<&'p [Option<Parametric>], Unsolved<'p>> {
        let (types, scratch) = (self.types, self.scratch);
        let DeclaredType::Scheme(scheme) = shape else {
            return Ok(&[]);
        };
        let mut slots = BumpVec::new_in(scratch);
        slots.extend(scheme_slots(scheme, types));
        let bounds = quantifier_bounds(types, scheme);
        let TypeNode::ExpressionShape { classes, .. } = types.scheme_node(scheme) else {
            unreachable!("a candidate's shape is an expression shape")
        };
        let class = |slot: usize| class_of(classes, slot);
        let solving = solving_slots(types, scratch, shape);
        let names = |slot: usize, variable: usize| {
            types.references_quantifier(scratch, slots[slot], variable)
        };
        let earlier = |slot: usize| class(slot) < class(position);
        let mut shared = BumpVec::new_in(scratch);
        shared.extend((0..bounds.len()).filter(|variable| {
            names(position, *variable)
                && (0..slots.len()).any(|slot| earlier(slot) && names(slot, *variable))
        }));
        if shared.is_empty() {
            return Ok(&[]);
        }
        let open = |slot: usize| {
            let mut open = BumpVec::new_in(scratch);
            open.extend(shared.iter().copied().filter(|v| names(slot, *v)));
            Unsolved::Open(open)
        };
        let all_open = || {
            let mut open = BumpVec::new_in(scratch);
            open.extend(shared.iter().copied());
            Unsolved::Open(open)
        };
        // Each earlier class in order, in a collector of its own pinned to what the classes before
        // it solved, as `admit_by_class` admits the call. Only the shared variables are pinned: a
        // variable these slots name beside them may take contributions from slots skipped here.
        let first = |variable: usize| {
            (0..slots.len())
                .filter(|slot| names(*slot, variable))
                .map(class)
                .min()
        };
        let mut earlier_classes = BumpVec::new_in(scratch);
        earlier_classes.extend((0..slots.len()).filter(|slot| earlier(*slot)).map(class));
        earlier_classes.sort_unstable();
        earlier_classes.dedup();
        let mut fixed = BumpVec::with_capacity_in(bounds.len(), scratch);
        fixed.resize(bounds.len(), None);
        for current in earlier_classes.iter().copied() {
            let mut collector = Collector::<Parametric>::new(scratch, bounds);
            for (variable, to) in fixed.iter().enumerate() {
                if let Some(to) = to {
                    collector.pin(variable, bounds[variable], *to);
                }
            }
            let mut admitted_any = false;
            for slot in 0..slots.len() {
                if !(class(slot) == current
                    && solving[slot]
                    && shared.iter().any(|v| names(slot, *v)))
                {
                    continue;
                }
                admitted_any = true;
                let contribution = match contributions[slot] {
                    Static::Closed(contribution) => contribution.into(),
                    Static::Rigid { value, .. } => value,
                    Static::Unknown => return Err(open(slot)),
                };
                let admitted = admits_with(
                    types,
                    scratch,
                    slots[slot],
                    contribution,
                    Variance::Co,
                    &mut collector,
                );
                // A failure a run may not reproduce may admit at some binding: it fixes nothing.
                if admitted.is_err() {
                    return Err(match collector.reproducible(types) {
                        true => Unsolved::Misfit,
                        false => open(slot),
                    });
                }
            }
            if !admitted_any {
                continue;
            }
            let reproducible = collector.reproducible(types);
            let solution = match collector.solve(types) {
                Ok(solution) if reproducible => solution,
                Err(_) if reproducible => return Err(Unsolved::Misfit),
                _ => return Err(all_open()),
            };
            for variable in shared.iter().copied() {
                if first(variable) == Some(current) {
                    fixed[variable] = Some(solution[variable]);
                }
            }
        }
        Ok(scratch.alloc_slice_fill_iter(fixed.iter().copied()))
    }

    /// What `shape`'s solve reads at each slot: a slot that solves, whose argument contributes,
    /// exactly that argument's upper end — the call solves from that type — and every other slot
    /// its argument's static type. `contributions` past its end are `Unknown`.
    fn solved_from(
        &self,
        shape: DeclaredType<Parametric>,
        arguments: &[Interval],
        contributions: &[StaticType<'graph>],
    ) -> &'p [Interval] {
        let solving = solving_slots(self.types, self.scratch, shape);
        let mut read = BumpVec::with_capacity_in(arguments.len(), self.scratch);
        read.extend(arguments.iter().enumerate().map(|(position, argument)| {
            let contributed = solving.get(position) == Some(&true)
                && contributions
                    .get(position)
                    .is_some_and(|each| !matches!(each, Static::Unknown));
            match contributed {
                true => Interval::point(argument.upper),
                false => *argument,
            }
        }));
        read.leak()
    }

    /// The instance each of `sites`, the instance arguments of a use whose arguments' static types
    /// are `arguments` and whose contributions are `contributions`, takes at a candidate whose
    /// registered shape is `shape`: wanted at its slot's type, read through the candidate's group
    /// solved from its other arguments where it has one, each as the solve reads it
    /// ([`solved_from`](Self::solved_from)) — a variable a class before the slot's solves taken
    /// from that class's solving slots at their contributions, as the call solves it, class by
    /// class. Refused where the load does not know the shape or makes no instance; `None` where the
    /// other arguments do not fit their slots, which the judge refuses.
    fn instances(
        &self,
        shape: Option<DeclaredType<Parametric>>,
        sites: &[InstanceArgument<'graph>],
        arguments: &[Interval],
        contributions: &[StaticType<'graph>],
    ) -> Result<&'p [Made<'p>], Option<ShapeError<'graph>>> {
        let (types, scratch) = (self.types, self.scratch);
        let mut slots = BumpVec::new_in(scratch);
        let Some(shape) = shape else {
            return Err(Some(self.unfixed(sites[0].scheme, None, sites[0].at)));
        };
        let (bounds, quantifiers) = match shape {
            DeclaredType::Type(shape) => {
                slots.extend(shape_slots(shape, types));
                (&[][..], &[][..])
            }
            DeclaredType::Scheme(scheme) => {
                slots.extend(scheme_slots(scheme, types));
                let quantifiers = match types.scheme_node(scheme) {
                    TypeNode::ExpressionShape { quantifiers, .. } => quantifiers,
                    _ => &[],
                };
                (quantifier_bounds(types, scheme), quantifiers)
            }
        };
        let read = self.solved_from(shape, arguments, contributions);
        let mut others = BumpVec::with_capacity_in(slots.len(), scratch);
        for (position, slot) in slots.iter().enumerate() {
            if !sites.iter().any(|site| site.position == position) {
                others.push((*slot, read[position]));
            }
        }
        let mut made = BumpVec::with_capacity_in(sites.len(), scratch);
        for site in sites.iter() {
            let slot = slots[site.position];
            let wanted = if bounds.is_empty() {
                slot
            } else {
                let fixed = self.solved_earlier(shape, site.position, contributions);
                match fixed.and_then(|fixed| self.slot_wanted(bounds, slot, &others, fixed)) {
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
        if let Some((body, returns)) = never_satisfies(types, scratch, body, ret) {
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
            Form::Block(nested) => self
                .nested(level, nested, false, None)?
                .unwrap_or_else(unknown),
            Form::Lambda(node) => match Site::of_body(node).and_then(|site| shape.nested(site)) {
                Some(body) => match callable(body) {
                    DeclaredType::Type(typed) => typed,
                    DeclaredType::Scheme(scheme) => {
                        let (typed, solution) = self.instantiate(scheme, wanted, node.source)?;
                        if !self.unfilled {
                            body.fix_born_instance(self.solution(level, solution, node.source));
                        }
                        typed
                    }
                },
                None => unknown(),
            },
            Form::Declaration => Interval::point(KType::NULL.into()),
            Form::Ascribe(node) => self.ascribe(level, node)?,
            Form::Eval(node) => self.eval(level, node)?,
            Form::Using(node) => self.using(level, node)?,
            Form::Call(node, list) => match self.member_scheme(level, node) {
                Some((scheme, None)) => {
                    let (typed, solution) = self.instantiate(scheme, wanted, node.source)?;
                    if !self.unfilled {
                        let solution = self.solution(level, solution, node.source);
                        self.chain[level]
                            .instances
                            .push((Site::of_node(node), solution));
                    }
                    typed
                }
                Some((_, Some(parameter))) => {
                    let (_, name) = member_read(node).expect("a member read");
                    return Err(ShapeError::UnpinnedMember {
                        member: name,
                        parameter,
                        at: node.source,
                    });
                }
                None => self.narrow(level, node, list)?,
            },
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
            // A member read at a call's head is read as it is, a quantified member by its scheme:
            // its site is recorded with no solution, which the run reads as the head's mark.
            Form::Call(node, list) if member_read(node).is_some() => {
                if !self.unfilled {
                    self.chain[level]
                        .instances
                        .push((Site::of_node(node), Static::Unknown));
                }
                match self.member_scheme(level, node) {
                    Some((scheme, None)) => Some(DeclaredType::Scheme(scheme)),
                    // A scheme over a parameter the load cannot name: the run solves the call.
                    Some((_, Some(_))) => {
                        let typed = self.narrow(level, node, list)?;
                        self.chain[level].parts.push((Site::of(head), typed));
                        return Ok(DeclaredType::Type(unknown()));
                    }
                    None => None,
                }
            }
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
        if let Some(declared) = declared
            && let Some(viewed) = self.viewed(level, node, typed, declared)
        {
            return viewed;
        }
        let held = self.held(level, typed, &ascribed.value, Site::of_node(node));
        held.map_err(|(value, ascribed)| ShapeError::AscriptionNeverSatisfied {
            value,
            ascribed,
            at: node.source,
        })
    }

    /// `<module> :! <Sig>` or `<module> :| <Sig>`, the ascription `node` of the shape at `level`
    /// whose operand's static type `typed` is a signature type, as is `declared`: a view, which the
    /// run builds where the module fits. Refused where the operand is exactly a signature that does
    /// not fit `declared` — signatures meet to a set, never to `Never`, so the meet test cannot say
    /// so. A transparent view of an exact operand is exactly the signature the view door lays down;
    /// any other is at most `declared`. `None` where either type is no signature type.
    fn viewed(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        typed: Interval,
        declared: Parametric,
    ) -> Option<Result<Interval, ShapeError<'graph>>> {
        let (types, scratch) = (self.types, self.scratch);
        let signature = |handle: KType| {
            matches!(
                types.node(handle),
                TypeNode::Signature { .. }
                    | TypeNode::SignatureApply { .. }
                    | TypeNode::SignatureMeet { .. }
            )
        };
        let ascribed = bound_above(types, scratch, declared);
        if !signature(ascribed) || !signature(bound_above(types, scratch, typed.upper)) {
            return None;
        }
        if fits(types, scratch, typed.upper, declared) {
            self.chain[level].settled.push(Site::of_node(node));
        }
        let Some(exact) = typed
            .is_exact()
            .then(|| types.concrete(typed.upper))
            .flatten()
        else {
            return Some(Ok(under(declared)));
        };
        if !fits(types, scratch, exact, declared) {
            return Some(Err(ShapeError::AscriptionNeverSatisfied {
                value: exact,
                ascribed,
                at: node.source,
            }));
        }
        let opaque = node.cache().builtin_shape().map(|shape| shape.id)
            == Some(BuiltinShapeId::AscribeOpaque);
        let view = types
            .concrete(declared)
            .filter(|_| !opaque)
            .and_then(|declared| transparent_view_type(exact, declared, types, scratch));
        Some(Ok(view.map_or_else(
            || under(declared),
            |view| Interval::point(view.into()),
        )))
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
        if let Some(refused) = never_satisfies(types, scratch, typed.upper, declared) {
            return Err(refused);
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

    /// `USING <module> SCOPE <body>`: its body's last statement's type, the body typed with each
    /// surfaced name at the member read of the operand it names.
    fn using(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let [_, operand, _, _] = node.parts else {
            unreachable!("a `USING … SCOPE` has its keyword, its module, `SCOPE` and a body")
        };
        let typed = self.part(level, &operand.value)?;
        let shape = self.chain[level].shape;
        let Some(body) = Site::of_body(node).and_then(|site| shape.nested(site)) else {
            return Ok(unknown());
        };
        Ok(self
            .nested(level, body, false, Some(typed.upper))?
            .unwrap_or_else(unknown))
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
        if let Some((value, _)) =
            never_satisfies(types, scratch, typed.upper, KType::ANY_CODE.into())
        {
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
            && let Some((code, returns)) = never_satisfies(types, scratch, code.into(), declared)
        {
            return Err(ShapeError::EvalNeverSatisfied {
                code,
                returns,
                at: node.source,
            });
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
                            let solution = self.solution(level, solution, at);
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
        let Followed { landing, crossed } = self.follow(level, coordinate);
        let read = match landing {
            Landing::Builtin(index) => {
                return self
                    .builtins
                    .get(index)
                    .ktype()
                    .map(|ktype| Interval::point(ktype.into()));
            }
            Landing::Slot { level, slot } => self.chain[level].binders[slot.index()],
            Landing::Open { .. } => return DeclaredType::Type(unknown()),
        };
        if !crossed {
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

    /// Follow `coordinate`, read in the shape at `level`, to where it lands: a hop per block, and
    /// each capture to its source in the shape outside. The one walk every reader of a capture
    /// takes.
    fn follow(&self, mut level: usize, mut coordinate: Coordinate) -> Followed {
        let mut crossed = false;
        let open = |hole, crossed| Followed {
            landing: Landing::Open { hole },
            crossed,
        };
        loop {
            let (hops, target) = match coordinate {
                Coordinate::Builtin(index) => {
                    return Followed {
                        landing: Landing::Builtin(index),
                        crossed,
                    };
                }
                Coordinate::Activation { hops, target } => (hops, target),
            };
            let Some(at) = level.checked_sub(hops as usize) else {
                return open(false, crossed);
            };
            let capture = match target {
                Target::Local(slot) => {
                    return Followed {
                        landing: Landing::Slot { level: at, slot },
                        crossed,
                    };
                }
                Target::Capture(capture) => capture,
            };
            let holder = &self.chain[at];
            let outer = at.checked_sub(1);
            match (holder.shape.captures()[capture.index()].source, outer) {
                (CaptureSource::Hole, _) => return open(true, crossed),
                (CaptureSource::Offered, _) | (_, None) => return open(false, crossed),
                (CaptureSource::Read(inner), Some(outer)) => {
                    crossed |= holder.roots_chain;
                    (level, coordinate) = (outer, inner);
                }
                (CaptureSource::Member { component, index }, Some(outer)) => {
                    let members = self.chain[outer].shape.components()[component.index()].members;
                    return Followed {
                        landing: Landing::Slot {
                            level: outer,
                            slot: members[index as usize],
                        },
                        crossed: crossed || holder.roots_chain,
                    };
                }
            }
        }
    }

    /// The slot `coordinate`, read in the shape at `level`, lands at, whether or not the walk to
    /// it crossed into a quote's code.
    fn landed_slot(&self, level: usize, coordinate: Coordinate) -> Option<(usize, Slot)> {
        match self.follow(level, coordinate).landing {
            Landing::Slot { level, slot } => Some((level, slot)),
            Landing::Builtin(_) | Landing::Open { .. } => None,
        }
    }

    /// `(head argument)`: a construction's identity when the head is a type the load knows, a
    /// call by name's [judged](Self::called) return when the head is a function, else
    /// `[Never, Any]`.
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
        let (types, scratch) = (self.types, self.scratch);
        if let Some(identity) = self.head_handle(level, head) {
            let payload = self.part(level, argument)?;
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
        match self.callee(callee) {
            Some(callee) => self.called(level, callee, argument, node),
            None => {
                self.part(level, argument)?;
                Ok(unknown())
            }
        }
    }

    /// The function a call by name's head of the static type `head` calls, read as a keyworded
    /// use's lone candidate: its parameters laid out as one class, in symbol order. `None` where
    /// the load knows of no function there.
    fn callee(&self, head: Bound) -> Option<Callee<'p>> {
        let (types, scratch) = (self.types, self.scratch);
        let (node, function, exact) = match head {
            DeclaredType::Type(interval) => (
                types.node(interval.upper),
                DeclaredType::Type(interval.upper),
                interval.is_exact(),
            ),
            DeclaredType::Scheme(scheme) => (
                types.scheme_node(scheme),
                DeclaredType::Scheme(scheme),
                true,
            ),
        };
        let TypeNode::KFunction {
            quantifiers,
            bounds,
            params,
            ret,
        } = node
        else {
            return None;
        };
        let mut sorted = BumpVec::with_capacity_in(params.len(), scratch);
        sorted.extend(params.iter());
        sorted.sort_unstable_by_key(|(name, _)| name.symbol());
        let mut elements = BumpVec::with_capacity_in(sorted.len(), scratch);
        elements.extend(
            sorted
                .iter()
                .map(|(_, param)| DispatchTokenElement::Slot(*param)),
        );
        let classes = scratch.alloc_slice_fill_copy(sorted.len(), 0u8);
        // The shape's group may be numbered otherwise than the callee's, and nothing reads one
        // through the other: the shape is judged and returns through its own group, and the frame
        // solves the callee's.
        let shape = types.shape_scheme(scratch, quantifiers, bounds, &elements, classes, ret);
        let wanted = match function {
            DeclaredType::Type(_) => Some(record_type(types, scratch, params.iter())),
            DeclaredType::Scheme(_) => None,
        };
        Some(Callee {
            function,
            shape: shape.handle,
            names: scratch.alloc_slice_fill_iter(sorted.iter().map(|(name, _)| *name)),
            wanted,
            exact: exact && (function.as_type().is_none() || bounds.is_empty()),
        })
    }

    /// The static type `payload` of a call by name's argument laid onto `callee`'s slots: each
    /// slot the field of its name at each end, `Never` below and `Any` above where that end is no
    /// record. Records are width-superset, so every carried record lies above the lower end and
    /// names no field it lacks, and lies under the upper end and names every field it names:
    /// `None` where the lower end is a record lacking a parameter, or the upper end a record naming
    /// a field no parameter declares — the call can never name the parameters exactly.
    fn laid_out(&self, callee: Callee<'_>, payload: Interval) -> Option<&'p [Interval]> {
        let types = self.types;
        let fields = |end: Parametric| match types.node(end) {
            TypeNode::Record { fields } => Some(fields),
            _ => None,
        };
        let (lower, upper) = (fields(payload.lower), fields(payload.upper));
        let declares =
            |field: BinderSymbol| (callee.names.iter()).any(|name| name.symbol() == field.symbol());
        if lower.is_some_and(|lower| {
            (callee.names.iter()).any(|name| lower.get(name.symbol()).is_none())
        }) || upper.is_some_and(|upper| upper.keys().any(|field| !declares(field)))
        {
            return None;
        }
        let mut laid = BumpVec::with_capacity_in(callee.names.len(), self.scratch);
        laid.extend(callee.names.iter().map(|name| {
            Interval {
                lower: (lower.and_then(|lower| lower.get(name.symbol())))
                    .unwrap_or_else(|| KType::NEVER.into()),
                upper: (upper.and_then(|upper| upper.get(name.symbol())))
                    .unwrap_or_else(|| KType::ANY.into()),
            }
        }));
        Some(laid.leak())
    }

    /// A call by name `node` of `callee` over `argument`, judged as a keyworded use of its lone
    /// candidate: the argument [laid out](Self::laid_out) onto the slots, each solving slot's
    /// contribution recorded by the argument's site for the frame, and the shape judged class by
    /// class ([`judge_by_class`]). Refused where the argument can never name the parameters or
    /// fit them; `Never` where a field never arrives. Only an exact callee is judged: one known at
    /// most may bind a function admitting more, or naming other parameters, so its call is at most
    /// its return. A quantified callee's record literal types each field that is an instance site
    /// at its slot read through the group the other fields solve.
    fn called(
        &mut self,
        level: usize,
        callee: Callee<'p>,
        argument: &'graph ExpressionPart<'graph>,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let never = Interval::point(KType::NEVER.into());
        let at_most = |pass: &Self| Ok(under(pass.returned(callee.shape, None)));
        let (payload, arguments, contributions) = match (callee.function, argument) {
            (DeclaredType::Scheme(_), ExpressionPart::RecordLiteral(fields)) => {
                match self.literal(level, callee, argument, fields, node)? {
                    Some(laid) => laid,
                    None => return Ok(never),
                }
            }
            _ => {
                let payload = self.part_at(level, argument, callee.wanted)?;
                if payload.upper == KType::NEVER.into() {
                    return Ok(never);
                }
                let Some(arguments) = self.laid_out(callee, payload) else {
                    return match callee.exact {
                        true => Err(self.never(callee, payload.upper, node)),
                        false => at_most(self),
                    };
                };
                if arguments
                    .iter()
                    .any(|each| each.upper == KType::NEVER.into())
                {
                    return Ok(never);
                }
                if !callee.exact {
                    return at_most(self);
                }
                let contributions =
                    self.contributions(level, callee, arguments, node.source, |_| true);
                (payload.upper, arguments, contributions)
            }
        };
        if !self.unfilled && (contributions.iter()).any(|each| !matches!(each, Static::Unknown)) {
            let recorded = collect(self.writer, contributions.iter().copied());
            self.chain[level].named.push((Site::of(argument), recorded));
        }
        let (types, scratch) = (self.types, self.scratch);
        let read = self.solved_from(callee.shape, arguments, contributions);
        let judged = judge_by_class(types, scratch, callee.shape, read);
        if judged.verdict == Verdict::Never {
            return Err(self.never(callee, payload, node));
        }
        Ok(self.registered_return(callee.shape, judged.intervals, false))
    }

    /// What each of `callee`'s slots contributes to its solve, the argument there of the static
    /// type in `arguments`: a solving slot `typed` holds its argument's upper end, read as
    /// [`contribution`](Self::contribution) reads it, and every other `Unknown`.
    fn contributions(
        &mut self,
        level: usize,
        callee: Callee<'_>,
        arguments: &[Interval],
        at: SourceRef,
        typed: impl Fn(usize) -> bool,
    ) -> &'p [StaticType<'graph>] {
        let solving = solving_slots(self.types, self.scratch, callee.shape);
        let mut contributions = BumpVec::with_capacity_in(arguments.len(), self.scratch);
        for (slot, argument) in arguments.iter().enumerate() {
            contributions.push(match solving.get(slot) == Some(&true) && typed(slot) {
                true => self.contribution(level, argument.upper, at),
                false => Static::Unknown,
            });
        }
        contributions.leak()
    }

    /// A quantified callee's record literal `fields`, the argument of the call by name `node`:
    /// each field that is no instance site typed, then each that is at its slot, wanted at the
    /// slot read through the group the others solve ([`instances`](Self::instances)), and the
    /// record recorded by the argument's site. `None` where a field never arrives. Refused where
    /// the fields do not name the parameters exactly, or where the other fields can never fit their
    /// slots.
    fn literal(
        &mut self,
        level: usize,
        callee: Callee<'p>,
        argument: &'graph ExpressionPart<'graph>,
        fields: &'graph [(BinderSymbol, ExpressionPart<'graph>)],
        node: &'graph KExpression<'graph>,
    ) -> Result<Option<Laid<'p, 'graph>>, ShapeError<'graph>> {
        let (types, scratch) = (self.types, self.scratch);
        let slot_of = |name: BinderSymbol| {
            (callee.names.iter()).position(|each| each.symbol() == name.symbol())
        };
        let mut typed = BumpVec::with_capacity_in(fields.len(), scratch);
        let mut sites = BumpVec::new_in(scratch);
        for (name, value) in fields.iter() {
            let Some((instanced, scheme, at)) = self.instance_site(level, value) else {
                typed.push(Some(self.part(level, value)?));
                continue;
            };
            // A field naming no parameter refuses the call below, before it is made.
            if let Some(position) = slot_of(*name) {
                sites.push(InstanceArgument {
                    position,
                    part: value,
                    instanced,
                    scheme,
                    at,
                });
            }
            typed.push(None);
        }
        // Each field's upper end, an untyped instance field's at `Any`: what a refusal names.
        let uppers = |typed: &[Option<Interval>]| {
            let each = fields.iter().zip(typed.iter());
            record_type(
                types,
                scratch,
                each.map(|((name, _), typed)| {
                    (*name, typed.map_or(KType::ANY.into(), |typed| typed.upper))
                }),
            )
        };
        let names_exactly = fields.len() == callee.names.len()
            && fields.iter().all(|(name, _)| slot_of(*name).is_some());
        if !names_exactly {
            return Err(self.never(callee, uppers(&typed), node));
        }
        if typed
            .iter()
            .flatten()
            .any(|typed| typed.upper == KType::NEVER.into())
        {
            let never = Interval::point(KType::NEVER.into());
            self.chain[level].parts.push((Site::of(argument), never));
            return Ok(None);
        }
        let mut arguments = BumpVec::with_capacity_in(callee.names.len(), scratch);
        arguments.resize(callee.names.len(), unknown());
        for ((name, _), typed) in fields.iter().zip(typed.iter()) {
            if let (Some(typed), Some(slot)) = (typed, slot_of(*name)) {
                arguments[slot] = *typed;
            }
        }
        let instanced = |slot: usize| sites.iter().any(|site| site.position == slot);
        let contributions = self.contributions(level, callee, &arguments, node.source, |slot| {
            !instanced(slot)
        });
        if !sites.is_empty() {
            let made = match self.instances(Some(callee.shape), &sites, &arguments, contributions) {
                Ok(made) => made,
                Err(None) => return Err(self.never(callee, uppers(&typed), node)),
                Err(Some(error)) => return Err(error),
            };
            for (site, made) in sites.iter().zip(made.iter()) {
                self.record_instance(level, site, *made);
                arguments[site.position] = made.0;
                let field = fields
                    .iter()
                    .position(|(_, value)| std::ptr::eq(value, site.part));
                typed[field.expect("an instance site is a field's value")] = Some(made.0);
            }
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
        let payload = Interval {
            lower: ends(|each| each.lower),
            upper: ends(|each| each.upper),
        };
        self.chain[level].parts.push((Site::of(argument), payload));
        Ok(Some((payload.upper, arguments.leak(), contributions)))
    }

    /// `CallNeverSatisfied` for the call by name `node` of `callee` over an argument whose static
    /// upper end is `payload`, both read through their bounds.
    fn never(
        &self,
        callee: Callee<'_>,
        payload: Parametric,
        node: &KExpression<'_>,
    ) -> ShapeError<'graph> {
        let (types, scratch) = (self.types, self.scratch);
        ShapeError::CallNeverSatisfied {
            callee: match callee.function {
                DeclaredType::Type(function) => {
                    DeclaredType::Type(bound_above(types, scratch, function))
                }
                DeclaredType::Scheme(scheme) => {
                    DeclaredType::Scheme(scheme_bound_above(types, scratch, scheme))
                }
            },
            arguments: bound_above(types, scratch, payload),
            at: node.source,
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
                let (at, slot) = self.landed_slot(level, coordinate)?;
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
                    self.contribution(level, arguments[position].upper, node.source)
                }
                _ => Static::Unknown,
            });
        }
        let contributes = contributions
            .iter()
            .any(|contribution| !matches!(contribution, Static::Unknown));

        // Each argument as a refusal lists it: its upper end, or an instance argument's scheme.
        let uppers = || {
            collect(
                self.writer,
                arguments.iter().enumerate().map(|(position, argument)| {
                    match sites.iter().find(|site| site.position == position) {
                        Some(site) => DeclaredType::Scheme(site.scheme),
                        None => DeclaredType::Type(argument.upper),
                    }
                }),
            )
        };

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
            let mut instances: &[Made<'_>] = &[];
            if !sites.is_empty() {
                match self.instances(known.shape(), &sites, &arguments, &contributions) {
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
                    if contributes {
                        here = self.solved_from(registered, here, &contributions);
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
            // A surfaced key's list may hold functions admitting more than the head it is typed
            // by — a view carries every overload its member admits — so it is at most *maybe*,
            // and *never* only where one body definition is the whole of the key.
            let surfaced = self.surfaced(level, *candidate);
            if let Some(heads) = surfaced {
                let exact = matches!(heads, [SurfacedHead::Body { .. }]);
                verdict = match verdict {
                    Verdict::Never if exact => Verdict::Never,
                    _ => Verdict::Maybe,
                };
            }
            if verdict != Verdict::Never {
                judged.push(Judgement {
                    candidate: *candidate,
                    known,
                    surfaced: surfaced.is_some(),
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
        // The narrowing law's unnarrowed load: see `unnarrowed`.
        #[cfg(test)]
        if UNNARROWED.with(std::cell::Cell::get) {
            return Ok(unknown());
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
            let missing = dropped.and_then(|(lower, need)| self.missing(lower, need, node.source));
            return Err(missing.unwrap_or_else(|| ShapeError::NoAdmittingCandidate {
                key: list.elements,
                arguments: uppers(),
                at: node.source,
            }));
        }
        // A *maybe* an *always* one strictly outranks at the first class never runs.
        let always = || {
            judged.iter().filter_map(|other| match other.known {
                Known::Closed(always) if other.verdict == Verdict::Always => Some(always.into()),
                _ => None,
            })
        };
        let mut left = BumpVec::with_capacity_in(judged.len(), scratch);
        left.extend(judged.iter().copied().filter(|judgement| {
            !matches!(judgement.known, Known::Closed(maybe)
                if judgement.verdict == Verdict::Maybe
                    && select::never_runs(types, scratch, maybe.into(), always()))
        }));
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
                    let builtin =
                        |survivor: usize| self.builtin(judged[survivor].candidate).is_some();
                    match select::winner(&survivors, builtin) {
                        Ok(survivor) => Some(judged[survivor]),
                        Err(count) => {
                            return Err(ShapeError::Ambiguous {
                                key: list.elements,
                                arguments: uppers(),
                                count,
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
        kept: &[Judgement<'_>],
    ) -> Result<(), ShapeError<'graph>> {
        for (index, site) in sites.iter().enumerate() {
            let made = kept[0].instances[index];
            if kept
                .iter()
                .any(|judgement| judgement.instances[index].1 != made.1)
            {
                return Err(ShapeError::AmbiguousInstance {
                    key: list.elements,
                    at: node.source,
                });
            }
            self.record_instance(level, site, made);
        }
        Ok(())
    }

    /// Record the instance `made` the instance argument `site` of the shape at `level` takes: its
    /// part's static type, and its solution by its site or in its body's born-instance cell.
    fn record_instance(&mut self, level: usize, site: &InstanceArgument<'graph>, made: Made<'_>) {
        let (typed, solution) = made;
        self.chain[level].parts.push((Site::of(site.part), typed));
        if self.unfilled {
            return;
        }
        let solution = self.solution(level, solution, site.at);
        match site.instanced {
            Instanced::Name(leaf) => self.chain[level].instances.push((Site::of(leaf), solution)),
            Instanced::Literal(body) => body.fix_born_instance(solution),
            Instanced::Member(node) => {
                self.chain[level]
                    .instances
                    .push((Site::of_node(node), solution));
            }
        }
    }

    /// What a judged candidate returns: a builtin's return as its [rule](super::rules) gives it,
    /// and a registration's its [registered return](Self::registered_return).
    fn candidate_return(&self, judgement: Judgement<'_>) -> Interval {
        if let Some(ruled) = judgement.ruled {
            return ruled;
        }
        let Some(registered) = judgement.known.shape() else {
            return unknown();
        };
        self.registered_return(registered, judgement.intervals, judgement.surfaced)
    }

    /// What a call of the registered shape `registered` returns, its group solved to `intervals`:
    /// its return read through them — exactly that return where the retype makes it so and the
    /// solve is the call's, the shape unquantified or every interval a point, since its frame
    /// retypes its value to it, and at most it otherwise. A `surfaced` head's return is at most
    /// whatever the solve: the module's own definition answers the call, and the signature states
    /// only a bound on what it returns.
    fn registered_return(
        &self,
        registered: DeclaredType<Parametric>,
        intervals: Option<&[Interval]>,
        surfaced: bool,
    ) -> Interval {
        let returned = self.returned(registered, intervals);
        let solved = !surfaced
            && match registered {
                DeclaredType::Type(_) => true,
                DeclaredType::Scheme(_) => {
                    intervals.is_some_and(|all| all.iter().all(|each| each.is_exact()))
                }
            };
        if solved {
            retyped_to(self.types, returned)
        } else {
            under(returned)
        }
    }

    /// Where a builtin's need `need` dropped a candidate over an argument whose lower end is
    /// `lower`, the refusal in the native's own words: a record lacking the first field the need
    /// names, or a module's signature lacking the member a read needs — that end read through its
    /// bounds.
    fn missing(&self, lower: Parametric, need: KType, at: SourceRef) -> Option<ShapeError<'graph>> {
        let (types, scratch) = (self.types, self.scratch);
        let of = bound_above(types, scratch, lower);
        match (types.node(lower), types.node(need)) {
            (TypeNode::Record { fields: has }, TypeNode::Record { fields: needs }) => {
                let field = needs.keys().find(|name| has.get(name.symbol()).is_none())?;
                Some(ShapeError::NoField { of, field, at })
            }
            (TypeNode::Signature { .. }, TypeNode::Signature { schema, .. }) => {
                let member = (schema
                    .value_slots
                    .iter()
                    .map(|(name, _)| BinderSymbol::Value(*name)))
                .chain(
                    schema
                        .parameters
                        .iter()
                        .map(|(name, _)| BinderSymbol::Type(*name)),
                )
                .next()?;
                Some(ShapeError::NoMember { of, member, at })
            }
            _ => None,
        }
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
                let (at, slot) = self.landed_slot(level, coordinate)?;
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
    /// `USING` fills. A hole past a quote's code root is not this code's.
    fn hole(&self, level: usize, candidate: Candidate) -> bool {
        let Candidate::Spread(coordinate) = candidate else {
            return false;
        };
        matches!(
            self.follow(level, coordinate),
            Followed {
                landing: Landing::Open { hole: true },
                crossed: false,
            }
        )
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

    /// The heads a `USING … SCOPE` block's surfaced key holds, where `candidate`, read from the
    /// shape at `level`, is one.
    fn surfaced(
        &self,
        level: usize,
        candidate: Candidate,
    ) -> Option<&'graph [SurfacedHead<'graph>]> {
        let Candidate::Spread(coordinate) = candidate else {
            return None;
        };
        let (at, slot) = self.landed_slot(level, coordinate)?;
        self.chain[at].shape.registration(slot)?.surfaced
    }

    /// What the load knows of `candidate`'s registered shape, read from the shape at `level`. A
    /// candidate reached across a quote's code root whose registered shape names a variable of the
    /// chain outside is unknown: the code roots a chain of its own.
    fn candidate(&self, level: usize, candidate: Candidate) -> Known {
        let coordinate = match candidate {
            // A surfaced key's list is typed by its head; any other spread is unknown.
            Candidate::Spread(coordinate) if self.surfaced(level, candidate).is_some() => {
                coordinate
            }
            Candidate::Spread(_) => return Known::Unknown,
            Candidate::One(Coordinate::Builtin(_)) => {
                return match self.builtin(candidate) {
                    Some(builtin) => Known::Closed(builtin.ktype().into()),
                    None => Known::Unknown,
                };
            }
            Candidate::One(coordinate) => coordinate,
        };
        let Followed {
            landing: Landing::Slot { level: at, slot },
            crossed,
        } = self.follow(level, coordinate)
        else {
            return Known::Unknown;
        };
        let holder = self.chain[at].shape;
        if holder.registration(slot).is_none() {
            return Known::Unknown;
        }
        match holder.registered_type(slot) {
            Static::Closed(registered) => Known::Closed(registered.shape),
            Static::Rigid { value, .. } if !crossed => Known::Rigid(value.shape),
            Static::Rigid { .. } | Static::Unknown => Known::Unknown,
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

/// The operand and the written label of `node` where it is a member read, `ATTR <operand> <label>`.
fn member_read<'graph>(
    node: &'graph KExpression<'graph>,
) -> Option<(&'graph ExpressionPart<'graph>, BinderSymbol)> {
    if node.cache().builtin_shape()?.id != BuiltinShapeId::Attribute {
        return None;
    }
    let [_, operand, label] = node.parts else {
        return None;
    };
    let name = match label.value {
        ExpressionPart::Identifier(name) => BinderSymbol::Value(name),
        ExpressionPart::Type(name) => BinderSymbol::Type(name),
        _ => return None,
    };
    Some((&operand.value, name))
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
