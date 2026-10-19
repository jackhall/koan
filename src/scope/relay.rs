//! The **relay**: what a pass's write half copies a fact it keeps through, out of the walk's
//! scratch and into the region its output rests in
//! ([build/README.md § Steps and the drain](../build/README.md#steps-and-the-drain)).
//!
//! A pass's rules lay every value they compute in the walk's scratch and intern every type through
//! the chain they are handed for work. The write half keeps only some of those values — the
//! shape's cells at the load, a resumption's `Settled` in a step — and copies each through a
//! [`Relay`]: every run and reference it holds is re-laid through the output's writer, and every
//! type handle it names is adopted into the output's chain from the work chain, as a caching
//! body's exit adopts what leaves it. The vocabulary is covariant and names a body by its segment
//! ([`tree`](super::tree), [`typed`](super::typed)), so a copy is checked by lifetimes; an adopt is
//! not, so each copy here destructures its value with no `..` — a field added later fails to
//! compile until the relay copies it — and a debug build asserts each handle resolves once
//! adopted.
//!
//! At the load the work chain is the output chain, so every handle resolves already and each adopt
//! returns at once.

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Writer, bump_table, collect, resident};
use crate::type_lattice::{DeclaredType, Habitat, Interval, Parametric, TypeChain};
use crate::values::Types;

use super::keyed::Keyed;
use super::settled::{ChannelSettled, Settled, StaticsSettled};
use super::shape::facts::{Binding, Frontier, Residual};
use super::shape::{Coordinate, ShapeError, Site, Slot, Target};
use super::tree::{KnotNode, Leaf, NodeCapture, Tree, Unfolded, Within};
use super::typed::{
    Caching, Callable, Elaboration, FunctionGroupMap, InstanceRead, Narrowing, ParameterBinding,
    PathEntry, PathRead, Registered, Resolution, Resolved, ShapeFacts, ShapeGroupMap, Static,
    StaticCallable, StaticRegistered, StaticSolution, StaticType, Statics, UseStatics, ViewSlots,
};

/// What a write half copies a kept fact through: the writer it lays into, the chain it adopts
/// into, and the chain the fact's types were interned through.
pub struct Relay<'out, 'w, 'from, H: Habitat<'from>> {
    writer: Writer<'out, 'w>,
    into: &'w Types<'out, 'w>,
    from: &'w TypeChain<'from, H>,
    scratch: BumpAllocator<'w>,
    /// Each tree reference relayed, by its source's address and by its copy's own, so a shared
    /// subtree is copied once and a fact embedding one already relayed copies it no more.
    trees: Cell<BumpBackedMap<'w, usize, &'out Tree<'out>>>,
    /// Each capture run relayed, by its source's address and length, and by its copy's own.
    captures: Cell<BumpBackedMap<'w, (usize, usize), Captures<'out>>>,
}

