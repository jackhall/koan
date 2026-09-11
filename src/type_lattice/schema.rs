//! [`SigSchema`] — the normalized carrier of a signature's shape, and the canonical orders its
//! channels are stored in.
//!
//! Members are split by *representation*, not by surface syntax: an abstract member is a rigid
//! variable with no concrete witness (an [`TypeNode::AbstractType`] node, of either order), a
//! manifest member fixes a concrete type. A module self-sig never has abstract members — `TYPE` is
//! a SIG-body-only construct.
//!
//! A `SigSchema` is what the [`TypeNode::Signature`] node owns; the node computes and stores the
//! schema's content digest once at intern time, so the schema itself carries no digest field.
//! Projecting a declaration into a schema needs the declaring scope and lives with the elaborator;
//! the lattice takes a finished schema.
//!
//! The relations over two schemas live in [`sig_relations`](super::sig_relations).

use std::collections::HashMap;

use crate::memory::ScopeId;
use crate::parse::{IdentityBuildHasher, KeywordSymbol, TypeSymbol, ValueSymbol};

use super::handle::KType;
use super::kind::KKind;
use super::node::{NodeSchema, TypeNode};
use super::operators::{FoldDirection, ReductionMode};
use super::order::is_subtype_of;
use super::registry::TypeRegistry;
use super::shape::DispatchTokenElement;
use super::substitute::substitute_sig_members;

/// A schema's type-member table: Type-class name → the member's type, identity-hashed on the
/// symbol's digest bits.
pub type TypeMemberMap = HashMap<TypeSymbol, KType, IdentityBuildHasher>;

/// Normalized signature schema — the carrier the signature relations are defined over.
#[derive(Clone)]
pub struct SigSchema {
    /// The binder this schema's own abstract members are sourced at: `Some(ScopeId::SENTINEL)`
    /// for a SIG declaration, `None` for a module self-sig (whose slot types name no
    /// SIG-declared refs).
    ///
    /// The binder is *canonical*, not the declaring scope's id: projection rewrites every
    /// SIG-own member's `source` to [`ScopeId::SENTINEL`], so two textually identical `SIG`
    /// declarations project to one schema and intern to one type. `SENTINEL` is never a minted
    /// scope id, so a canonical binder cannot alias a real one.
    pub sig_id: Option<ScopeId>,
    /// Abstract type members: name → the rigid variable standing for it. Its `param_names` carry
    /// the member's order (empty = first-order, non-empty = a constructor over those parameters)
    /// and its `bound` what it stands over.
    pub abstract_members: TypeMemberMap,
    /// Manifest type members: name → the fixed type.
    pub manifest_members: TypeMemberMap,
    /// Value slots: name → declared (SIG) or derived (self-sig) type.
    pub value_slots: HashMap<ValueSymbol, KType, IdentityBuildHasher>,
    /// Keyworded (dispatch-bucket) members: the expression shapes the interface declares, in
    /// [`canonical_overloads`] order. The bucket key is each member's own element run with its
    /// slot types erased ([`shape_keys_equal`]) — read off the member's type, never stored beside
    /// it — so two overloads under one key are two entries here.
    pub keyworded: Vec<KType>,
    /// Operator members: the chaining records the interface declares, in [`canonical_groups`]
    /// order. A record says which operators chain together and how a run of them reduces. The
    /// buckets themselves are ordinary [`keyworded`](Self::keyworded) members.
    pub operators: OperatorMembers,
}

/// One declared chaining record: which operators chain together, and how a run of them reduces.
/// `members` is sorted by symbol bits and deduped, so two records over the same set compare and
/// digest alike whatever order they were written in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeclaredGroup {
    pub members: Vec<KeywordSymbol>,
    pub mode: ReductionMode,
}

/// A schema's operator channel, in [`canonical_groups`] order. Two records in one channel never
/// share a member: a scope declares one chaining mode per operator.
pub type OperatorMembers = Vec<DeclaredGroup>;

impl SigSchema {
    /// The member-free schema — the module-lattice top the `:Module` name lowers to, and the
    /// content any zero-member `SIG E = ()` declaration projects to. `sig_id` is `None`: an empty
    /// interface names no abstract member for a slot type to substitute against.
    pub fn empty() -> SigSchema {
        SigSchema {
            sig_id: None,
            abstract_members: TypeMemberMap::default(),
            manifest_members: TypeMemberMap::default(),
            value_slots: HashMap::default(),
            keyworded: Vec::new(),
            operators: OperatorMembers::default(),
        }
    }

