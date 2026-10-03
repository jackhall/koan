//! [`SigSchema`] — the normalized carrier of a signature's shape, the canonical orders its channels
//! are stored in, and [`SchemaDraft`], the transient a schema is assembled in before the signature
//! door canonicalizes and interns it.
//!
//! A declared signature's head parameters (`SIG Stack FOR ALL #[Elt] = …`) are its
//! [`parameters`](SigSchema::parameters), each a [`TypeNode::Parameter`] its members read by name;
//! a manifest member fixes a concrete type. A module's self-signature has no parameters.
//!
//! Every channel is a slice in the run region, stored in one canonical order: the named tables
//! are [`Members`], symbol-sorted by name with each name once — an order the type holds
//! itself, since a table is only ever built sorted — the keyworded channel in
//! [`canonical_overloads`] order, the operator channel in [`canonical_groups`] order. A reader walks
//! a table in that order and never sorts one; a lookup by name is a binary search ([`member`]). The
//! two unnamed channels' order is fixed in one place, [`TypeRegistry::signature`], the door a
//! schema enters the lattice through.
//!
//! A `SigSchema` is what the [`TypeNode::Signature`] node owns; the node computes and stores the
//! schema's content digest once at intern time, so the schema itself carries no digest field.
//! Projecting a declaration into a schema needs the declaring scope and lives with the elaborator;
//! the lattice takes a finished draft.
//!
//! The relations over two schemas live in [`sig_relations`](super::sig_relations).

use std::ops::Deref;

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{KeywordSymbol, TypeSymbol, ValueSymbol};

use super::handle::{DeclaredType, Handle, KType, Parametric, Scheme, TypeHandle};
use super::kind::KKind;
use super::node::{NodeSchema, TypeNode};
use super::operators::{FoldDirection, ReductionMode};
use super::order::{Dropped, unsubsumed};
use super::registry::TypeRegistry;
use super::shape::DispatchTokenElement;

/// A named member table: `(name, T)` pairs, symbol-sorted by name with each name once. The shape
/// every name-keyed channel of a schema is stored in, and the shape a substitution's bindings
/// travel as.
///
/// The order is the type's own invariant, not a caller's promise: a table is built only by
/// [`Members::from_table`], which sorts and dedups, or derived from one by a step that keeps its
/// names in place ([`copied_into`](Self::copied_into)). So every
/// reader may binary-search a table ([`member`]) without checking it. Reading a table is reading
/// its slice, through `Deref`.
pub struct Members<'a, N, T = Parametric>(&'a [(N, T)]);

impl<N, T> Clone for Members<'_, N, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<N, T> Copy for Members<'_, N, T> {}

impl<N, T> Deref for Members<'_, N, T> {
    type Target = [(N, T)];

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'a, N, T> IntoIterator for Members<'a, N, T> {
    type Item = &'a (N, T);
    type IntoIter = std::slice::Iter<'a, (N, T)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a, N: Copy, T: Copy> Members<'a, N, T> {
    /// The table binding nothing.
    pub const EMPTY: Self = Members(&[]);

    /// This table copied into `bump`, in its stored order — how a table staged in scratch moves
    /// into the region that keeps it.
    pub(super) fn copied_into<'b>(self, bump: BumpAllocator<'b>) -> Members<'b, N, T> {
        if self.0.is_empty() {
            Members(&[])
        } else {
            Members(bump.alloc_slice_copy(self.0))
        }
    }
}

impl<'a, N: Ord + Copy, T: Copy> Members<'a, N, T> {
    /// `table` as a member table: sorted by name, a name bound twice keeping its later binding.
    /// The door every table is built through. The sort is stable, which is what makes "later" the
    /// order `table` was filled in.
    pub fn from_table(mut table: BumpVec<'a, (N, T)>) -> Self {
        table.sort_by_key(|(name, _)| *name);
        table.dedup_by(|later, earlier| {
            let same = later.0 == earlier.0;
            if same {
                earlier.1 = later.1;
            }
            same
        });
        Members(table.leak())
    }

