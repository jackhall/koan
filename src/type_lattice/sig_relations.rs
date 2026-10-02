//! *Fits* over two signature types, the two binder relations *fits*' instantiation clauses read,
//! and the specificity verdict a dispatch ranks candidates by.
//!
//! [`sig_fits`] is *fits* for signatures: `offered` fits `asked` when, for each application `asked`
//! holds ([`signatures`](super::signatures)), the offered members — pooled across the offered
//! applications — supply every member the asked one names: each manifest member equal, each value
//! slot under the declared type, each keyworded member satisfied by some overload at its key, and
//! each operator record covered at an equal mode. An application's unpinned head parameters are
//! solved first, as a call solves its group: pass A pools what the asked members contribute, one
//! offered overload per keyworded member and tried in turn, and takes the least instance; pass C
//! checks every member under that solution. A member's own `FOR ALL` variable is solved afresh on
//! the offered side and held rigid on the asked one.

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, KeywordSymbol, TypeSymbol, ValueSymbol};

use super::handle::{DeclaredType, Handle, KType, Parametric, TypeHandle, wrap};
use super::lattice::meet_through_variables;
use super::node::TypeNode;
use super::operators::ReductionMode;
use super::order::{fits, satisfied_by};
use super::ranking::{Ranked, STAND_IN_LEVEL, admits_by_class, class_at_least};
use super::registry::TypeRegistry;
use super::schema::{
    DeclaredGroup, Members, SigOrigin, SigSchema, elements_key_equal, keys_equal, shape_classes,
    shape_quantifiers, shape_return, shape_slots,
};
use super::shape::Specificity;
use super::signatures::{Application, applications, applications_under};
use super::substitute::{instantiate_quantified, substitute_parameters};
use super::unify::{Collector, UnifyFailure, admits};
use super::walk::Variance;

// --- Specificity ---

/// Whether `declared` admits `candidate` class by class — `declared`'s variables solved,
/// `candidate`'s rigid — then `declared`'s return under `candidate`'s: *fits*' instantiation
/// clause for two shapes.
///
/// Prenex instantiation through the collector: each slot pair asks the candidate's slot to lie
/// under the declared one (covariant for the collector, since a slot's own polarity is
/// contravariant), class by class, then the return pair asks the declared return to lie under the
/// candidate's. Since each candidate slot stands for every type a call carries under it, a later
/// class reads an earlier variable at its reach interval, not at the point a call would pin
/// (README § Priority classes). The candidate's `Quantified` nodes fall to the rigid rule
/// automatically, because the collector only ever solves declared-side variables and the carried
/// side is never substituted. Two things that are not both shapes under one key and one ranking
/// admit nothing.
pub(super) fn admits_shape(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: Handle,
    candidate: Handle,
) -> bool {
    let (Some(declared), Some(candidate)) =
        (Ranked::of(types, declared), Ranked::of(types, candidate))
    else {
        return false;
    };
    if !elements_key_equal(declared.elements, candidate.elements)
        || declared.classes != candidate.classes
    {
        return false;
    }
    admits_by_class(types, scratch, declared, candidate)
}

/// Whether `declared` admits `candidate` name by name — `declared`'s variables solved,
/// `candidate`'s rigid — with `declared`'s return under `candidate`'s. The function twin of
/// [`admits_shape`], reached from *fits* alone.
///
/// Width is the order's own: every name `declared` asks for, `candidate` must have, and a name
/// only `candidate` has is one `declared` never needs. A parameter pair asks the candidate's
/// parameter to lie under the declared one (covariant for the collector, since a parameter's own
/// polarity is contravariant) and the return pair asks the declared return to lie under the
/// candidate's.
pub(super) fn admits_function(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: Handle,
    candidate: Handle,
) -> bool {
    let (
        TypeNode::KFunction {
            bounds,
            params: declared_params,
            ret: declared_ret,
            ..
        },
        TypeNode::KFunction {
            params: candidate_params,
            ret: candidate_ret,
            ..
        },
    ) = (types.node(declared), types.node(candidate))
    else {
        return false;
    };
    let mut collector = Collector::<Handle>::new(scratch, bounds);
    for (name, slot) in declared_params.iter() {
        let Some(argument) = candidate_params.get(name.symbol()) else {
            return false;
        };
        if admits(types, scratch, slot, argument, Variance::Co, &mut collector).is_err() {
            return false;
        }
    }
    if admits(
        types,
        scratch,
        declared_ret,
        candidate_ret,
        Variance::Contra,
        &mut collector,
    )
    .is_err()
    {
        return false;
    }
    collector.solve(types).is_ok()
}

