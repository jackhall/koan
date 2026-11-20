//! The **records copier**: a unit's record or a closing laid again through another layer, so it
//! outlives the memory it was read from. A frame's activation walks at its step's borrow, over
//! facts a depot slot holds; what it decides is relaid into the frame's region, which no depot
//! node outlives the type system's view of; a closure holding those records lays them again into
//! the region a copy of it lands in. Every slice and tree a record reaches is laid again, so
//! nothing it holds points where it was read from; a frontier is laid flat, its unknowns past the
//! cut of each read, less those the binding binds, which no later event of the frame rebinds.
//!
//! Each record is destructured whole and each match lists every variant, as the
//! [type visitor](super::record_types)'s are, so a field a record gains does not compile until it
//! is relaid. Types are handles and copy as they are; what resolves them is the caller's to adopt.

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Lays, bump_table, collect, resident};

use super::facts::{Binding, Bound, Flattened, Frontier, Mark, Noted, OpenMark};
use super::keyed::Keyed;
use super::question::Answered;
use super::records::{Born, Closing, Deps, Retypes, SiteDeps, Traced, Typed, UnitFacts};
use super::shape::{Repeated, Scalar, ShapeError};
use super::tree::{
    Bare, Called, Callee, KnotNode, Leaf, NodeCapture, Sealing, Tree, UndecidedCall, Unfolded,
    Within,
};
use super::typed::{
    Arms, Callable, Elimination, FunctionGroupMap, InstanceRead, Kept, Narrowing, ParameterBinding,
    PathEntry, Pick, Points, Registered, Resolution, Route, ShapeGroupMap, Static, StaticInstance,
    UseStatics,
};

/// `error` laid again through `lay`, every slice it reaches copied: what a settlement, a
/// composition or an activation keeps of the refusal its walk reached. What resolves its types is
/// the caller's to adopt.
pub(crate) fn relaid_error<'to>(
    lay: impl Lays<'to>,
    scratch: BumpAllocator<'_>,
    error: &ShapeError<'_>,
) -> &'to ShapeError<'to> {
    let relay = Relay::new(lay, scratch, Binding::EMPTY);
    resident(lay, relay.error(error))
}

/// A copier laying records read at `'from` through `lay`, flattening frontiers under `binding`
/// through `flat`, a memo of each frontier's unknowns by identity in `scratch`.
pub(crate) struct Relay<'to, 'x, 'bind, 'from, L> {
    lay: L,
    scratch: BumpAllocator<'x>,
    binding: Binding<'bind>,
    flat: Flattened<'x, 'from>,
    /// Each answered question laid again, by where it was read: laid once per relay.
    answered: BumpBackedMap<'x, usize, &'to Answered<'to>>,
}