    /// [`from_table`](Self::from_table) over `pairs`, staged in `scratch`.
    pub fn from_pairs(scratch: BumpAllocator<'a>, pairs: impl IntoIterator<Item = (N, T)>) -> Self {
        let pairs = pairs.into_iter();
        let mut table = BumpVec::with_capacity_in(pairs.size_hint().0, scratch);
        table.extend(pairs);
        Self::from_table(table)
    }
}

/// The type `members` binds `name` to — a binary search over the table's stored order.
pub fn member<N: Ord + Copy, T: Copy>(members: Members<'_, N, T>, name: N) -> Option<T> {
    members
        .binary_search_by(|(held, _)| held.cmp(&name))
        .ok()
        .map(|index| members[index].1)
}

/// Normalized signature schema — the carrier the signature relations are defined over.
#[derive(Clone, Copy)]
pub struct SigSchema<'run> {
    /// Whether a `SIG` declared this schema or a module carries it: a module's self-signature, a
    /// view's signature, the empty signature. *Fits* reads the two differently on its asking side.
    pub origin: SigOrigin,
    /// Head parameters: name → the [`TypeNode::Parameter`] its members read, bound inside. Empty
    /// for a module's schema.
    pub parameters: Members<'run, TypeSymbol>,
    /// Manifest type members: name → the fixed type, which may read a head parameter.
    pub manifest_members: Members<'run, TypeSymbol>,
    /// Value slots: name → declared (SIG) or derived (self-sig) type, a quantified callable's
    /// scheme included.
    pub value_slots: Members<'run, ValueSymbol, DeclaredType<Parametric>>,
    /// Keyworded (dispatch-bucket) members: the expression shapes the interface declares, each a
    /// type or a scheme, in [`canonical_overloads`] order. The bucket key is each member's own
    /// element run with its slot types erased ([`shape_keys_equal`]) — read off the member's type,
    /// never stored beside it — so two overloads under one key are two entries here.
    pub keyworded: &'run [DeclaredType<Parametric>],
    /// Operator members: the chaining records the interface declares, in [`canonical_groups`]
    /// order. A record says which operators chain together and how a run of them reduces. The
    /// buckets themselves are ordinary [`keyworded`](Self::keyworded) members. Two records in one
    /// channel never share a member: a scope declares one chaining mode per operator.
    pub operators: &'run [DeclaredGroup<'run>],
}

/// Where a schema comes from — part of its identity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SigOrigin {
    /// A `SIG` declaration.
    Declared,
    /// A module: its self-signature, a view's signature, or the empty signature.
    Module,
}

/// One declared chaining record: which operators chain together, and how a run of them reduces.
/// `members` is sorted by symbol bits and deduped, so two records over the same set compare and
/// digest alike whatever order they were written in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeclaredGroup<'run> {
    pub members: &'run [KeywordSymbol],
    pub mode: ReductionMode,
}

impl SigSchema<'_> {
    /// The member-free schema — the module-lattice top the `:Module` name lowers to, and the
    /// content any declaration with no parameter and no member normalizes to.
    pub const EMPTY: SigSchema<'static> = SigSchema {
        origin: SigOrigin::Module,
        parameters: Members::EMPTY,
        manifest_members: Members::EMPTY,
        value_slots: Members::EMPTY,
        keyworded: &[],
        operators: &[],
    };

    /// Whether this schema declares no parameter and names no member — the lattice top, whatever
    /// its origin.
    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty()
            && self.manifest_members.is_empty()
            && self.value_slots.is_empty()
            && self.keyworded.is_empty()
            && self.operators.is_empty()
    }

    /// This schema's manifest binding for the type member `name`.
    pub fn type_member(&self, name: TypeSymbol) -> Option<Parametric> {
        member(self.manifest_members, name)
    }
}