    /// This schema with `pins` fixed manifest and substituted through every remaining member and
    /// slot type — what `WITH` specialization produces.
    ///
    /// The folded schema is fully concrete in the pinned members, so `Ordered WITH {Carrier =
    /// Number}` interns the same content a SIG declaring `Carrier = Number` outright carries and
    /// specialization introduces no second spelling of a concrete interface. A no-op clone when
    /// `pins` is empty.
    pub fn fold_pins(&self, pins: &[(TypeSymbol, KType)], types: &TypeRegistry) -> SigSchema {
        let mut schema = self.clone();
        if pins.is_empty() {
            return schema;
        }
        let substitutions: TypeMemberMap = pins.iter().copied().collect();
        for (name, kt) in pins {
            schema.abstract_members.remove(name);
            schema.manifest_members.insert(*name, *kt);
        }
        if let Some(sig_id) = schema.sig_id {
            for kt in schema
                .manifest_members
                .values_mut()
                .chain(schema.value_slots.values_mut())
            {
                *kt = substitute_sig_members(types, *kt, sig_id, &substitutions);
            }
            // Two declared overloads that became identical under a pin are one overload.
            schema.keyworded = canonical_overloads(
                schema
                    .keyworded
                    .iter()
                    .map(|shape| substitute_sig_members(types, *shape, sig_id, &substitutions))
                    .collect(),
                types,
            );
        }
        schema
    }

    /// Whether this schema names no member at all — the lattice top, and the identity a
    /// [`meet_schemas`](super::sig_relations::meet_schemas) that keeps nothing lands on.
    pub fn is_empty(&self) -> bool {
        self.abstract_members.is_empty()
            && self.manifest_members.is_empty()
            && self.value_slots.is_empty()
            && self.keyworded.is_empty()
            && self.operators.is_empty()
    }

    /// This schema's binding for the type member `name`, manifest first — the reading every
    /// relation over two schemas takes, since a manifest binding satisfies an abstract
    /// requirement but not the reverse.
    pub fn type_member(&self, name: TypeSymbol) -> Option<KType> {
        self.manifest_members
            .get(&name)
            .or_else(|| self.abstract_members.get(&name))
            .copied()
    }

    /// Every type member's name paired with its binding, manifest first — the substitution a
    /// relation carries into the other side's slot types.
    pub fn member_bindings(&self) -> TypeMemberMap {
        let mut bindings = TypeMemberMap::default();
        for (name, repr) in &self.abstract_members {
            bindings.insert(*name, *repr);
        }
        for (name, kt) in &self.manifest_members {
            bindings.insert(*name, *kt);
        }
        bindings
    }
}

