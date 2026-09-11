//! The run's type registry: the single owner of every type's content, plus a flat map of relation
//! verdicts.
//!
//! Content lives in `nodes`, a persistent hash-array-mapped trie keyed by [`TypeDigest`]. A
//! [`KType`] handle *is* the digest of its node, so the handle is also its own lookup key, and the
//! digest is already a uniformly distributed hash — the map hashes it with `IdentityHasher`,
//! making a lookup cost about what an array index would. Interning is insert-if-absent, so
//! building the same content twice in a run yields one node and two equal handles. Nothing ever
//! leaves the map: the graph drops with the run frame that owns it.
//!
//! Verdicts are a separate map keyed by `(subject digest, candidate digest, relation)`. A verdict
//! over a digest pair is a pure function — once computed it never changes — so verdicts are never
//! load-bearing: a cold registry costs a re-walk of the relation, never a wrong answer.
//!
//! See [design/typing/type-lattice.md](../../design/typing/type-lattice.md).

use std::cell::RefCell;
use std::collections::HashMap;

use imbl::shared_ptr::RcK;
use smallvec::SmallVec;

use crate::memory::ScopeId;
use crate::parse::{IdentityBuildHasher, Symbol, TypeSymbol};

use super::digest::{self, TypeDigest, schema_content_digest};
use super::handle::KType;
use super::kind::KKind;
use super::node::{NodeSchema, TypeNode};
use super::order::is_subtype_of;
use super::record::Record;
use super::schema::SigSchema;
use super::shape::DispatchTokenElement;
use super::substitute::substitute_quantified;
use super::walk::Variance;
use super::walk::unary::{Descent, Step, Visit, visit, visit_in};

/// A union's members under construction. Inline up to four — the width that covers a hand-written
/// `A | B | C` and the variant lists of all but the widest `UNION` declarations — so the common
/// union costs no heap allocation to canonicalize.
type MemberList = SmallVec<[KType; 4]>;

/// The node table: a persistent HAMT over `RcK`, the non-atomic shared pointer. A registry is
/// owned by exactly one run frame and never crosses a thread. Persistence buys an `O(1)` snapshot
/// for bulk walks.
pub type NodeMap = imbl::GenericHashMap<TypeDigest, TypeNode, IdentityBuildHasher, RcK>;

/// Which question a recorded verdict answers. The two never alias — each digest domain is disjoint
/// by construction — but the enum still keys the map explicitly.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Relation {
    /// [`is_subtype_of`](super::order::is_subtype_of), the one order.
    Subtype,
    /// [`sig_subtype`](super::sig_relations::sig_subtype) over the two schemas.
    SigSatisfies,
}

/// What interning a shape produced: the canonical handle, and how the caller's declaration-order
/// quantifier indices map onto the canonical group.
#[derive(Clone, Debug)]
pub struct ShapeIntern {
    pub handle: KType,
    /// Declaration index → canonical index, `None` for a variable canonical form dropped.
    pub quantifier_map: SmallVec<[Option<usize>; 4]>,
}

/// The store of type content and relation verdicts. Interior mutability via `RefCell`, in
/// independent cells: a read of `nodes` takes an `O(1)` snapshot and releases the cell before its
/// closure runs, so reads nest freely and a reader may intern, while `verdicts` is written under
/// its own borrow.
pub struct TypeRegistry {
    nodes: RefCell<NodeMap>,
    verdicts: RefCell<HashMap<(TypeDigest, TypeDigest, Relation), bool>>,
    quantified: RefCell<HashMap<TypeDigest, bool, IdentityBuildHasher>>,
    quantifiers_exist: std::cell::Cell<bool>,
}

impl Default for TypeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeRegistry {
    /// Pre-seeds the fixed handles — the leaves, the `OfKind` values, `List<Any>`, `Dict<Any, Any>`
    /// and the empty signature — so the constants those names lower to are dereferenceable in a
    /// registry that has interned nothing else.
    pub fn new() -> Self {
        let registry = Self {
            nodes: RefCell::new(NodeMap::with_hasher(IdentityBuildHasher::default())),
            verdicts: RefCell::new(HashMap::new()),
            quantified: RefCell::new(HashMap::default()),
            quantifiers_exist: std::cell::Cell::new(false),
        };
        registry.seed_constants();
        registry
    }