/// A signature schema under construction, staged in scratch. Built by a declaration, a module or
/// a view, and consumed by [`TypeRegistry::signature`], which fixes every channel's canonical order
/// and interns the result — so a draft may be filled in any order.
///
/// A named insert replaces an earlier binding for the same name, so each name lands once. A member
/// type may read the head parameters, and a value slot or a keyworded member may be a quantified
/// callable's scheme — the schema's own doors for those — while the signature the draft interns is
/// concrete: no walk enters a signature.
pub struct SchemaDraft<'s> {
    /// See [`SigSchema::origin`].
    pub origin: SigOrigin,
    pub(super) parameters: BumpVec<'s, (TypeSymbol, Parametric)>,
    pub(super) manifest_members: BumpVec<'s, (TypeSymbol, Parametric)>,
    pub(super) value_slots: BumpVec<'s, (ValueSymbol, DeclaredType<Parametric>)>,
    pub(super) keyworded: BumpVec<'s, DeclaredType<Parametric>>,
    pub(super) operators: BumpVec<'s, DeclaredGroup<'s>>,
    scratch: BumpAllocator<'s>,
}

impl<'s> SchemaDraft<'s> {
    /// An empty draft of a module's schema — the member-free schema until something is inserted.
    pub fn new(scratch: BumpAllocator<'s>) -> Self {
        SchemaDraft {
            origin: SigOrigin::Module,
            parameters: BumpVec::new_in(scratch),
            manifest_members: BumpVec::new_in(scratch),
            value_slots: BumpVec::new_in(scratch),
            keyworded: BumpVec::new_in(scratch),
            operators: BumpVec::new_in(scratch),
            scratch,
        }
    }

    /// Declare the head parameter `name`, read by the members as `parameter`.
    pub fn insert_parameter(&mut self, name: TypeSymbol, parameter: Parametric) {
        upsert(&mut self.parameters, name, parameter);
    }

    /// Fix the manifest member `name` to `kt`.
    pub fn insert_manifest(&mut self, name: TypeSymbol, kt: impl Into<Parametric>) {
        upsert(&mut self.manifest_members, name, kt.into());
    }

    /// Declare the value slot `name` at `kt` — a type, or a quantified callable's scheme.
    pub fn insert_value_slot(
        &mut self,
        name: ValueSymbol,
        kt: impl Into<DeclaredType<Parametric>>,
    ) {
        upsert(&mut self.value_slots, name, kt.into());
    }

    /// Declare a keyworded member — a shape, or a quantified shape's scheme.
    pub fn push_keyworded(&mut self, shape: impl Into<DeclaredType<Parametric>>) {
        self.keyworded.push(shape.into());
    }

    /// Declare a chaining record over `members`, which may arrive in any order and repeat.
    pub fn push_operator_group(&mut self, members: &[KeywordSymbol], mode: ReductionMode) {
        let mut run = BumpVec::with_capacity_in(members.len(), self.scratch);
        run.extend_from_slice(members);
        run.sort_unstable();
        run.dedup();
        self.operators.push(DeclaredGroup {
            members: run.leak(),
            mode,
        });
    }
}

/// Bind `name` in a draft table, replacing an earlier binding for it.
fn upsert<N: PartialEq + Copy, T>(table: &mut BumpVec<'_, (N, T)>, name: N, kt: T) {
    match table.iter_mut().find(|(held, _)| *held == name) {
        Some(slot) => slot.1 = kt,
        None => table.push((name, kt)),
    }
}

/// Put a schema's operator channel in canonical order — sorted by member run, then by mode, exact
/// duplicates collapsed. Every channel is stored through this, so the digest, the rendering and
/// equality all read one order rather than the order of declaration.
pub(super) fn canonical_groups(groups: &mut BumpVec<'_, DeclaredGroup<'_>>) {
    groups.sort_by(|a, b| {
        a.members
            .cmp(b.members)
            .then_with(|| mode_order(a.mode).cmp(&mode_order(b.mode)))
    });
    groups.dedup();
}

