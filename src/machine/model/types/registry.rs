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
//! One [`TypeRegistry`] hangs off the scheduler-owned run frame (`CallFrame::type_registry`),
//! reached by reference through the execution context, so there is no process-global state.
//!
//! See [design/typing/type-registry.md](../../../../design/typing/type-registry.md).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use imbl::shared_ptr::RcK;

use super::kkind::KKind;
use super::ktype::KType;
use super::node::TypeNode;
use super::record::Record;
use super::sig_schema::SigSchema;
use super::type_digest::{self, schema_content_digest, TypeDigest};

/// The node table's hasher. A [`TypeDigest`] is the low 128 bits of a BLAKE3 hash, so it is
/// already uniformly distributed and re-hashing it would only cost cycles: keep the low 64 bits
/// and use them directly as the bucket index.
///
/// Every other write is a bug — the map is keyed by `TypeDigest` and nothing else, so a call to
/// any other `write_*` means a key type slipped in that this hasher cannot distribute.
#[derive(Default)]
pub struct IdentityHasher(u64);

impl Hasher for IdentityHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, _bytes: &[u8]) {
        panic!("the node table is keyed by TypeDigest alone, which hashes as one u128");
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

/// The run-scoped store of type content and subtype verdicts. Interior mutability via `RefCell`;
/// every borrow is confined to a single method call, so no borrow spans the structural walk a
/// verdict miss falls back to, nor an `intern` a node read might feed. Both maps are uncapped:
/// they drop with the run frame that owns them, so growth is bounded by the run.
pub struct TypeRegistry {
    nodes: RefCell<NodeMap>,
    verdicts: RefCell<HashMap<(TypeDigest, TypeDigest, Relation), bool>>,
    #[cfg(test)]
    hits: std::cell::Cell<usize>,
    #[cfg(test)]
    misses: std::cell::Cell<usize>,
}

impl TypeRegistry {
    /// Crate-internal: the run frame is the only production site that mints one
    /// (`CallFrame::adopting`), and tests reach the run's registry through the
    /// `builtins::test_support::TestRun` bundle rather than minting a cold one.
    ///
    /// Pre-seeds the fixed handles — the nine leaves, the five `OfKind` values, `List<Any>`,
    /// `Dict<Any, Any>`, and the empty signature — so the constants those names lower to are
    /// dereferenceable in a registry that has interned nothing else.
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
        self.signature(SigSchema::empty(), Vec::new());
    }

    // --- Content: interning and node reads ---
    //
    // `intern` and `node` each take the `nodes` borrow for the length of one map operation and
    // drop it before returning. No method here holds that borrow across a call that can intern,
    // and no caller can: `node` hands back an owned clone rather than a reference into the map.

    /// Intern `node` and return its handle. Computes the node's digest, inserts it if the digest
    /// is not already present, and returns the digest as a [`KType`]. Interning the same content
    /// twice yields one node and two equal handles.
    pub fn intern(&self, node: TypeNode) -> KType {
        let digest = type_digest::node_digest(&node);
        let mut nodes = self.nodes.borrow_mut();
        if !nodes.contains_key(&digest) {
            nodes.insert(digest, node);
        }
        KType::from_digest(digest)
    }

