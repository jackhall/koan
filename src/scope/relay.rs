//! The **records copier**: a unit's record or a closing laid again through another layer, so it
//! outlives the memory it was read from. A frame's activation walks at its step's borrow, over
//! facts an arena node holds; what it decides is relaid into the frame's region, which no arena
//! node outlives the type system's view of; a closure holding those records lays them again into
//! the region a copy of it lands in. Every slice and tree a record reaches is laid again, so
//! nothing it holds points where it was read from; a frontier is laid flat, its unknowns past the
//! cut of each read, less those the binding binds, which no later event of the frame rebinds.
//!
//! Each record is destructured whole and each match lists every variant, as the
//! [type visitor](super::record_types)'s are, so a field a record gains does not compile until it
//! is relaid. Types are handles and copy as they are; what resolves them is the caller's to adopt.

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Lays, bump_table, collect, resident};

use super::facts::{Binding, Bound, Frontier, Unknown};
use super::keyed::Keyed;
use super::records::{Born, Closing, Deps, SiteDeps, Typed, UnitFacts};
use super::shape::ShapeError;
use super::tree::{KnotNode, Leaf, NodeCapture, Tree, Unfolded, Within};
use super::typed::{
    Callable, Elimination, FunctionGroupMap, InstanceRead, Kept, Narrowing, ParameterBinding,
    PathEntry, Points, Registered, Resolution, ShapeGroupMap, Static, StaticSolution, UseStatics,
};

/// A copier laying records through `lay`, flattening frontiers under `binding` with a memo of
/// each frontier's unknowns, by identity, in `scratch`.
pub(crate) struct Relay<'to, 'x, 'bind, L> {
    lay: L,
    scratch: BumpAllocator<'x>,
    binding: Binding<'bind>,
    flat: BumpBackedMap<'x, (usize, usize), &'x [Unknown]>,
    _to: std::marker::PhantomData<&'to ()>,
}