/// Rank two candidates under one bucket key and ranking, lexicographically by class.
///
/// At each class in turn, `a` is at least as specific as `b` when `b`'s slots there admit `a`'s
/// ([`class_at_least`]); the first class at which exactly one side holds decides. For monomorphic
/// shapes each class is the pointwise order over its slots; for a generic candidate it is the
/// classic "more specific method" rule, so `(f _ :Number)` beats `(f FOR ALL (Elt) _ :Elt)` and
/// `(f _ :Any)` ties with it. Every class holding both ways is `Equal`; a pair no class orders and
/// some class leaves unrelated is `Incomparable`, as are shapes under different keys or rankings.
pub(super) fn shape_specificity(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: Handle,
    b: Handle,
) -> Specificity {
    let (Some(ranked_a), Some(ranked_b)) = (Ranked::of(types, a), Ranked::of(types, b)) else {
        // Two things that are not both shapes have no bucket in common to rank under, which is a
        // refusal rather than a tie: the empty element run a non-shape reads as would otherwise
        // make every pair of leaves compare `Equal`.
        return Specificity::Incomparable;
    };
    if !elements_key_equal(ranked_a.elements, ranked_b.elements)
        || ranked_a.classes != ranked_b.classes
    {
        return Specificity::Incomparable;
    }
    let mut every_class_both = true;
    for class in 0..ranked_a.class_count() {
        let class = u8::try_from(class).expect("a shape has fewer than 256 classes");
        let more = class_at_least(types, scratch, a, b, class);
        let less = class_at_least(types, scratch, b, a, class);
        match (more, less) {
            (true, false) => return Specificity::StrictlyMore,
            (false, true) => return Specificity::StrictlyLess,
            (true, true) => {}
            (false, false) => every_class_both = false,
        }
    }
    if every_class_both {
        Specificity::Equal
    } else {
        Specificity::Incomparable
    }
}

// --- Fits ---

/// Why `offered` does not fit an asked signature type — the per-member rule that rejected, carrying
/// the offending member's symbol and the handles that disagreed, each typed as a member type is: a
/// value slot or a keyworded member may be a scheme.
///
/// Symbols and handles rather than rendered text: rendering needs the symbol interner, which is the
/// caller's, so [`render_fits_failure`](super::render::render_fits_failure) produces the fragment.
/// `Copy` and unboxed, so a negative verdict allocates nothing it does not name. A run it names is
/// either an asked schema's own (`'run`) or staged in the caller's scratch (`'s`).
#[derive(Clone, Copy, Debug)]
pub enum FitsFailure<'run, 's> {
    MissingTypeMember {
        name: TypeSymbol,
    },
    ManifestMismatch {
        name: TypeSymbol,
        got: Parametric,
        expected: Parametric,
    },
    MissingValueSlot {
        name: ValueSymbol,
    },
    ValueSlotMismatch {
        name: ValueSymbol,
        got: DeclaredType<Parametric>,
        expected: DeclaredType<Parametric>,
    },
    /// The offered side declares no dispatch bucket under the asked member's key at all.
    MissingKeyworded {
        head: DeclaredType<Parametric>,
    },
    /// The bucket exists under a different ranking than the asked member's: one keyword pattern
    /// carries one order, so no overload in it can satisfy the member.
    RankingMismatch {
        head: DeclaredType<Parametric>,
        got: DeclaredType<Parametric>,
    },
    /// The bucket exists but no overload in it satisfies the asked member.
    KeywordedMismatch {
        head: DeclaredType<Parametric>,
        got: &'s [DeclaredType<Parametric>],
    },
    /// Every overload under the key failed, and the first failed at a position the asked member
    /// **quantifies** over: a concrete position there says the module implements one instantiation
    /// where the signature declares an operation holding at every one.
    QuantifiedMismatch {
        head: DeclaredType<Parametric>,
        parameter: TypeSymbol,
        got: Parametric,
    },
    /// No record in the offered operator registry covers the asked record's members.
    MissingOperatorGroup {
        members: &'run [KeywordSymbol],
    },
    /// A record covering the asked members exists, but chains them a different way.
    OperatorModeMismatch {
        members: &'run [KeywordSymbol],
        expected: ReductionMode,
        got: ReductionMode,
    },
    /// What the offered members contribute to a head parameter the application leaves unpinned
    /// denotes no type.
    Unsolved {
        parameter: TypeSymbol,
    },
    /// A module's self-signature is asked, and only a set holding that very handle fits it.
    SelfSignature {
        expected: KType,
    },
}