    /// Intern every constant node, so a fixed handle always resolves.
    fn seed_constants(&self) {
        for leaf in [
            TypeNode::Number,
            TypeNode::Str,
            TypeNode::Bool,
            TypeNode::Null,
            TypeNode::Identifier,
            TypeNode::NameToken,
            TypeNode::TypeNameToken,
            TypeNode::KExpression,
            TypeNode::SigiledTypeExpr,
            TypeNode::RecordType,
            TypeNode::Any,
            TypeNode::Never,
        ] {
            self.intern(leaf);
        }
        for kind in [
            KKind::ProperType,
            KKind::Signature,
            KKind::AnyType,
            KKind::NewType,
            KKind::TypeConstructor,
        ] {
            self.intern(TypeNode::OfKind(kind));
        }
        let any = self.intern(TypeNode::Any);
        self.list(any);
        self.dict(any, any);
        self.signature(SigSchema::empty());
    }

    // --- Content: interning and node reads ---

    /// Intern `node` and return its handle. Interning the same content twice yields one node and
    /// two equal handles.
    ///
    /// Reachable from inside a [`with_node`](Self::with_node) closure: that read borrows the cell
    /// only long enough to take its snapshot, so the write borrow here is uncontended.
    pub fn intern(&self, node: TypeNode) -> KType {
        self.intern_digested(digest::node_digest(&node), || node)
    }

    /// Insert the node `build` produces under `digest` if the digest is not already present — the
    /// one insert-if-absent path every interning door takes.
    ///
    /// One borrow around one probe: a hit — the steady state, since a repeated spelling of a type
    /// names an already-interned node — never calls `build`, so a caller that can compute its
    /// digest without materializing the node pays for neither.
    ///
    /// `build` runs under the write borrow, so unlike a [`with_node`](Self::with_node) closure it
    /// may not read or intern.
    fn intern_digested(&self, digest: TypeDigest, build: impl FnOnce() -> TypeNode) -> KType {
        let mut nodes = self.nodes.borrow_mut();
        if !nodes.contains_key(&digest) {
            nodes.insert(digest, build());
        }
        KType::from_digest(digest)
    }

    /// Read the content `handle` names **by reference** and hand back whatever `read` derives.
    ///
    /// The one read door. `read`'s result type is fixed at the call site and the node's lifetime is
    /// this call's, so no reference into the table can escape.
    ///
    /// The node is read out of a **snapshot** — `nodes` is a persistent HAMT, so cloning it is a
    /// root-pointer bump and the `RefCell` borrow ends before `read` runs. That is what lets a
    /// reader intern, and what makes reads nest freely.
    ///
    /// A miss is a bug, not a state: a handle is only ever produced by [`Self::intern`], and the
    /// table is insert-only.
    pub fn with_node<R>(&self, handle: KType, read: impl FnOnce(&TypeNode) -> R) -> R {
        let digest = handle.digest();
        let snapshot = self.nodes.borrow().clone();
        match snapshot.get(&digest) {
            Some(node) => read(node),
            None => panic!("type handle 0x{:032x} names no interned node", digest.0),
        }
    }

    /// The content `handle` names, cloned out of the table — [`with_node`](Self::with_node) for a
    /// caller that needs the node to outlive the read.
    pub fn node(&self, handle: KType) -> TypeNode {
        self.with_node(handle, TypeNode::clone)
    }

    /// Whether `handle` names a union. The construction lane's first probe, so it answers from the
    /// node's shape alone and never reads out the member list.
    pub fn is_union(&self, handle: KType) -> bool {
        matches!(
            self.nodes.borrow().get(&handle.digest()),
            Some(TypeNode::Union { .. })
        )
    }

    /// The `union` member named `name`, whatever schema it declares. `name` probes by bare symbol
    /// bits: the token arrives from a reference site with no class attached.
    pub fn union_member_named(&self, union: KType, name: Symbol) -> Option<KType> {
        let nodes = self.nodes.borrow();
        let Some(TypeNode::Union { members }) = nodes.get(&union.digest()) else {
            return None;
        };
        members.iter().copied().find(|m| {
            matches!(
                nodes.get(&m.digest()),
                Some(TypeNode::SetMember {
                    name: member_name, ..
                }) if member_name.symbol() == name
            )
        })
    }

