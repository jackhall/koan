//! The **shape builder**: one recursive walk that every kind of shape goes through.
//!
//! A body is built in four passes over a draft kept in scratch. The binders pass lays out the
//! declared names and refuses a repeated name or a builtin's. The mention pass walks each statement
//! from its root, carrying the class a mention met there would take, resolving each mention as it
//! is met and building each nested body or arm as a draft of its own on top of the chain of
//! enclosing drafts. The components pass condenses the bindings' reference graph, refuses a
//! component with an eager internal mention, turns a nested callable's capture of a fellow member
//! into a knot edge, and seals the nested drafts into program storage. The units pass orders the
//! body's units — its components and the statements that bind nothing — so each follows every unit
//! it reads and independent ones come out as written; the runner performs them in that order.
//!
//! A callable body records the form node holding it, a binder whose right-hand side is a callable
//! form at its root — or a combined form that is one, or a `MODULE`/`GROUP` binder — records the
//! body it births, and a `LET` value binder records its right-hand side, so a component's tie reads
//! each member's signature, body or data off the shape.
//!
//! See [README.md § Visibility](../README.md#visibility).

use crate::memory::{
    BumpAllocator, BumpBackedMap, BumpVec, ProgramBrand, bump_table, collect, resident,
    strongly_connected_components,
};
use crate::parse::builtin_shapes::binder::bounded;
use crate::parse::builtin_shapes::{BuiltinShape, BuiltinShapeId, KEYWORDS};
use crate::parse::{ExpressionPart, KExpression};
use crate::symbols::{BinderSymbol, StaticName, TypeSymbol, ValueSymbol};
use crate::values::{Knotted, KnottedFamily};

use crate::type_lattice::DeclaredGroup;

use super::super::activation::ActivationView;
use super::super::builtins::Builtins;
use super::super::channels::Channels;
use super::super::groups::{self, Claim, Claims, GroupFrame};
use super::super::signature::{
    body_of, declare_parameters, declare_quantifiers, pair_name, quantifier_bounds, signature_run,
};
use super::{
    BodyShape, BuiltinIndex, CaptureSlot, CaptureSource, CaptureSpec, Component, ComponentIndex,
    Coordinate, Mention, MentionClass, Position, ShapeError, ShapeKind, Site, Slot, Target, Unit,
    UnitWork, resolve_here,
};
use crate::parse::builtin_shapes::role::{BodyKind, DefinitionKind, Heads, Role};

mod rewrite;
mod surface;

use surface::Surfaced;

/// The names a body declares without a binder statement.
struct ImplicitNames {
    /// An arm's matched value.
    it: StaticName<ValueSymbol>,
    /// A binary operator's operands.
    left: StaticName<ValueSymbol>,
    right: StaticName<ValueSymbol>,
    /// A unary operator's operand run.
    operands: StaticName<ValueSymbol>,
}

static IMPLICIT: ImplicitNames = ImplicitNames {
    it: crate::static_name!(ValueSymbol, "it"),
    left: crate::static_name!(ValueSymbol, "left"),
    right: crate::static_name!(ValueSymbol, "right"),
    operands: crate::static_name!(ValueSymbol, "operands"),
};

/// The shape of a program's top-level statements.
pub(super) fn program<'graph, X: Knotted>(
    brand: ProgramBrand<'graph>,
    statements: &[KExpression<'graph>],
    builtins: &Builtins<'_, '_, X>,
    scratch: BumpAllocator<'_>,
) -> Result<&'graph BodyShape<'graph>, ShapeError> {
    let lookup = |name| builtins.lookup(name);
    // Every claim the program makes over an operator symbol is collected before the first draft, so
    // how a symbol chains never depends on where its declarations sit.
    let claims = groups::claims(brand, scratch, statements.iter(), None)?;
    let frame = resident(brand.writer(), GroupFrame::new(&[], None, claims));
    let mut builder = Builder::new(brand, scratch, &lookup, None, claims, frame);
    let statements = statements
        .iter()
        .enumerate()
        .map(|(index, statement)| (statement, index + 1));
    let draft = builder.draft(
        ShapeKind::Program,
        Position::PARAMETER,
        &[],
        &[],
        statements,
    )?;
    Ok(builder.seal(draft, None))
}

/// An `EVAL` body's block shape over `site`'s chain, reading at `at`.
pub(super) fn eval<'graph, XF: KnottedFamily<'graph>>(
    brand: ProgramBrand<'graph>,
    body: &KExpression<'graph>,
    site: &ActivationView<'graph, '_, XF>,
    at: Position,
    scratch: BumpAllocator<'_>,
) -> Result<&'graph BodyShape<'graph>, ShapeError> {
    let lookup = |name| site.builtins().lookup(name);
    let outer = |name, position| site.through_chain(name, position);
    // Evaluated code is held to the program's declarations, and its operator runs chain under the
    // groups the shapes enclosing the `EVAL` hold — found by this walk, which reads no activation
    // slot.
    let enclosing = site.shape().group_frame();
    let claims = groups::claims(
        brand,
        scratch,
        body.body_statements().map(|(statement, _)| statement),
        Some(enclosing.claims()),
    )?;
    let frame = resident(
        brand.writer(),
        GroupFrame::new(&[], Some(enclosing), claims),
    );
    let mut builder = Builder::new(brand, scratch, &lookup, Some((&outer, at)), claims, frame);
    let draft = builder.draft(ShapeKind::Block, at, &[], &[], body.body_statements())?;
    Ok(builder.seal(draft, None))
}

/// One unit's wait on another, by unit: the unit waited on, the waiter, and for an `EVAL`'s wait
/// the slot it waits through and the `EVAL`'s statement.
type Wait = (u32, u32, Option<(Slot, u32)>);

/// Why a body's waits cycle, from the waits no emitted unit released: walk back from an unemitted
/// unit through waits on unemitted ones until on a cycle, and name an `EVAL`'s wait on it — the
/// slot it waits through and its statement. One exists, since the reads alone are acyclic.
fn eval_cycle(waits: &[Wait], emitted: &[bool]) -> (Slot, u32) {
    let outstanding = |waiter: u32| {
        waits
            .iter()
            .find(|(bound, at, _)| *at == waiter && !emitted[*bound as usize])
            .expect("an unemitted unit waits on an unemitted one")
    };
    let start = emitted
        .iter()
        .position(|emitted| !emitted)
        .expect("a stalled order has an unemitted unit") as u32;
    // Walk far enough back to be on the cycle, then once around it.
    let mut unit = start;
    for _ in 0..emitted.len() {
        unit = outstanding(unit).0;
    }
    let on_cycle = unit;
    loop {
        let (bound, _, through) = *outstanding(unit);
        if let Some(through) = through {
            return through;
        }
        unit = bound;
        assert_ne!(unit, on_cycle, "the reads alone are acyclic");
    }
}

