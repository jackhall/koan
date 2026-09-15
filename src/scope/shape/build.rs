//! The **shape builder**: one recursive walk that every kind of shape goes through.
//!
//! A body is built in three passes over a draft kept in scratch. The binders pass lays out the
//! declared names and refuses a repeated name or a builtin's. The mention pass walks each statement
//! from its root, carrying the class a mention met there would take, resolving each mention as it
//! is met and building each nested body or arm as a draft of its own on top of the chain of
//! enclosing drafts. The components pass condenses the bindings' reference graph, refuses a
//! component with an eager internal mention, turns a nested callable's capture of a fellow member
//! into a knot edge, and seals the nested drafts into program storage.
//!
//! A callable body records the form node holding it, a binder whose right-hand side is a callable
//! form at its root — or a combined form that is one — records the body it births, and a `LET` value
//! binder records its right-hand side, so a component's tie reads each member's signature, body or
//! data off the shape.
//!
//! See [README.md § Visibility](../README.md#visibility).

use crate::memory::{
    BumpAllocator, BumpBackedMap, BumpVec, ProgramBrand, bump_table, strongly_connected_components,
};
use crate::parse::forms::{FormId, KEYWORDS};
use crate::parse::{
    BinderSymbol, ExpressionPart, KExpression, StaticName, TypeSymbol, ValueSymbol,
};
use crate::values::Knotted;

use super::super::activation::Activation;
use super::super::builtins::Builtins;
use super::super::channels::Channels;
use super::super::roles::{BodyKind, Heads, Role, SchemaKind, roles};
use super::super::signature::{
    body_of, declare_parameters, declare_quantifiers, pair_name, signature_run,
};
use super::{
    BuiltinIndex, CaptureSlot, CaptureSource, CaptureSpec, Component, ComponentIndex, Coordinate,
    Mention, MentionClass, Position, Shape, ShapeError, ShapeKind, Site, Slot, Target,
    resolve_here,
};

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
) -> Result<&'graph Shape<'graph>, ShapeError> {
    let lookup = |name| builtins.lookup(name);
    let mut builder = Builder::new(brand, scratch, &lookup, None);
    let statements = statements
        .iter()
        .enumerate()
        .map(|(index, statement)| (statement, index + 1));
    let draft = builder.draft(ShapeKind::Program, Position::PARAMETER, &[], statements)?;
    Ok(builder.seal(draft, None))
}

/// An `EVAL` body's block shape over `site`'s chain, reading at `at`.
pub(super) fn eval<'graph, X: Knotted>(
    brand: ProgramBrand<'graph>,
    body: &KExpression<'graph>,
    site: &Activation<'graph, '_, X>,
    at: Position,
    scratch: BumpAllocator<'_>,
) -> Result<&'graph Shape<'graph>, ShapeError> {
    let lookup = |name| site.builtins().lookup(name);
    let outer = |name, position| site.through_chain(name, position);
    let mut builder = Builder::new(brand, scratch, &lookup, Some((&outer, at)));
    let draft = builder.draft(ShapeKind::Block, at, &[], body.body_statements())?;
    Ok(builder.seal(draft, None))
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
    /// Entering a constructor slot: a list element, a dict value, a record field, a schema, a
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
struct Draft<'x> {
    kind: ShapeKind,
    entered_at: Position,
    /// The statement of the enclosing draft this body sits in.
    parent_statement: u32,
    statements: u32,
    values: BumpVec<'x, (ValueSymbol, Position)>,
    types: BumpVec<'x, (TypeSymbol, Position)>,
    /// The slot each statement binds, if it binds one.
    statement_binder: BumpVec<'x, Option<Slot>>,
    mentions: BumpVec<'x, Mention>,
    captures: BumpVec<'x, CaptureSpec>,
    /// `(binder, bound, class)`: the binder's statement reads the bound slot.
    edges: BumpVec<'x, (Slot, Slot, MentionClass)>,
    /// Finished nested drafts, waiting on this draft's components to settle their captures.
    children: BumpVec<'x, (Site, Draft<'x>)>,
    /// `(binder, body)`: the binder's right-hand side births the callable whose body sits at `body`.
    births: BumpVec<'x, (Slot, Site)>,
    /// `(binder, rhs)`: a `LET` value binder's right-hand side part sits at `rhs`.
    rhs: BumpVec<'x, (Slot, Site)>,
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