impl<'to, 'x, 'bind, 'from, L: Lays<'to>> Relay<'to, 'x, 'bind, 'from, L> {
    /// A copier laying through `lay`, whose frontiers drop what `binding` binds.
    pub(crate) fn new(lay: L, scratch: BumpAllocator<'x>, binding: Binding<'bind>) -> Self {
        Relay::flattening(lay, scratch, binding, Flattened::new(scratch))
    }

    /// A copier as [`new`](Self::new)'s, adopting `flat`: a frontier a walk's caller already
    /// flattened is not walked again.
    pub(crate) fn flattening(
        lay: L,
        scratch: BumpAllocator<'x>,
        binding: Binding<'bind>,
        flat: Flattened<'x, 'from>,
    ) -> Self {
        Relay {
            lay,
            scratch,
            binding,
            flat,
            answered: bump_table(scratch),
        }
    }

    /// This copier dropping what `binding` binds from the frontiers it lays from here on. What it
    /// flattened and laid is kept: neither depends on the binding.
    pub(crate) fn rebind(&mut self, binding: Binding<'bind>) {
        self.binding = binding;
    }

    /// `frontier` laid flat: each unknown it holds and its marks, past each read's cut, that the
    /// binding leaves unbound, sorted.
    pub(crate) fn frontier(&mut self, frontier: Frontier<'from>) -> Frontier<'to> {
        if frontier.is_empty() {
            return Frontier::EMPTY;
        }
        let flat = self.flat.of(frontier);
        let mut unbound = BumpVec::new_in(self.scratch);
        for each in flat {
            if self.binding.bound(each.unknown).is_none() {
                unbound.push(noted(self.lay, &mut self.answered, *each));
            }
        }
        let laid = collect(self.lay, unbound.iter().copied());
        match laid.is_empty() {
            true => Frontier::EMPTY,
            false => Frontier::from_sorted(laid),
        }
    }

    /// `answered` laid again, once per relay.
    pub(crate) fn answered(&mut self, answered: &Answered<'_>) -> &'to Answered<'to> {
        relaid_answered(self.lay, &mut self.answered, answered)
    }

    /// `facts`, laid again.
    pub(crate) fn unit(&mut self, facts: &UnitFacts<'from>) -> UnitFacts<'to> {
        let UnitFacts {
            typed,
            born,
            paths,
            sites,
            named_variables,
            seeds,
        } = facts;
        let seeds = seeds.entries();
        let mut flat = BumpVec::with_capacity_in(seeds.len(), self.scratch);
        for (slot, seed) in seeds {
            flat.push((*slot, self.frontier(*seed)));
        }
        let seeds = Keyed::from_sorted(collect(self.lay, flat.iter().copied()));
        UnitFacts {
            typed: self.keyed(*typed, |typed| self.typed(typed)),
            born: self.keyed(*born, |born| self.born(born)),
            paths: self.keyed(*paths, |(entry, resolution)| {
                (self.path(entry), self.resolution(resolution))
            }),
            sites: self.lay.fill(sites.len(), |at| self.site(&sites[at])),
            named_variables: self.copied(named_variables),
            seeds,
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
            retypes,
        } = closing;
        Closing {
            class: *class,
            summary: summary.map(|summary| summary.map(|tree| self.tree(&tree))),
            recursive: *recursive,
            ran: *ran,
            refusal: refusal.map(|error| resident(self.lay, self.error(error))),
            overlap: overlap.map(|error| resident(self.lay, self.error(error))),
            retypes: Retypes {
                parameters: self.copied(retypes.parameters),
                returns: retypes.returns,
            },
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

    /// `bare`, its record's hidden carriers laid again.
    fn bare(&self, bare: &Bare<'_>) -> Bare<'to> {
        Bare {
            signature: bare.signature,
            sealing: bare.sealing.map(|sealing| Sealing {
                record: sealing.record.laid(self.lay),
                ..sealing
            }),
        }
    }

    /// `tree`, every node laid again.
    pub(crate) fn tree(&self, tree: &Tree<'_>) -> Tree<'to> {
        match tree {
            Tree::Digest(digest, bare) => Tree::Digest(
                *digest,
                bare.map(|bare| resident(self.lay, self.bare(bare))),
            ),
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
                application,
                source,
                underlying,
                identity,
            } => Tree::View {
                application: *application,
                source: resident(self.lay, self.tree(source)),
                underlying: resident(self.lay, self.tree(underlying)),
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
            Tree::TypeValue { named, identity } => Tree::TypeValue {
                named: *named,
                identity: *identity,
            },
            Tree::Leaf(Leaf::Call {
                call,
                holder,
                chain,
                identity,
            }) => Tree::Leaf(Leaf::Call {
                call: resident(
                    self.lay,
                    Called {
                        at: call.at,
                        callees: self.lay.fill(call.callees.len(), |at| Callee {
                            parameters: self.copied(call.callees[at].parameters),
                            ..call.callees[at]
                        }),
                        arguments: self
                            .lay
                            .fill(call.arguments.len(), |at| self.tree(&call.arguments[at])),
                        undecided: self.undecided(&call.undecided),
                    },
                ),
                holder: *holder,
                chain: self.copied(chain),
                identity: *identity,
            }),
            Tree::Leaf(Leaf::Unnamed(site)) => Tree::Leaf(Leaf::Unnamed(*site)),
        }
    }

    /// Each `(slot, tree)` laid again.
    /// What a use an answer selects among keeps, laid again.
    fn undecided(&self, undecided: &UndecidedCall<'_>) -> UndecidedCall<'to> {
        UndecidedCall {
            kept: self
                .lay
                .fill(undecided.kept.len(), |at| undecided.kept[at].laid(self.lay)),
            positions: self.copied(undecided.positions),
            unfilled: undecided.unfilled,
        }
    }

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
            traced,
            by_name,
            routes,
            arms,
            retypes,
            trees,
            picks,
            contents,
            holding,
            minting,
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
                InstanceRead::Solved(solved) => InstanceRead::Solved(self.instance(solved)),
                InstanceRead::AsIs => InstanceRead::AsIs,
            }),
            uses: self.keyed(*uses, |statics| self.uses(statics)),
            selections: self.keyed(*selections, |points| self.points(points)),
            traced: self.keyed(*traced, |traced| Traced {
                callee: traced.callee,
                call: traced.call,
                by_name: traced.by_name,
                fixed: self.copied(traced.fixed),
            }),
            by_name: self.keyed(*by_name, |contributed| {
                self.lay
                    .fill(contributed.len(), |at| self.static_type(&contributed[at]))
            }),
            routes: self.keyed(*routes, |route| Route {
                per: self.copied(route.per),
            }),
            arms: self.keyed(*arms, |arms| Arms {
                kept: self.copied(arms.kept),
                ranking: match arms.ranking {
                    Elimination::Open => Elimination::Open,
                    Elimination::Table(table) => Elimination::Table(table.relaid(self.lay)),
                },
                chosen: arms.chosen,
            }),
            retypes: self.keyed_copied(*retypes),
            trees: self.keyed(*trees, |tree| resident(self.lay, self.tree(tree))),
            picks: self.keyed(*picks, |picks| {
                self.lay.fill(picks.len(), |at| Pick {
                    member: picks[at].member,
                    offered: self.copied(picks[at].offered),
                })
            }),
            contents: self.keyed(*contents, |tree| self.tree(tree)),
            holding: self.keyed(*holding, |held| self.copied(held)),
            minting: self.keyed_copied(*minting),
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
            instance: instance.as_ref().map(|solved| self.instance(solved)),
            digestible: self.copied(digestible),
        }
    }

    fn path(&self, entry: &PathEntry<'_>) -> PathEntry<'to> {
        let PathEntry {
            root,
            chain,
            label,
            name,
            through,
        } = entry;
        PathEntry {
            root: *root,
            chain: self.copied(chain),
            label: *label,
            name: *name,
            through: *through,
        }
    }

    fn resolution(&self, resolution: &Resolution<'_>) -> Resolution<'to> {
        let Resolution {
            tree,
            resolved,
            lacks,
        } = resolution;
        Resolution {
            tree: self.tree(tree),
            resolved: *resolved,
            lacks: *lacks,
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
            pending: (self.lay).fill(pending.len(), |at| self.tree(&pending[at])),
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

    fn instance(&self, instance: &StaticInstance<'_>) -> StaticInstance<'to> {
        match instance {
            StaticInstance::Closed { solution, ktype } => StaticInstance::Closed {
                solution: self.copied(solution),
                ktype: *ktype,
            },
            StaticInstance::Rigid {
                solution,
                ktype,
                variables,
            } => StaticInstance::Rigid {
                solution: self.copied(solution),
                ktype: *ktype,
                variables: self.copied(variables),
            },
        }
    }

    fn uses(&self, statics: &UseStatics<'_>) -> UseStatics<'to> {
        let UseStatics {
            narrowing,
            contributions,
            undecided,
        } = statics;
        UseStatics {
            undecided: undecided.map(|undecided| resident(self.lay, self.undecided(undecided))),
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
            ShapeError::NoAdmittingCandidate {
                key,
                arguments,
                listed,
                shapes,
                at,
            } => ShapeError::NoAdmittingCandidate {
                key: self.copied(key),
                arguments: self.copied(arguments),
                listed: self.copied(listed),
                shapes: self.copied(shapes),
                at,
            },
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
            ShapeError::ReturnNeverSatisfied { refused, at } => {
                ShapeError::ReturnNeverSatisfied { refused, at }
            }
            ShapeError::AscriptionNeverSatisfied { refused, at } => {
                ShapeError::AscriptionNeverSatisfied { refused, at }
            }
            ShapeError::ViewReveals {
                member,
                ascribed,
                at,
            } => ShapeError::ViewReveals {
                member,
                ascribed,
                at,
            },
            ShapeError::AnnotationNeverSatisfied { refused, at } => {
                ShapeError::AnnotationNeverSatisfied { refused, at }
            }
            ShapeError::CallNeverSatisfied {
                callee,
                payload,
                parameters,
                at,
            } => ShapeError::CallNeverSatisfied {
                callee,
                payload,
                parameters,
                at,
            },
            ShapeError::ReadNeverSatisfied {
                container,
                argument,
                at,
            } => ShapeError::ReadNeverSatisfied {
                container,
                argument,
                at,
            },
            ShapeError::SpreadNeverAList { value, at } => {
                ShapeError::SpreadNeverAList { value, at }
            }
            ShapeError::SpliceNeverCode { value, at } => ShapeError::SpliceNeverCode { value, at },
            ShapeError::SpliceInKnot { at } => ShapeError::SpliceInKnot { at },
            ShapeError::NotAGuard { at } => ShapeError::NotAGuard { at },
            ShapeError::NoAdmittingArm {
                scrutinee,
                union,
                at,
            } => ShapeError::NoAdmittingArm {
                scrutinee,
                union,
                at,
            },
            ShapeError::AmbiguousArms {
                count,
                scrutinee,
                at,
            } => ShapeError::AmbiguousArms {
                count,
                scrutinee,
                at,
            },
            ShapeError::NotUnion { clause, at } => ShapeError::NotUnion { clause, at },
            ShapeError::NoVariant { label, union, at } => {
                ShapeError::NoVariant { label, union, at }
            }
            ShapeError::Uncovered { union, variant, at } => {
                ShapeError::Uncovered { union, variant, at }
            }
            ShapeError::LoneVariant { name, at } => ShapeError::LoneVariant { name, at },
            ShapeError::NotCode { refused, at } => ShapeError::NotCode { refused, at },
            ShapeError::BodyNotCode { refused, at } => ShapeError::BodyNotCode { refused, at },
            ShapeError::ParameterOutsideBody { name, at } => {
                ShapeError::ParameterOutsideBody { name, at }
            }
            ShapeError::NotAModule { refused, at } => ShapeError::NotAModule { refused, at },
            ShapeError::NotASignature { ascribed, at } => {
                ShapeError::NotASignature { ascribed, at }
            }
            ShapeError::EvalNeverSatisfied { refused, at } => {
                ShapeError::EvalNeverSatisfied { refused, at }
            }
            ShapeError::Type { error, at } => ShapeError::Type { error, at },
            ShapeError::UnderCall { call, refusal } => ShapeError::UnderCall {
                call,
                refusal: resident(self.lay, self.error(refusal)),
            },
            ShapeError::RepeatedGuard { guard, first, at } => ShapeError::RepeatedGuard {
                guard: match guard {
                    Repeated::Type(handle) => Repeated::Type(handle),
                    Repeated::Value(Scalar::Str(text)) => Repeated::Value(Scalar::Str(
                        std::str::from_utf8(self.copied(text.as_bytes()))
                            .expect("a copy of a string is one"),
                    )),
                    Repeated::Value(Scalar::Number(bits)) => Repeated::Value(Scalar::Number(bits)),
                    Repeated::Value(Scalar::Bool(truth)) => Repeated::Value(Scalar::Bool(truth)),
                    Repeated::Value(Scalar::Null) => Repeated::Value(Scalar::Null),
                    Repeated::Label(label) => Repeated::Label(label),
                    Repeated::Default => Repeated::Default,
                },
                first,
                at,
            },
        }
    }
}

/// `noted`, its answered question laid again through `lay` once per `laid`.
fn noted<'to>(
    lay: impl Lays<'to>,
    laid: &mut BumpBackedMap<'_, usize, &'to Answered<'to>>,
    noted: Noted<'_>,
) -> Noted<'to> {
    let mark = match noted.mark {
        Mark::Named => Mark::Named,
        Mark::Open(open) => Mark::Open(OpenMark {
            kind: open.kind,
            slot: open.slot,
            judged: open.judged,
            answered: open
                .answered
                .map(|answered| relaid_answered(lay, laid, answered)),
        }),
    };
    Noted {
        unknown: noted.unknown,
        mark,
    }
}

/// `answered` laid again through `lay`, once per `laid`, by where it was read.
fn relaid_answered<'to>(
    lay: impl Lays<'to>,
    laid: &mut BumpBackedMap<'_, usize, &'to Answered<'to>>,
    answered: &Answered<'_>,
) -> &'to Answered<'to> {
    let at = std::ptr::from_ref(answered).addr();
    if let Some(held) = laid.get(&at) {
        return held;
    }
    let held = resident(lay, answered.laid(lay));
    laid.insert(at, held);
    held
}