/// The class a mention met in this context takes, before the context's own role applies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// A binding's right-hand side, before any context: a mention here is the root and eager.
    Root,
    /// Under constructor slots and at most one callable-body boundary.
    Deferred,
    /// Under any other context.
    Eager,
}

impl State {
    /// Entering a constructor slot: a list element, a dict value, a record field, a definition, a
    /// nominal construction's payload.
    fn constructor(self) -> State {
        match self {
            State::Root | State::Deferred => State::Deferred,
            State::Eager => State::Eager,
        }
    }

    fn class(self) -> MentionClass {
        match self {
            State::Deferred => MentionClass::Deferred,
            State::Root | State::Eager => MentionClass::Eager,
        }
    }
}

/// The reading end of a resolution: the level and statement a mention sits in, and its class.
#[derive(Clone, Copy)]
struct Reader {
    level: usize,
    statement: u32,
    class: MentionClass,
}

type Lookup<'e> = &'e dyn Fn(BinderSymbol) -> Option<BuiltinIndex>;
type Outer<'e> = &'e dyn Fn(BinderSymbol, Position) -> Option<Coordinate>;

/// One body under construction, in scratch.
struct Draft<'graph, 'x> {
    kind: ShapeKind,
    entered_at: Position,
    /// The statement of the enclosing draft this body sits in.
    parent_statement: u32,
    statements: u32,
    values: BumpVec<'x, (ValueSymbol, Position)>,
    types: BumpVec<'x, (TypeSymbol, Position)>,
    /// The slot each statement binds, if it binds one.
    statement_binder: BumpVec<'x, Option<Slot>>,
    /// This body's statement spines, **rewritten**, copied into scratch so the surfaced-name reader
    /// can reach a binder's own statement. Each spine's parts run stays where it is in program
    /// storage.
    nodes: BumpVec<'x, KExpression<'graph>>,
    /// The group frame this body was built under, and the groups it holds itself.
    frame: &'graph GroupFrame<'graph>,
    held: &'graph [&'graph DeclaredGroup<'graph>],
    mentions: BumpVec<'x, Mention>,
    captures: BumpVec<'x, CaptureSpec>,
    /// `(binder, bound, class)`: the binder's statement reads the bound slot.
    edges: BumpVec<'x, (Slot, Slot, MentionClass)>,
    /// `(statement, bound)`: the statement reads the bound slot, whether or not it binds — what a
    /// unit waits on.
    reads: BumpVec<'x, (u32, Slot)>,
    /// Each statement that holds an `EVAL`, whose free names no shape can enumerate.
    eval_statements: BumpVec<'x, u32>,
    /// The body's units in the order they are performed, once the components pass has run.
    units: BumpVec<'x, Unit>,
    /// Finished nested drafts, waiting on this draft's components to settle their captures.
    children: BumpVec<'x, (Site, Draft<'graph, 'x>)>,
    /// `(binder, body)`: the binder's right-hand side births the callable whose body sits at `body`.
    births: BumpVec<'x, (Slot, Site)>,
    /// `(binder, rhs)`: a `LET` value binder's right-hand side part sits at `rhs`.
    rhs: BumpVec<'x, (Slot, Site)>,
    /// `(binder, node)`: a type binder's whole declaration node, in program storage.
    declarations: BumpVec<'x, (Slot, &'graph KExpression<'graph>)>,
    component_of: BumpVec<'x, ComponentIndex>,
    /// Every component's members, one run after another, each run sorted.
    members: BumpVec<'x, Slot>,
    /// Each component's run in `members`, whether every mention between its members is deferred,
    /// and whether it is cyclic.
    components: BumpVec<'x, DraftComponent>,
    keeps_defining_scope: bool,
    /// The statement being walked when a nested draft was entered, and the class that path takes
    /// at this level.
    current: (u32, MentionClass),
}

/// A component under construction: its run in [`Draft::members`].
#[derive(Clone, Copy)]
struct DraftComponent {
    start: u32,
    len: u32,
    deferred_only: bool,
    cyclic: bool,
}

impl DraftComponent {
    /// This component's run within every component's `members`.
    fn run(self, members: &[Slot]) -> &[Slot] {
        let start = self.start as usize;
        &members[start..start + self.len as usize]
    }
}

impl Draft<'_, '_> {
    fn members_of(&self, component: ComponentIndex) -> &[Slot] {
        self.components[component.index()].run(&self.members)
    }

    fn end(&self) -> Position {
        Position(self.statements + 1)
    }

    /// Whether `slot` is in the type channel, read through the one owner of the index split.
    fn is_type_slot(&self, slot: Slot) -> bool {
        matches!(self.channels().name(slot.index()), BinderSymbol::Type(_))
    }

    /// The declared names, once the binders pass has sorted them.
    fn channels(&self) -> Channels<'_, Position> {
        Channels::new(&self.values, &self.types)
    }

    /// Where the path into the nested draft being walked reads at this level.
    fn boundary(&self) -> Position {
        match self.current.1 {
            MentionClass::Deferred => self.end(),
            MentionClass::Eager => Position::statement(self.current.0 as usize),
        }
    }
}

struct Builder<'graph, 'x, 'e> {
    brand: ProgramBrand<'graph>,
    scratch: BumpAllocator<'x>,
    builtins: Lookup<'e>,
    /// For an `EVAL` body: the by-name resolver over the site's chain, and `EVAL`'s position.
    outer: Option<(Outer<'e>, Position)>,
    /// Every claim the code being built makes over an operator symbol, collected once up front.
    claims: &'graph Claims<'graph>,
    /// The frame of the draft being built: which groups an operator run written there sees.
    frame: &'graph GroupFrame<'graph>,
    /// The address of every parts run the pairwise rewrite synthesized as a statement block, so
    /// the mention pass enters it as a block rather than walking it as a call.
    blocks: BumpBackedMap<'x, usize, ()>,
    chain: BumpVec<'x, Draft<'graph, 'x>>,
    /// Each callable body's site beside the form node holding it, in program storage.
    forms: BumpBackedMap<'x, Site, &'graph KExpression<'graph>>,
    /// Each recorded right-hand side's site beside the part, in program storage.
    parts: BumpBackedMap<'x, Site, &'graph ExpressionPart<'graph>>,
    /// Type parameters of the forms enclosing the current walk path: never mentions.
    skip: BumpVec<'x, TypeSymbol>,
    /// Where the current draft's own entries in `skip` start; a nested body declares its enclosing
    /// form's type parameters as parameters and reads them as mentions.
    skip_floor: usize,
}