/// A capture run of a body's tree or a leaf's run.
type Captures<'a> = &'a [(u32, Tree<'a>)];

/// A run's identity in the relay's memos: its address and its length.
fn run_key<T>(run: &[T]) -> (usize, usize) {
    (run.as_ptr().addr(), run.len())
}

impl<'out, 'w, 'from, H: Habitat<'from>> Relay<'out, 'w, 'from, H> {
    /// A relay laying through `writer` and adopting into `into` what `from` interned; `scratch`
    /// holds its memos and staging.
    pub fn new(
        writer: Writer<'out, 'w>,
        into: &'w Types<'out, 'w>,
        from: &'w TypeChain<'from, H>,
        scratch: BumpAllocator<'w>,
    ) -> Self {
        Relay {
            writer,
            into,
            from,
            scratch,
            trees: Cell::new(bump_table(scratch)),
            captures: Cell::new(bump_table(scratch)),
        }
    }

    /// The writer the relay lays through.
    pub fn writer(&self) -> Writer<'out, 'w> {
        self.writer
    }

    /// The chain the relay adopts into.
    pub fn types(&self) -> &'w Types<'out, 'w> {
        self.into
    }

    /// `kt`, made to resolve in the output chain.
    pub fn handle<K: Copy + Into<DeclaredType<Parametric>>>(&self, kt: K) -> K {
        self.into.adopt_from(self.from, kt);
        debug_assert!(self.into.resolves(kt), "a relayed handle resolves");
        kt
    }

    fn declared<T: Copy>(&self, declared: DeclaredType<T>) -> DeclaredType<T>
    where
        DeclaredType<T>: Into<DeclaredType<Parametric>>,
    {
        self.handle(declared)
    }

    pub fn interval<T: Copy + Into<DeclaredType<Parametric>>>(
        &self,
        Interval { lower, upper }: Interval<T>,
    ) -> Interval<T> {
        Interval {
            lower: self.handle(lower),
            upper: self.handle(upper),
        }
    }

    /// `items` copied into the output, each through `each`: staged in scratch first, so `each` may
    /// itself lay.
    fn run<T, U>(&self, items: &[T], mut each: impl FnMut(&T) -> U) -> &'out [U] {
        let mut staged = BumpVec::with_capacity_in(items.len(), self.scratch);
        staged.extend(items.iter().map(&mut each));
        collect(self.writer, staged.into_iter())
    }

    /// `items`, copied as they are.
    fn copied<T: Copy>(&self, items: &[T]) -> &'out [T] {
        collect(self.writer, items.iter().copied())
    }

    /// `keyed`, each value copied through `each`, which relays what the value holds.
    pub fn keyed<K: Ord + Copy, V, U>(
        &self,
        keyed: Keyed<'_, (K, V)>,
        mut each: impl FnMut(&V) -> U,
    ) -> Keyed<'out, (K, U)> {
        Keyed::from_sorted(self.run(keyed.entries(), |(key, value)| (*key, each(value))))
    }

    fn keyed_copied<T: Copy + super::keyed::KeyedEntry>(
        &self,
        keyed: Keyed<'_, T>,
    ) -> Keyed<'out, T> {
        Keyed::from_sorted(self.copied(keyed.entries()))
    }

    pub fn frontier(&self, frontier: Frontier<'_>) -> Frontier<'out> {
        Frontier::from_sorted(self.copied(frontier.unknowns()))
    }

    pub fn residual(&self, residual: Residual<'_>) -> Residual<'out> {
        self.keyed(residual, |frontier| self.frontier(*frontier))
    }

    pub fn binding(&self, binding: Binding<'_>) -> Binding<'out> {
        let mut staged = BumpVec::with_capacity_in(binding.entries().len(), self.scratch);
        staged.extend(
            (binding.entries().iter()).map(|(unknown, bound)| (*unknown, self.handle(*bound))),
        );
        Binding::laid(self.writer, &mut staged)
    }

    fn statically<'a, C, R, C2, R2>(
        &self,
        value: Static<'a, C, R>,
        closed: impl FnOnce(C) -> C2,
        rigid: impl FnOnce(R) -> R2,
    ) -> Static<'out, C2, R2> {
        match value {
            Static::Unknown => Static::Unknown,
            Static::Closed(value) => Static::Closed(closed(value)),
            Static::Rigid { value, variables } => Static::Rigid {
                value: rigid(value),
                variables: self.copied(variables),
            },
        }
    }

    pub fn static_type(&self, typed: StaticType<'_>) -> StaticType<'out> {
        self.statically(typed, |kt| self.handle(kt), |kt| self.handle(kt))
    }

    fn callable<T: Copy>(
        &self,
        Callable {
            ktype,
            quantifier_map: FunctionGroupMap(map),
            registered,
        }: Callable<'_, T>,
    ) -> Callable<'out, T>
    where
        DeclaredType<T>: Into<DeclaredType<Parametric>>,
    {
        Callable {
            ktype: self.declared(ktype),
            quantifier_map: FunctionGroupMap(self.copied(map)),
            registered: registered.map(|registered| self.registered(registered)),
        }
    }

    fn registered<T: Copy>(
        &self,
        Registered {
            shape,
            quantifier_map: ShapeGroupMap(map),
            parameters,
        }: Registered<'_, T>,
    ) -> Registered<'out, T>
    where
        DeclaredType<T>: Into<DeclaredType<Parametric>>,
    {
        Registered {
            shape: self.declared(shape),
            quantifier_map: ShapeGroupMap(self.copied(map)),
            parameters: match parameters {
                ParameterBinding::Named(names) => ParameterBinding::Named(self.copied(names)),
                ParameterBinding::Operands => ParameterBinding::Operands,
            },
        }
    }

    pub fn static_callable(&self, callable: StaticCallable<'_>) -> StaticCallable<'out> {
        self.statically(
            callable,
            |callable| self.callable(callable),
            |callable| self.callable(callable),
        )
    }

    pub fn static_registered(&self, registered: StaticRegistered<'_>) -> StaticRegistered<'out> {
        self.statically(
            registered,
            |registered| self.registered(registered),
            |registered| self.registered(registered),
        )
    }

    pub fn static_solution(&self, solution: StaticSolution<'_>) -> StaticSolution<'out> {
        match solution {
            StaticSolution::Closed(points) => {
                StaticSolution::Closed(self.run(points, |point| self.handle(*point)))
            }
            StaticSolution::Rigid { value, variables } => StaticSolution::Rigid {
                value: self.run(value, |point| self.handle(*point)),
                variables: self.copied(variables),
            },
        }
    }

    pub fn instance_read(&self, read: InstanceRead<'_>) -> InstanceRead<'out> {
        match read {
            InstanceRead::Solved(solution) => InstanceRead::Solved(self.static_solution(solution)),
            InstanceRead::AsIs => InstanceRead::AsIs,
        }
    }

    pub fn use_statics(
        &self,
        UseStatics {
            narrowing,
            contributions,
        }: UseStatics<'_>,
    ) -> UseStatics<'out> {
        UseStatics {
            narrowing: match narrowing {
                Narrowing::Full => Narrowing::Full,
                Narrowing::Kept(kept) => Narrowing::Kept(self.copied(kept)),
                Narrowing::Selected(at) => Narrowing::Selected(at),
            },
            contributions: self.run(contributions, |typed| self.static_type(*typed)),
        }
    }

    pub fn statics(
        &self,
        Statics {
            parts,
            statements,
            binders,
            settled,
            instances,
            uses,
            selections,
            named,
            class,
            minting,
            trees,
            throughs,
            digested,
            view_slots,
            handoffs,
            use_frontiers,
        }: Statics<'_>,
    ) -> Statics<'out> {
        Statics {
            parts: self.keyed(parts, |typed| self.interval(*typed)),
            statements: self.run(statements, |typed| self.interval(*typed)),
            binders: self.run(binders, |typed| match *typed {
                DeclaredType::Type(typed) => DeclaredType::Type(self.interval(typed)),
                DeclaredType::Scheme(scheme) => DeclaredType::Scheme(self.handle(scheme)),
            }),
            settled: self.keyed_copied(settled),
            instances: self.keyed(instances, |read| self.instance_read(*read)),
            uses: self.keyed(uses, |held| self.use_statics(*held)),
            selections: self.keyed(selections, |points| {
                self.run(points, |point| point.map(|point| self.handle(point)))
            }),
            named: self.keyed(named, |typed| {
                self.run(typed, |typed| self.static_type(*typed))
            }),
            class,
            minting: self.keyed_copied(minting),
            trees: self.keyed(trees, |tree| self.tree_ref(tree)),
            throughs: self.keyed(throughs, |located| self.copied(located)),
            digested: self.copied(digested),
            view_slots: self.keyed(view_slots, |ViewSlots { body, captures }| ViewSlots {
                body: *body,
                captures: self.copied(captures),
            }),
            handoffs: self.keyed(handoffs, |handed| self.copied(handed)),
            use_frontiers: self.run(use_frontiers, |frontier| self.frontier(*frontier)),
        }
    }

    pub fn shape_facts(
        &self,
        ShapeFacts {
            summary,
            recursive,
            signature,
            ran,
            summary_frontier,
            signature_frontier,
            ran_frontier,
            born_frontier,
        }: ShapeFacts<'_>,
    ) -> ShapeFacts<'out> {
        ShapeFacts {
            summary: summary.map(|summary| summary.map(|tree| self.tree(tree))),
            recursive,
            signature: signature.map(|signature| self.handle(signature)),
            ran: ran.map(|ran| self.interval(ran)),
            summary_frontier: self.frontier(summary_frontier),
            signature_frontier: self.frontier(signature_frontier),
            ran_frontier: self.frontier(ran_frontier),
            born_frontier: self.frontier(born_frontier),
        }
    }

    pub fn caching(
        &self,
        Caching {
            caches,
            mints,
            warm,
            invariant,
            keyable,
            escapes,
        }: Caching<'_>,
    ) -> Caching<'out> {
        Caching {
            caches,
            mints: self.keyed_copied(mints),
            warm: self.keyed_copied(warm),
            invariant: invariant.map(|slots| self.copied(slots)),
            keyable: self.copied(keyable),
            escapes,
        }
    }

    /// `tree`, copied into the output with every subtree it shares copied once.
    pub fn tree(&self, tree: Tree<'_>) -> Tree<'out> {
        match tree {
            Tree::Digest(digest) => Tree::Digest(digest),
            Tree::Body {
                body,
                captures,
                print,
            } => Tree::Body {
                body,
                captures: self.captures(captures),
                print,
            },
            Tree::Knot {
                members,
                index,
                print,
            } => Tree::Knot {
                members: self.run(members, |node| self.knot_node(*node)),
                index,
                print,
            },
            Tree::View {
                operator,
                application,
                source,
                print,
            } => Tree::View {
                operator,
                application: self.handle(application),
                source: self.tree_ref(source),
                print,
            },
            Tree::Retyped {
                carried,
                source,
                print,
            } => Tree::Retyped {
                carried: self.handle(carried),
                source: self.tree_ref(source),
                print,
            },
            Tree::Leaf(Leaf::Bound {
                binder,
                chain,
                within,
                print,
            }) => Tree::Leaf(Leaf::Bound {
                binder,
                chain: self.copied(chain),
                within: within.map(|within| resident(self.writer, self.within(*within))),
                print,
            }),
            Tree::Leaf(Leaf::Unnamed(site)) => Tree::Leaf(Leaf::Unnamed(site)),
        }
    }

    /// A tree reference, copied once per source.
    pub fn tree_ref(&self, tree: &Tree<'_>) -> &'out Tree<'out> {
        let key = std::ptr::from_ref(tree).addr();
        let mut memo = self.trees.replace(bump_table(self.scratch));
        let held = memo.get(&key).copied();
        self.trees.set(memo);
        if let Some(held) = held {
            return held;
        }
        let copy = resident(self.writer, self.tree(*tree));
        memo = self.trees.replace(bump_table(self.scratch));
        memo.insert(key, copy);
        memo.insert(std::ptr::from_ref(copy).addr(), copy);
        self.trees.set(memo);
        copy
    }

    /// A capture run, copied once per source.
    fn captures(&self, captures: Captures<'_>) -> Captures<'out> {
        let key = run_key(captures);
        let mut memo = self.captures.replace(bump_table(self.scratch));
        let held = memo.get(&key).copied();
        self.captures.set(memo);
        if let Some(held) = held {
            return held;
        }
        let copy = self.run(captures, |(slot, tree)| (*slot, self.tree(*tree)));
        memo = self.captures.replace(bump_table(self.scratch));
        memo.insert(key, copy);
        memo.insert(run_key(copy), copy);
        self.captures.set(memo);
        copy
    }

    fn knot_node(
        &self,
        KnotNode {
            body,
            instance,
            captures,
        }: KnotNode<'_>,
    ) -> KnotNode<'out> {
        KnotNode {
            body,
            instance: self.run(instance, |point| self.handle(*point)),
            captures: self.run(captures, |(slot, capture)| {
                (
                    *slot,
                    match capture {
                        NodeCapture::Fellow(fellow) => NodeCapture::Fellow(*fellow),
                        NodeCapture::Tree(tree) => NodeCapture::Tree(self.tree(*tree)),
                    },
                )
            }),
        }
    }

    fn within(&self, within: Within<'_>) -> Within<'out> {
        match within {
            Within::Body(captures) => Within::Body(self.captures(captures)),
            Within::Call(call) => Within::Call(self.unfolded(call)),
        }
    }

    fn unfolded(
        &self,
        Unfolded {
            callee,
            arguments,
            solution,
        }: Unfolded<'_>,
    ) -> Unfolded<'out> {
        Unfolded {
            callee,
            arguments: self.run(arguments, |(slot, tree): &(Slot, Tree<'_>)| {
                (*slot, self.tree(*tree))
            }),
            solution: self.run(solution, |(slot, point)| (*slot, self.handle(*point))),
        }
    }

    pub fn resolution(
        &self,
        Resolution {
            tree,
            resolved,
            frontier,
        }: Resolution<'_>,
    ) -> Resolution<'out> {
        Resolution {
            tree: self.tree(tree),
            resolved: match resolved {
                Resolved::Closed(closed) => Resolved::Closed(self.handle(closed)),
                Resolved::Class(level) => Resolved::Class(level),
            },
            frontier: self.frontier(frontier),
        }
    }

    pub fn path_entry(
        &self,
        PathEntry {
            root,
            chain,
            label,
            level,
            through,
        }: PathEntry<'_>,
    ) -> PathEntry<'out> {
        PathEntry {
            root,
            chain: self.copied(chain),
            label,
            level,
            through: through.map(|through| self.handle(through)),
        }
    }

    /// A run of path entries, each copied.
    pub fn path_entries(&self, entries: &[PathEntry<'_>]) -> &'out [PathEntry<'out>] {
        self.run(entries, |entry| self.path_entry(*entry))
    }

    /// Each path spelling a shape reads, by site.
    pub fn path_reads(&self, reads: Keyed<'_, (Site, PathRead)>) -> Keyed<'out, (Site, PathRead)> {
        self.keyed_copied(reads)
    }

    /// The coordinates of a callable's type captures.
    pub fn coordinates(&self, coordinates: &[Coordinate]) -> &'out [Coordinate] {
        self.copied(coordinates)
    }

    /// Each lexical variable a shape declares, beside where its activation holds it.
    pub fn declared_variables(&self, variables: &[(usize, Target)]) -> &'out [(usize, Target)] {
        self.copied(variables)
    }

    /// A callable's own group as its body reads it.
    pub fn levels(&self, levels: &[Parametric]) -> &'out [Parametric] {
        self.run(levels, |level| self.handle(*level))
    }

    fn elaboration(&self, error: Elaboration) -> Elaboration {
        match error {
            Elaboration::NoSuchMember { owner, name, site } => Elaboration::NoSuchMember {
                owner: self.handle(owner),
                name,
                site,
            },
            Elaboration::TiedUnion {
                members: (a, b),
                site,
            } => Elaboration::TiedUnion {
                members: (self.handle(a), self.handle(b)),
                site,
            },
            Elaboration::NotAType { name, site } => Elaboration::NotAType { name, site },
            Elaboration::Unsupported { site } => Elaboration::Unsupported { site },
            Elaboration::Bound { site } => Elaboration::Bound { site },
            Elaboration::Quantified { site } => Elaboration::Quantified { site },
            Elaboration::MeetOverVariable { site } => Elaboration::MeetOverVariable { site },
            Elaboration::Unknown { site } => Elaboration::Unknown { site },
        }
    }

    pub fn shape_error(&self, error: ShapeError<'_>) -> ShapeError<'out> {
        use ShapeError as E;
        match error {
            E::Rebind {
                name,
                first,
                second,
            } => E::Rebind {
                name,
                first,
                second,
            },
            E::ShadowsBuiltin { name, at } => E::ShadowsBuiltin { name, at },
            E::Unbound { name, site, at } => E::Unbound { name, site, at },
            E::QuantifiedValue { at } => E::QuantifiedValue { at },
            E::Unfixed {
                variables,
                wanted,
                at,
            } => E::Unfixed {
                variables: self.copied(variables),
                wanted: wanted.map(|wanted| self.handle(wanted)),
                at,
            },
            E::NoInstance { scheme, wanted, at } => E::NoInstance {
                scheme: self.handle(scheme),
                wanted: self.handle(wanted),
                at,
            },
            E::AmbiguousInstance { key, at } => E::AmbiguousInstance {
                key: self.copied(key),
                at,
            },
            E::NoInstanceAtCandidates { key, at } => E::NoInstanceAtCandidates {
                key: self.copied(key),
                at,
            },
            E::QuantifiedRead { name, site, at } => E::QuantifiedRead { name, site, at },
            E::EagerCycle {
                members,
                definitions,
                at,
            } => E::EagerCycle {
                members: self.copied(members),
                definitions: self.run(definitions, |key| self.copied(key)),
                at,
            },
            E::MarkOutsideQuote { at } => E::MarkOutsideQuote { at },
            E::Unsupported { form, at } => E::Unsupported { form, at },
            E::Malformed { form, at } => E::Malformed { form, at },
            E::Unsurfaced { at, site } => E::Unsurfaced { at, site },
            E::Unchained { symbol, at } => E::Unchained { symbol, at },
            E::MixedGroups { first, second, at } => E::MixedGroups { first, second, at },
            E::RedeclaresGroup { symbol, at } => E::RedeclaresGroup { symbol, at },
            E::ResultOutsidePairwise { symbol, at } => E::ResultOutsidePairwise { symbol, at },
            E::SpellsForm { symbol, at } => E::SpellsForm { symbol, at },
            E::Derived { symbol, at } => E::Derived { symbol, at },
            E::Unquoted { form, part, at } => E::Unquoted { form, part, at },
            E::Inadmissible {
                form,
                index,
                slot,
                at,
            } => E::Inadmissible {
                form,
                index,
                slot: self.handle(slot),
                at,
            },
            E::DictDefault { site, at } => E::DictDefault { site, at },
            E::ClosedBucket { key, at } => E::ClosedBucket {
                key: self.copied(key),
                at,
            },
            E::NoKeyword { at } => E::NoKeyword { at },
            E::RankedDefinition { at } => E::RankedDefinition { at },
            E::NestedBinder { at } => E::NestedBinder { at },
            E::RankingDisagrees { key, at } => E::RankingDisagrees {
                key: self.copied(key),
                at,
            },
            E::NoCandidate { key, at } => E::NoCandidate {
                key: self.copied(key),
                at,
            },
            E::Overlaps { key, builtin, at } => E::Overlaps {
                key: self.copied(key),
                builtin: self.handle(builtin),
                at,
            },
            E::NoAdmittingCandidate { key, arguments, at } => E::NoAdmittingCandidate {
                key: self.copied(key),
                arguments: self.run(arguments, |argument| self.handle(*argument)),
                at,
            },
            E::NoField { of, field, at } => E::NoField {
                of: self.handle(of),
                field,
                at,
            },
            E::NoMember { of, member, at } => E::NoMember {
                of: self.handle(of),
                member,
                at,
            },
            E::Unlisted { name, at } => E::Unlisted { name, at },
            E::OpenPathInType { at } => E::OpenPathInType { at },
            E::UnpinnedMember {
                member,
                parameter,
                at,
            } => E::UnpinnedMember {
                member,
                parameter,
                at,
            },
            E::Ambiguous {
                key,
                arguments,
                count,
                at,
            } => E::Ambiguous {
                key: self.copied(key),
                arguments: self.run(arguments, |argument| self.handle(*argument)),
                count,
                at,
            },
            E::ReturnNeverSatisfied { body, returns, at } => E::ReturnNeverSatisfied {
                body: self.handle(body),
                returns: self.handle(returns),
                at,
            },
            E::AscriptionNeverSatisfied {
                value,
                ascribed,
                at,
            } => E::AscriptionNeverSatisfied {
                value: self.handle(value),
                ascribed: self.handle(ascribed),
                at,
            },
            E::AnnotationNeverSatisfied {
                value,
                annotated,
                at,
            } => E::AnnotationNeverSatisfied {
                value: self.handle(value),
                annotated: self.handle(annotated),
                at,
            },
            E::CallNeverSatisfied {
                callee,
                arguments,
                at,
            } => E::CallNeverSatisfied {
                callee: self.declared(callee),
                arguments: self.handle(arguments),
                at,
            },
            E::NotCode { value, at } => E::NotCode {
                value: self.handle(value),
                at,
            },
            E::NotAModule { value, at } => E::NotAModule {
                value: self.handle(value),
                at,
            },
            E::NotASignature { ascribed, at } => E::NotASignature {
                ascribed: self.handle(ascribed),
                at,
            },
            E::EvalNeverSatisfied { code, returns, at } => E::EvalNeverSatisfied {
                code: self.handle(code),
                returns: self.handle(returns),
                at,
            },
            E::Type { error, at } => E::Type {
                error: self.elaboration(error),
                at,
            },
            E::RepeatedGuard { guard, first, at } => E::RepeatedGuard {
                guard: self.handle(guard),
                first,
                at,
            },
        }
    }

    pub fn shape_error_ref(&self, error: &ShapeError<'_>) -> &'out ShapeError<'out> {
        resident(self.writer, self.shape_error(*error))
    }

    pub fn channel_settled(
        &self,
        ChannelSettled {
            declared,
            registered,
            expressions,
            callable,
            levels,
            residual,
            refusal,
        }: ChannelSettled<'_>,
    ) -> ChannelSettled<'out> {
        ChannelSettled {
            declared: self.keyed(declared, |typed| self.static_type(*typed)),
            registered: self.keyed(registered, |held| self.static_registered(*held)),
            expressions: self.keyed(expressions, |typed| self.static_type(*typed)),
            callable: callable.map(|callable| self.static_callable(callable)),
            levels: levels.map(|levels| self.run(levels, |level| self.handle(*level))),
            residual: self.residual(residual),
            refusal: refusal.map(|refusal| self.shape_error_ref(refusal)),
        }
    }

    pub fn statics_settled(
        &self,
        StaticsSettled { statics, residual }: StaticsSettled<'_>,
    ) -> StaticsSettled<'out> {
        StaticsSettled {
            statics: self.statics(statics),
            residual: self.residual(residual),
        }
    }

    pub fn settled(
        &self,
        Settled {
            binding,
            channel,
            statics,
            facts,
            born,
            refusals,
            resolutions,
            reran,
        }: Settled<'_>,
    ) -> Settled<'out> {
        Settled {
            binding: self.binding(binding),
            channel: self.keyed(channel, |held| self.channel_settled(*held)),
            statics: self.keyed(statics, |held| self.statics_settled(*held)),
            facts: self.keyed(facts, |held| self.shape_facts(*held)),
            born: self.keyed(born, |held| self.static_solution(*held)),
            refusals: self.keyed(refusals, |refusal| self.shape_error_ref(refusal)),
            resolutions: self.keyed(resolutions, |held| self.resolution(*held)),
            reran,
        }
    }
}