/// Whether `offered` fits every application `asked` holds — *fits*, the relation a question reads,
/// over two signature types. Each asked application is fitted on its own
/// ([`fits_application`]), so the solution each one takes is its own.
pub(super) fn sig_fits<'run, 's>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    offered: Handle,
    asked: Handle,
) -> Result<(), FitsFailure<'run, 's>> {
    let asked = applications(types, scratch, asked).expect("fits asks a signature type");
    // *Fits* contains the order: a set under the asked one by R-5 fits it with nothing to solve.
    let held = applications(types, scratch, offered).expect("fits offers a signature type");
    if applications_under(&held, &asked) {
        return Ok(());
    }
    for application in asked.iter() {
        fits_one(types, scratch, offered, *application)?;
    }
    Ok(())
}

/// `offered` fitted against the one application of `signature` with `pins`, and what that solves
/// each of `signature`'s head parameters to: the pins, then the solution for each unpinned one.
/// What the view door reads as the source's bindings.
pub(super) fn fits_application<'run, 's>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    offered: Handle,
    signature: Handle,
    pins: &[(BinderSymbol, Handle)],
) -> Result<Members<'s, TypeSymbol, Handle>, FitsFailure<'run, 's>> {
    let pins = scratch.alloc_slice_copy(pins);
    fits_one(types, scratch, offered, Application { signature, pins })
}

/// The class walks' stand-in level less one: the offered side's own unpinned parameters read as
/// lexical variables at `OFFERED_LEVEL - k` for its `k`th application, apart from every level the
/// elaborator mints and from the stand-ins an asked binder opens its group to.
const OFFERED_LEVEL: usize = STAND_IN_LEVEL - 1;

/// What the offered side's applications hold, pooled, every member type read under its
/// application's pins and stand-ins.
struct Offered<'run, 's> {
    /// Every manifest binding, in application order: a name two applications fix is listed twice,
    /// and the first is the one read.
    manifest: BumpVec<'s, (TypeSymbol, Handle)>,
    /// Each value slot once, at the meet of every type an application offers for it.
    values: BumpVec<'s, (ValueSymbol, Handle)>,
    keyworded: BumpVec<'s, Handle>,
    operators: BumpVec<'s, DeclaredGroup<'run>>,
}