impl<'graph, 'x, 'e> Builder<'graph, 'x, 'e> {
    fn new(
        brand: ProgramBrand<'graph>,
        scratch: BumpAllocator<'x>,
        builtins: Lookup<'e>,
        outer: Option<(Outer<'e>, Position)>,
        claims: &'graph Claims<'graph>,
        frame: &'graph GroupFrame<'graph>,
    ) -> Self {
        Builder {
            brand,
            scratch,
            builtins,
            outer,
            claims,
            frame,
            blocks: bump_table(scratch),
            chain: BumpVec::new_in(scratch),
            forms: bump_table(scratch),
            parts: bump_table(scratch),
            skip: BumpVec::new_in(scratch),
            skip_floor: 0,
        }
    }

    /// Build one body: its binders, its mentions (and every nested body, depth first), and its
    /// components. The draft comes back unsealed, since whether a capture is a knot edge is
    /// settled by the enclosing draft's components.
    fn draft<'n>(
        &mut self,
        kind: ShapeKind,
        entered_at: Position,
        parameters: &[BinderSymbol],
        held: &[&'graph DeclaredGroup<'graph>],
        statements: impl Iterator<Item = (&'n KExpression<'graph>, usize)>,
    ) -> Result<Draft<'graph, 'x>, ShapeError>
    where
        'graph: 'n,
    {
        // A body holding a group is built under a frame of its own, which every operator run inside
        // it — and every body nested in it — chains against.
        let writer = self.brand.writer();
        let held: &'graph [&'graph DeclaredGroup<'graph>] = collect(writer, held.iter().copied());
        let outer = self.frame;
        if !held.is_empty() {
            self.frame = resident(writer, GroupFrame::new(held, Some(outer), self.claims));
        }
        let built = self.body(kind, entered_at, parameters, held, statements);
        self.frame = outer;
        built
    }