    /// An `O(1)` snapshot of the node table, for a bulk walk that would otherwise want to hold the
    /// borrow open. The snapshot shares structure with the live table and does not observe later
    /// interning — which is what makes it safe to walk while interning.
    pub fn nodes_snapshot(&self) -> NodeMap {
        self.nodes.borrow().clone()
    }

    // --- Composite construction ---
    //
    // The single entry point per composite shape. Each takes child handles and returns the
    // parent's handle, so building a type is bottom-up interning and no site can construct a
    // composite the registry has not seen.

    /// `List<element>`.
    pub fn list(&self, element: KType) -> KType {
        self.intern(TypeNode::List { element })
    }

    /// `Dict<key, value>`.
    pub fn dict(&self, key: KType, value: KType) -> KType {
        self.intern(TypeNode::Dict { key, value })
    }

    /// A structural record type over `fields`.
    pub fn record(&self, fields: Record<KType>) -> KType {
        self.intern(TypeNode::Record { fields })
    }

    /// A function type `(params) -> ret`.
    pub fn function_type(&self, params: Record<KType>, ret: KType) -> KType {
        self.intern(TypeNode::KFunction { params, ret })
    }

    /// Application of a higher-kinded type constructor to the parameter-name-keyed `arguments`.
    pub fn constructor_apply(&self, constructor: KType, arguments: Record<KType>) -> KType {
        self.intern(TypeNode::ConstructorApply {
            constructor,
            arguments,
        })
    }

    /// A named rigid variable — a signature's abstract member, or an opaque ascription's
    /// per-application mint when `nonce` is set. `bound` is what it stands over, [`KType::ANY`]
    /// where the declaration constrains nothing.
    ///
    /// A bound holds no rigid variable of its own — see [`contains_rigid`](Self::contains_rigid).
    pub fn abstract_type(
        &self,
        source: ScopeId,
        name: TypeSymbol,
        param_names: Vec<TypeSymbol>,
        nonce: Option<ScopeId>,
        bound: KType,
    ) -> KType {
        debug_assert!(
            !self.contains_rigid(bound),
            "a rigid variable's bound holds no rigid variable of its own",
        );
        self.intern(TypeNode::AbstractType {
            source,
            name,
            param_names,
            nonce,
            bound,
        })
    }

    /// The `index`-th rigid variable of the enclosing shape's group, standing over `bound`. The one
    /// door a `Quantified` node is born through, so it is also where the run learns it has any —
    /// see [`contains_quantified`](Self::contains_quantified).
    ///
    /// A bound holds no rigid variable of its own — see [`contains_rigid`](Self::contains_rigid).
    pub fn quantified(&self, index: usize, bound: KType) -> KType {
        debug_assert!(
            !self.contains_rigid(bound),
            "a rigid variable's bound holds no rigid variable of its own",
        );
        self.quantifiers_exist.set(true);
        self.intern(TypeNode::Quantified { index, bound })
    }

    /// A relative sibling reference against an open recursive-group window.
    pub fn sibling(&self, index: usize) -> KType {
        self.intern(TypeNode::Sibling(index))
    }

    /// One sealed member of a recursive group. Built only by the seal, which derives the handle
    /// from the component digest first and interns the node under it.
    pub(super) fn set_member(
        &self,
        scc_digest: TypeDigest,
        index: usize,
        scc_size: usize,
        name: TypeSymbol,
        kind: KKind,
        schema: NodeSchema,
    ) -> KType {
        self.intern(TypeNode::SetMember {
            scc_digest,
            index,
            scc_size,
            name,
            kind,
            schema,
        })
    }

    /// A module-signature type over `schema`. Computes the schema's content digest once, here, so
    /// the node carries it and identity is one compare.
    pub fn signature(&self, schema: SigSchema) -> KType {
        let schema_digest = schema_content_digest(&schema, self);
        self.intern(TypeNode::Signature {
            schema,
            schema_digest,
        })
    }

    // --- Shapes ---