impl<'run, 's> Offered<'run, 's> {
    fn pool(types: &TypeRegistry<'run>, scratch: BumpAllocator<'s>, offered: Handle) -> Self {
        let mut pool = Offered {
            manifest: BumpVec::new_in(scratch),
            values: BumpVec::new_in(scratch),
            keyworded: BumpVec::new_in(scratch),
            operators: BumpVec::new_in(scratch),
        };
        let set = applications(types, scratch, offered).expect("fits offers a signature type");
        for (k, application) in set.iter().enumerate() {
            let schema = schema_of(types, application.signature);
            let mut bindings = BumpVec::with_capacity_in(schema.parameters.len(), scratch);
            bindings.extend(schema.parameters.iter().map(|(name, parameter)| {
                let read = application.pin(name.symbol()).unwrap_or_else(|| {
                    let bound = types.node(*parameter).rigid_bound().unwrap_or(KType::ANY);
                    types.lexical(OFFERED_LEVEL - k, *name, bound).raw()
                });
                (*name, read)
            }));
            let bindings = Members::from_table(bindings);
            let read = |kt: Handle| substitute_parameters(types, scratch, kt, bindings);
            pool.manifest.extend(
                schema
                    .manifest_members
                    .iter()
                    .map(|(n, kt)| (*n, read(kt.raw()))),
            );
            for (name, kt) in schema.value_slots.iter().copied() {
                let kt = read(kt.raw());
                match pool.values.iter_mut().find(|(held, _)| *held == name) {
                    Some(held) => held.1 = meet_through_variables(types, scratch, held.1, kt),
                    None => pool.values.push((name, kt)),
                }
            }
            pool.keyworded
                .extend(schema.keyworded.iter().map(|kt| read(kt.raw())));
            pool.operators.extend_from_slice(schema.operators);
        }
        pool
    }

    fn manifest(&self, name: TypeSymbol) -> Option<Handle> {
        self.manifest
            .iter()
            .find(|(held, _)| *held == name)
            .map(|(_, kt)| *kt)
    }

    fn value(&self, name: ValueSymbol) -> Option<Handle> {
        self.values
            .iter()
            .find(|(held, _)| *held == name)
            .map(|(_, kt)| *kt)
    }
}

/// [`fits_application`] over an application already in hand.
fn fits_one<'run, 's>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    offered: Handle,
    asked: Application<'_>,
) -> Result<Members<'s, TypeSymbol, Handle>, FitsFailure<'run, 's>> {
    let schema = schema_of(types, asked.signature);
    // A module's self-signature on the asking side compares by handle.
    if schema.origin == SigOrigin::Module {
        let set = applications(types, scratch, offered).expect("fits offers a signature type");
        return if set
            .iter()
            .any(|held| held.signature == asked.signature && held.pins.is_empty())
        {
            Ok(Members::EMPTY)
        } else {
            Err(FitsFailure::SelfSignature {
                expected: wrap(asked.signature),
            })
        };
    }
    let pool = Offered::pool(types, scratch, offered);
    let mut pinned = BumpVec::with_capacity_in(asked.pins.len(), scratch);
    pinned.extend(asked.pins.iter().map(|(name, kt)| {
        let BinderSymbol::Type(name) = name else {
            unreachable!("an application pins type parameters")
        };
        (*name, *kt)
    }));
    let mut unpinned = BumpVec::new_in(scratch);
    unpinned.extend(
        schema
            .parameters
            .iter()
            .filter(|(name, _)| asked.pin(name.symbol()).is_none())
            .map(|(name, parameter)| (*name, parameter.raw())),
    );
    if unpinned.is_empty() {
        let solution = Members::from_table(pinned);
        return check(types, scratch, schema, &pool, solution).map(|()| solution);
    }
    let mut search = Search {
        types,
        scratch,
        schema,
        pool: &pool,
        pinned: &pinned,
        unpinned: &unpinned,
        first: None,
    };
    search.solve()
}

/// Pass A's choice search over one asked application with unpinned parameters: what each asked
/// member contributes to them, one offered overload per keyworded member, tried in turn.
struct Search<'a, 'run, 's> {
    types: &'a TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    schema: SigSchema<'run>,
    pool: &'a Offered<'run, 's>,
    pinned: &'a [(TypeSymbol, Handle)],
    unpinned: &'a [(TypeSymbol, Handle)],
    /// The failure the first leaf gave, which is the one reported when no leaf passes.
    first: Option<FitsFailure<'run, 's>>,
}