    /// [`draft`](Self::draft) with this body's frame already standing: rewrite each statement's
    /// operator runs, lay out the binders over the rewritten statements, walk them, and condense.
    fn body<'n>(
        &mut self,
        kind: ShapeKind,
        entered_at: Position,
        parameters: &[BinderSymbol],
        held: &'graph [&'graph DeclaredGroup<'graph>],
        statements: impl Iterator<Item = (&'n KExpression<'graph>, usize)>,
    ) -> Result<Draft<'graph, 'x>, ShapeError>
    where
        'graph: 'n,
    {
        let writer = self.brand.writer();
        let mut nodes: BumpVec<'x, &'n KExpression<'graph>> = BumpVec::new_in(self.scratch);
        nodes.extend(statements.map(|(node, _)| node));
        for index in 0..nodes.len() {
            if let Some(rewritten) = self.rewrite_statement(index, nodes[index])? {
                nodes[index] = resident(writer, rewritten);
            }
        }
        let parent_statement = self
            .chain
            .last()
            .map_or(u32::MAX, |parent| parent.current.0);
        let mut draft = self.binders(kind, entered_at, parent_statement, parameters, &nodes)?;
        draft.frame = self.frame;
        draft.held = held;
        draft.nodes.extend(nodes.iter().map(|node| **node));
        self.chain.push(draft);
        let level = self.chain.len() - 1;
        for (statement, node) in nodes.iter().enumerate() {
            if let Err(error) = self.walk_node(level, statement as u32, node, State::Root) {
                self.chain.pop();
                return Err(error);
            }
        }
        let mut draft = self.chain.pop().expect("this draft was pushed above");
        self.components(&mut draft)?;
        self.units(&mut draft)?;
        Ok(draft)
    }

    /// The binders pass: every parameter at `0` and every statement's binder at its position, a
    /// repeated name and a builtin's name refused in that order.
    fn binders(
        &self,
        kind: ShapeKind,
        entered_at: Position,
        parent_statement: u32,
        parameters: &[BinderSymbol],
        nodes: &[&KExpression<'graph>],
    ) -> Result<Draft<'graph, 'x>, ShapeError> {
        let scratch = self.scratch;
        let declared = parameters
            .iter()
            .map(|name| (Some(*name), Position::PARAMETER))
            .chain(nodes.iter().enumerate().map(|(index, node)| {
                let name = node.statement_binder_plan().and_then(|plan| plan.name);
                (name, Position::statement(index))
            }));
        let mut seen: BumpBackedMap<BinderSymbol, Position> = bump_table(scratch);
        let mut values = BumpVec::new_in(scratch);
        let mut types = BumpVec::new_in(scratch);
        for (name, position) in declared {
            let Some(name) = name else { continue };
            if let Some(first) = seen.insert(name, position) {
                return Err(ShapeError::Rebind {
                    name,
                    first,
                    second: position,
                });
            }
            if (self.builtins)(name).is_some() {
                return Err(ShapeError::ShadowsBuiltin { name, at: position });
            }
            match name {
                BinderSymbol::Value(name) => values.push((name, position)),
                BinderSymbol::Type(name) => types.push((name, position)),
            }
        }
        values.sort_unstable_by_key(|(name, _)| *name);
        types.sort_unstable_by_key(|(name, _)| *name);
        let mut statement_binder = BumpVec::with_capacity_in(nodes.len(), scratch);
        statement_binder.resize(nodes.len(), None);
        let entries = values
            .iter()
            .map(|(_, position)| *position)
            .chain(types.iter().map(|(_, position)| *position));
        for (slot, position) in entries.enumerate() {
            // A parameter writes at `0` and binds no statement.
            if let Some(statement) = position.0.checked_sub(1) {
                statement_binder[statement as usize] = Some(Slot(slot as u32));
            }
        }
        Ok(Draft {
            kind,
            entered_at,
            parent_statement,
            statements: nodes.len() as u32,
            values,
            types,
            statement_binder,
            mentions: BumpVec::new_in(scratch),
            captures: BumpVec::new_in(scratch),
            edges: BumpVec::new_in(scratch),
            reads: BumpVec::new_in(scratch),
            eval_statements: BumpVec::new_in(scratch),
            units: BumpVec::new_in(scratch),
            children: BumpVec::new_in(scratch),
            births: BumpVec::new_in(scratch),
            rhs: BumpVec::new_in(scratch),
            declarations: BumpVec::new_in(scratch),
            nodes: BumpVec::new_in(scratch),
            frame: self.frame,
            held: &[],
            component_of: BumpVec::new_in(scratch),
            members: BumpVec::new_in(scratch),
            components: BumpVec::new_in(scratch),
            keeps_defining_scope: false,
            current: (0, MentionClass::Eager),
        })
    }

    /// Walk a node's parts under `state`, by its form's roles. A formless node is a call, a grouping
    /// or a construction: a single-part group is transparent; a two-part node headed by a type name
    /// is a nominal construction, its head eager and its payload a constructor slot — which also
    /// classifies a type-constructor application written in value position; every part of a call
    /// is eager.
    fn walk_node(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        state: State,
    ) -> Result<(), ShapeError> {
        let Some(form) = node.cache().builtin_shape() else {
            if let [only] = node.parts {
                return self.walk_part(level, statement, &only.value, state);
            }
            if let [head, payload] = node.parts
                && let ExpressionPart::Type(name) = &head.value
                && !self.skips(name)
            {
                self.walk_part(level, statement, &head.value, State::Eager)?;
                return self.walk_part(level, statement, &payload.value, state.constructor());
            }
            for part in node.parts {
                self.walk_part(level, statement, &part.value, State::Eager)?;
            }
            return Ok(());
        };
        if !form.supported() {
            return Err(ShapeError::Unsupported {
                form: form.id,
                at: Position::statement(statement as usize),
            });
        }
        debug_assert_eq!(
            form.elements.len(),
            node.parts.len(),
            "a builtin shape's parts match its run"
        );
        // A binary operator declaring a result type of its own is admitted only where its symbol
        // chains pairwise: a fold hands its own result back as the next operand.
        if matches!(
            form.id,
            BuiltinShapeId::OperatorDefinitionReturning | BuiltinShapeId::CombinedOperatorReturning
        ) && let Ok(symbol) = groups::declaration_symbol(node)
            && !self.frame.pairwise(symbol)
        {
            return Err(ShapeError::ResultOutsidePairwise {
                symbol,
                at: Position::statement(statement as usize),
            });
        }
        if form.id == BuiltinShapeId::Eval {
            for (at, draft) in self.chain.iter_mut().enumerate() {
                draft.keeps_defining_scope = true;
                let walked = if at == level {
                    statement
                } else {
                    draft.current.0
                };
                draft.eval_statements.push(walked);
            }
        }
        // A type binder records its whole declaration node: the door reads which declaration it
        // is, and where its declared part sits, off the node's own builtin shape. The statement's
        // own spine is reached first, so a declarator nested under it records nothing.
        let draft = &mut self.chain[level];
        if state == State::Root
            && let Some(binder) = draft.statement_binder[statement as usize]
            && draft.is_type_slot(binder)
            && !draft.declarations.iter().any(|(held, _)| *held == binder)
        {
            draft
                .declarations
                .push((binder, resident(self.brand.writer(), *node)));
        }

        // A callable's parameters are declared before any part is read, so a type parameter a
        // signature names is never taken for a mention of the enclosing shape.
        let declares = form
            .roles()
            .any(|role| matches!(role, Role::Signature | Role::Quantifiers));
        let mut parameters = BumpVec::new_in(self.scratch);
        if declares {
            for (role, part) in form.roles().zip(node.parts) {
                match role {
                    Role::Signature => declare_parameters(&part.value, &mut parameters),
                    Role::Quantifiers => declare_quantifiers(&part.value, &mut parameters),
                    _ => {}
                }
            }
        }
        let mark = self.skip.len();
        self.skip
            .extend(parameters.iter().filter_map(|name| match name {
                BinderSymbol::Type(name) => Some(*name),
                BinderSymbol::Value(_) => None,
            }));
        let walked = self.walk_parts(level, statement, node, form, &parameters, state);
        self.skip.truncate(mark);
        walked
    }

    /// Walk each of a form's parts by its role, with the form's type parameters on the skip stack.
    #[allow(clippy::too_many_arguments)]
    fn walk_parts(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        form: &'static BuiltinShape,
        parameters: &[BinderSymbol],
        state: State,
    ) -> Result<(), ShapeError> {
        for (role, part) in form.roles().zip(node.parts) {
            let part = &part.value;
            match role {
                Role::Keyword | Role::Name | Role::Data | Role::Label => {}
                // A group's names are the body's; its bounds are read where the form runs, as the
                // signature's types are. The group's own names are already skipped, so a bound
                // naming one records nothing and the elaborator refuses it.
                Role::Quantifiers => {
                    for bound in quantifier_bounds(part) {
                        self.walk_part(level, statement, bound, State::Eager)?;
                    }
                }
                Role::Rhs => {
                    let draft = &mut self.chain[level];
                    if state == State::Root
                        && let Some(binder) = draft.statement_binder[statement as usize]
                    {
                        draft.rhs.push((binder, Site::of(part)));
                        self.parts.insert(Site::of(part), part);
                    }
                    self.walk_part(level, statement, part, state)?
                }
                Role::Argument | Role::TypeExpression => {
                    self.walk_part(level, statement, part, State::Eager)?
                }
                Role::Signature => self.walk_signature(level, statement, part)?,
                Role::Body(kind) => {
                    self.enter_body(level, statement, node, part, kind, parameters, state)?
                }
                Role::Branches(heads) => self.enter_arms(level, statement, form.id, part, heads)?,
                Role::Definition(kind) => {
                    self.walk_definition(level, statement, part, kind, state.constructor())?
                }
                Role::Unsupported => unreachable!("an unsupported form returned above"),
            }
        }
        Ok(())
    }

    fn walk_part(
        &mut self,
        level: usize,
        statement: u32,
        part: &ExpressionPart<'graph>,
        state: State,
    ) -> Result<(), ShapeError> {
        match part {
            ExpressionPart::Identifier(name) => {
                self.mention(level, statement, part, BinderSymbol::Value(*name), state)
            }
            ExpressionPart::Type(name) if !self.skips(name) => {
                self.mention(level, statement, part, BinderSymbol::Type(*name), state)
            }
            ExpressionPart::Expression(node) => {
                // A block the pairwise rewrite synthesized runs as a block: its hoists are its
                // binders, and its value is its last statement's.
                if self
                    .blocks
                    .contains_key(&(node.reference().parts.as_ptr() as usize))
                {
                    return self.enter_child(
                        level,
                        statement,
                        MentionClass::Eager,
                        Site::of(part),
                        ShapeKind::Block,
                        &[],
                        &[],
                        node.reference().body_statements(),
                    );
                }
                self.walk_node(level, statement, node.reference(), state)
            }
            ExpressionPart::SigiledTypeExpr(node) => {
                self.walk_node(level, statement, node.reference(), State::Eager)
            }
            ExpressionPart::RecordType(node) => {
                self.walk_fields(level, statement, node.reference(), State::Eager)
            }
            ExpressionPart::ListLiteral(items) => {
                for item in items.iter() {
                    self.walk_part(level, statement, item, state.constructor())?;
                }
                Ok(())
            }
            ExpressionPart::DictLiteral(pairs) => {
                for (key, value) in pairs.iter() {
                    self.walk_part(level, statement, key, State::Eager)?;
                    self.walk_part(level, statement, value, state.constructor())?;
                }
                Ok(())
            }
            ExpressionPart::RecordLiteral(pairs) => {
                for (_, value) in pairs.iter() {
                    self.walk_part(level, statement, value, state.constructor())?;
                }
                Ok(())
            }
            ExpressionPart::Type(_)
            | ExpressionPart::Keyword(_)
            | ExpressionPart::Literal(_)
            | ExpressionPart::QuotedExpression(_) => Ok(()),
        }
    }

    /// A field list or a signature run: each pair's name is a label, its type is read under
    /// `state`.
    fn walk_fields(
        &mut self,
        level: usize,
        statement: u32,
        run: &KExpression<'graph>,
        state: State,
    ) -> Result<(), ShapeError> {
        let mut index = 0;
        while index < run.parts.len() {
            if pair_name(run, index).is_some() {
                self.walk_part(level, statement, &run.parts[index + 1].value, state)?;
                index += 2;
            } else {
                self.walk_part(level, statement, &run.parts[index].value, state)?;
                index += 1;
            }
        }
        Ok(())
    }

    fn walk_signature(
        &mut self,
        level: usize,
        statement: u32,
        part: &ExpressionPart<'graph>,
    ) -> Result<(), ShapeError> {
        match signature_run(part) {
            Some(run) => self.walk_fields(level, statement, run, State::Eager),
            None => self.walk_part(level, statement, part, State::Eager),
        }
    }

    /// A type declaration's definition, under the constructor state: symbols and every name the
    /// definition itself declares are not mentions, and every other type name is.
    fn walk_definition(
        &mut self,
        level: usize,
        statement: u32,
        part: &ExpressionPart<'graph>,
        kind: DefinitionKind,
        state: State,
    ) -> Result<(), ShapeError> {
        // A definition declares its own names — a `SIG` body's abstract `TYPE` members and its
        // manifest `LET` members alike — so a later statement naming one is no mention of the
        // enclosing shape. The door resolves them against the definition it is elaborating.
        let mut own = BumpVec::new_in(self.scratch);
        if let ExpressionPart::Expression(run) = part {
            own.extend(run.body_statements().filter_map(|(node, _)| {
                match node.statement_binder_plan()?.name? {
                    BinderSymbol::Type(name) => Some(name),
                    BinderSymbol::Value(_) => None,
                }
            }));
        }
        let mark = self.skip.len();
        self.skip.extend_from_slice(&own);
        let walked = self.walk_definition_part(level, statement, part, kind, state);
        self.skip.truncate(mark);
        walked
    }

    /// One statement of a definition, by its own builtin shape's roles — the same authority the
    /// top-level walk reads a form's parts through. Its name, symbols, quoted data and `FOR ALL`
    /// names are the statement's own; everything else is read under the definition's state, so a
    /// `SIG` body's `VAL` type is as deferred as the definition holding it.
    fn walk_definition_statement(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        form: &'static BuiltinShape,
        state: State,
    ) -> Result<(), ShapeError> {
        if !form.supported() {
            return Err(ShapeError::Unsupported {
                form: form.id,
                at: Position::statement(statement as usize),
            });
        }
        let mut parameters = BumpVec::new_in(self.scratch);
        for (role, part) in form.roles().zip(node.parts) {
            match role {
                Role::Signature => declare_parameters(&part.value, &mut parameters),
                Role::Quantifiers => declare_quantifiers(&part.value, &mut parameters),
                _ => {}
            }
        }
        let mark = self.skip.len();
        self.skip
            .extend(parameters.iter().filter_map(|name| match name {
                BinderSymbol::Type(name) => Some(*name),
                BinderSymbol::Value(_) => None,
            }));
        let walked = self.walk_definition_roles(level, statement, node, form, state);
        self.skip.truncate(mark);
        walked
    }

    fn walk_definition_roles(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        form: &'static BuiltinShape,
        state: State,
    ) -> Result<(), ShapeError> {
        for (role, part) in form.roles().zip(node.parts) {
            let part = &part.value;
            match role {
                Role::Keyword | Role::Data | Role::Label => {}
                Role::Quantifiers => {
                    for bound in quantifier_bounds(part) {
                        self.walk_definition_part(
                            level,
                            statement,
                            bound,
                            DefinitionKind::Plain,
                            state,
                        )?;
                    }
                }
                // A `TYPE` member's bound is read where the `SIG` runs; its name is the body's.
                Role::Name => {
                    if form.id == BuiltinShapeId::TypeDeclaration
                        && let Some((_, bound)) = bounded(part)
                    {
                        self.walk_definition_part(
                            level,
                            statement,
                            bound,
                            DefinitionKind::Plain,
                            state,
                        )?;
                    }
                }
                Role::Definition(inner) => {
                    self.walk_definition(level, statement, part, inner, state)?
                }
                Role::Body(_) | Role::Branches(_) | Role::Unsupported => {
                    return Err(ShapeError::Unsupported {
                        form: form.id,
                        at: Position::statement(statement as usize),
                    });
                }
                Role::Rhs | Role::Argument | Role::TypeExpression | Role::Signature => {
                    self.walk_definition_part(level, statement, part, DefinitionKind::Plain, state)?
                }
            }
        }
        Ok(())
    }

    fn walk_definition_part(
        &mut self,
        level: usize,
        statement: u32,
        part: &ExpressionPart<'graph>,
        kind: DefinitionKind,
        state: State,
    ) -> Result<(), ShapeError> {
        let run = match part {
            ExpressionPart::Type(name) if !self.skips(name) => {
                return self.mention(level, statement, part, BinderSymbol::Type(*name), state);
            }
            // A statement of the definition, and a type expression written inside one, are nodes
            // with builtin shapes of their own: read their parts by their roles rather than
            // descending into them blind.
            ExpressionPart::Expression(run) | ExpressionPart::SigiledTypeExpr(run)
                if let Some(form) = run.statement_spine().cache().builtin_shape() =>
            {
                return self.walk_definition_statement(
                    level,
                    statement,
                    run.statement_spine(),
                    form,
                    state,
                );
            }
            ExpressionPart::Expression(run)
            | ExpressionPart::SigiledTypeExpr(run)
            | ExpressionPart::RecordType(run) => run.reference(),
            ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_) => {
                return self.walk_part(level, statement, part, state);
            }
            ExpressionPart::Type(_)
            | ExpressionPart::Identifier(_)
            | ExpressionPart::Keyword(_)
            | ExpressionPart::Literal(_)
            | ExpressionPart::QuotedExpression(_) => return Ok(()),
        };
        for (index, inner) in run.parts.iter().enumerate() {
            let tag = kind == DefinitionKind::Union && index % 2 == 0;
            if tag && matches!(inner.value, ExpressionPart::Type(_)) {
                continue;
            }
            self.walk_definition_part(
                level,
                statement,
                &inner.value,
                DefinitionKind::Plain,
                state,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn enter_body(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        part: &ExpressionPart<'graph>,
        kind: BodyKind,
        signature: &[BinderSymbol],
        state: State,
    ) -> Result<(), ShapeError> {
        let Some(body) = body_of(part) else {
            return Err(ShapeError::Malformed {
                form: node
                    .cache()
                    .builtin_shape()
                    .expect("a body role is a form's")
                    .id,
                at: Position::statement(statement as usize),
            });
        };
        // A module body has no callable type, so it records no form; it is still what its binder
        // births. A `USING` body is a block of the enclosing shape and births nothing.
        let site = Site::of(part);
        if kind != BodyKind::Surfaced {
            if kind != BodyKind::Module {
                self.forms
                    .insert(site, resident(self.brand.writer(), *node));
            }
            let draft = &mut self.chain[level];
            if state == State::Root
                && let Some(binder) = draft.statement_binder[statement as usize]
            {
                draft.births.push((binder, site));
            }
        }
        let (shape_kind, class) = match kind {
            BodyKind::Lambda | BodyKind::Operator | BodyKind::UnaryOperator => {
                (ShapeKind::Callable, state.constructor().class())
            }
            BodyKind::Module => (ShapeKind::Module, MentionClass::Eager),
            BodyKind::Surfaced => (ShapeKind::Block, MentionClass::Eager),
        };
        let operator = [
            BinderSymbol::Value(IMPLICIT.left.symbol()),
            BinderSymbol::Value(IMPLICIT.right.symbol()),
        ];
        let unary = [BinderSymbol::Value(IMPLICIT.operands.symbol())];
        let at = Position::statement(statement as usize);
        let mut surfaced = Surfaced::new(self.scratch);
        let kept;
        let mut held: &[&'graph DeclaredGroup<'graph>] = &[];
        if kind == BodyKind::Surfaced {
            self.surfaced(level, statement, node, &mut surfaced)?;
            kept = self.surfaced_groups(at, &surfaced)?;
            held = &kept;
        }
        // A `GROUP`'s own body holds its group — the one record every equal declaration shares —
        // so an operator run of its members may be written inside it and nowhere else.
        let own;
        if kind == BodyKind::Module
            && let Some(declared) =
                groups::declared_group(node, self.scratch).map_err(|()| ShapeError::Malformed {
                    form: node
                        .cache()
                        .builtin_shape()
                        .expect("a body role is a form's")
                        .id,
                    at,
                })?
            && let Some(Claim::Group(record)) = self.claims.get(declared.members[0])
        {
            for member in record.members {
                if let Some(visible) = self.frame.visible(*member)
                    && !groups::groups_equal(visible, record)
                {
                    return Err(ShapeError::RedeclaresGroup {
                        symbol: *member,
                        at,
                    });
                }
            }
            own = [record];
            held = &own;
        }
        let parameters: &[BinderSymbol] = match kind {
            BodyKind::Lambda => signature,
            BodyKind::Operator => &operator,
            BodyKind::UnaryOperator => &unary,
            BodyKind::Module => &[],
            BodyKind::Surfaced => &surfaced.names,
        };
        self.enter_child(
            level,
            statement,
            class,
            Site::of(part),
            shape_kind,
            parameters,
            held,
            body.body_statements(),
        )
    }

    fn enter_arms(
        &mut self,
        level: usize,
        statement: u32,
        form: BuiltinShapeId,
        part: &ExpressionPart<'graph>,
        heads: Heads,
    ) -> Result<(), ShapeError> {
        let malformed = ShapeError::Malformed {
            form,
            at: Position::statement(statement as usize),
        };
        let Some(branches) = body_of(part) else {
            return Err(malformed);
        };
        let parts = branches.parts;
        let arrow = KEYWORDS.arrow.symbol();
        if !parts.len().is_multiple_of(3) {
            return Err(malformed);
        }
        for arm in parts.chunks_exact(3) {
            let (head, separator, body_part) = (&arm[0].value, &arm[1].value, &arm[2].value);
            let (ExpressionPart::Keyword(symbol), Some(body)) = (separator, body_of(body_part))
            else {
                return Err(malformed);
            };
            if *symbol != arrow {
                return Err(malformed);
            }
            if heads == Heads::Types {
                self.walk_part(level, statement, head, State::Eager)?;
            }
            let it = [BinderSymbol::Value(IMPLICIT.it.symbol())];
            self.enter_child(
                level,
                statement,
                MentionClass::Eager,
                Site::of(body_part),
                ShapeKind::Block,
                &it,
                &[],
                body.body_statements(),
            )?;
        }
        Ok(())
    }

    /// Build a nested draft under the statement being walked at `level`, entered with `class`, and
    /// hold it under `site` until this draft's components settle its captures.
    #[allow(clippy::too_many_arguments)]
    fn enter_child<'n>(
        &mut self,
        level: usize,
        statement: u32,
        class: MentionClass,
        site: Site,
        kind: ShapeKind,
        parameters: &[BinderSymbol],
        held: &[&'graph DeclaredGroup<'graph>],
        statements: impl Iterator<Item = (&'n KExpression<'graph>, usize)>,
    ) -> Result<(), ShapeError>
    where
        'graph: 'n,
    {
        let parent = &mut self.chain[level];
        parent.current = (statement, class);
        let entered_at = parent.boundary();
        let floor = std::mem::replace(&mut self.skip_floor, self.skip.len());
        let child = self.draft(kind, entered_at, parameters, held, statements);
        self.skip_floor = floor;
        self.chain[level].children.push((site, child?));
        Ok(())
    }

    /// Whether `name` is a type parameter of a form enclosing the walk within the current draft.
    fn skips(&self, name: &TypeSymbol) -> bool {
        self.skip[self.skip_floor..].contains(name)
    }

    /// Resolve and record the mention of `name` at `part`.
    fn mention(
        &mut self,
        level: usize,
        statement: u32,
        part: &ExpressionPart<'graph>,
        name: BinderSymbol,
        state: State,
    ) -> Result<(), ShapeError> {
        let class = state.class();
        let at = match class {
            MentionClass::Eager => Position::statement(statement as usize),
            MentionClass::Deferred => self.chain[level].end(),
        };
        let reader = Reader {
            level,
            statement,
            class,
        };
        let site = Site::of(part);
        let coordinate = match (self.builtins)(name) {
            Some(index) => Coordinate::Builtin(index),
            None => self
                .resolve(level, name, at, reader)
                .ok_or(ShapeError::Unbound {
                    name,
                    site,
                    at: Position::statement(statement as usize),
                })?,
        };
        self.chain[level].mentions.push(Mention {
            site,
            name,
            at,
            class,
            coordinate,
        });
        Ok(())
    }

    /// `name` read at `at` in the draft at `level`: a visible local, else outward — a capturing
    /// draft captures what it finds there, a block steps one activation out.
    fn resolve(
        &mut self,
        level: usize,
        name: BinderSymbol,
        at: Position,
        reader: Reader,
    ) -> Option<Coordinate> {
        let draft = &self.chain[level];
        let kind = draft.kind;
        if let Some(target) = resolve_here(draft.channels(), &draft.captures, name, at) {
            if let Target::Local(slot) = target {
                self.edge(level, slot, reader);
            }
            return Some(Coordinate::Activation { hops: 0, target });
        }
        let outer = |builder: &mut Self| match level.checked_sub(1) {
            Some(parent) => {
                let at = builder.chain[parent].boundary();
                builder.resolve(parent, name, at, reader)
            }
            None => builder
                .outer
                .and_then(|(outer, position)| outer(name, position)),
        };
        match kind {
            ShapeKind::Program => None,
            ShapeKind::Callable | ShapeKind::Module => {
                let source = outer(self)?;
                let captures = &mut self.chain[level].captures;
                captures.push(CaptureSpec {
                    name,
                    source: CaptureSource::Read(source),
                });
                Some(Coordinate::Activation {
                    hops: 0,
                    target: Target::Capture(CaptureSlot(captures.len() as u32 - 1)),
                })
            }
            ShapeKind::Block => Some(outer(self)?.through_block()),
        }
    }

    /// Record that the binder of the reading path's statement at `level` reads `slot`.
    fn edge(&mut self, level: usize, slot: Slot, reader: Reader) {
        let draft = &mut self.chain[level];
        let (statement, class) = if level == reader.level {
            (reader.statement, reader.class)
        } else {
            draft.current
        };
        draft.reads.push((statement, slot));
        if let Some(binder) = draft.statement_binder[statement as usize] {
            draft.edges.push((binder, slot, class));
        }
    }

    /// The components pass: condense, refuse an eager cycle, settle each nested draft's captures
    /// of a fellow member as edges, and seal the nested drafts.
    fn components(&mut self, draft: &mut Draft<'graph, 'x>) -> Result<(), ShapeError> {
        let scratch = self.scratch;
        let count = draft.channels().len();
        // Compressed rows: `offsets[i]..offsets[i + 1]` of `targets` are the slots binder `i` reads.
        let mut offsets: BumpVec<'x, usize> = BumpVec::with_capacity_in(count + 1, scratch);
        offsets.resize(count + 1, 0);
        for (binder, _, _) in draft.edges.iter() {
            offsets[binder.index() + 1] += 1;
        }
        for index in 0..count {
            offsets[index + 1] += offsets[index];
        }
        let mut fill = BumpVec::with_capacity_in(count, scratch);
        fill.extend_from_slice(&offsets[..count]);
        let mut targets: BumpVec<'x, usize> = BumpVec::with_capacity_in(draft.edges.len(), scratch);
        targets.resize(draft.edges.len(), 0);
        for (binder, bound, _) in draft.edges.iter() {
            let at = &mut fill[binder.index()];
            targets[*at] = bound.index();
            *at += 1;
        }
        let mut borrowed: BumpVec<'x, &[usize]> = BumpVec::with_capacity_in(count, scratch);
        borrowed.extend((0..count).map(|index| &targets[offsets[index]..offsets[index + 1]]));
        let condensed = strongly_connected_components(scratch, &borrowed);

        draft.component_of.resize(count, ComponentIndex(0));
        for (index, mut members) in condensed.into_iter().enumerate() {
            members.sort_unstable();
            let start = draft.members.len() as u32;
            for member in members.iter() {
                draft.component_of[*member] = ComponentIndex(index as u32);
                draft.members.push(Slot(*member as u32));
            }
            draft.components.push(DraftComponent {
                start,
                len: members.len() as u32,
                deferred_only: true,
                cyclic: false,
            });
        }
        for (binder, bound, class) in draft.edges.iter() {
            let component = &mut draft.components[draft.component_of[binder.index()].index()];
            if draft.component_of[binder.index()] != draft.component_of[bound.index()] {
                continue;
            }
            component.cyclic |= binder == bound || component.len > 1;
            if *class == MentionClass::Eager {
                component.deferred_only = false;
            }
        }
        let refused = (0..draft.components.len())
            .map(|index| ComponentIndex(index as u32))
            .filter(|component| {
                let component = draft.components[component.index()];
                component.cyclic && !component.deferred_only
            })
            .min_by_key(|component| {
                draft
                    .members_of(*component)
                    .iter()
                    .map(|slot| draft.channels().get(slot.index()))
                    .min()
            });
        if let Some(component) = refused {
            return Err(ShapeError::EagerCycle {
                members: draft
                    .members_of(component)
                    .iter()
                    .map(|slot| draft.channels().name(slot.index()))
                    .collect(),
            });
        }

        for (_, child) in draft.children.iter_mut() {
            let binder = draft.statement_binder[child.parent_statement as usize];
            for capture in child.captures.iter_mut() {
                let CaptureSource::Read(Coordinate::Activation {
                    hops: 0,
                    target: Target::Local(bound),
                }) = capture.source
                else {
                    continue;
                };
                let component = draft.component_of[bound.index()];
                if binder.is_some_and(|binder| draft.component_of[binder.index()] == component) {
                    let members = draft.components[component.index()].run(&draft.members);
                    let index = members
                        .binary_search(&bound)
                        .expect("a slot sits in its own component");
                    capture.source = CaptureSource::Member {
                        component,
                        index: index as u32,
                    };
                }
            }
        }
        Ok(())
    }

    /// The units pass: one unit per component whose members are not all parameters, keyed by its
    /// lowest statement, and one per statement that binds nothing, keyed by that statement; each
    /// unit waits on the units binding a slot it reads, and a unit holding an `EVAL` on every unit
    /// binding a name declared before it — a wait that closes a cycle refuses the body. The units are
    /// emitted in the smallest-key order that respects every wait, so independent units come out
    /// as they are written. Every read, wait and
    /// count is scratch: the shape keeps only the order.
    fn units(&self, draft: &mut Draft<'graph, 'x>) -> Result<(), ShapeError> {
        let scratch = self.scratch;
        let statements = draft.statements as usize;
        let channels = draft.channels();
        // The unit each statement belongs to, and the key-ordered works: a statement belongs to
        // its binder's component's unit, or to a unit of its own.
        let mut component_key: BumpVec<'x, Option<u32>> =
            BumpVec::with_capacity_in(draft.components.len(), scratch);
        component_key.resize(draft.components.len(), None);
        for (slot, component) in draft.component_of.iter().enumerate() {
            if let Some(statement) = channels.get(slot).0.checked_sub(1) {
                let key = &mut component_key[component.index()];
                *key = Some(key.map_or(statement, |key| key.min(statement)));
            }
        }
        // A unit's key is a statement, and no two units share one, so a run indexed by statement
        // orders them.
        let mut by_key: BumpVec<'x, Option<UnitWork>> =
            BumpVec::with_capacity_in(statements, scratch);
        by_key.resize(statements, None);
        for (component, key) in component_key.iter().enumerate() {
            if let Some(key) = key {
                by_key[*key as usize] = Some(UnitWork::Component(ComponentIndex(component as u32)));
            }
        }
        for (statement, binder) in draft.statement_binder.iter().enumerate() {
            if binder.is_none() {
                by_key[statement] = Some(UnitWork::Statement(statement as u32));
            }
        }
        let mut works: BumpVec<'x, UnitWork> = BumpVec::with_capacity_in(statements, scratch);
        let mut unit_of_key: BumpVec<'x, u32> = BumpVec::with_capacity_in(statements, scratch);
        unit_of_key.resize(statements, u32::MAX);
        for (key, work) in by_key.iter().enumerate() {
            if let Some(work) = work {
                unit_of_key[key] = works.len() as u32;
                works.push(*work);
            }
        }
        let unit_of_statement = |statement: u32| match draft.statement_binder[statement as usize] {
            Some(binder) => {
                let component = draft.component_of[binder.index()];
                let key = component_key[component.index()].expect("a binder's component has a key");
                unit_of_key[key as usize]
            }
            None => unit_of_key[statement as usize],
        };
        let unit_of_slot = |slot: Slot| {
            let component = draft.component_of[slot.index()];
            component_key[component.index()].map(|key| unit_of_key[key as usize])
        };
        // `(waited on, waiter, through)`, one per wait — duplicates only raise a count they also
        // lower. A read waits through nothing, and the reads are acyclic: a cycle among bindings
        // is one component. An `EVAL` waits through each slot declared before it, which it may
        // read, so a cycle holds at least one `EVAL` and the body is refused.
        let mut waits: BumpVec<'x, Wait> = BumpVec::new_in(scratch);
        for (statement, slot) in draft.reads.iter() {
            let waiter = unit_of_statement(*statement);
            if let Some(bound) = unit_of_slot(*slot)
                && bound != waiter
            {
                waits.push((bound, waiter, None));
            }
        }
        for statement in draft.eval_statements.iter() {
            let waiter = unit_of_statement(*statement);
            let before = Position::statement(*statement as usize);
            for slot in 0..channels.len() {
                let declared = channels.get(slot);
                if declared == Position::PARAMETER || declared >= before {
                    continue;
                }
                if let Some(bound) = unit_of_slot(Slot(slot as u32))
                    && bound != waiter
                {
                    waits.push((bound, waiter, Some((Slot(slot as u32), *statement))));
                }
            }
        }
        waits.sort_unstable();
        // Per unit: the waits outstanding.
        let mut pending: BumpVec<'x, u32> = BumpVec::with_capacity_in(works.len(), scratch);
        pending.resize(works.len(), 0);
        for (_, waiter, _) in waits.iter() {
            pending[*waiter as usize] += 1;
        }
        let mut emitted: BumpVec<'x, bool> = BumpVec::with_capacity_in(works.len(), scratch);
        emitted.resize(works.len(), false);
        let last = statements
            .checked_sub(1)
            .map(|last| unit_of_statement(last as u32));
        let mut cursor = 0;
        while draft.units.len() < works.len() {
            let Some(unit) =
                (cursor..works.len()).find(|unit| !emitted[*unit] && pending[*unit] == 0)
            else {
                let (slot, statement) = eval_cycle(&waits, &emitted);
                return Err(ShapeError::EvalCycle {
                    name: draft.channels().name(slot.index()),
                    eval: Position::statement(statement as usize),
                });
            };
            emitted[unit] = true;
            draft.units.push(Unit {
                work: works[unit],
                last: last == Some(unit as u32),
            });
            cursor = unit + 1;
            let first = waits.partition_point(|(bound, _, _)| (*bound as usize) < unit);
            for (bound, waiter, _) in waits[first..].iter() {
                if *bound as usize != unit {
                    break;
                }
                pending[*waiter as usize] -= 1;
                if pending[*waiter as usize] == 0 {
                    cursor = cursor.min(*waiter as usize);
                }
            }
        }
        Ok(())
    }

    /// Lay a finished draft down in program storage, its nested drafts first; `form` is the node
    /// holding a callable draft's body.
    fn seal(
        &self,
        draft: Draft<'graph, 'x>,
        form: Option<&'graph KExpression<'graph>>,
    ) -> &'graph BodyShape<'graph> {
        let writer = self.brand.writer();
        let mut nested = BumpVec::with_capacity_in(draft.children.len(), self.scratch);
        for (site, child) in draft.children {
            let form = self.forms.get(&site).copied();
            nested.push((site, self.seal(child, form)));
        }
        nested.sort_unstable_by_key(|(site, _)| *site);
        let mut births = BumpVec::with_capacity_in(draft.births.len(), self.scratch);
        births.extend(draft.births.iter().map(|(binder, site)| {
            let index = nested
                .binary_search_by_key(site, |(nested, _)| *nested)
                .expect("a birth's body is a nested shape");
            (*binder, nested[index].1)
        }));
        births.sort_unstable_by_key(|(binder, _)| *binder);
        let mut rhs = BumpVec::with_capacity_in(draft.rhs.len(), self.scratch);
        rhs.extend(draft.rhs.iter().map(|(binder, site)| {
            let part = self
                .parts
                .get(site)
                .copied()
                .expect("a recorded right-hand side is kept by site");
            (*binder, part)
        }));
        rhs.sort_unstable_by_key(|(binder, _)| *binder);
        let mut declarations = draft.declarations;
        declarations.sort_unstable_by_key(|(binder, _)| *binder);
        let mut mentions = draft.mentions;
        mentions.sort_unstable_by_key(|mention| mention.site);
        let members = collect(writer, draft.members.iter().copied());
        let mut components = BumpVec::with_capacity_in(draft.components.len(), self.scratch);
        components.extend(draft.components.iter().map(|component| Component {
            members: component.run(members),
            deferred_only: component.deferred_only,
            cyclic: component.cyclic,
        }));
        resident(
            writer,
            BodyShape {
                kind: draft.kind,
                names: Channels::new(
                    collect(writer, draft.values.iter().copied()),
                    collect(writer, draft.types.iter().copied()),
                ),
                body: collect(writer, draft.nodes.iter().copied()),
                group_frame: draft.frame,
                held: draft.held,
                entered_at: draft.entered_at,
                component_of: collect(writer, draft.component_of.iter().copied()),
                components: collect(writer, components.iter().copied()),
                mentions: collect(writer, mentions.iter().copied()),
                captures: collect(writer, draft.captures.iter().copied()),
                nested: collect(writer, nested.iter().copied()),
                form,
                births: collect(writer, births.iter().copied()),
                rhs: collect(writer, rhs.iter().copied()),
                declarations: collect(writer, declarations.iter().copied()),
                units: collect(writer, draft.units.iter().copied()),
                keeps_defining_scope: draft.keeps_defining_scope,
            },
        )
    }
}