    /// The one door that mints an expression shape, in **canonical form**.
    ///
    /// A variable with no occurrence is dropped; one that occurs exactly once is replaced by its
    /// bound where the occurrence is contravariant and by [`KType::NEVER`] where it is covariant,
    /// since a single occurrence is equivalent to that replacement in every admission and every
    /// subtype question; survivors are renumbered by first occurrence in element order, then
    /// return. Canonical form is what makes the order antisymmetric once quantified shapes relate
    /// by instantiation: two shapes each below the other must be one handle.
    ///
    /// Argument names never reach here — they are binder-side — and the surviving quantifier names
    /// are render-only, so two shapes alpha-equivalent under a renaming intern to the node
    /// whichever spelling built first. The returned map translates a caller's declaration-order
    /// bindings into the canonical group's order.
    pub fn shape_type(
        &self,
        quantifiers: &[TypeSymbol],
        elements: &[DispatchTokenElement],
        ret: KType,
    ) -> ShapeIntern {
        if quantifiers.is_empty() {
            return ShapeIntern {
                handle: self.intern_shape(&[], elements, ret),
                quantifier_map: SmallVec::new(),
            };
        }
        let census = self.quantifier_census(elements, ret, quantifiers.len());

        // Survivors keep their occurrences; everything else substitutes away. Renumbering follows
        // first occurrence, which the census recorded as a visit sequence number.
        let mut survivors: Vec<usize> = (0..quantifiers.len())
            .filter(|index| census[*index].occurrences() >= 2)
            .collect();
        survivors.sort_by_key(|index| census[*index].first);
        let mut quantifier_map: SmallVec<[Option<usize>; 4]> =
            smallvec::smallvec![None; quantifiers.len()];
        for (canonical, declared) in survivors.iter().enumerate() {
            quantifier_map[*declared] = Some(canonical);
        }

        let bindings: Vec<KType> = (0..quantifiers.len())
            .map(|index| {
                let seen = &census[index];
                match quantifier_map[index] {
                    Some(canonical) => self.quantified(canonical, seen.bound),
                    // A lone covariant occurrence must hold at every instantiation, which only the
                    // bottom does; a lone contravariant one is free for the caller to pick, which
                    // is exactly its bound.
                    None if seen.covariant > 0 => KType::NEVER,
                    None => seen.bound,
                }
            })
            .collect();

        let canonical_names: Vec<TypeSymbol> =
            survivors.iter().map(|index| quantifiers[*index]).collect();
        let elements: SmallVec<[DispatchTokenElement; 12]> = elements
            .iter()
            .map(|element| match element {
                DispatchTokenElement::Slot(kt) => {
                    DispatchTokenElement::Slot(substitute_quantified(self, *kt, &bindings))
                }
                keyword => *keyword,
            })
            .collect();
        let ret = substitute_quantified(self, ret, &bindings);
        debug_assert!(
            canonical_names.is_empty()
                || self.quantifier_indices_in_range(ret, canonical_names.len()),
            "every quantified position names an index of the shape's own canonical group",
        );
        ShapeIntern {
            handle: self.intern_shape(&canonical_names, &elements, ret),
            quantifier_map,
        }
    }

    /// Intern a shape whose form is already canonical.
    ///
    /// Probe-first: every definition mints its callable's shape, and re-running one definition —
    /// a lambda inside a loop body — mints a shape already interned. Taking the digest off the
    /// borrowed run means the boxed run and the quantifier vector are built only on a genuine miss.
    fn intern_shape(
        &self,
        quantifiers: &[TypeSymbol],
        elements: &[DispatchTokenElement],
        ret: KType,
    ) -> KType {
        let handle = digest::shape_digest(quantifiers.len(), elements, ret.digest());
        self.intern_digested(handle, || TypeNode::ExpressionShape {
            quantifiers: quantifiers.to_vec(),
            elements: elements.iter().copied().collect(),
            ret,
        })
    }

    /// Count each variable's free occurrences across a shape's argument positions and return,
    /// under the polarity of the position it was met at, recording its bound and the visit order of
    /// its first occurrence. A nested shape's own group shadows this one, so the census skips it.
    fn quantifier_census(
        &self,
        elements: &[DispatchTokenElement],
        ret: KType,
        arity: usize,
    ) -> Vec<Occurrences> {
        let mut census = vec![Occurrences::default(); arity];
        let mut seen = 0usize;
        let leaf = Descent {
            signature: Step::Leaf,
            set_member: Step::Leaf,
        };
        let mut count = |kt: KType, position: Variance| {
            visit_in(
                self,
                kt,
                leaf,
                position,
                &mut |_, node, context| match node {
                    TypeNode::ExpressionShape { .. } => Visit::Skip,
                    TypeNode::Quantified { index, bound } => {
                        if let Some(record) = census.get_mut(*index) {
                            if record.first == usize::MAX {
                                record.first = seen;
                                record.bound = *bound;
                            }
                            match context.variance() {
                                Variance::Co => record.covariant += 1,
                                Variance::Contra => record.contravariant += 1,
                            }
                            seen += 1;
                        }
                        Visit::Skip
                    }
                    _ => Visit::Descend,
                },
            );
        };
        for element in elements {
            if let DispatchTokenElement::Slot(kt) = element {
                count(*kt, Variance::Contra);
            }
        }
        count(ret, Variance::Co);
        census
    }