/// A schema's operator channel in canonical order — sorted by member run, then by mode, exact
/// duplicates collapsed. Every channel is stored through this, so the digest, the rendering and
/// equality all read one order rather than the order of declaration.
pub fn canonical_groups(mut groups: OperatorMembers) -> OperatorMembers {
    groups.sort_by(|a, b| {
        a.members
            .cmp(&b.members)
            .then_with(|| mode_order(a.mode).cmp(&mode_order(b.mode)))
    });
    groups.dedup();
    groups
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

/// A keyworded member set in canonical order — sorted by content digest, exact duplicates
/// collapsed, and every shape another member **is below** dropped.
///
/// Subsumption is by the order, the same rule
/// [`union_of`](super::registry::TypeRegistry::union_of) canonicalizes a union with: a member below
/// another promises everything that other one does, so keeping it says nothing new. That is what
/// makes an interface canonical — two schemas that satisfy each other are one schema, so a module's
/// self-signature *is* its interface — and it is why the rule cannot be the slots-only admission
/// [`shape_specificity`](super::sig_relations::shape_specificity) ranks by: dropping a member whose
/// *return* the survivor does not supply would weaken the interface.
///
/// The order is antisymmetric, so two mutually-subsuming shapes are one handle and the dedup above
/// has already collapsed them; nothing here can drop both sides of a pair.
pub fn canonical_overloads(mut overloads: Vec<KType>, types: &TypeRegistry) -> Vec<KType> {
    overloads.sort_unstable();
    overloads.dedup();
    if overloads.len() < 2 {
        return overloads;
    }
    // Quadratic in a bucket's width, which is the width an interface declares overloads at.
    let candidates = overloads.clone();
    overloads.retain(|shape| {
        !candidates
            .iter()
            .any(|peer| peer != shape && is_subtype_of(types, *peer, *shape))
    });
    overloads
}

/// Whether two shapes key the same dispatch bucket — each one's element sequence with the slot
/// types erased, compared position by position.
///
/// The bucket key is a *reading* of the member's type, never a second copy of it, and a reading
/// only ever has to be compared, so no consumer materializes one and the pairwise readers
/// allocate nothing at all.
pub fn shape_keys_equal(left: KType, right: KType, types: &TypeRegistry) -> bool {
    types.with_node(left, |left_node| {
        types.with_node(right, |right_node| {
            let (left, right) = (shape_elements(left_node), shape_elements(right_node));
            left.len() == right.len()
                && left.iter().zip(right).all(|(a, b)| match (a, b) {
                    (DispatchTokenElement::Keyword(x), DispatchTokenElement::Keyword(y)) => x == y,
                    (DispatchTokenElement::Slot(_), DispatchTokenElement::Slot(_)) => true,
                    _ => false,
                })
        })
    })
}

/// A shape node's element sequence; empty for a node that is not a shape, which no schema member
/// ever is. The one place the "not a shape reads as the empty key" convention lives.
pub(super) fn shape_elements(node: &TypeNode) -> &[DispatchTokenElement] {
    match node {
        TypeNode::ExpressionShape { elements, .. } => elements,
        _ => &[],
    }
}

/// A shape's argument-position types, in order — the bucket key's typed half, for the readers that
/// compare or render one position at a time.
pub fn shape_slots(kt: KType, types: &TypeRegistry) -> Vec<KType> {
    // Owns: the slot list is the function's return value, so it outlives the read.
    types.with_node(kt, |node| {
        shape_elements(node)
            .iter()
            .filter_map(|element| match element {
                DispatchTokenElement::Slot(kt) => Some(*kt),
                DispatchTokenElement::Keyword(_) => None,
            })
            .collect()
    })
}

/// Whether `kt` names an expression shape.
///
/// The relations keyed on a bucket read a non-shape as the *empty* key, so two unrelated leaves
/// would compare key-equal; every door that ranks or admits by key asks this first, so a caller
/// that hands one a plain type gets a refusal rather than a vacuous verdict.
pub fn is_shape(kt: KType, types: &TypeRegistry) -> bool {
    types.with_node(kt, |node| matches!(node, TypeNode::ExpressionShape { .. }))
}

/// A shape's return type, or `None` for anything that is not a shape.
pub fn shape_return(kt: KType, types: &TypeRegistry) -> Option<KType> {
    types.with_node(kt, |node| match node {
        TypeNode::ExpressionShape { ret, .. } => Some(*ret),
        _ => None,
    })
}

/// A shape's quantifier group — the render-only names, in index order. Empty for a monomorphic
/// shape and for anything that is not a shape.
pub(super) fn shape_quantifiers(kt: KType, types: &TypeRegistry) -> Vec<TypeSymbol> {
    types.with_node(kt, |node| match node {
        TypeNode::ExpressionShape { quantifiers, .. } => quantifiers.clone(),
        _ => Vec::new(),
    })
}

/// `Some(parameter names)` iff `kt` is a type constructor — a declared family (a
/// `TypeConstructor`-kind member, whose names ride its sealed schema) or a SIG's abstract
/// higher-kinded member (an `AbstractType` node carrying them directly). `None` for a first-order
/// type. Arity is the returned list's length.
pub fn constructor_param_names(kt: KType, types: &TypeRegistry) -> Option<Vec<TypeSymbol>> {
    // Owns: the parameter list is the function's own return value, so it must outlive this read.
    types.with_node(kt, |node| match node {
        TypeNode::AbstractType { param_names, .. } if !param_names.is_empty() => {
            Some(param_names.clone())
        }
        TypeNode::SetMember {
            kind: KKind::TypeConstructor,
            schema: NodeSchema::TypeConstructor { param_names, .. },
            ..
        } => Some(param_names.clone()),
        _ => None,
    })
}

/// Order-blind comparison of two constructor parameter lists: identity is the name set, and
/// declaration order is presentation. Symbol order is the canonical order — an arbitrary but
/// stable total order over the same names, which is all a set comparison needs.
pub(super) fn name_sets_equal(left: &[TypeSymbol], right: &[TypeSymbol]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut left: Vec<TypeSymbol> = left.to_vec();
    let mut right: Vec<TypeSymbol> = right.to_vec();
    left.sort_unstable();
    right.sort_unstable();
    left == right
}

/// Classify a SIG type-table entry by its *representation*: an abstract member is a rigid variable
/// with no concrete witness, which is exactly an `AbstractType` node — the first-order `TYPE Elt`
/// slot and the higher-kinded `TYPE (Elem AS Wrap)` slot alike. Everything else — a manifest
/// binding of a concrete type, a minted constructor family — is manifest.
pub fn is_abstract_sig_member(kt: KType, types: &TypeRegistry) -> bool {
    types.with_node(kt, |node| matches!(node, TypeNode::AbstractType { .. }))
}
