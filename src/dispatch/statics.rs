//! Static selection: every value expression and value binder given a **static type** where the
//! program loads, and every keyworded use's candidates narrowed by them.
//!
//! A static type is a bound: every type the run carries at that expression lies under it. The pass
//! is local and bidirectional over what the [type channel's load pass](crate::elaborate::type_channel)
//! fixed — a parameter's declared type is its reads', a callee's declared return its calls', and a
//! literal's, a container's, a construction's and a selected candidate's own type flow up. What the
//! load cannot bound is `Any`. A static type may hold the rigid variables of the **region** it is
//! read in — the program, a `FOR ALL` callable's body, a quote's code, numbered as the type channel
//! numbers them — and a capture crossing into a region is read through [`bound_above`], so no
//! variable leaks across.
//!
//! Each keyworded use **drops** a candidate some slot of which meets its argument's static type at
//! `Never`, each read through `bound_above`: it can never admit what the run passes. A use left
//! with none refuses the load. A use left with one closed candidate that admits the static types
//! outright **selects** it, and the call runs it without admitting. A callable body whose static
//! type meets its declared return at `Never` refuses the load too. What the pass fixes rests in
//! each shape's write-once [`Statics`] cell, which [`evaluate`](super::evaluate) reads.
//!
//! A node is read here exactly as the evaluator reads it, through its [`Form`]. Inside a quote's
//! code a refusal is kept on the code shape, and the `EVAL` running it reports it.
//!
//! See [README.md § Static types](README.md#static-types).

use crate::elaborate::writes_for_all;
use crate::knot::KBuiltins;
use crate::memory::{BumpAllocator, BumpVec, Writer, collect, resident};
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::scope::{
    BodyShape, Candidate, CandidateList, CaptureSource, Coordinate, Narrowing, Position,
    ShapeError, ShapeKind, Site, Slot, Static, Statics, Target, UnitWork,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    KType, TypeNode, TypeRegistry, admit_by_class, bound_above, join_iter, meet, shape_return,
    shape_slots,
};
use crate::values::{ConstructionRefused, Value, construction, dict_type, list_type, record_type};

use super::evaluate::{Form, Wanted, of_node, of_part, slots};

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
    };
    pass.push(root, true);
    let visited = pass.visit(0);
    pass.chain.pop();
    visited
}

/// One shape on the chain, innermost last, with what the pass has typed of it so far.
struct Level<'p, 'graph> {
    shape: &'graph BodyShape<'graph>,
    /// Whether the shape roots a region: the program, a quote's code, or a callable body whose
    /// declaration writes a `FOR ALL` group.
    opens_region: bool,
    parts: BumpVec<'p, (Site, KType)>,
    statements: BumpVec<'p, KType>,
    binders: BumpVec<'p, KType>,
    narrowings: BumpVec<'p, Narrowing<'graph>>,
}

/// The walk's state: the chain of enclosing shapes.
struct Pass<'p, 'x, 'graph> {
    builtins: &'x KBuiltins<'graph, 'graph>,
    types: &'x TypeRegistry<'graph>,
    writer: Writer<'graph>,
    scratch: BumpAllocator<'p>,
    chain: BumpVec<'p, Level<'p, 'graph>>,
}

/// What the load knows of one candidate's registered expression shape.
#[derive(Clone, Copy)]
enum Known {
    /// The same at every run.
    Closed(KType),
    /// Over rigid variables the run supplies.
    Rigid(KType),
    Unknown,
}