/// A mode's total order, for [`canonical_groups`] — the same discriminant the digest feeds, with a
/// pairwise mode's combiner and direction breaking its own ties.
fn mode_order(mode: ReductionMode) -> (u8, Option<KeywordSymbol>, u8) {
    match mode {
        ReductionMode::Unary => (0, None, 0),
        ReductionMode::FoldLeft => (1, None, 0),
        ReductionMode::FoldRight => (2, None, 0),
        ReductionMode::Pairwise {
            combiner,
            direction,
        } => (3, Some(combiner), direction_byte(direction)),
    }
}

/// A fold direction as the byte the digest and the mode order both read.
fn direction_byte(direction: FoldDirection) -> u8 {
    match direction {
        FoldDirection::Left => 0,
        FoldDirection::Right => 1,
    }
}

/// Put a keyworded member set in canonical order — sorted by content digest, exact duplicates
/// collapsed, and every shape another member **is below** dropped.
///
/// Subsumption is by the order, as [`union_of`](super::registry::TypeRegistry::union_of)
/// canonicalizes a union by, but from the other side: a member below another promises everything
/// that other one does, so keeping the upper one says nothing new. That is what
/// makes an interface canonical — two schemas that satisfy each other are one schema, so a module's
/// self-signature *is* its interface — and it is why the rule cannot be the slots-only admission
/// [`shape_specificity`](super::sig_relations::shape_specificity) ranks by: dropping a member whose
/// *return* the survivor does not supply would weaken the interface.
///
/// The order is antisymmetric, so two mutually-subsuming shapes are one handle and the dedup above
/// has already collapsed them; nothing here can drop both sides of a pair.
pub(super) fn canonical_overloads(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    overloads: &mut BumpVec<'_, DeclaredType<Parametric>>,
) {
    overloads.sort_unstable_by_key(|overload| overload.raw());
    overloads.dedup();
    if overloads.len() < 2 {
        return;
    }
    // Quadratic in a bucket's width, which is the width an interface declares overloads at. The
    // order runs the other way from a union's: a shape *above* another is the one dropped. A
    // parametric overload — over a head parameter, or a scheme — is deduplicated by handle alone.
    let mut raw = BumpVec::with_capacity_in(overloads.len(), scratch);
    raw.extend(overloads.iter().map(|overload| overload.raw()));
    let keep = unsubsumed(types, scratch, &raw, Dropped::Above);
    let mut keep = keep.iter();
    overloads.retain(|_| *keep.next().unwrap_or(&true));
}

/// Whether two shapes key the same dispatch bucket — each one's element sequence with the slot
/// types erased, compared position by position.
///
/// The bucket key is a *reading* of the member's type, never a second copy of it, and a reading
/// only ever has to be compared, so no consumer materializes one and the pairwise readers
/// allocate nothing at all.
pub fn shape_keys_equal(
    left: impl Into<DeclaredType<Parametric>>,
    right: impl Into<DeclaredType<Parametric>>,
    types: &TypeRegistry<'_>,
) -> bool {
    keys_equal(left.into().raw(), right.into().raw(), types)
}

/// [`shape_keys_equal`] over raw handles.
pub(super) fn keys_equal(left: Handle, right: Handle, types: &TypeRegistry<'_>) -> bool {
    elements_key_equal(
        shape_elements(&types.node(left)),
        shape_elements(&types.node(right)),
    )
}

/// [`shape_keys_equal`] over two element runs already in hand.
pub(super) fn elements_key_equal(
    left: &[DispatchTokenElement],
    right: &[DispatchTokenElement],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| match (a, b) {
            (DispatchTokenElement::Keyword(x), DispatchTokenElement::Keyword(y)) => x == y,
            (DispatchTokenElement::Slot(_), DispatchTokenElement::Slot(_)) => true,
            _ => false,
        })
}

/// A shape node's element sequence; empty for a node that is not a shape, which no schema member
/// ever is. The one place the "not a shape reads as the empty key" convention lives.
pub(super) fn shape_elements<'run, H>(node: &TypeNode<'run, H>) -> &'run [DispatchTokenElement] {
    match node {
        TypeNode::ExpressionShape { elements, .. } => elements.raw(),
        _ => &[],
    }
}