impl Draft<'_> {
    fn members_of(&self, component: ComponentIndex) -> &[Slot] {
        self.components[component.index()].run(&self.members)
    }

    fn end(&self) -> Position {
        Position(self.statements + 1)
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
    chain: BumpVec<'x, Draft<'x>>,
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
    ) -> Self {
        Builder {
            brand,
            scratch,
            builtins,
            outer,
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
        statements: impl Iterator<Item = (&'n KExpression<'graph>, usize)>,
    ) -> Result<Draft<'x>, ShapeError>
    where
        'graph: 'n,
    {
        let mut nodes: BumpVec<'x, &'n KExpression<'graph>> = BumpVec::new_in(self.scratch);
        nodes.extend(statements.map(|(node, _)| node));
        let parent_statement = self
            .chain
            .last()
            .map_or(u32::MAX, |parent| parent.current.0);
        let draft = self.binders(kind, entered_at, parent_statement, parameters, &nodes)?;
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
    ) -> Result<Draft<'x>, ShapeError> {
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
            children: BumpVec::new_in(scratch),
            births: BumpVec::new_in(scratch),
            rhs: BumpVec::new_in(scratch),
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
        let Some(form) = node.cache().form() else {
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
        let roles = roles(form.id);
        if roles == [Role::Unsupported] {
            return Err(ShapeError::Unsupported {
                form: form.id,
                at: Position::statement(statement as usize),
            });
        }
        debug_assert_eq!(
            roles.len(),
            node.parts.len(),
            "a form's parts match its key"
        );
        if form.id == FormId::Eval {
            for draft in self.chain.iter_mut() {
                draft.keeps_defining_scope = true;
            }
        }

        // A callable's parameters are declared before any part is read, so a type parameter a
        // signature names is never taken for a mention of the enclosing shape.
        let declares = roles
            .iter()
            .any(|role| matches!(role, Role::Signature | Role::Quantifiers));
        let mut parameters = BumpVec::new_in(self.scratch);
        if declares {
            for (role, part) in roles.iter().zip(node.parts) {
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
        let walked = self.walk_parts(level, statement, node, form.id, roles, &parameters, state);
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
        form: FormId,
        roles: &[Role],
        parameters: &[BinderSymbol],
        state: State,
    ) -> Result<(), ShapeError> {
        for (role, part) in roles.iter().zip(node.parts) {
            let part = &part.value;
            match *role {
                Role::Keyword | Role::Name | Role::Data | Role::Label | Role::Quantifiers => {}
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
                Role::Branches(heads) => self.enter_arms(level, statement, form, part, heads)?,
                Role::Schema(kind) => {
                    self.walk_schema(level, statement, part, kind, state.constructor())?
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

    /// A type's schema, under the constructor state: labels and the schema's own `TYPE`
    /// declarations are not mentions, and every other type name is.
    fn walk_schema(
        &mut self,
        level: usize,
        statement: u32,
        part: &ExpressionPart<'graph>,
        kind: SchemaKind,
        state: State,
    ) -> Result<(), ShapeError> {
        let mut own = BumpVec::new_in(self.scratch);
        if let ExpressionPart::Expression(run) = part {
            own.extend(run.body_statements().filter_map(|(node, _)| {
                let form = node.statement_spine().cache().form()?;
                (form.id == FormId::TypeDeclaration)
                    .then(|| node.statement_binder_plan()?.name)
                    .flatten()
                    .and_then(|name| match name {
                        BinderSymbol::Type(name) => Some(name),
                        BinderSymbol::Value(_) => None,
                    })
            }));
        }
        let mark = self.skip.len();
        self.skip.extend_from_slice(&own);
        let walked = self.walk_schema_part(level, statement, part, kind, state);
        self.skip.truncate(mark);
        walked
    }

    fn walk_schema_part(
        &mut self,
        level: usize,
        statement: u32,
        part: &ExpressionPart<'graph>,
        kind: SchemaKind,
        state: State,
    ) -> Result<(), ShapeError> {
        let run = match part {
            ExpressionPart::Type(name) if !self.skips(name) => {
                return self.mention(level, statement, part, BinderSymbol::Type(*name), state);
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
            let tag = kind == SchemaKind::Union && index % 2 == 0;
            if tag && matches!(inner.value, ExpressionPart::Type(_)) {
                continue;
            }
            self.walk_schema_part(level, statement, &inner.value, SchemaKind::Plain, state)?;
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
                form: node.cache().form().expect("a body role is a form's").id,
                at: Position::statement(statement as usize),
            });
        };
        if kind != BodyKind::Module {
            let site = Site::of(part);
            self.forms.insert(site, self.brand.allocator().alloc(*node));
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
        };
        let operator = [
            BinderSymbol::Value(IMPLICIT.left.symbol()),
            BinderSymbol::Value(IMPLICIT.right.symbol()),
        ];
        let unary = [BinderSymbol::Value(IMPLICIT.operands.symbol())];
        let parameters: &[BinderSymbol] = match kind {
            BodyKind::Lambda => signature,
            BodyKind::Operator => &operator,
            BodyKind::UnaryOperator => &unary,
            BodyKind::Module => &[],
        };
        self.enter_child(
            level,
            statement,
            class,
            Site::of(part),
            shape_kind,
            parameters,
            body.body_statements(),
        )
    }

    fn enter_arms(
        &mut self,
        level: usize,
        statement: u32,
        form: FormId,
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
        statements: impl Iterator<Item = (&'n KExpression<'graph>, usize)>,
    ) -> Result<(), ShapeError>
    where
        'graph: 'n,
    {
        let parent = &mut self.chain[level];
        parent.current = (statement, class);
        let entered_at = parent.boundary();
        let floor = std::mem::replace(&mut self.skip_floor, self.skip.len());
        let child = self.draft(kind, entered_at, parameters, statements);
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
        if let Some(binder) = draft.statement_binder[statement as usize] {
            draft.edges.push((binder, slot, class));
        }
    }

    /// The components pass: condense, refuse an eager cycle, settle each nested draft's captures
    /// of a fellow member as edges, and seal the nested drafts.
    fn components(&mut self, draft: &mut Draft<'x>) -> Result<(), ShapeError> {
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

    /// Lay a finished draft down in program storage, its nested drafts first; `form` is the node
    /// holding a callable draft's body.
    fn seal(
        &self,
        draft: Draft<'x>,
        form: Option<&'graph KExpression<'graph>>,
    ) -> &'graph Shape<'graph> {
        let storage = self.brand.allocator();
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
        let mut mentions = draft.mentions;
        mentions.sort_unstable_by_key(|mention| mention.site);
        let members = storage.alloc_slice_copy(&draft.members);
        let mut components = BumpVec::with_capacity_in(draft.components.len(), self.scratch);
        components.extend(draft.components.iter().map(|component| Component {
            members: component.run(members),
            deferred_only: component.deferred_only,
            cyclic: component.cyclic,
        }));
        storage.alloc(Shape {
            kind: draft.kind,
            names: Channels::new(
                storage.alloc_slice_copy(&draft.values),
                storage.alloc_slice_copy(&draft.types),
            ),
            statements: draft.statements,
            entered_at: draft.entered_at,
            component_of: storage.alloc_slice_copy(&draft.component_of),
            components: storage.alloc_slice_copy(&components),
            mentions: storage.alloc_slice_copy(&mentions),
            captures: storage.alloc_slice_copy(&draft.captures),
            nested: storage.alloc_slice_copy(&nested),
            form,
            births: storage.alloc_slice_copy(&births),
            rhs: storage.alloc_slice_copy(&rhs),
            keeps_defining_scope: draft.keeps_defining_scope,
        })
    }
}
