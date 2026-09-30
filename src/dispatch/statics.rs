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
//! checks nothing. What the pass fixes rests in each shape's write-once [`Statics`] cell, which
//! [`evaluate`](super::evaluate) reads.
//!
//! A node is read here exactly as the evaluator reads it, through its [`Form`]. A shape's code is
//! typed before its statements, so an `EVAL` finds it typed. It is typed twice: once for its cell,
//! where an unmarked key's hole is a candidate the load cannot read, since a `USING` may fill it;
//! and once as a traced `EVAL` runs it, unfilled, the hole holding nothing — which fixes nothing.
//! Inside a quote's code a refusal is kept on the code shape, and the `EVAL` running it reports it.
//!
//! See [README.md § Static types](README.md#static-types).

use crate::knot::{BuiltinFunction, KBuiltins};
use crate::memory::{BumpAllocator, BumpVec, Writer, collect, resident};
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::scope::{
    BodyShape, Candidate, CandidateList, CaptureSource, Coordinate, Narrowing, Position,
    ShapeError, ShapeKind, Site, Slot, Static, Statics, Target, UnitWork,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    Collector, Interval, KType, Record, Side, TypeNode, TypeRegistry, Variance, Verdict,
    admits_with, bound_above, class_at_least, instantiate_quantified, intervals, is_subtype_of,
    join_iter, judge_by_class, meet, quantifier_bounds, read_through, select_by_class,
    shape_return,
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
pub(super) fn under(upper: KType) -> Interval {
    match upper {
        KType::NUMBER | KType::STR | KType::BOOL | KType::NULL => Interval::point(upper),
        _ => Interval::within(upper),
    }
}

/// The static type of a value retyped to `declared` (an ascription's, a parameter's, a frame's
/// return): exactly `declared` where every value satisfying it is retyped to it — a list, dict or
/// record type, a family or its application, a newtype or a union's variant — and at most
/// `declared` otherwise; a union keeps each variant's own type.
pub(super) fn retyped_to(types: &TypeRegistry<'_>, declared: KType) -> Interval {
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
    binders: BumpVec<'p, Interval>,
    narrowings: BumpVec<'p, Narrowing<'graph>>,
    /// Each `:!` whose operand's static upper end lies under its type.
    settled: BumpVec<'p, Site>,
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
    Closed(KType),
    /// Over lexical variables the run supplies.
    Rigid(KType),
    Unknown,
}

/// One candidate a keyworded use kept: what the load knows of it, its verdict, its group's
/// intervals where the static solve succeeded, and a builtin's return as its rule gives it.
#[derive(Clone, Copy)]
struct Judgement<'x> {
    candidate: Candidate,
    known: Known,
    verdict: Verdict,
    intervals: Option<&'x [Interval]>,
    ruled: Option<Interval>,
}