impl<'p, 'graph> Pass<'p, '_, 'graph> {
    fn push(&mut self, shape: &'graph BodyShape<'graph>, opens_region: bool) {
        let scratch = self.scratch;
        let mut statements = BumpVec::with_capacity_in(shape.body().len(), scratch);
        statements.resize(shape.body().len(), KType::ANY);
        let mut binders = BumpVec::with_capacity_in(shape.slots(), scratch);
        binders.resize(shape.slots(), KType::ANY);
        let mut narrowings = BumpVec::with_capacity_in(shape.candidate_lists().len(), scratch);
        narrowings.resize(shape.candidate_lists().len(), Narrowing::Full);
        self.chain.push(Level {
            shape,
            opens_region,
            parts: BumpVec::new_in(scratch),
            statements,
            binders,
            narrowings,
        });
    }

    /// Type the shape at chain level `level`, fix its cell, then type every shape nested in it.
    fn visit(&mut self, level: usize) -> Result<(), ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        for slot in 0..shape.slots() {
            let seeded = self.seeded(level, Slot(slot as u32));
            self.chain[level].binders[slot] = seeded;
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
            if nested.statics().is_some()
                || (nested.kind() == ShapeKind::Code && nested.refusal().is_some())
            {
                continue;
            }
            let opens_region = match nested.kind() {
                ShapeKind::Code => true,
                ShapeKind::Callable => nested.form().is_some_and(writes_for_all),
                _ => false,
            };
            self.push(nested, opens_region);
            let visited = self.visit(level + 1);
            self.chain.pop();
            match visited {
                Err(error) if nested.kind() == ShapeKind::Code => {
                    nested.refuse_typing(resident(self.writer, error));
                }
                visited => visited?,
            }
        }
        Ok(())
    }

    /// Lay what the pass typed of the shape at `level` down in its cell.
    fn fix(&mut self, level: usize) {
        let writer = self.writer;
        let at = &mut self.chain[level];
        at.parts.sort_unstable_by_key(|(site, _)| *site);
        at.shape.fix_statics(Statics {
            parts: collect(writer, at.parts.iter().copied()),
            statements: collect(writer, at.statements.iter().copied()),
            binders: collect(writer, at.binders.iter().copied()),
            narrowings: collect(writer, at.narrowings.iter().copied()),
        });
    }

    /// A slot's static type before any unit runs: a registration's function type, a type name's
    /// type value's, a parameter's declared type, and `Any` for a local its unit sets.
    fn seeded(&self, level: usize, slot: Slot) -> KType {
        let at = &self.chain[level];
        let shape = at.shape;
        if shape.registration(slot).is_some() {
            return shape.births(slot).map_or(KType::ANY, |body| callable(body));
        }
        let name = shape.slot_name(slot);
        if let BinderSymbol::Type(_) = name {
            return match shape.declared_type(slot) {
                Static::Closed(handle) if shape.declarations(slot).is_some() => {
                    KType::of_kind(handle.kind_of(self.types))
                }
                _ => KType::ANY_TYPE,
            };
        }
        let is_parameter = shape
            .slot(name)
            .is_some_and(|(_, position)| position == Position::PARAMETER);
        if shape.kind() != ShapeKind::Callable || !is_parameter {
            return KType::ANY;
        }
        // A region's own rigid type would number the enclosing region's variables, not its own.
        let ktype = match shape.callable_type() {
            Static::Closed(callable) => callable.ktype,
            Static::Rigid { value, .. } if !at.opens_region => value.ktype,
            _ => return KType::ANY,
        };
        match self.types.node(ktype) {
            TypeNode::KFunction { params, .. } => params.get(name.symbol()).unwrap_or(KType::ANY),
            _ => KType::ANY,
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
                self.chain[level].binders[member.index()] = match body.form() {
                    Some(_) => callable(body),
                    None => KType::ANY,
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
        let ktype = match shape.callable_type() {
            Static::Closed(callable) => callable.ktype,
            Static::Rigid { value, .. } if !at.opens_region => value.ktype,
            _ => return Ok(()),
        };
        let TypeNode::KFunction { ret, .. } = self.types.node(ktype) else {
            return Ok(());
        };
        let last = shape.body().len() - 1;
        let body = at.statements[last];
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
    ) -> Result<KType, ShapeError<'graph>> {
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
    ) -> Result<KType, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        self.form(level, of_node(shape, node))
    }

    /// The static type of a node or part the evaluator reads as `form`.
    fn form(&mut self, level: usize, form: Form<'graph>) -> Result<KType, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        Ok(match form {
            Form::Leaf(part) => self.leaf(level, part)?,
            Form::Block(nested) => {
                self.push(nested, false);
                let visited = self.visit(level + 1);
                self.chain.pop();
                visited?;
                nested
                    .statement_type(nested.body().len().saturating_sub(1))
                    .unwrap_or(KType::ANY)
            }
            Form::Lambda(node) => Site::of_body(node)
                .and_then(|site| shape.nested(site))
                .map_or(KType::ANY, callable),
            Form::Declaration => KType::NULL,
            Form::Call(node, list) => self.narrow(level, node, list)?,
            Form::Apply(head, argument) => self.apply(level, head, argument)?,
            Form::Unevaluable(_) => KType::ANY,
        })
    }

    /// A leaf part: a literal's type, a name's binder's, a quote's code type, a type value's kind,
    /// or a container's over its parts.
    fn leaf(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Result<KType, ShapeError<'graph>> {
        let shape = self.chain[level].shape;
        let (types, scratch) = (self.types, self.scratch);
        Ok(match part {
            ExpressionPart::Literal(KLiteral::Number(_)) => KType::NUMBER,
            ExpressionPart::Literal(KLiteral::String(_)) => KType::STR,
            ExpressionPart::Literal(KLiteral::Boolean(_)) => KType::BOOL,
            ExpressionPart::Literal(KLiteral::Null) => KType::NULL,
            ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::MarkedName(..) => match shape.mention(Site::of(part)) {
                Some(mention) => self.read(level, mention.coordinate),
                None => KType::ANY,
            },
            ExpressionPart::QuotedExpression(_) => shape
                .nested(Site::of(part))
                .map_or(KType::ANY, |code| code.code_type()),
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                match shape.typed_expression(Site::of(part)) {
                    Static::Closed(handle) => KType::of_kind(handle.kind_of(types)),
                    _ => KType::ANY_TYPE,
                }
            }
            ExpressionPart::ListLiteral(items) => {
                let mut typed = BumpVec::with_capacity_in(items.len(), scratch);
                for item in items.iter() {
                    typed.push(self.part(level, item)?);
                }
                list_type(types, scratch, typed.iter().copied())
            }
            ExpressionPart::DictLiteral(pairs) => {
                let mut typed = BumpVec::with_capacity_in(pairs.len(), scratch);
                for (key, value) in pairs.iter() {
                    typed.push((self.part(level, key)?, self.part(level, value)?));
                }
                dict_type(types, scratch, typed.iter().copied())
            }
            ExpressionPart::RecordLiteral(fields) => {
                let mut typed = BumpVec::with_capacity_in(fields.len(), scratch);
                for (name, value) in fields.iter() {
                    typed.push((*name, self.part(level, value)?));
                }
                record_type(types, scratch, typed.iter().copied())
            }
            ExpressionPart::Keyword(_)
            | ExpressionPart::Expression(_)
            | ExpressionPart::MarkedUse(..) => KType::ANY,
        })
    }

    /// The static type of what `coordinate`, read in the shape at `level`, holds.
    fn read(&self, level: usize, coordinate: Coordinate) -> KType {
        let (hops, target) = match coordinate {
            Coordinate::Builtin(index) => return self.builtins.get(index).ktype(),
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
            _ => return KType::ANY,
        };
        if self.chain[at].opens_region {
            bound_above(self.types, self.scratch, read)
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
    /// callee's declared return read through its bounds when the head is a function, else `Any`.
    fn apply(
        &mut self,
        level: usize,
        head: &'graph ExpressionPart<'graph>,
        argument: &'graph ExpressionPart<'graph>,
    ) -> Result<KType, ShapeError<'graph>> {
        let callee = self.part(level, head)?;
        let payload = self.part(level, argument)?;
        let (types, scratch) = (self.types, self.scratch);
        if let Some(identity) = self.head_handle(level, head) {
            return Ok(match construction(types, scratch, identity, payload) {
                Ok(constructed) => constructed,
                Err(ConstructionRefused::NotConstructible(_)) => KType::ANY,
                // A misfit faults; an unsolved family lies under the bare family.
                Err(_) => identity,
            });
        }
        Ok(match types.node(callee) {
            TypeNode::KFunction { ret, .. } => bound_above(types, scratch, ret),
            _ => KType::ANY,
        })
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

    /// A keyworded use: its arguments typed, its candidates narrowed — and selected, where one is
    /// left that admits them outright — and its static type the join of what is left's returns.
    fn narrow(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        list: &'graph CandidateList<'graph>,
    ) -> Result<KType, ShapeError<'graph>> {
        let (types, scratch) = (self.types, self.scratch);
        let wanted = slots(node, scratch);
        let mut arguments = BumpVec::with_capacity_in(wanted.len(), scratch);
        for wanted in wanted.iter() {
            arguments.push(match wanted {
                Wanted::Label(BinderSymbol::Type(_)) => KType::TYPE_NAME_TOKEN,
                Wanted::Label(_) => KType::IDENTIFIER,
                Wanted::Evaluated(part) => self.part(level, part)?,
            });
        }
        let shape = self.chain[level].shape;
        let index = shape
            .candidate_lists()
            .binary_search_by_key(&Site::of_node(node), |(site, _)| *site)
            .expect("a keyworded use's candidate list is recorded at its node");
        // A use one of whose arguments never arrives never runs its call.
        if arguments.contains(&KType::NEVER) {
            return Ok(KType::NEVER);
        }
        let mut above = BumpVec::with_capacity_in(arguments.len(), scratch);
        above.extend(
            arguments
                .iter()
                .map(|argument| bound_above(types, scratch, *argument)),
        );

        let mut kept = BumpVec::new_in(scratch);
        for candidate in list.candidates {
            let known = self.candidate(level, *candidate);
            let registered = match known {
                Known::Closed(registered) | Known::Rigid(registered) => registered,
                Known::Unknown => {
                    kept.push((*candidate, known));
                    continue;
                }
            };
            let never = shape_slots(registered, types)
                .zip(above.iter())
                .any(|(slot, argument)| {
                    meet(types, scratch, bound_above(types, scratch, slot), *argument)
                        == KType::NEVER
                });
            if !never {
                kept.push((*candidate, known));
            }
        }
        if kept.is_empty() {
            return Err(ShapeError::NoAdmittingCandidate {
                key: list.elements,
                arguments: collect(self.writer, arguments.iter().copied()),
                at: node.source,
            });
        }
        let returned = |registered: KType| {
            let ret = shape_return(registered, types).expect("a registered shape returns");
            bound_above(types, scratch, ret)
        };
        if let [(Candidate::One(coordinate), Known::Closed(registered))] = kept[..]
            && admit_by_class(types, scratch, registered, &above).is_some()
        {
            self.chain[level].narrowings[index] = Narrowing::Selected(coordinate);
            return Ok(returned(registered));
        }
        if kept.len() < list.candidates.len() {
            let candidates = collect(self.writer, kept.iter().map(|(candidate, _)| *candidate));
            self.chain[level].narrowings[index] = Narrowing::Kept(candidates);
        }
        let mut returns = BumpVec::with_capacity_in(kept.len(), scratch);
        for (_, known) in kept.iter() {
            match known {
                Known::Closed(registered) | Known::Rigid(registered) => {
                    returns.push(returned(*registered))
                }
                Known::Unknown => return Ok(KType::ANY),
            }
        }
        Ok(join_iter(types, scratch, returns.iter().copied()))
    }

    /// What the load knows of `candidate`'s registered shape, read from the shape at `level`.
    fn candidate(&self, level: usize, candidate: Candidate) -> Known {
        let coordinate = match candidate {
            Candidate::Spread(_) => return Known::Unknown,
            Candidate::One(Coordinate::Builtin(index)) => {
                return match self
                    .builtins
                    .get(index)
                    .as_callable()
                    .and_then(|member| member.builtin())
                {
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

/// The function type of the callable body `body`, where the load typed it; `Any` elsewhere.
fn callable(body: &BodyShape<'_>) -> KType {
    match body.callable_type() {
        Static::Closed(callable)
        | Static::Rigid {
            value: callable, ..
        } => callable.ktype,
        Static::Unknown => KType::ANY,
    }
}