    // --- Unions ---

    /// The canonicalizing constructor for a union — the single entry point that builds one.
    ///
    /// Flattens any nested union member into its members, drops [`KType::NEVER`] (the identity
    /// element: it admits nothing, so it widens nothing), deduplicates by handle, then drops every
    /// member that is a subtype of another member, so [`KType::ANY`] absorbs and no two distinct
    /// members are ordered. One survivor collapses to that member; none is `Never`.
    pub fn union_of(&self, members: &[KType]) -> KType {
        let mut flat: MemberList = MemberList::with_capacity(members.len());
        let push_unique = |handle: KType, flat: &mut MemberList| {
            if handle != KType::NEVER && !flat.contains(&handle) {
                flat.push(handle);
            }
        };
        for member in members {
            // Read in place: the flatten pass pushes handles and interns nothing, so the table
            // borrow the read holds is closed again before the intern below opens its own.
            self.with_node(*member, |node| match node {
                TypeNode::Union { members: inner } => {
                    for nested in inner {
                        push_unique(*nested, &mut flat);
                    }
                }
                _ => push_unique(*member, &mut flat),
            });
        }
        if flat.len() > 1 {
            // Subsumption: a member below another contributes nothing the other does not already
            // admit. Mutually ordered members are equal handles, which the dedup above removed, so
            // the surviving set is an antichain and dropping is order-insensitive.
            let candidates = flat.clone();
            flat.retain(|member| {
                !candidates
                    .iter()
                    .any(|peer| peer != member && is_subtype_of(self, *member, *peer))
            });
        }
        if flat.is_empty() {
            return KType::NEVER;
        }
        if flat.len() == 1 {
            return flat[0];
        }
        self.intern_union_members(flat)
    }

    /// Intern a union from members that are already flat and already an antichain — dedup by handle
    /// and collapse a one-member result, but read no member nodes.
    ///
    /// The seal's door, and the seal's alone. A rewritten sibling handle names a still-uninterned
    /// member of the group being sealed, so [`union_of`](Self::union_of)'s flatten pass would fault
    /// on it. It is sound there because the rename `Sibling(i) ↦ member_i` preserves every
    /// subsumption verdict: both are atoms, below only themselves and `Never`, and distinct indices
    /// name distinct members — so a union canonical before the seal is canonical after it.
    pub fn intern_union_flat(&self, members: &[KType]) -> KType {
        let mut flat: MemberList = MemberList::with_capacity(members.len());
        for member in members {
            if !flat.contains(member) {
                flat.push(*member);
            }
        }
        if flat.is_empty() {
            return KType::NEVER;
        }
        if flat.len() == 1 {
            return flat[0];
        }
        self.intern_union_members(flat)
    }

    /// Intern the `Union` node over the already-canonical `flat`, probing the table before building
    /// the node. The `Union` arm of `node_digest` *is* `union_digest` over the node's member slice,
    /// so the digest taken here off `flat` equals the digest the node would key at — which makes
    /// the node itself needed only on a miss.
    fn intern_union_members(&self, flat: MemberList) -> KType {
        let handle = digest::union_digest(&flat);
        self.intern_digested(handle, || TypeNode::Union {
            members: flat.into_vec(),
        })
    }

    // --- Quantifier probes ---