impl<'p, 'graph> Pass<'p, '_, 'graph> {
    fn push(&mut self, shape: &'graph BodyShape<'graph>, roots_chain: bool) {
        let scratch = self.scratch;
        let unknown = under(KType::ANY);
        let mut statements = BumpVec::with_capacity_in(shape.body().len(), scratch);
        statements.resize(shape.body().len(), unknown);
        let mut binders = BumpVec::with_capacity_in(shape.slots(), scratch);
        binders.resize(shape.slots(), unknown);
        let mut narrowings = BumpVec::with_capacity_in(shape.candidate_lists().len(), scratch);
        narrowings.resize(shape.candidate_lists().len(), Narrowing::Full);
        self.chain.push(Level {
            shape,
            roots_chain,
            parts: BumpVec::new_in(scratch),
            statements,
            binders,
            narrowings,
            settled: BumpVec::new_in(scratch),
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
                    let typed = self.node(level, &shape.body()[index as usize])?;
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
        let (writer, types) = (self.writer, self.types);
        let at = &mut self.chain[level];
        at.parts.sort_unstable_by_key(|(site, _)| *site);
        at.settled.sort_unstable();
        debug_assert!(
            (at.parts.iter().map(|(_, typed)| typed))
                .chain(at.statements.iter())
                .chain(at.binders.iter())
                .all(|typed| !types.contains_quantified(typed.lower)
                    && !types.contains_quantified(typed.upper)),
            "no static type holds a free `Quantified`"
        );
        at.shape.fix_statics(Statics {
            parts: collect(writer, at.parts.iter().copied()),
            statements: collect(writer, at.statements.iter().copied()),
            binders: collect(writer, at.binders.iter().copied()),
            narrowings: collect(writer, at.narrowings.iter().copied()),
            settled: collect(writer, at.settled.iter().copied()),
        });
    }

    /// A slot's static type before any unit runs: a registration's function type, a type name's
    /// type value's, a parameter's declared type as its body reads it, and `[Never, Any]` for a
    /// local its unit sets.
    fn seeded(&self, level: usize, slot: Slot) -> Interval {
        let shape = self.chain[level].shape;
        if shape.registration(slot).is_some() {
            return shape.births(slot).map_or(under(KType::ANY), callable);
        }
        let name = shape.slot_name(slot);
        if let BinderSymbol::Type(_) = name {
            return match shape.declared_type(slot) {
                Static::Closed(handle) if shape.declarations(slot).is_some() => {
                    Interval::point(KType::of_kind(handle.kind_of(self.types)))
                }
                _ => under(KType::ANY_TYPE),
            };
        }
        let is_parameter = shape
            .slot(name)
            .is_some_and(|(_, position)| position == Position::PARAMETER);
        if shape.kind() != ShapeKind::Callable || !is_parameter {
            return under(KType::ANY);
        }
        let declared = self
            .in_body(shape)
            .and_then(|ktype| match self.types.node(ktype) {
                TypeNode::KFunction { params, .. } => params.get(name.symbol()),
                _ => None,
            });
        retyped_to(self.types, declared.unwrap_or(KType::ANY))
    }

    /// A callable body's function type as its body reads it: its own group instantiated at its
    /// group levels. `None` where the load did not type the callable.
    fn in_body(&self, shape: &BodyShape<'graph>) -> Option<KType> {
        let ktype = match shape.callable_type() {
            Static::Closed(callable)
            | Static::Rigid {
                value: callable, ..
            } => callable.ktype,
            Static::Unknown => return None,
        };
        let bounds = quantifier_bounds(self.types, ktype);
        if bounds.is_empty() {
            return Some(ktype);
        }
        debug_assert_eq!(
            bounds.len(),
            shape.group_levels().len(),
            "the type channel numbered the callable's own group"
        );
        Some(instantiate_quantified(
            self.types,
            self.scratch,
            ktype,
            shape.group_levels(),
        ))
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
                self.chain[level].binders[member.index()] = match body.form() {
                    Some(_) => callable(body),
                    None => under(KType::ANY),
                };
            }
        }
        for member in component.members {
            if shape.births(*member).is_some() {
                continue;
            }
            if let Some(rhs) = shape.rhs(*member) {
                let typed = self.part(level, rhs)?;
                self.chain[level].binders[member.index()] = typed;
            }
        }
        // A binding statement's value is the one binder it writes, where it writes only one.
        let mut values = component
            .members
            .iter()
            .filter(|member| shape.registration(**member).is_none());
        if let (Some(only), None) = (values.next(), values.next()) {
            let written = shape.slot(shape.slot_name(*only)).map(|(_, at)| at);
            if let Some(statement) = written.and_then(Position::statement_index) {
                let typed = self.chain[level].binders[only.index()];
                self.chain[level].statements[statement] = typed;
            }
        }
        Ok(())
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
        if body == KType::NEVER {
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

    /// The static type of the part `part` of the shape at `level`, recorded by its site.
    fn part(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        let typed = self.form(level, of_part(shape, part))?;
        self.chain[level].parts.push((Site::of(part), typed));
        Ok(typed)
    }

    /// The static type of the statement `node` of the shape at `level`.
    fn node(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        self.form(level, of_node(shape, node))
    }

    /// The static type of a node or part the evaluator reads as `form`.
    fn form(&mut self, level: usize, form: Form<'graph>) -> Result<Interval, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        Ok(match form {
            Form::Leaf(part) => self.leaf(level, part)?,
            Form::Block(nested) => self
                .nested(level, nested, false)?
                .unwrap_or(under(KType::ANY)),
            Form::Lambda(node) => Site::of_body(node)
                .and_then(|site| shape.nested(site))
                .map_or(under(KType::ANY), callable),
            Form::Declaration => Interval::point(KType::NULL),
            Form::Ascribe(node) => self.ascribe(level, node)?,
            Form::Eval(node) => self.eval(level, node)?,
            Form::Call(node, list) => self.narrow(level, node, list)?,
            Form::Apply(head, argument) => self.apply(level, head, argument)?,
            Form::Unevaluable(_) => under(KType::ANY),
        })
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
        let typed = self.part(level, &operand.value)?;
        // An operand that never arrives has no value to check.
        if typed.upper == KType::NEVER {
            return Ok(Interval::point(KType::NEVER));
        }
        let Some(ascribed) = self.declared(level, &ascribed.value) else {
            return Ok(under(KType::ANY));
        };
        let (types, scratch) = (self.types, self.scratch);
        // Compared, and named, through their bounds: a variable's name is its binder's.
        let (value, bounded) = (
            bound_above(types, scratch, typed.upper),
            bound_above(types, scratch, ascribed),
        );
        if meet(types, scratch, value, bounded) == KType::NEVER {
            return Err(ShapeError::AscriptionNeverSatisfied {
                value,
                ascribed: bounded,
                at: node.source,
            });
        }
        if is_subtype_of(types, scratch, typed.upper, ascribed) {
            self.chain[level].settled.push(Site::of_node(node));
        }
        Ok(retyped_to(types, ascribed))
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
        if typed.upper == KType::NEVER {
            return Ok(Interval::point(KType::NEVER));
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
            return Ok(under(KType::ANY));
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
    fn declared(&self, level: usize, part: &'graph ExpressionPart<'graph>) -> Option<KType> {
        match self.chain[level].shape.typed_expression(Site::of(part)) {
            Static::Closed(handle) | Static::Rigid { value: handle, .. } => Some(handle),
            Static::Unknown => None,
        }
    }

    /// A leaf part: a literal's type, a name's binder's, a quote's code type, a type value's kind,
    /// or a container's over its parts, end by end.
    fn leaf(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        let (types, scratch) = (self.types, self.scratch);
        Ok(match part {
            ExpressionPart::Literal(KLiteral::Number(_)) => Interval::point(KType::NUMBER),
            ExpressionPart::Literal(KLiteral::String(_)) => Interval::point(KType::STR),
            ExpressionPart::Literal(KLiteral::Boolean(_)) => Interval::point(KType::BOOL),
            ExpressionPart::Literal(KLiteral::Null) => Interval::point(KType::NULL),
            ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::MarkedName(..) => match shape.mention(Site::of(part)) {
                Some(mention) => self.read(level, mention.coordinate),
                None => under(KType::ANY),
            },
            ExpressionPart::QuotedExpression(_) => shape
                .nested(Site::of(part))
                .map_or(under(KType::ANY), |code| Interval::point(code.code_type())),
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                match shape.typed_expression(Site::of(part)) {
                    Static::Closed(handle) => {
                        Interval::point(KType::of_kind(handle.kind_of(types)))
                    }
                    _ => under(KType::ANY_TYPE),
                }
            }
            ExpressionPart::ListLiteral(items) => {
                let mut typed = BumpVec::with_capacity_in(items.len(), scratch);
                for item in items.iter() {
                    typed.push(self.part(level, item)?);
                }
                Interval {
                    lower: list_type(types, scratch, typed.iter().map(|each| each.lower)),
                    upper: list_type(types, scratch, typed.iter().map(|each| each.upper)),
                }
            }
            ExpressionPart::DictLiteral(pairs) => {
                let mut typed = BumpVec::with_capacity_in(pairs.len(), scratch);
                for (key, value) in pairs.iter() {
                    typed.push((self.part(level, key)?, self.part(level, value)?));
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
                let mut typed = BumpVec::with_capacity_in(fields.len(), scratch);
                for (name, value) in fields.iter() {
                    typed.push((*name, self.part(level, value)?));
                }
                Interval {
                    lower: record_type(types, scratch, typed.iter().map(|(n, v)| (*n, v.lower))),
                    upper: record_type(types, scratch, typed.iter().map(|(n, v)| (*n, v.upper))),
                }
            }
            ExpressionPart::Keyword(_)
            | ExpressionPart::Expression(_)
            | ExpressionPart::MarkedUse(..) => under(KType::ANY),
        })
    }

    /// The static type of what `coordinate`, read in the shape at `level`, holds. A capture along
    /// the chain reads its source's; one crossing into a quote's code reads it through its bounds.
    fn read(&self, level: usize, coordinate: Coordinate) -> Interval {
        let (hops, target) = match coordinate {
            Coordinate::Builtin(index) => return Interval::point(self.builtins.get(index).ktype()),
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
            (CaptureSource::Read(inner), Some(outer)) => self.read(outer, inner),
            (CaptureSource::Member { component, index }, Some(outer)) => {
                let member =
                    self.chain[outer].shape.components()[component.index()].members[index as usize];
                self.chain[outer].binders[member.index()]
            }
            _ => return under(KType::ANY),
        };
        if self.chain[at].roots_chain {
            under(bound_above(self.types, self.scratch, read.upper))
        } else {
            read
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
        head: &'graph ExpressionPart<'graph>,
        argument: &'graph ExpressionPart<'graph>,
    ) -> Result<Interval, ShapeError<'graph>> {
        let callee = self.part(level, head)?;
        let payload = self.part(level, argument)?;
        let (types, scratch) = (self.types, self.scratch);
        if let Some(identity) = self.head_handle(level, head) {
            let upper = match construction(types, scratch, identity, payload.upper) {
                Ok(constructed) => constructed,
                Err(ConstructionRefused::NotConstructible(_)) => KType::ANY,
                // A misfit faults; an unsolved family lies under the bare family.
                Err(_) => identity,
            };
            let lower =
                construction(types, scratch, identity, payload.lower).unwrap_or(KType::NEVER);
            return Ok(Interval { lower, upper });
        }
        Ok(match types.node(callee.upper) {
            TypeNode::KFunction {
                bounds,
                params,
                ret,
                ..
            } => {
                let (returned, solved) = self.called(bounds, params, ret, payload);
                // Only an exact callee: a function is never retyped, so one at most its type may
                // return less.
                if callee.is_exact() && solved {
                    retyped_to(types, returned)
                } else {
                    under(returned)
                }
            }
            _ => under(KType::ANY),
        })
    }

    /// What a call by name of a function over `params` returning `ret`, its group bounded by
    /// `bounds`, returns for an argument of the static type `payload`: `ret` read through the
    /// intervals the argument's fields solve the group to, or its bounds where they do not; beside
    /// whether that solve is the call's — the group empty, or every interval a point.
    fn called(
        &self,
        bounds: &[KType],
        params: Record<'_>,
        ret: KType,
        payload: Interval,
    ) -> (KType, bool) {
        let (types, scratch) = (self.types, self.scratch);
        if bounds.is_empty() {
            return (ret, true);
        }
        let fields = |typed| match types.node(typed) {
            TypeNode::Record { fields } => Some(fields),
            _ => None,
        };
        let Some(upper) = fields(payload.upper) else {
            return (self.through(ret, None), false);
        };
        let lower = fields(payload.lower);
        let mut collector = Collector::new(scratch, bounds);
        let mut declared = BumpVec::with_capacity_in(params.len(), scratch);
        let mut exact = true;
        for (name, param) in params.iter() {
            let Some(field) = upper.get(name.symbol()) else {
                return (self.through(ret, None), false);
            };
            if admits_with(types, scratch, param, field, Variance::Co, &mut collector).is_err() {
                return (self.through(ret, None), false);
            }
            if types.contains_quantified(param) {
                // As a keyworded use judges it: a rigid variable makes no solve the call's.
                exact &= lower.and_then(|lower| lower.get(name.symbol())) == Some(field)
                    && !types.contains_rigid(field);
            }
            declared.push(param);
        }
        match collector.solve(types) {
            Ok(solution) => {
                let solved = intervals(types, scratch, &declared, bounds, &solution, exact);
                (self.through(ret, Some(&solved)), exact)
            }
            Err(_) => (self.through(ret, None), false),
        }
    }

    /// `ret`, a return under its own group, read through that group's `intervals` — each variable
    /// at `[Never, bound]` where there are none — keeping every lexical variable.
    fn through(&self, ret: KType, intervals: Option<&[Interval]>) -> KType {
        read_through(
            self.types,
            self.scratch,
            ret,
            Side::Above,
            &mut |node| match *node {
                TypeNode::Quantified { index, bound } => Some(
                    intervals
                        .and_then(|all| all.get(index).copied())
                        .unwrap_or(Interval::within(bound)),
                ),
                _ => None,
            },
        )
    }

    /// `shape`'s declared return read through its own group's `intervals`.
    fn returned(&self, shape: KType, intervals: Option<&[Interval]>) -> KType {
        let ret = shape_return(shape, self.types).expect("a registered shape returns");
        self.through(ret, intervals)
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
        for wanted in wanted.iter() {
            let each = match wanted {
                Wanted::Label(name) => Given {
                    typed: Interval::point(match name {
                        BinderSymbol::Type(_) => KType::TYPE_NAME_TOKEN,
                        _ => KType::IDENTIFIER,
                    }),
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
            .any(|argument| argument.upper == KType::NEVER)
        {
            return Ok(Interval::point(KType::NEVER));
        }
        let uppers = || collect(self.writer, arguments.iter().map(|argument| argument.upper));

        let mut judged = BumpVec::with_capacity_in(list.candidates.len(), scratch);
        // The first argument a builtin's need dropped a candidate over, beside that need.
        let mut dropped = None;
        for candidate in list.candidates {
            // Unfilled code holds no function at an unmarked key's hole.
            if self.unfilled && self.hole(level, *candidate) {
                continue;
            }
            let known = self.candidate(level, *candidate);
            let (mut verdict, intervals) = match known {
                Known::Closed(registered) | Known::Rigid(registered) => {
                    let judged = judge_by_class(types, scratch, registered, &arguments);
                    (judged.verdict, judged.intervals)
                }
                Known::Unknown => (Verdict::Maybe, None),
            };
            let builtin = self.builtin(*candidate);
            let mut ruled = None;
            if let Some(builtin) = builtin.filter(|_| verdict != Verdict::Never) {
                let native = Native::of(builtin.id());
                let typed = rules::typed(native, builtin.ktype(), &given, types, scratch);
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
                    verdict,
                    intervals,
                    ruled,
                });
            }
        }
        if judged.is_empty() {
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
        let beats = |a: KType, m: KType| {
            class_at_least(types, scratch, a, m, 0) && !class_at_least(types, scratch, m, a, 0)
        };
        let outranked = |judgement: &Judgement<'_>| match judgement.known {
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
                    let mut shapes = BumpVec::with_capacity_in(judged.len(), scratch);
                    shapes.extend(judged.iter().map(|judgement| match judgement.known {
                        Known::Closed(registered) => registered,
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
        let mut returns = BumpVec::with_capacity_in(judged.len(), scratch);
        for judgement in judged.iter() {
            if let Known::Unknown = judgement.known {
                return Ok(under(KType::ANY));
            }
            returns.push(self.candidate_return(*judgement).upper);
        }
        Ok(under(join_iter(types, scratch, returns.iter().copied())))
    }

    /// What a judged candidate returns: a builtin's return as its [rule](super::rules) gives it; a
    /// registration's shape's return read through its group's intervals — exactly that return where
    /// the retype makes it so and the solve is the call's, the group empty or every interval a
    /// point, since its frame retypes its value to it, and at most it otherwise.
    fn candidate_return(&self, judgement: Judgement<'_>) -> Interval {
        if let Some(ruled) = judgement.ruled {
            return ruled;
        }
        let registered = match judgement.known {
            Known::Closed(registered) | Known::Rigid(registered) => registered,
            Known::Unknown => return under(KType::ANY),
        };
        let returned = self.returned(registered, judgement.intervals);
        let solved =
            (judgement.intervals).is_some_and(|all| all.iter().all(|each| each.is_exact()));
        if solved {
            retyped_to(self.types, returned)
        } else {
            under(returned)
        }
    }

    /// Where a builtin's need `need` dropped a candidate over an argument whose lower end is
    /// `lower`, both records: that end, read through its bounds, and the first field the need names
    /// that it lacks.
    fn missing(&self, lower: KType, need: KType) -> Option<(KType, BinderSymbol)> {
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

    /// What the load knows of `candidate`'s registered shape, read from the shape at `level`.
    fn candidate(&self, level: usize, candidate: Candidate) -> Known {
        let coordinate = match candidate {
            Candidate::Spread(_) => return Known::Unknown,
            Candidate::One(Coordinate::Builtin(_)) => {
                return match self.builtin(candidate) {
                    Some(builtin) => Known::Closed(builtin.ktype()),
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

/// The static type of the callable body `body`: a point at its function type where the load typed
/// it, `[Never, Any]` elsewhere.
fn callable(body: &BodyShape<'_>) -> Interval {
    match body.callable_type() {
        Static::Closed(callable)
        | Static::Rigid {
            value: callable, ..
        } => Interval::point(callable.ktype),
        Static::Unknown => under(KType::ANY),
    }
}