impl<'run, 's> Search<'_, 'run, 's> {
    fn solve(&mut self) -> Result<Members<'s, TypeSymbol, Handle>, FitsFailure<'run, 's>> {
        let (types, scratch) = (self.types, self.scratch);
        let mut bounds = BumpVec::with_capacity_in(self.unpinned.len(), scratch);
        bounds.extend(
            self.unpinned
                .iter()
                .map(|(_, parameter)| types.node(*parameter).rigid_bound().unwrap_or(KType::ANY)),
        );
        let mut collector = Collector::new(scratch, &bounds);
        // Manifest members and value slots involve no choice, so they contribute first.
        for (name, declared) in self.schema.manifest_members.iter().copied() {
            let Some(got) = self.pool.manifest(name) else {
                return Err(FitsFailure::MissingTypeMember { name });
            };
            let asked = self.read_open(declared.raw());
            contribute(
                types,
                scratch,
                &mut collector,
                &[(asked, got, Variance::Co)],
            );
            contribute(
                types,
                scratch,
                &mut collector,
                &[(asked, got, Variance::Contra)],
            );
        }
        for (name, declared) in self.schema.value_slots.iter().copied() {
            let Some(got) = self.pool.value(name) else {
                return Err(FitsFailure::MissingValueSlot { name });
            };
            if !types.node(got).binds_quantifiers() {
                let asked = self.read_open(declared.raw());
                contribute(
                    types,
                    scratch,
                    &mut collector,
                    &[(asked, got, Variance::Co)],
                );
            }
        }
        match self.choose(&mut collector, 0) {
            Some(solution) => Ok(solution),
            None => Err(self.first.expect("the search reaches a leaf")),
        }
    }

    /// `declared`, a member type of the asked schema, read for pass A: a binder's own group opened
    /// to stand-ins first, then each pin, and each unpinned parameter as the collector's variable.
    fn read_open(&self, declared: Handle) -> Handle {
        let opened = open_to_stand_ins(self.types, self.scratch, declared);
        self.read_quantified(opened)
    }

    /// `kt` with each pin, and each unpinned parameter as its `Quantified` in the collector.
    fn read_quantified(&self, kt: Handle) -> Handle {
        let mut bindings =
            BumpVec::with_capacity_in(self.pinned.len() + self.unpinned.len(), self.scratch);
        bindings.extend_from_slice(self.pinned);
        bindings.extend(
            self.unpinned
                .iter()
                .enumerate()
                .map(|(j, (name, parameter))| {
                    let bound = self
                        .types
                        .node(*parameter)
                        .rigid_bound()
                        .unwrap_or(KType::ANY);
                    (*name, self.types.quantified(j, bound).raw())
                }),
        );
        substitute_parameters(self.types, self.scratch, kt, Members::from_table(bindings))
    }

    /// The search from asked keyworded member `index` on: each candidate that contributes, then —
    /// where none did, or the key holds an offered overload with a group of its own — nothing.
    /// `Some` with the first solution pass C accepts.
    fn choose(
        &mut self,
        collector: &mut Collector<'s, Handle>,
        index: usize,
    ) -> Option<Members<'s, TypeSymbol, Handle>> {
        let (types, scratch) = (self.types, self.scratch);
        let Some(declared) = self
            .schema
            .keyworded
            .get(index)
            .map(|declared| declared.raw())
        else {
            return self.leaf(collector);
        };
        let opened = open_to_stand_ins(types, scratch, declared);
        let mut contributed = false;
        let mut quantified_at_key = false;
        for candidate in self.pool.keyworded.iter().copied() {
            if !keys_equal(declared, candidate, types)
                || shape_classes(declared, types) != shape_classes(candidate, types)
            {
                continue;
            }
            if !shape_quantifiers(candidate, types).is_empty() {
                quantified_at_key = true;
                continue;
            }
            let mut pairs = BumpVec::new_in(scratch);
            for (asked, got) in shape_slots(opened, types).zip(shape_slots(candidate, types)) {
                pairs.push((self.read_quantified(asked), got, Variance::Contra));
            }
            let (Some(asked), Some(got)) =
                (shape_return(opened, types), shape_return(candidate, types))
            else {
                continue;
            };
            pairs.push((self.read_quantified(asked), got, Variance::Co));
            let mark = collector.mark();
            if !contribute(types, scratch, collector, &pairs) {
                continue;
            }
            contributed = true;
            if let Some(solution) = self.choose(collector, index + 1) {
                return Some(solution);
            }
            collector.rollback(mark);
        }
        if !contributed || quantified_at_key {
            return self.choose(collector, index + 1);
        }
        None
    }

    /// Past the last keyworded member: solve, then pass C.
    fn leaf(
        &mut self,
        collector: &Collector<'s, Handle>,
    ) -> Option<Members<'s, TypeSymbol, Handle>> {
        let (types, scratch) = (self.types, self.scratch);
        let outcome = match collector.solve(types) {
            Err(failure) => {
                let index = match failure {
                    UnifyFailure::Disagree { index, .. } => index,
                    UnifyFailure::Mismatch => 0,
                };
                Err(FitsFailure::Unsolved {
                    parameter: self.unpinned[index].0,
                })
            }
            Ok(least) => {
                let mut table =
                    BumpVec::with_capacity_in(self.pinned.len() + self.unpinned.len(), scratch);
                table.extend_from_slice(self.pinned);
                table.extend(
                    self.unpinned
                        .iter()
                        .zip(least.iter())
                        .map(|((name, _), solved)| (*name, *solved)),
                );
                let solution = Members::from_table(table);
                check(types, scratch, self.schema, self.pool, solution).map(|()| solution)
            }
        };
        match outcome {
            Ok(solution) => Some(solution),
            Err(failure) => {
                self.first.get_or_insert(failure);
                None
            }
        }
    }
}

