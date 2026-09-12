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
//! See [old_design/typing/type-registry.md](../../../../old_design/typing/type-registry.md).

use std::cell::RefCell;
use std::collections::HashMap;

use imbl::shared_ptr::RcK;
use smallvec::SmallVec;

use crate::machine::model::{Carried, Held};
use crate::parse::{BinderSymbol, IdentityBuildHasher, Symbol, TypeSymbol};

use super::kkind::KKind;
use super::ktype::KType;
use super::node::TypeNode;
use super::record::Record;
use super::sig_schema::{SigSchema, join_schemas};
use super::signature::DispatchTokenElement;
use super::type_digest::{self, TypeDigest, schema_content_digest};

/// A union's members under construction. Inline up to four — the width that covers a hand-written
/// `A | B | C` and the variant lists of all but the widest `UNION` declarations — so the common
/// union costs no heap allocation to canonicalize, and none at all when the result is a node the
/// registry already holds.
type MemberList = SmallVec<[KType; 4]>;

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
/// in two independent cells: a read of `nodes` takes an `O(1)` snapshot and releases the cell
/// before its closure runs, so reads nest freely and a reader may intern, while `verdicts` is
/// written under its own borrow, so a memoizing walk records its verdict without touching the read
/// it is running under. Both maps are uncapped: they drop with the run frame that owns them, so
/// growth is bounded by the run.
pub struct TypeRegistry {
    nodes: RefCell<NodeMap>,
    verdicts: RefCell<HashMap<(TypeDigest, TypeDigest, Relation), bool>>,
    quantified: RefCell<HashMap<TypeDigest, bool, IdentityBuildHasher>>,
    quantifiers_exist: std::cell::Cell<bool>,
    #[cfg(test)]
    hits: std::cell::Cell<usize>,
    #[cfg(test)]
    misses: std::cell::Cell<usize>,
}

impl TypeRegistry {
    /// The shallow type tag of a produced value: an object's own `ktype()`, or a type-channel arm's
    /// `OfKind` classification. A registry method rather than a cell method because the answer for
    /// the type arms is the registry's — the kind of a `KType` is a lookup here — and the cells hold
    /// no registry of their own.
    pub fn ktype_of_carried(&self, carried: Carried<'_>) -> KType {
        match carried {
            Carried::Object(o) => o.ktype(),
            Carried::Type(t) => KType::of_kind(t.kind_of(self)),
            // An unlowered name denotes a proper type once resolved.
            Carried::UnresolvedType(_) => KType::of_kind(KKind::ProperType),
        }
    }

    /// [`Self::ktype_of_carried`] for an owned cell: the same three arms, plus the two a capture
    /// slot mints — a captured name token and a raw record-type expression, each of which denotes
    /// its own part-kind-exact type.
    pub fn ktype_of(&self, cell: &Held<'_>) -> KType {
        match cell {
            Held::Object(o) => o.ktype(),
            Held::Type(t) => KType::of_kind(t.kind_of(self)),
            Held::UnresolvedType(_) => KType::of_kind(KKind::ProperType),
            Held::Name(BinderSymbol::Value(_)) => KType::IDENTIFIER,
            Held::Name(BinderSymbol::Type(_)) => KType::NAME_TOKEN,
            Held::RecordType(_) => KType::RECORD_TYPE,
        }
    }

