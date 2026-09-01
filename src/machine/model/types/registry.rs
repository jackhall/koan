//! The run's type registry: the single owner of every type's content, plus a flat map of subtype
//! verdicts.
//!
//! Content lives in `nodes`, a persistent hash-array-mapped trie keyed by [`TypeDigest`]. A
//! [`KType`] handle *is* the digest of its node ([`type_digest`]), so the handle is also its own
//! lookup key, and the digest is already a uniformly distributed hash — the map hashes it with
//! [`IdentityHasher`], making a lookup cost about what an array index would. Interning is
//! insert-if-absent, so building the same content twice in a run yields one node and two equal
//! handles. Nothing ever leaves the map: the graph drops with the run frame that owns it.
//!
//! Verdicts are a separate map keyed by `(subject digest, candidate digest, relation)`. A subtype
//! verdict over a digest pair is a pure function — once computed it never changes — so any
//! granularity is observationally identical, and verdicts are never load-bearing: a cold registry
//! costs a re-walk of the structural predicate, never a wrong answer. Keeping them separable from
//! content is what lets a future cross-thread transfer move nodes without moving cache.
//!
//! One [`TypeRegistry`] hangs off the scheduler-owned run frame, inside the
//! [`RunRegistries`](crate::machine::model::RunRegistries) bundle it shares with the label
//! interner, reached by reference through the execution context — so there is no process-global
//! state.
//!
//! See [design/typing/type-registry.md](../../../../design/typing/type-registry.md).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use imbl::shared_ptr::RcK;
use smallvec::SmallVec;

use crate::machine::model::labels::Symbol;

use super::kkind::KKind;
use super::ktype::KType;
use super::node::TypeNode;
use super::record::Record;
use super::sig_schema::SigSchema;
use super::type_digest::{self, TypeDigest, schema_content_digest};

/// A union's members under construction. Inline up to four — the width that covers a hand-written
/// `A | B | C` and the variant lists of all but the widest `UNION` declarations — so the common
/// union costs no heap allocation to canonicalize, and none at all when the result is a node the
/// registry already holds.
type MemberList = SmallVec<[KType; 4]>;

/// The hasher every 128-bit-digest-keyed table runs: the node table here, the label interner, and
/// the classified scope binding tables
/// ([design/label-interning.md](../../../../design/label-interning.md)). A [`TypeDigest`] and a
/// [`Symbol`](crate::machine::model::Symbol) are each the low 128 bits of a BLAKE3 hash, so they
/// are already uniformly distributed and re-hashing would only cost cycles: keep the low 64 bits
/// and use them directly as the bucket index.
///
/// Every other write is a bug — such a table is keyed by one `u128` digest and nothing else, so a
/// call to any other `write_*` means a key type slipped in that this hasher cannot distribute.
#[derive(Default)]
pub struct IdentityHasher(u64);

impl Hasher for IdentityHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, _bytes: &[u8]) {
        panic!("an identity-hashed table is keyed by a single 128-bit digest and nothing else");
    }

    fn write_u128(&mut self, value: u128) {
        self.0 = value as u64;
    }
}

/// [`IdentityHasher`] as a `BuildHasher`.
pub type IdentityBuildHasher = BuildHasherDefault<IdentityHasher>;

/// The node table: a persistent HAMT over `RcK`, the non-atomic shared pointer. A registry is
/// owned by exactly one run frame and never crosses a thread, so the atomic pointer kind would
/// pay for a guarantee nothing needs. Persistence buys an `O(1)` snapshot for bulk walks — and
/// keeps the structure-sharing merge live as a cross-thread transfer mechanism.
pub type NodeMap = imbl::GenericHashMap<TypeDigest, TypeNode, IdentityBuildHasher, RcK>;

/// Which subtype question a recorded verdict answers. `MoreSpecific` is
/// `KType::is_more_specific_than`'s strict specificity walk; `SigSatisfies` is
/// `sig_subtype(schema-of-subject, schema-of-candidate).is_ok()`, where "schema-of" a
/// module-identity digest is the module's self-sig and "schema-of" a signature-identity
/// digest is `SigSchema::of_sig`. The two relations never alias — each digest domain
/// (`TAG_SIGNATURE` / the composite tags) is disjoint by construction — but the enum still
/// keys the map explicitly so the two questions never share an entry.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Relation {
    MoreSpecific,
    SigSatisfies,
}