/// Admit each `(asked, offered, variance)` pair into `collector`, all or nothing: `false`, with
/// nothing contributed, where any pair does not admit.
fn contribute(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    collector: &mut Collector<'_, Handle>,
    pairs: &[(Handle, Handle, Variance)],
) -> bool {
    let mark = collector.mark();
    for (asked, offered, variance) in pairs.iter().copied() {
        if admits(types, scratch, asked, offered, variance, collector).is_err() {
            collector.rollback(mark);
            return false;
        }
    }
    true
}

/// `kt` with its own group, where it binds one, opened to the class walks' stand-ins: a rigid
/// lexical variable per variable, named and bounded as declared. A shape keeps its node — its
/// positions are paired by hand — and a function type loses its group.
fn open_to_stand_ins(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, kt: Handle) -> Handle {
    let (names, bounds) = match types.node(kt) {
        TypeNode::ExpressionShape {
            quantifiers,
            bounds,
            ..
        }
        | TypeNode::KFunction {
            quantifiers,
            bounds,
            ..
        } => (quantifiers, bounds),
        _ => return kt,
    };
    if names.is_empty() {
        return kt;
    }
    let mut stand_ins = BumpVec::with_capacity_in(names.len(), scratch);
    stand_ins.extend(
        names
            .iter()
            .zip(bounds)
            .map(|(name, bound)| types.lexical(STAND_IN_LEVEL, *name, *bound).raw()),
    );
    instantiate_quantified(types, scratch, kt, &stand_ins)
}