    /// Whether any `Quantified` position is reachable from `kt` without crossing a shape's own
    /// binder — the probe that lets a slot type with nothing to solve answer the relations in one
    /// step instead of walking under a unifier.
    ///
    /// A run that has interned no `Quantified` node at all — every run that writes no `FOR ALL` —
    /// answers from one flag and never walks: [`Self::quantified`] is the only door such a node is
    /// born through, and a handle names an interned node, so the flag is exact. A run that does
    /// quantify memoizes per digest, on the same terms as the verdict cache: a digest names fixed
    /// content, so the answer is a pure function of the key and can never go stale.
    pub fn contains_quantified(&self, kt: KType) -> bool {
        if !self.quantifiers_exist.get() {
            return false;
        }
        if let Some(known) = self.quantified.borrow().get(&kt.digest()) {
            return *known;
        }
        let answer = visit(
            self,
            kt,
            Descent {
                signature: Step::Leaf,
                set_member: Step::Leaf,
            },
            &mut |node_handle, node, _| match node {
                // A shape's own variables are bound by it, so nothing under one is free here.
                TypeNode::ExpressionShape { .. } => Visit::Skip,
                TypeNode::Quantified { .. } => Visit::Stop,
                _ => match self.quantified.borrow().get(&node_handle.digest()) {
                    Some(true) => Visit::Stop,
                    Some(false) => Visit::Skip,
                    None => Visit::Descend,
                },
            },
        );
        self.quantified.borrow_mut().insert(kt.digest(), answer);
        answer
    }

    /// Whether any rigid variable — `Quantified` or `AbstractType` — is reachable from `kt`.
    ///
    /// The invariant a **bound** carries: a bound is a variable-free type. That is what keeps the
    /// order's two rigid clauses consistent, since below a rigid variable are only itself and
    /// `Never` while above it is everything above its bound — and a rigid bound would put a
    /// variable in both sets at once. The two doors that mint a rigid variable assert it, so a
    /// caller that reaches for a rigid bound fails a test rather than producing a wrong verdict.
    pub fn contains_rigid(&self, kt: KType) -> bool {
        visit(
            self,
            kt,
            Descent {
                signature: Step::Leaf,
                set_member: Step::Leaf,
            },
            &mut |_, node, _| match node {
                TypeNode::Quantified { .. } | TypeNode::AbstractType { .. } => Visit::Stop,
                _ => Visit::Descend,
            },
        )
    }

    /// Whether `kt` reads the `index`-th quantifier of the enclosing shape — what a definition asks
    /// of each name its `FOR ALL` group lists.
    pub fn references_quantifier(&self, kt: KType, index: usize) -> bool {
        self.contains_quantified(kt)
            && visit(
                self,
                kt,
                Descent {
                    signature: Step::Leaf,
                    set_member: Step::Leaf,
                },
                &mut |_, node, _| match node {
                    TypeNode::ExpressionShape { .. } => Visit::Skip,
                    TypeNode::Quantified { index: found, .. } if *found == index => Visit::Stop,
                    TypeNode::Quantified { .. } => Visit::Skip,
                    _ => Visit::Descend,
                },
            )
    }

    /// Whether every free `Quantified` position reachable from `kt` names an index below `arity` —
    /// the [`shape_type`](Self::shape_type) well-formedness probe.
    pub(super) fn quantifier_indices_in_range(&self, kt: KType, arity: usize) -> bool {
        !visit(
            self,
            kt,
            Descent {
                signature: Step::Leaf,
                set_member: Step::Leaf,
            },
            &mut |_, node, _| match node {
                TypeNode::ExpressionShape { .. } => Visit::Skip,
                TypeNode::Quantified { index, .. } if *index >= arity => Visit::Stop,
                TypeNode::Quantified { .. } => Visit::Skip,
                _ => Visit::Descend,
            },
        )
    }

    // --- Verdicts ---

    /// Consult the registry for a recorded verdict.
    pub(super) fn verdict(
        &self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
    ) -> Option<bool> {
        self.verdicts
            .borrow()
            .get(&(subject, candidate, relation))
            .copied()
    }

    /// Record `verdict` for the key. Negative verdicts are recorded exactly as positive ones.
    pub(super) fn record_verdict(
        &self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
        verdict: bool,
    ) {
        self.verdicts
            .borrow_mut()
            .insert((subject, candidate, relation), verdict);
    }
}

/// One variable's census entry while a shape is being canonicalized.
#[derive(Clone, Copy)]
struct Occurrences {
    covariant: usize,
    contravariant: usize,
    /// Visit sequence number of the first occurrence — the renumbering key. `usize::MAX` until the
    /// variable is met at all.
    first: usize,
    bound: KType,
}

impl Occurrences {
    fn occurrences(&self) -> usize {
        self.covariant + self.contravariant
    }
}

impl Default for Occurrences {
    fn default() -> Self {
        Occurrences {
            covariant: 0,
            contravariant: 0,
            first: usize::MAX,
            bound: KType::ANY,
        }
    }
}