/// The run-scoped store of type content and subtype verdicts. Interior mutability via `RefCell`,
/// in two independent cells: a read of `nodes` spans the reading closure and may nest freely
/// inside another, while `verdicts` is written under its own borrow, so a memoizing walk records
/// its verdict without touching the read it is running under. What no read may do is intern —
/// [`TypeRegistry::intern`] names the rule. Both maps are uncapped: they drop with the run frame
/// that owns them, so growth is bounded by the run.
pub struct TypeRegistry {
    nodes: RefCell<NodeMap>,
    verdicts: RefCell<HashMap<(TypeDigest, TypeDigest, Relation), bool>>,
    #[cfg(test)]
    hits: std::cell::Cell<usize>,
    #[cfg(test)]
    misses: std::cell::Cell<usize>,
}

impl TypeRegistry {
    /// Crate-internal. Pre-seeds the fixed handles — the leaves, the `OfKind` values,
    /// `List<Any>`, `Dict<Any, Any>`, and the empty signature — so the constants those names
    /// lower to are dereferenceable in a registry that has interned nothing else.
    pub(crate) fn new() -> Self {
        let registry = Self {
            nodes: RefCell::new(NodeMap::with_hasher(IdentityBuildHasher::default())),
            verdicts: RefCell::new(HashMap::new()),
            #[cfg(test)]
            hits: std::cell::Cell::new(0),
            #[cfg(test)]
            misses: std::cell::Cell::new(0),
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
    //
    // [`with_node`](Self::with_node) is the one read door; `node` and the per-query verbs below are
    // written over it or over the same borrow. The borrow spans the reading closure, so **a read
    // must not intern** — the single rule this section rests on, checked by
    // [`intern`](Self::intern)'s own diagnostic below.

    /// Intern `node` and return its handle. Computes the node's digest, inserts it if the digest
    /// is not already present, and returns the digest as a [`KType`]. Interning the same content
    /// twice yields one node and two equal handles.
    ///
    /// Never reachable from inside a [`with_node`](Self::with_node) closure — the read holds the
    /// table borrow, so an intern under one is the rule violation the `expect` below names.
    pub fn intern(&self, node: TypeNode) -> KType {
        let digest = type_digest::node_digest(&node);
        let mut nodes = self
            .nodes
            .try_borrow_mut()
            .expect("a node read must not intern: the read holds the table borrow");
        if !nodes.contains_key(&digest) {
            nodes.insert(digest, node);
        }
        KType::from_digest(digest)
    }

    /// Read the content `handle` names **by reference**, under the table borrow, and hand back
    /// whatever `read` derives from it.
    ///
    /// The one read door. `read`'s result type is fixed at the call site and the node's lifetime is
    /// this call's, so no reference into the table can escape: a reader is confined to derived
    /// data by construction, which is what lets a shape probe answer without copying the node.
    /// Reads nest freely — the borrow is shared — but a reader must not intern.
    ///
    /// A miss is a bug, not a state: a handle is only ever produced by [`Self::intern`], and the
    /// table is insert-only.
    pub fn with_node<R>(&self, handle: KType, read: impl FnOnce(&TypeNode) -> R) -> R {
        let digest = handle.digest();
        match self.nodes.borrow().get(&digest) {
            Some(node) => read(node),
            None => panic!("type handle 0x{:032x} names no interned node", digest.0),
        }
    }

    /// The content `handle` names, cloned out of the table — [`with_node`](Self::with_node) for a
    /// caller that needs the node to outlive the read. A node is shallow — scalar payload plus
    /// child handles — so the clone never copies a type subtree, but a variant carrying a field
    /// record, a member list or a schema allocates, which is why a shape probe reads by reference
    /// instead.
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

    /// The `union` member named `name`, whatever schema it declares — the probe every
    /// variant-reference surface reads through ([`union_member`](crate::builtins::union::union_member)),
    /// where naming a member yields its type value and a constructor family is as referenceable as
    /// a variant. `name` probes by bare symbol bits: the token arrives from a reference site with no
    /// class attached, and the member nodes it is matched against carry the `TypeSymbol` their
    /// declaration minted.
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

    /// An `O(1)` snapshot of the node table, for a bulk walk that would otherwise want to hold
    /// the borrow open. The snapshot shares structure with the live table and does not observe
    /// later interning — which is what makes it safe to walk while interning.
    pub fn nodes_snapshot(&self) -> NodeMap {
        self.nodes.borrow().clone()
    }

    // --- Composite construction ---
    //
    // The single entry point per composite shape. Each takes child handles and returns the
    // parent's handle, so building a type is bottom-up interning and no site can construct a
    // composite that the registry has not seen.

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

    /// Application of a higher-kinded type constructor to the parameter-name-keyed `arguments`,
    /// which the caller builds in the constructor's declared parameter order.
    pub fn constructor_apply(&self, constructor: KType, arguments: Record<KType>) -> KType {
        self.intern(TypeNode::ConstructorApply {
            constructor,
            arguments,
        })
    }

    /// A module-signature type over `schema`. Computes the schema's content digest once, here,
    /// so the node carries it and identity is one compare. `WITH` specialization folds its pins
    /// into the schema first ([`SigSchema::fold_pins`]) and interns through this same door —
    /// there is one signature constructor and one spelling per interface content.
    pub fn signature(&self, schema: SigSchema) -> KType {
        let schema_digest = schema_content_digest(&schema, self);
        self.intern(TypeNode::Signature {
            schema,
            schema_digest,
        })
    }

    /// Canonicalizing constructor for a union — the single entry point that builds one. Flattens
    /// any nested union member into its members, deduplicates by handle, and collapses a single
    /// surviving member to that member (`:(A | A)` is `:A`). Callers guarantee at least one
    /// member.
    pub fn union_of(&self, members: &[KType]) -> KType {
        debug_assert!(!members.is_empty(), "union_of requires at least one member");
        let mut flat: MemberList = MemberList::with_capacity(members.len());
        let push_unique = |handle: KType, flat: &mut MemberList| {
            if !flat.contains(&handle) {
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
        if flat.len() == 1 {
            return flat[0];
        }
        self.intern_union_members(flat)
    }

    /// Intern a union from members that are already flat (no member is itself a `Union`) — dedup by
    /// handle and collapse a one-member result, but read no member nodes. The seal's sibling
    /// rewrite ([`rewrite_siblings`](super::recursive_group_window)) uses this: a rewritten sibling
    /// handle names a still-uninterned member of the group being sealed, so the node-reading
    /// [`Self::union_of`] flatten pass would fault on it — and a group member is always a
    /// `SetMember`, never a nested `Union`, so flattening is a no-op here anyway.
    pub fn intern_union_flat(&self, members: &[KType]) -> KType {
        debug_assert!(
            !members.is_empty(),
            "intern_union_flat requires at least one member"
        );
        let mut flat: MemberList = MemberList::with_capacity(members.len());
        for member in members {
            if !flat.contains(member) {
                flat.push(*member);
            }
        }
        if flat.len() == 1 {
            return flat[0];
        }
        self.intern_union_members(flat)
    }

    /// Intern the `Union` node over the already-canonical `flat`, probing the table before
    /// building the node. The `Union` arm of [`node_digest`](type_digest::node_digest) *is*
    /// [`union_digest`](type_digest::union_digest) over the node's member slice, so the digest
    /// taken here off `flat` is equal by construction to the digest the node would key at — which
    /// makes the node itself needed only on a miss. Both union constructors above build their
    /// members in a stack-sized [`MemberList`], so a repeat union — the steady state inside a
    /// loop, where every evaluation of `A | B` names one already-interned node — builds neither a
    /// member buffer nor a node, and `union_digest` sorts its member digests inline. So a union up
    /// to [`MemberList`]'s inline width allocates nothing at all. Past it the `with_capacity` above
    /// takes a heap buffer off the *input* length, hit or miss — one allocation a wide `UNION`'s
    /// canonicalization pays, which is what the inline width is chosen against. `flat` arrives
    /// owned so the miss path hands its buffer to the node rather than copying it: a member list
    /// wide enough to have spilled is moved, not reallocated.
    ///
    /// Keeps [`intern`](Self::intern)'s "a node read must not intern" discipline: the probe's
    /// shared borrow ends with the statement that takes it, before the mutable borrow below.
    fn intern_union_members(&self, flat: MemberList) -> KType {
        let digest = type_digest::union_digest(&flat);
        let present = self.nodes.borrow().contains_key(&digest);
        if !present {
            let mut nodes = self
                .nodes
                .try_borrow_mut()
                .expect("a node read must not intern: the read holds the table borrow");
            nodes.insert(
                digest,
                TypeNode::Union {
                    members: flat.into_vec(),
                },
            );
        }
        KType::from_digest(digest)
    }

    /// Least-upper-bound of two types. `[1, 2]` → `List<Number>`, `[1, "x"]` → `List<Any>`;
    /// nested containers join element-wise.
    pub fn join(&self, a: KType, b: KType) -> KType {
        if a == b {
            return a;
        }
        match (self.node(a), self.node(b)) {
            (TypeNode::List { element: x }, TypeNode::List { element: y }) => {
                let element = self.join(x, y);
                self.list(element)
            }
            (
                TypeNode::Dict {
                    key: xk, value: xv, ..
                },
                TypeNode::Dict {
                    key: yk, value: yv, ..
                },
            ) => {
                let key = self.join(xk, yk);
                let value = self.join(xv, yv);
                self.dict(key, value)
            }
            (
                TypeNode::KFunction {
                    params: xp,
                    ret: xr,
                    ..
                },
                TypeNode::KFunction {
                    params: yp,
                    ret: yr,
                    ..
                },
            ) => match self.join_param_record(&xp, &yp) {
                Some(params) => {
                    let ret = self.join(xr, yr);
                    self.function_type(params, ret)
                }
                None => self.intern(TypeNode::Any),
            },
            _ => self.intern(TypeNode::Any),
        }
    }

    /// Reduce an iterator of types to their least upper bound. Empty iterator → `Any`.
    pub fn join_iter<I: IntoIterator<Item = KType>>(&self, iter: I) -> KType {
        iter.into_iter()
            .reduce(|a, b| self.join(a, b))
            .unwrap_or_else(|| self.intern(TypeNode::Any))
    }

    /// Name-keyed join of two parameter records. `Some(joined)` when the records have equal
    /// length and the same key set; `None` on differing key sets, which [`Self::join`] coarsens
    /// to `Any`.
    fn join_param_record(&self, a: &Record<KType>, b: &Record<KType>) -> Option<Record<KType>> {
        if a.len() != b.len() || !a.keys().all(|k| b.get(k.symbol()).is_some()) {
            return None;
        }
        // The joined record keeps the left operand's classified keys; both sides agree on the
        // symbol bits, which is what identity reads.
        Some(
            a.iter()
                .map(|(name, x)| (name, self.join(*x, *b.get(name.symbol()).unwrap())))
                .collect(),
        )
    }

    // --- Verdicts ---

    /// Consult the registry for a recorded verdict.
    pub(crate) fn verdict(
        &self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
    ) -> Option<bool> {
        let hit = self
            .verdicts
            .borrow()
            .get(&(subject, candidate, relation))
            .copied();
        #[cfg(test)]
        {
            if hit.is_some() {
                self.hits.set(self.hits.get() + 1);
            } else {
                self.misses.set(self.misses.get() + 1);
            }
        }
        hit
    }

    /// Record `verdict` for the key. Negative verdicts are recorded exactly as positive ones.
    pub(crate) fn record_verdict(
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

    /// Counter-verified hit assertions in the predicate tests. A fresh registry per run replaces
    /// resetting, so there is no counter reset.
    #[cfg(test)]
    pub(crate) fn hit_count(&self) -> usize {
        self.hits.get()
    }

    /// Counter-verified miss assertions in the predicate tests.
    #[cfg(test)]
    pub(crate) fn miss_count(&self) -> usize {
        self.misses.get()
    }
}

#[cfg(test)]
mod tests;