impl<'to, 'x, 'bind, L: Lays<'to>> Relay<'to, 'x, 'bind, L> {
    /// A copier laying through `lay`, whose frontiers drop what `binding` binds.
    pub(crate) fn new(lay: L, scratch: BumpAllocator<'x>, binding: Binding<'bind>) -> Self {
        Relay {
            lay,
            scratch,
            binding,
            flat: bump_table(scratch),
            _to: std::marker::PhantomData,
        }
    }

    /// `frontier` laid flat: each unknown it holds, past each read's cut, that the binding leaves
    /// unbound, sorted.
    pub(crate) fn frontier(&mut self, frontier: Frontier<'_>) -> Frontier<'to> {
        if frontier.is_empty() {
            return Frontier::EMPTY;
        }
        let mut stack = BumpVec::new_in(self.scratch);
        stack.push((frontier, false));
        while let Some((at, expanded)) = stack.pop() {
            if self.flat.contains_key(&at.identity()) {
                continue;
            }
            if !expanded {
                stack.push((at, true));
                stack.extend((at.reads().iter()).map(|cut| (cut.frontier, false)));
                continue;
            }
            let mut unknowns = BumpVec::new_in(self.scratch);
            unknowns.extend_from_slice(at.own());
            for cut in at.reads() {
                let read = self.flat[&cut.frontier.identity()];
                unknowns.extend(read.iter().filter(|unknown| unknown.segment >= cut.from));
            }
            unknowns.sort_unstable();
            unknowns.dedup();
            let laid = collect(self.scratch, unknowns.iter().copied());
            self.flat.insert(at.identity(), laid);
        }
        let mut unbound = BumpVec::new_in(self.scratch);
        unbound.extend(
            (self.flat[&frontier.identity()].iter())
                .filter(|unknown| self.binding.bound(**unknown).is_none()),
        );
        let laid = collect(self.lay, unbound.iter().copied());
        match laid.is_empty() {
            true => Frontier::EMPTY,
            false => Frontier::from_sorted(laid),
        }
    }

    /// `facts`, laid again.
    pub(crate) fn unit(&self, facts: &UnitFacts<'_>) -> UnitFacts<'to> {
        let UnitFacts {
            typed,
            born,
            paths,
            sites,
            named_variables,
        } = facts;
        UnitFacts {
            typed: self.keyed(*typed, |typed| self.typed(typed)),
            born: self.keyed(*born, |born| self.born(born)),
            paths: self.keyed(*paths, |(entry, resolution)| {
                (self.path(entry), self.resolution(resolution))
            }),
            sites: self.lay.fill(sites.len(), |at| self.site(&sites[at])),
            named_variables: self.copied(named_variables),
        }
    }

    /// `closing`, laid again.
    pub(crate) fn closing(&self, closing: &Closing<'_>) -> Closing<'to> {
        let Closing {
            class,
            summary,
            recursive,
            ran,
            refusal,
            overlap,
        } = closing;
        Closing {
            class: *class,
            summary: summary.map(|summary| summary.map(|tree| self.tree(&tree))),
            recursive: *recursive,
            ran: *ran,
            refusal: refusal.map(|error| resident(self.lay, self.error(error))),
            overlap: overlap.map(|error| resident(self.lay, self.error(error))),
        }
    }

    /// `bound`, laid again.
    pub(crate) fn bound(&self, bound: &Bound<'_>) -> Bound<'to> {
        match bound {
            Bound::Type(ktype) => Bound::Type(*ktype),
            Bound::Module { carried, tree } => Bound::Module {
                carried: *carried,
                tree: resident(self.lay, self.tree(tree)),
            },
        }
    }

    /// `tree`, every node laid again.
    pub(crate) fn tree(&self, tree: &Tree<'_>) -> Tree<'to> {
        match tree {
            Tree::Digest(digest) => Tree::Digest(*digest),
            Tree::Body {
                body,
                captures,
                identity,
            } => Tree::Body {
                body: *body,
                captures: self.trees(captures),
                identity: *identity,
            },
            Tree::Knot {
                members,
                index,
                identity,
            } => Tree::Knot {
                members: self.lay.fill(members.len(), |at| {
                    let KnotNode {
                        body,
                        instance,
                        captures,
                    } = &members[at];
                    KnotNode {
                        body: *body,
                        instance: self.copied(instance),
                        captures: self.lay.fill(captures.len(), |at| {
                            let (slot, capture) = &captures[at];
                            let capture = match capture {
                                NodeCapture::Fellow(fellow) => NodeCapture::Fellow(*fellow),
                                NodeCapture::Tree(tree) => NodeCapture::Tree(self.tree(tree)),
                            };
                            (*slot, capture)
                        }),
                    }
                }),
                index: *index,
                identity: *identity,
            },
            Tree::View {
                operator,
                application,
                source,
                identity,
            } => Tree::View {
                operator: *operator,
                application: *application,
                source: resident(self.lay, self.tree(source)),
                identity: *identity,
            },
            Tree::Retyped {
                carried,
                source,
                identity,
            } => Tree::Retyped {
                carried: *carried,
                source: resident(self.lay, self.tree(source)),
                identity: *identity,
            },
            Tree::Leaf(Leaf::Bound {
                binder,
                chain,
                within,
                identity,
            }) => Tree::Leaf(Leaf::Bound {
                binder: *binder,
                chain: self.copied(chain),
                within: within.map(|within| {
                    let within = match within {
                        Within::Body(captures) => Within::Body(self.trees(captures)),
                        Within::Call(Unfolded {
                            callee,
                            arguments,
                            solution,
                        }) => Within::Call(Unfolded {
                            callee: *callee,
                            arguments: self.lay.fill(arguments.len(), |at| {
                                (arguments[at].0, self.tree(&arguments[at].1))
                            }),
                            solution: self.copied(solution),
                        }),
                    };
                    resident(self.lay, within)
                }),
                identity: *identity,
            }),
            Tree::Leaf(Leaf::Unnamed(site)) => Tree::Leaf(Leaf::Unnamed(*site)),
        }
    }

    /// Each `(slot, tree)` laid again.
    fn trees(&self, trees: &[(u32, Tree<'_>)]) -> &'to [(u32, Tree<'to>)] {
        self.lay
            .fill(trees.len(), |at| (trees[at].0, self.tree(&trees[at].1)))
    }

    /// A slice of plain data laid again.
    fn copied<T: Copy>(&self, values: &[T]) -> &'to [T] {
        collect(self.lay, values.iter().copied())
    }

    /// A keyed table laid again, each value through `value`.
    fn keyed<K: Ord + Copy, A, B>(
        &self,
        table: Keyed<'_, (K, A)>,
        value: impl Fn(&A) -> B,
    ) -> Keyed<'to, (K, B)> {
        let entries = table.entries();
        Keyed::from_sorted(
            self.lay
                .fill(entries.len(), |at| (entries[at].0, value(&entries[at].1))),
        )
    }

    /// A keyed table of plain data laid again.
    fn keyed_copied<T: super::keyed::KeyedEntry + Copy>(
        &self,
        table: Keyed<'_, T>,
    ) -> Keyed<'to, T> {
        Keyed::from_sorted(self.copied(table.entries()))
    }

    fn typed(&self, typed: &Typed<'_>) -> Typed<'to> {
        let Typed {
            declared,
            registered,
            expressions,
            parts,
            statements,
            binders,
            settled,
            instances,
            uses,
            selections,
            by_name,
            trees,
            contents,
            minting,
            throughs,
            signature,
            exits,
            closed,
            folded,
        } = typed;
        Typed {
            declared: self.keyed(*declared, |typed| self.static_type(typed)),
            registered: self.keyed(*registered, |typed| {
                self.statik(
                    typed,
                    |closed| self.registered(closed),
                    |rigid| self.registered(rigid),
                )
            }),
            expressions: self.keyed(*expressions, |typed| self.static_type(typed)),
            parts: self.keyed_copied(*parts),
            statements: self.keyed_copied(*statements),
            binders: self.keyed_copied(*binders),
            settled: self.keyed_copied(*settled),
            instances: self.keyed(*instances, |read| match read {
                InstanceRead::Solved(solved) => InstanceRead::Solved(self.solution(solved)),
                InstanceRead::AsIs => InstanceRead::AsIs,
            }),
            uses: self.keyed(*uses, |statics| self.uses(statics)),
            selections: self.keyed(*selections, |points| self.points(points)),
            by_name: self.keyed(*by_name, |contributed| {
                self.lay
                    .fill(contributed.len(), |at| self.static_type(&contributed[at]))
            }),
            trees: self.keyed(*trees, |tree| resident(self.lay, self.tree(tree))),
            contents: self.keyed(*contents, |tree| self.tree(tree)),
            minting: self.keyed_copied(*minting),
            throughs: self.keyed(*throughs, |variables| self.copied(variables)),
            signature: *signature,
            exits: *exits,
            closed: *closed,
            folded: *folded,
        }
    }

    fn born(&self, born: &Born<'_>) -> Born<'to> {
        let Born {
            callable,
            levels,
            instance,
            digestible,
        } = born;
        Born {
            callable: self.statik(
                callable,
                |closed| self.callable(closed),
                |rigid| self.callable(rigid),
            ),
            levels: self.copied(levels),
            instance: instance.as_ref().map(|solved| self.solution(solved)),
            digestible: self.copied(digestible),
        }
    }

    fn path(&self, entry: &PathEntry<'_>) -> PathEntry<'to> {
        let PathEntry {
            root,
            chain,
            label,
            level,
            through,
        } = entry;
        PathEntry {
            root: *root,
            chain: self.copied(chain),
            label: *label,
            level: *level,
            through: *through,
        }
    }

    fn resolution(&self, resolution: &Resolution<'_>) -> Resolution<'to> {
        let Resolution { tree, resolved } = resolution;
        Resolution {
            tree: self.tree(tree),
            resolved: *resolved,
        }
    }

    fn site(&self, site: &SiteDeps<'_>) -> SiteDeps<'to> {
        let SiteDeps {
            holder,
            frame,
            site,
            minting,
            deps,
            pending,
        } = site;
        SiteDeps {
            holder: *holder,
            frame: *frame,
            site: *site,
            minting: *minting,
            deps: match deps {
                Deps::Closed => Deps::Closed,
                Deps::On(slots) => Deps::On(self.copied(slots)),
                Deps::Varying => Deps::Varying,
            },
            pending: self.copied(pending),
        }
    }

    fn static_type<C: Copy, R: Copy>(&self, typed: &Static<'_, C, R>) -> Static<'to, C, R> {
        self.statik(typed, |closed| *closed, |rigid| *rigid)
    }

    fn statik<C, R, C2, R2>(
        &self,
        typed: &Static<'_, C, R>,
        closed: impl FnOnce(&C) -> C2,
        rigid: impl FnOnce(&R) -> R2,
    ) -> Static<'to, C2, R2> {
        match typed {
            Static::Unknown => Static::Unknown,
            Static::Closed(value) => Static::Closed(closed(value)),
            Static::Rigid { value, variables } => Static::Rigid {
                value: rigid(value),
                variables: self.copied(variables),
            },
        }
    }

    fn callable<T: Copy>(&self, typed: &Callable<'_, T>) -> Callable<'to, T> {
        let Callable {
            ktype,
            quantifier_map: FunctionGroupMap(map),
            registered,
        } = typed;
        Callable {
            ktype: *ktype,
            quantifier_map: FunctionGroupMap(self.copied(map)),
            registered: registered
                .as_ref()
                .map(|registered| self.registered(registered)),
        }
    }

    fn registered<T: Copy>(&self, typed: &Registered<'_, T>) -> Registered<'to, T> {
        let Registered {
            shape,
            quantifier_map: ShapeGroupMap(map),
            parameters,
        } = typed;
        Registered {
            shape: *shape,
            quantifier_map: ShapeGroupMap(self.copied(map)),
            parameters: match parameters {
                ParameterBinding::Named(names) => ParameterBinding::Named(self.copied(names)),
                ParameterBinding::Operands => ParameterBinding::Operands,
            },
        }
    }

    fn solution(&self, solved: &StaticSolution<'_>) -> StaticSolution<'to> {
        match solved {
            StaticSolution::Closed(types) => StaticSolution::Closed(self.copied(types)),
            StaticSolution::Rigid { value, variables } => StaticSolution::Rigid {
                value: self.copied(value),
                variables: self.copied(variables),
            },
        }
    }

    fn uses(&self, statics: &UseStatics<'_>) -> UseStatics<'to> {
        let UseStatics {
            narrowing,
            contributions,
        } = statics;
        UseStatics {
            narrowing: match narrowing {
                Narrowing::Full => Narrowing::Full,
                Narrowing::Kept(Kept {
                    entries,
                    points,
                    elimination,
                }) => Narrowing::Kept(resident(
                    self.lay,
                    Kept {
                        entries: self.copied(entries),
                        points: self.lay.fill(points.len(), |at| self.points(&points[at])),
                        elimination: match elimination {
                            Elimination::Open => Elimination::Open,
                            Elimination::Table(table) => Elimination::Table(table.relaid(self.lay)),
                        },
                    },
                )),
                Narrowing::Selected(at) => Narrowing::Selected(*at),
            },
            contributions: self.lay.fill(contributions.len(), |at| {
                self.static_type(&contributions[at])
            }),
        }
    }

    /// `points`, laid again.
    fn points(&self, points: &Points<'_>) -> Points<'to> {
        let Points { points, variables } = points;
        Points {
            points: self.copied(points),
            variables: self.copied(variables),
        }
    }

    /// `error`, laid again.
    fn error(&self, error: &ShapeError<'_>) -> ShapeError<'to> {
        match *error {
            ShapeError::Rebind {
                name,
                first,
                second,
            } => ShapeError::Rebind {
                name,
                first,
                second,
            },
            ShapeError::ShadowsBuiltin { name, at } => ShapeError::ShadowsBuiltin { name, at },
            ShapeError::Unbound { name, site, at } => ShapeError::Unbound { name, site, at },
            ShapeError::QuantifiedValue { at } => ShapeError::QuantifiedValue { at },
            ShapeError::Unfixed {
                variables,
                wanted,
                at,
            } => ShapeError::Unfixed {
                variables: self.copied(variables),
                wanted,
                at,
            },
            ShapeError::NoInstance { scheme, wanted, at } => {
                ShapeError::NoInstance { scheme, wanted, at }
            }
            ShapeError::AmbiguousInstance { key, at } => ShapeError::AmbiguousInstance {
                key: self.copied(key),
                at,
            },
            ShapeError::NoInstanceAtCandidates { key, at } => ShapeError::NoInstanceAtCandidates {
                key: self.copied(key),
                at,
            },
            ShapeError::QuantifiedRead { name, site, at } => {
                ShapeError::QuantifiedRead { name, site, at }
            }
            ShapeError::EagerCycle {
                members,
                definitions,
                at,
            } => ShapeError::EagerCycle {
                members: self.copied(members),
                definitions: self
                    .lay
                    .fill(definitions.len(), |index| self.copied(definitions[index])),
                at,
            },
            ShapeError::MarkOutsideQuote { at } => ShapeError::MarkOutsideQuote { at },
            ShapeError::Unsupported { form, at } => ShapeError::Unsupported { form, at },
            ShapeError::Malformed { form, at } => ShapeError::Malformed { form, at },
            ShapeError::Unsurfaced { at, site } => ShapeError::Unsurfaced { at, site },
            ShapeError::Unchained { symbol, at } => ShapeError::Unchained { symbol, at },
            ShapeError::MixedGroups { first, second, at } => {
                ShapeError::MixedGroups { first, second, at }
            }
            ShapeError::RedeclaresGroup { symbol, at } => {
                ShapeError::RedeclaresGroup { symbol, at }
            }
            ShapeError::ResultOutsidePairwise { symbol, at } => {
                ShapeError::ResultOutsidePairwise { symbol, at }
            }
            ShapeError::SpellsForm { symbol, at } => ShapeError::SpellsForm { symbol, at },
            ShapeError::Derived { symbol, at } => ShapeError::Derived { symbol, at },
            ShapeError::Unquoted { form, part, at } => ShapeError::Unquoted { form, part, at },
            ShapeError::Inadmissible {
                form,
                index,
                slot,
                at,
            } => ShapeError::Inadmissible {
                form,
                index,
                slot,
                at,
            },
            ShapeError::DictDefault { site, at } => ShapeError::DictDefault { site, at },
            ShapeError::ClosedBucket { key, at } => ShapeError::ClosedBucket {
                key: self.copied(key),
                at,
            },
            ShapeError::NoKeyword { at } => ShapeError::NoKeyword { at },
            ShapeError::RankedDefinition { at } => ShapeError::RankedDefinition { at },
            ShapeError::NestedBinder { at } => ShapeError::NestedBinder { at },
            ShapeError::RankingDisagrees { key, at } => ShapeError::RankingDisagrees {
                key: self.copied(key),
                at,
            },
            ShapeError::NoCandidate { key, at } => ShapeError::NoCandidate {
                key: self.copied(key),
                at,
            },
            ShapeError::Overlaps { key, builtin, at } => ShapeError::Overlaps {
                key: self.copied(key),
                builtin,
                at,
            },
            ShapeError::NoAdmittingCandidate { key, arguments, at } => {
                ShapeError::NoAdmittingCandidate {
                    key: self.copied(key),
                    arguments: self.copied(arguments),
                    at,
                }
            }
            ShapeError::NoField { of, field, at } => ShapeError::NoField { of, field, at },
            ShapeError::NoMember { of, member, at } => ShapeError::NoMember { of, member, at },
            ShapeError::Unlisted { name, at } => ShapeError::Unlisted { name, at },
            ShapeError::OpenPathInType { at } => ShapeError::OpenPathInType { at },
            ShapeError::UnpinnedMember {
                member,
                parameter,
                at,
            } => ShapeError::UnpinnedMember {
                member,
                parameter,
                at,
            },
            ShapeError::Ambiguous {
                key,
                arguments,
                count,
                at,
            } => ShapeError::Ambiguous {
                key: self.copied(key),
                arguments: self.copied(arguments),
                count,
                at,
            },
            ShapeError::ReturnNeverSatisfied { body, returns, at } => {
                ShapeError::ReturnNeverSatisfied { body, returns, at }
            }
            ShapeError::AscriptionNeverSatisfied {
                value,
                ascribed,
                at,
            } => ShapeError::AscriptionNeverSatisfied {
                value,
                ascribed,
                at,
            },
            ShapeError::AnnotationNeverSatisfied {
                value,
                annotated,
                at,
            } => ShapeError::AnnotationNeverSatisfied {
                value,
                annotated,
                at,
            },
            ShapeError::CallNeverSatisfied {
                callee,
                arguments,
                at,
            } => ShapeError::CallNeverSatisfied {
                callee,
                arguments,
                at,
            },
            ShapeError::NotCode { value, at } => ShapeError::NotCode { value, at },
            ShapeError::NotAModule { value, at } => ShapeError::NotAModule { value, at },
            ShapeError::NotASignature { ascribed, at } => {
                ShapeError::NotASignature { ascribed, at }
            }
            ShapeError::EvalNeverSatisfied { code, returns, at } => {
                ShapeError::EvalNeverSatisfied { code, returns, at }
            }
            ShapeError::Type { error, at } => ShapeError::Type { error, at },
            ShapeError::RepeatedGuard { guard, first, at } => {
                ShapeError::RepeatedGuard { guard, first, at }
            }
        }
    }
}