    /// Crate-internal. Pre-seeds the fixed handles — the leaves, the `OfKind` values,
    /// `List<Any>`, `Dict<Any, Any>`, and the empty signature — so the constants those names
    /// lower to are dereferenceable in a registry that has interned nothing else.
    pub(crate) fn new() -> Self {
        let registry = Self {
            nodes: RefCell::new(NodeMap::with_hasher(IdentityBuildHasher::default())),
            verdicts: RefCell::new(HashMap::new()),
            quantified: RefCell::new(HashMap::default()),
            quantifiers_exist: std::cell::Cell::new(false),
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
    //
    // [`with_node`](Self::with_node) is the one read door; `node` and the per-query verbs below are
    // written over it. A read holds no borrow while its closure runs — it reads through an `O(1)`
    // persistent snapshot — so **a reader may intern freely**, and a walk that rewrites types as it
    // recurses runs at any depth under a read.

    /// Intern `node` and return its handle. Computes the node's digest, inserts it if the digest
    /// is not already present, and returns the digest as a [`KType`]. Interning the same content
    /// twice yields one node and two equal handles.
    ///
    /// Reachable from inside a [`with_node`](Self::with_node) closure: that read borrows the cell
    /// only long enough to take its snapshot, so the write borrow here is uncontended.
    pub fn intern(&self, node: TypeNode) -> KType {
        self.intern_digested(type_digest::node_digest(&node), || node)
    }

    /// Insert the node `build` produces under `digest` if the digest is not already present, and
    /// hand back the digest as a handle — the one insert-if-absent path, which every interning door
    /// takes.
    ///
    /// One borrow around one probe: a hit — the steady state, since a repeated spelling of a type
    /// names an already-interned node — never calls `build`, so a caller that can compute its
    /// digest without materializing the node ([`intern_union_members`](Self::intern_union_members))
    /// pays for neither. The table's `entry` API would fold the probe and the insert into one
    /// lookup, but its occupied arm copies the whole lookup path out of the shared structure, which
    /// is exactly the hit path; a digest hashes by identity, so the second lookup here costs a
    /// descent and no hashing at all.
    ///
    /// `build` runs under the write borrow, so unlike a
    /// [`with_node`](Self::with_node) closure it may not read or intern: it materializes the node
    /// the caller already has the parts for and nothing else.
    fn intern_digested(&self, digest: TypeDigest, build: impl FnOnce() -> TypeNode) -> KType {
        let mut nodes = self.nodes.borrow_mut();
        if !nodes.contains_key(&digest) {
            nodes.insert(digest, build());
        }
        KType::from_digest(digest)
    }

    /// Read the content `handle` names **by reference** and hand back whatever `read` derives
    /// from it.
    ///
    /// The one read door. `read`'s result type is fixed at the call site and the node's lifetime is
    /// this call's, so no reference into the table can escape: a reader is confined to derived
    /// data by construction, which is what lets a shape probe answer without copying the node.
    ///
    /// The node is read out of a **snapshot** — `nodes` is a persistent HAMT, so cloning it is a
    /// root-pointer bump and the `RefCell` borrow ends before `read` runs. That is what lets a
    /// reader intern: [`Self::intern`] inserts into the live table, which the snapshot in hand does
    /// not share, and a handle minted mid-read resolves through the fresh snapshot its own
    /// `with_node` takes. Reads nest freely.
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

    /// An expression shape: the interleaved keyword / argument-position run a call spells, the
    /// type parameters it binds ahead of that run, and the return type.
    ///
    /// The one door that mints a shape. Argument names never reach it — they are binder-side —
    /// and the quantifier names are render-only, so two shapes alpha-equivalent under a renaming
    /// intern to the node whichever spelling built first.
    pub fn shape_type(
        &self,
        quantifiers: &[TypeSymbol],
        elements: &[DispatchTokenElement],
        ret: KType,
    ) -> KType {
        debug_assert!(
            elements.iter().all(|element| match element {
                DispatchTokenElement::Slot(kt) =>
                    self.quantifier_indices_in_range(*kt, quantifiers.len()),
                DispatchTokenElement::Keyword(_) => true,
            }) && self.quantifier_indices_in_range(ret, quantifiers.len()),
            "every quantified position names an index of the shape's own group",
        );
        // Probe-first: every definition mints its callable's shape, and re-running one definition
        // — a lambda inside a loop body — mints a shape already interned. Taking the digest off
        // the borrowed run means the boxed run and the quantifier vector are built only on a
        // genuine miss, so the steady state allocates nothing.
        let digest = type_digest::shape_digest(quantifiers.len(), elements, ret.digest());
        self.intern_digested(digest, || TypeNode::ExpressionShape {
            quantifiers: quantifiers.to_vec(),
            elements: elements.iter().copied().collect(),
            ret,
        })
    }

    /// Rebuild a shape with every argument position's type and the return mapped through `map` —
    /// the one shape arm every structural rewrite over `TypeNode` shares (member substitution,
    /// binder canonicalization, sibling resolution). Keywords pass through untouched, and the
    /// quantifier group is carried over: a rewrite substitutes *inside* a binder, never across it.
    pub fn rebuild_shape(
        &self,
        quantifiers: &[TypeSymbol],
        elements: &[DispatchTokenElement],
        ret: KType,
        mut map: impl FnMut(KType) -> KType,
    ) -> KType {
        let mapped: SmallVec<[DispatchTokenElement; 12]> = elements
            .iter()
            .map(|element| match element {
                DispatchTokenElement::Slot(kt) => DispatchTokenElement::Slot(map(*kt)),
                keyword => *keyword,
            })
            .collect();
        let ret = map(ret);
        self.shape_type(quantifiers, &mapped, ret)
    }

    /// The `index`-th quantifier of the enclosing shape. The one door a `Quantified` node is born
    /// through, so it is also where the run learns it has any — see
    /// [`contains_quantified`](Self::contains_quantified).
    pub fn quantified(&self, index: usize) -> KType {
        self.quantifiers_exist.set(true);
        self.intern(TypeNode::Quantified(index))
    }

    /// Rewrite every `Quantified(i)` inside `kt` to `bindings[i]` — the per-call substitution a
    /// solved call applies to a shape's return, and the erasure `erase_quantified` runs with
    /// `Any` in every cell. A unary rebuild: only the shapes that can *hold* a quantified position
    /// recurse, and everything else is returned unchanged.
    ///
    /// A **nested** shape rebinds the indices with its own group, exactly as it shadows them in
    /// the relations, so the walk stops at one.
    pub fn substitute_quantified(&self, kt: KType, bindings: &[KType]) -> KType {
        self.with_node(kt, |node| match node {
            TypeNode::Quantified(index) => bindings.get(*index).copied().unwrap_or(kt),
            TypeNode::List { element } => {
                let element = self.substitute_quantified(*element, bindings);
                self.list(element)
            }
            TypeNode::Dict { key, value } => {
                let key = self.substitute_quantified(*key, bindings);
                let value = self.substitute_quantified(*value, bindings);
                self.dict(key, value)
            }
            TypeNode::Record { fields } => {
                let fields = fields.map(|v| self.substitute_quantified(*v, bindings));
                self.record(fields)
            }
            TypeNode::KFunction { params, ret } => {
                let params = params.map(|v| self.substitute_quantified(*v, bindings));
                let ret = self.substitute_quantified(*ret, bindings);
                self.function_type(params, ret)
            }
            TypeNode::Union { members } => {
                let substituted: Vec<KType> = members
                    .iter()
                    .map(|m| self.substitute_quantified(*m, bindings))
                    .collect();
                self.union_of(&substituted)
            }
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => {
                let constructor = self.substitute_quantified(*constructor, bindings);
                let arguments = arguments.map(|a| self.substitute_quantified(*a, bindings));
                self.constructor_apply(constructor, arguments)
            }
            _ => kt,
        })
    }

    /// `kt` at a solved call. A **shape** is its own binder, so instantiating one empties its
    /// group and substitutes through its slots and return: the result is the shape this call has,
    /// as against the shape the declaration wrote. Every other type carries no binder of its own
    /// and substitutes in place.
    pub fn instantiate_quantified(&self, kt: KType, bindings: &[KType]) -> KType {
        self.with_node(kt, |node| match node {
            TypeNode::ExpressionShape {
                quantifiers,
                elements,
                ret,
            } if !quantifiers.is_empty() => self.rebuild_shape(&[], elements, *ret, |child| {
                self.substitute_quantified(child, bindings)
            }),
            _ => self.substitute_quantified(kt, bindings),
        })
    }

    /// `kt` with every quantified position erased to `Any` — what a quantified callable reports on
    /// the value lane, where a lambda type has no binder to carry the parameter.
    pub fn erase_quantified(&self, kt: KType, arity: usize) -> KType {
        if arity == 0 {
            return kt;
        }
        let bindings = vec![KType::ANY; arity];
        self.substitute_quantified(kt, &bindings)
    }

    /// Whether any `Quantified` position is reachable from `kt` without crossing a nested shape's
    /// own binder — the probe that lets a slot type with nothing to solve answer the relations in
    /// one step instead of walking under a unifier.
    ///
    /// Every dispatched call in the language asks this once per slot per candidate, and the
    /// unification walk asks it again at each level it descends — where it is load-bearing, not
    /// merely a fast path, since an unquantified subtree must answer through the memoized subtype
    /// relation rather than through a structural descent that would re-derive it wrongly. Two
    /// readings keep that affordable, in the order a run meets them:
    ///
    /// A run that has interned no `Quantified` node at all — every run that writes no `FOR ALL` —
    /// answers from one flag and never walks. [`Self::quantified`] is the only door such a node is
    /// born through, and a handle names an interned node, so the flag is exact: false means no
    /// quantified position exists anywhere for a type to reach.
    ///
    /// A run that does quantify memoizes per digest, in its own cell alongside the verdict cache
    /// and on the same terms: a digest names fixed content, so the answer is a pure function of the
    /// key and can never go stale, and the memo is never load-bearing — a cold registry costs a
    /// re-walk, never a wrong answer. The walk recurses through this door rather than past it, so a
    /// miss warms every subtree it crosses and the graph is walked at most once.
    pub fn contains_quantified(&self, kt: KType) -> bool {
        if !self.quantifiers_exist.get() {
            return false;
        }
        if let Some(known) = self.quantified.borrow().get(&kt.digest()) {
            return *known;
        }
        let answer = self.with_node(kt, |node| match node {
            TypeNode::Quantified(_) => true,
            TypeNode::List { element } => self.contains_quantified(*element),
            TypeNode::Dict { key, value } => {
                self.contains_quantified(*key) || self.contains_quantified(*value)
            }
            TypeNode::Record { fields } => fields.values().any(|v| self.contains_quantified(*v)),
            TypeNode::KFunction { params, ret } => {
                params.values().any(|v| self.contains_quantified(*v))
                    || self.contains_quantified(*ret)
            }
            TypeNode::Union { members } => members.iter().any(|m| self.contains_quantified(*m)),
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => {
                self.contains_quantified(*constructor)
                    || arguments.values().any(|a| self.contains_quantified(*a))
            }
            _ => false,
        });
        self.quantified.borrow_mut().insert(kt.digest(), answer);
        answer
    }

    /// Whether `kt` reads the `index`-th quantifier of the enclosing shape — what a definition
    /// asks of each name its `FOR ALL` group lists. Definition-time and one index at a time, so it
    /// takes the exact walk; the memoized probe above is what keeps a type that reads no
    /// quantifier at all from paying for it.
    pub fn references_quantifier(&self, kt: KType, index: usize) -> bool {
        self.contains_quantified(kt) && self.reads_quantifier(kt, index)
    }

    /// [`Self::references_quantifier`]'s walk: the `index`-th quantified position, reachable
    /// without crossing a nested shape's binder (whose indices are its own, not this shape's).
    fn reads_quantifier(&self, kt: KType, index: usize) -> bool {
        self.with_node(kt, |node| match node {
            TypeNode::Quantified(found) => *found == index,
            TypeNode::List { element } => self.reads_quantifier(*element, index),
            TypeNode::Dict { key, value } => {
                self.reads_quantifier(*key, index) || self.reads_quantifier(*value, index)
            }
            TypeNode::Record { fields } => {
                fields.values().any(|v| self.reads_quantifier(*v, index))
            }
            TypeNode::KFunction { params, ret } => {
                params.values().any(|v| self.reads_quantifier(*v, index))
                    || self.reads_quantifier(*ret, index)
            }
            TypeNode::Union { members } => members.iter().any(|m| self.reads_quantifier(*m, index)),
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => {
                self.reads_quantifier(*constructor, index)
                    || arguments.values().any(|a| self.reads_quantifier(*a, index))
            }
            _ => false,
        })
    }

    /// Whether every `Quantified` position reachable from `kt` without crossing a nested shape's
    /// own binder names an index below `arity` — the [`Self::shape_type`] well-formedness probe.
    fn quantifier_indices_in_range(&self, kt: KType, arity: usize) -> bool {
        self.with_node(kt, |node| match node {
            TypeNode::Quantified(index) => *index < arity,
            TypeNode::List { element } => self.quantifier_indices_in_range(*element, arity),
            TypeNode::Dict { key, value } => {
                self.quantifier_indices_in_range(*key, arity)
                    && self.quantifier_indices_in_range(*value, arity)
            }
            TypeNode::Record { fields } => fields
                .values()
                .all(|v| self.quantifier_indices_in_range(*v, arity)),
            TypeNode::KFunction { params, ret } => {
                params
                    .values()
                    .all(|v| self.quantifier_indices_in_range(*v, arity))
                    && self.quantifier_indices_in_range(*ret, arity)
            }
            TypeNode::Union { members } => members
                .iter()
                .all(|m| self.quantifier_indices_in_range(*m, arity)),
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => {
                self.quantifier_indices_in_range(*constructor, arity)
                    && arguments
                        .values()
                        .all(|a| self.quantifier_indices_in_range(*a, arity))
            }
            _ => true,
        })
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
    /// any nested union member into its members, drops `Never` (the identity element: it admits
    /// nothing, so it widens nothing — `:(Never | Number)` is `:Number`), deduplicates by handle,
    /// and collapses a single surviving member to that member (`:(A | A)` is `:A`). A union of
    /// nothing but `Never` is `Never`. Callers guarantee at least one member.
    pub fn union_of(&self, members: &[KType]) -> KType {
        debug_assert!(!members.is_empty(), "union_of requires at least one member");
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
        if flat.is_empty() {
            return KType::NEVER;
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
    fn intern_union_members(&self, flat: MemberList) -> KType {
        let digest = type_digest::union_digest(&flat);
        self.intern_digested(digest, || TypeNode::Union {
            members: flat.into_vec(),
        })
    }

    /// Least-upper-bound of two types. `[1, 2]` → `List<Number>`, `[1, "x"]` → `List<Any>`;
    /// nested containers join element-wise.
    ///
    /// `Never` is the identity — joining the bottom with anything is that thing, which is what
    /// makes an empty container's element type absorb the first element it is joined against.
    /// A function joins its **parameters by [`meet`](Self::meet)**: parameters are
    /// contravariant, so only their greatest lower bound is admitted by both operands, and
    /// joining them instead would produce a type neither operand fills.
    pub fn join(&self, a: KType, b: KType) -> KType {
        if a == b {
            return a;
        }
        if a == KType::NEVER {
            return b;
        }
        if b == KType::NEVER {
            return a;
        }
        self.with_node(a, |na| {
            self.with_node(b, |nb| match (na, nb) {
                (TypeNode::List { element: x }, TypeNode::List { element: y }) => {
                    let element = self.join(*x, *y);
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
                    let key = self.join(*xk, *yk);
                    let value = self.join(*xv, *yv);
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
                ) => match self.param_record_pointwise(xp, yp, |s, x, y| s.meet(x, y)) {
                    Some(params) => {
                        let ret = self.join(*xr, *yr);
                        self.function_type(params, ret)
                    }
                    None => self.intern(TypeNode::Any),
                },
                // Two shapes under one key bound positionally: slots meet (contravariant), the
                // return joins. Anything else about them — a different key, a different quantifier
                // arity — has no common shape, so the pair coarsens to `Any` with everything else.
                (
                    TypeNode::ExpressionShape {
                        quantifiers: xq,
                        elements: xe,
                        ret: xr,
                    },
                    TypeNode::ExpressionShape {
                        quantifiers: yq,
                        elements: ye,
                        ret: yr,
                    },
                ) if xq.len() == yq.len() => {
                    match self.shape_elements_pointwise(xe, ye, |s, x, y| s.meet(x, y)) {
                        Some(elements) => {
                            let ret = self.join(*xr, *yr);
                            self.shape_type(xq, &elements, ret)
                        }
                        None => self.intern(TypeNode::Any),
                    }
                }
                // Two interfaces bound at their least common interface, not at `Any`: width
                // intersection with a per-member depth reconciliation ([`join_schemas`]). Disjoint
                // operands land on the empty schema — the module-lattice top `:Module` — by digest.
                (
                    TypeNode::Signature { schema: xs, .. },
                    TypeNode::Signature { schema: ys, .. },
                ) => self.signature(join_schemas(xs, ys, self)),
                _ => self.intern(TypeNode::Any),
            })
        })
    }

    /// Greatest-lower-bound of two types — the dual of [`join`](Self::join), and total: a pair
    /// with no common refinement meets at `Never`, which is always a sound lower bound.
    ///
    /// `join` calls this for function parameters, and this calls `join` for a function's own
    /// parameters and for a union's distribution; the mutual recursion descends the two operands
    /// together and terminates with them.
    pub fn meet(&self, a: KType, b: KType) -> KType {
        if a == b {
            return a;
        }
        if a == KType::ANY {
            return b;
        }
        if b == KType::ANY {
            return a;
        }
        if a == KType::NEVER || b == KType::NEVER {
            return KType::NEVER;
        }
        self.with_node(a, |na| {
            self.with_node(b, |nb| match (na, nb) {
                // A union meets by distribution: a value in both operands is in some member of the
                // union and in the other side, so the meet is the union of the per-member meets.
                // Members that meet at `Never` contribute nothing, and `union_of` drops them.
                (TypeNode::Union { members }, _) => {
                    let met: Vec<KType> = members.iter().map(|m| self.meet(*m, b)).collect();
                    self.union_of(&met)
                }
                (_, TypeNode::Union { members }) => {
                    let met: Vec<KType> = members.iter().map(|m| self.meet(a, *m)).collect();
                    self.union_of(&met)
                }
                (TypeNode::List { element: x }, TypeNode::List { element: y }) => {
                    let element = self.meet(*x, *y);
                    self.list(element)
                }
                (TypeNode::Dict { key: xk, value: xv }, TypeNode::Dict { key: yk, value: yv }) => {
                    let key = self.meet(*xk, *yk);
                    let value = self.meet(*xv, *yv);
                    self.dict(key, value)
                }
                // Record values are width-superset subtypes, so the greatest lower bound keeps
                // every field of either operand — shared names meeting pointwise.
                (TypeNode::Record { fields: xf }, TypeNode::Record { fields: yf }) => {
                    let mut merged: Record<KType> = xf
                        .iter()
                        .map(|(name, x)| match yf.get(name.symbol()) {
                            Some(y) => (name, self.meet(*x, *y)),
                            None => (name, *x),
                        })
                        .collect();
                    for (name, y) in yf.iter() {
                        if xf.get(name.symbol()).is_none() {
                            merged.insert(name, *y);
                        }
                    }
                    self.record(merged)
                }
                // Dual of the join arm: parameters are contravariant, so they join here, and the
                // return meets. Differing key sets have no common function shape below them.
                (
                    TypeNode::KFunction {
                        params: xp,
                        ret: xr,
                    },
                    TypeNode::KFunction {
                        params: yp,
                        ret: yr,
                    },
                ) => match self.param_record_pointwise(xp, yp, |s, x, y| s.join(x, y)) {
                    Some(params) => {
                        let ret = self.meet(*xr, *yr);
                        self.function_type(params, ret)
                    }
                    None => KType::NEVER,
                },
                // Dual of the join arm: slots join, the return meets.
                (
                    TypeNode::ExpressionShape {
                        quantifiers: xq,
                        elements: xe,
                        ret: xr,
                    },
                    TypeNode::ExpressionShape {
                        quantifiers: yq,
                        elements: ye,
                        ret: yr,
                    },
                ) if xq.len() == yq.len() => {
                    match self.shape_elements_pointwise(xe, ye, |s, x, y| s.join(x, y)) {
                        Some(elements) => {
                            let ret = self.meet(*xr, *yr);
                            self.shape_type(xq, &elements, ret)
                        }
                        None => KType::NEVER,
                    }
                }
                // No structural rule relates the two shapes, so nothing inhabits both.
                _ => KType::NEVER,
            })
        })
    }

    /// Reduce an iterator of types to their least upper bound. Empty iterator → `Never`, the
    /// join's identity element: an empty container carries the bottom element type, which every
    /// typed element slot admits, and which absorbs the first element joined against it.
    pub fn join_iter<I: IntoIterator<Item = KType>>(&self, iter: I) -> KType {
        iter.into_iter()
            .reduce(|a, b| self.join(a, b))
            .unwrap_or(KType::NEVER)
    }

    /// Positional pointwise combination of two shape element runs under `combine`. `Some(built)`
    /// when the runs agree keyword-for-keyword in every position, `None` when they key different
    /// buckets — which the caller coarsens to its own bound. [`join`](Self::join) passes `meet`
    /// here and [`meet`](Self::meet) passes `join`, the contravariance of an argument position.
    fn shape_elements_pointwise(
        &self,
        a: &[DispatchTokenElement],
        b: &[DispatchTokenElement],
        combine: impl Fn(&Self, KType, KType) -> KType,
    ) -> Option<SmallVec<[DispatchTokenElement; 12]>> {
        if a.len() != b.len() {
            return None;
        }
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| match (x, y) {
                (DispatchTokenElement::Keyword(s), DispatchTokenElement::Keyword(t)) if s == t => {
                    Some(*x)
                }
                (DispatchTokenElement::Slot(sx), DispatchTokenElement::Slot(sy)) => {
                    Some(DispatchTokenElement::Slot(combine(self, *sx, *sy)))
                }
                _ => None,
            })
            .collect()
    }

    /// Name-keyed pointwise combination of two parameter records under `combine`. `Some(built)`
    /// when the records have equal length and the same key set; `None` on differing key sets,
    /// which the caller coarsens to its own bound. [`join`](Self::join) passes `meet` here and
    /// [`meet`](Self::meet) passes `join` — the contravariance of the parameter position.
    fn param_record_pointwise(
        &self,
        a: &Record<KType>,
        b: &Record<KType>,
        combine: impl Fn(&Self, KType, KType) -> KType,
    ) -> Option<Record<KType>> {
        if a.len() != b.len() || !a.keys().all(|k| b.get(k.symbol()).is_some()) {
            return None;
        }
        // The built record keeps the left operand's classified keys; both sides agree on the
        // symbol bits, which is what identity reads.
        Some(
            a.iter()
                .map(|(name, x)| (name, combine(self, *x, *b.get(name.symbol()).unwrap())))
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