/// Pass C: each asked member of `schema`, read under `solution`, against the pooled offer.
fn check<'run, 's>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    schema: SigSchema<'run>,
    pool: &Offered<'run, 's>,
    solution: Members<'_, TypeSymbol, Handle>,
) -> Result<(), FitsFailure<'run, 's>> {
    let read = |declared: Handle| substitute_parameters(types, scratch, declared, solution);

    for (name, fixed) in schema.manifest_members.iter().copied() {
        let expected = read(fixed.raw());
        match pool.manifest(name) {
            Some(got) if got == expected => {}
            Some(got) => {
                return Err(FitsFailure::ManifestMismatch {
                    name,
                    got: wrap(got),
                    expected: wrap(expected),
                });
            }
            None => return Err(FitsFailure::MissingTypeMember { name }),
        }
    }

    for (name, declared) in schema.value_slots.iter().copied() {
        let Some(got) = pool.value(name) else {
            return Err(FitsFailure::MissingValueSlot { name });
        };
        let expected = read(declared.raw());
        if !satisfied_by(types, scratch, expected, got) {
            return Err(FitsFailure::ValueSlotMismatch {
                name,
                got: types.declared(got),
                expected: types.declared(expected),
            });
        }
    }

    // Keyworded members: each needs some offered overload at its key that satisfies it. A tie
    // among several is an ambiguity where a call meets it, not here.
    for head in schema.keyworded.iter().copied() {
        let declared = head.raw();
        let ranking = shape_classes(declared, types);
        if let Some(got) = pool.keyworded.iter().copied().find(|candidate| {
            keys_equal(declared, *candidate, types) && shape_classes(*candidate, types) != ranking
        }) {
            return Err(FitsFailure::RankingMismatch {
                head,
                got: types.declared(got),
            });
        }
        let mut candidates = BumpVec::with_capacity_in(pool.keyworded.len(), scratch);
        candidates.extend(
            pool.keyworded
                .iter()
                .copied()
                .filter(|candidate| keys_equal(declared, *candidate, types)),
        );
        let expected = read(declared);
        if candidates
            .iter()
            .any(|candidate| satisfied_by(types, scratch, expected, *candidate))
        {
            continue;
        }
        let Some(first) = candidates.first().copied() else {
            return Err(FitsFailure::MissingKeyworded { head });
        };
        if let Some((parameter, got)) = quantified_position_failure(types, scratch, expected, first)
        {
            // A slot holds no binder, so the candidate's slot is no scheme.
            return Err(FitsFailure::QuantifiedMismatch {
                head,
                parameter,
                got: wrap(got),
            });
        }
        return Err(FitsFailure::KeywordedMismatch {
            head,
            got: scratch.alloc_slice_fill_iter(
                candidates
                    .iter()
                    .map(|candidate| types.declared(*candidate)),
            ),
        });
    }

    // Operator members: each asked record needs an offered record whose member set **includes** it
    // — width, as for every other channel — under an **equal** mode. A run folded right and the
    // same run folded left compute different things, so a mode is matched exactly.
    for declared in schema.operators.iter().copied() {
        let covering = pool
            .operators
            .iter()
            .find(|record| declared.members.iter().all(|m| record.members.contains(m)));
        let Some(covering) = covering else {
            return Err(FitsFailure::MissingOperatorGroup {
                members: declared.members,
            });
        };
        if covering.mode != declared.mode {
            return Err(FitsFailure::OperatorModeMismatch {
                members: declared.members,
                expected: declared.mode,
                got: covering.mode,
            });
        }
    }
    Ok(())
}

/// The schema of `signature`, a [`TypeNode::Signature`].
fn schema_of<'run>(types: &TypeRegistry<'run>, signature: Handle) -> SigSchema<'run> {
    match types.node(signature) {
        TypeNode::Signature { schema, .. } => schema,
        _ => unreachable!("an application applies a signature"),
    }
}

/// Why `candidate` failed the asked member `declared`, when the reason is a **quantified**
/// position: the first slot whose declared type reads a quantifier and whose candidate slot is not
/// at least as general, as the parameter's name and the type the candidate fixes it to.
fn quantified_position_failure(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: Handle,
    candidate: Handle,
) -> Option<(TypeSymbol, Handle)> {
    let quantifiers = shape_quantifiers(declared, types);
    if quantifiers.is_empty() {
        return None;
    }
    let mut candidate_slots = shape_slots(candidate, types);
    for declared_slot in shape_slots(declared, types) {
        let candidate_slot = candidate_slots.next()?;
        // Contravariance: a candidate position fills a declared one by being equal or more general.
        if fits(types, scratch, declared_slot, candidate_slot) {
            continue;
        }
        let named = (0..quantifiers.len())
            .find(|i| types.references_quantifier(scratch, declared_slot, *i))?;
        return Some((quantifiers[named], candidate_slot));
    }
    None
}