/// A shape's ranking — each slot's dense priority class, empty for written order — or empty for
/// anything that is not a shape.
pub(super) fn shape_classes<'run>(kt: Handle, types: &TypeRegistry<'run>) -> &'run [u8] {
    match types.node(kt) {
        TypeNode::ExpressionShape { classes, .. } => classes,
        _ => &[],
    }
}

/// A shape's argument-position types, in order — the bucket key's typed half, for the readers that
/// compare or render one position at a time. Read straight off the node's element run, so the
/// read builds nothing.
pub fn shape_slots<'run, H: TypeHandle>(
    kt: H,
    types: &TypeRegistry<'run>,
) -> impl Iterator<Item = H::Child> + use<'run, H> {
    slots_of(shape_elements(&types.node(kt.raw())))
}

/// A scheme's argument-position types, in order, each of which may read the scheme's own group.
pub fn scheme_slots<'run>(
    scheme: Scheme,
    types: &TypeRegistry<'run>,
) -> impl Iterator<Item = Parametric> + use<'run> {
    slots_of(shape_elements(&types.node(scheme.raw())))
}

/// The slot types of an element run, read as `C`.
fn slots_of<C: TypeHandle>(
    elements: &[DispatchTokenElement],
) -> impl Iterator<Item = C> + use<'_, C> {
    elements.iter().filter_map(|element| match element {
        DispatchTokenElement::Slot(kt) => Some(super::handle::wrap(*kt)),
        DispatchTokenElement::Keyword(_) => None,
    })
}

/// Whether `kt` names an expression shape.
///
/// The relations keyed on a bucket read a non-shape as the *empty* key, so two unrelated leaves
/// would compare key-equal; every door that ranks or admits by key asks this first, so a caller
/// that hands one a plain type gets a refusal rather than a vacuous verdict.
pub fn is_shape(kt: impl Into<DeclaredType<Parametric>>, types: &TypeRegistry<'_>) -> bool {
    matches!(
        types.node(kt.into().raw()),
        TypeNode::ExpressionShape { .. }
    )
}

/// A shape's return type, or `None` for anything that is not a shape.
pub fn shape_return<H: TypeHandle>(kt: H, types: &TypeRegistry<'_>) -> Option<H::Child> {
    match types.node(kt) {
        TypeNode::ExpressionShape { ret, .. } => Some(ret),
        _ => None,
    }
}

/// A scheme's return type, which may read the scheme's own group; `None` for a function scheme.
pub fn scheme_return(scheme: Scheme, types: &TypeRegistry<'_>) -> Option<Parametric> {
    match types.scheme_node(scheme) {
        TypeNode::ExpressionShape { ret, .. } => Some(ret),
        _ => None,
    }
}

/// A shape's quantifier group — the render-only names, in index order. Empty for a monomorphic
/// shape and for anything that is not a shape.
pub(super) fn shape_quantifiers<'run>(
    kt: Handle,
    types: &TypeRegistry<'run>,
) -> &'run [TypeSymbol] {
    match types.node(kt) {
        TypeNode::ExpressionShape { quantifiers, .. } => quantifiers,
        _ => &[],
    }
}

/// `Some(parameter names)` iff `kt` is a type constructor — a declared family, a
/// `TypeConstructor`-kind member whose names ride its sealed schema. `None` for a first-order type.
/// Arity is the returned list's length; the list is symbol-sorted.
pub fn constructor_param_names<'run>(
    kt: KType,
    types: &TypeRegistry<'run>,
) -> Option<&'run [TypeSymbol]> {
    match types.node(kt) {
        TypeNode::SetMember {
            kind: KKind::TypeConstructor,
            schema: NodeSchema::TypeConstructor { param_names, .. },
            ..
        } => Some(param_names),
        _ => None,
    }
}