    /// The content `handle` names, cloned out of the table. A node is shallow — scalar payload
    /// plus child handles — so the clone never copies a type subtree.
    ///
    /// A miss is a bug, not a state: a handle is only ever produced by [`Self::intern`], and the
    /// table is insert-only.
    pub fn node(&self, handle: KType) -> TypeNode {
        let digest = handle.digest();
        let found = self.nodes.borrow().get(&digest).cloned();
        found.unwrap_or_else(|| {
            panic!("type handle 0x{:032x} names no interned node", digest.0);
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

    /// A module-signature type over `schema`, specialized by `pinned_slots`. Computes the
    /// schema's content digest once, here, so the node carries it and identity is one compare.
    /// Canonicalizes `pinned_slots` by name-sorting, so pin-set identity is order-independent.
    pub fn signature(&self, schema: SigSchema, pinned_slots: Vec<(String, KType)>) -> KType {
        let schema_digest = schema_content_digest(&schema, self);
        self.intern(TypeNode::Signature {
            schema,
            schema_digest,
            pinned_slots: canonical_pins(pinned_slots),
        })
    }

    /// A `Signature` deriving from `handle` with `pinned_slots` **accumulated** onto its existing
    /// pins — the derived constructor for further specializing an already-interned signature
    /// (WITH). Reuses the stored schema and its carried digest rather than re-running the content
    /// walk; the merged pin set is canonicalized by name-sorting, so chained and one-shot
    /// specialization intern the same type. A non-`Signature` `handle` or a `pinned_slots` name
    /// colliding with an existing pin is a caller bug — the caller validates shape and re-pins
    /// first (an equal re-pin normalizes away, an unequal one is the caller's type error).
    pub fn signature_pinned(&self, handle: KType, pinned_slots: Vec<(String, KType)>) -> KType {
        match self.node(handle) {
            TypeNode::Signature {
                schema,
                schema_digest,
                pinned_slots: mut merged,
            } => {
                debug_assert!(
                    pinned_slots
                        .iter()
                        .all(|(name, _)| !merged.iter().any(|(m, _)| m == name)),
                    "signature_pinned: caller passed a pin colliding with an existing pin"
                );
                merged.extend(pinned_slots);
                self.intern(TypeNode::Signature {
                    schema,
                    schema_digest,
                    pinned_slots: canonical_pins(merged),
                })
            }
            _ => panic!("signature_pinned: handle names a non-Signature node"),
        }
    }

    /// Canonicalizing constructor for a union — the single entry point that builds one. Flattens
    /// any nested union member into its members, deduplicates by handle, and collapses a single
    /// surviving member to that member (`:(A | A)` is `:A`). Callers guarantee at least one
    /// member.
    pub fn union_of(&self, members: Vec<KType>) -> KType {
        debug_assert!(!members.is_empty(), "union_of requires at least one member");
        let mut flat: Vec<KType> = Vec::with_capacity(members.len());
        let push_unique = |handle: KType, flat: &mut Vec<KType>| {
            if !flat.contains(&handle) {
                flat.push(handle);
            }
        };
        for member in members {
            // Reading the member's node drops the table borrow before the intern below.
            match self.node(member) {
                TypeNode::Union { members: inner } => {
                    for nested in inner {
                        push_unique(nested, &mut flat);
                    }
                }
                _ => push_unique(member, &mut flat),
            }
        }
        if flat.len() == 1 {
            return flat[0];
        }
        self.intern(TypeNode::Union { members: flat })
    }

    /// Intern a union from members that are already flat (no member is itself a `Union`) — dedup by
    /// handle and collapse a one-member result, but read no member nodes. The seal's sibling
    /// rewrite ([`rewrite_siblings`](super::recursive_group_window)) uses this: a rewritten sibling
    /// handle names a still-uninterned member of the group being sealed, so the node-reading
    /// [`Self::union_of`] flatten pass would fault on it — and a group member is always a
    /// `SetMember`, never a nested `Union`, so flattening is a no-op here anyway.
    pub fn intern_union_flat(&self, members: Vec<KType>) -> KType {
        debug_assert!(
            !members.is_empty(),
            "intern_union_flat requires at least one member"
        );
        let mut flat: Vec<KType> = Vec::with_capacity(members.len());
        for member in members {
            if !flat.contains(&member) {
                flat.push(member);
            }
        }
        if flat.len() == 1 {
            return flat[0];
        }
        self.intern(TypeNode::Union { members: flat })
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
        if a.len() != b.len() || !a.keys().all(|k| b.get(k).is_some()) {
            return None;
        }
        Some(
            a.iter()
                .map(|(name, x)| (name.clone(), self.join(*x, *b.get(name).unwrap())))
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

/// Canonical pin-set order: name-sorted. [`signature_digest`](type_digest) feeds pins
/// positionally, so the constructors establish one order per pin set — `S WITH {A, B}`,
/// `S WITH {B, A}`, and any chained accumulation of the two intern the same type.
fn canonical_pins(mut pins: Vec<(String, KType)>) -> Vec<(String, KType)> {
    pins.sort_by(|(a, _), (b, _)| a.cmp(b));
    pins
}

#[cfg(test)]
mod tests;
