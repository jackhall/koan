use std::collections::HashMap;

use crate::machine::core::KFunction;
use crate::machine::core::{FoldingBrand, FrameSet, KoanRegion, KoanRegionExt, Residence};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{KType, Parseable, Record, TypeNode, TypeRegistry};

use super::{
    ContainerSubstrate, DictSubstrate, Held, KKey, ListSubstrate, Module, PayloadSubstrate,
    RecordSubstrate, SubstrateMemos,
};

mod equality;
pub use equality::ValueEqualityError;

#[cfg(test)]
mod tests;

/// Which verb the escape seam selects for a top-level record. `CostDriven` is the production
/// policy (the ratio decision from the memos); the two forced variants exist only under their
/// verification-build cfg features, making the output-asserting suite an equivalence battery.
///
/// `#[allow(dead_code)]`: the forced variants are constructed only under their cfg features, so the
/// default build sees them unused; `SEAM_POLICY` itself has no consumer until the chooser lands.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SeamPolicy {
    CostDriven,
    ForceCopy,
    ForcePin,
}

// Verification builds force a single verb; the two features are mutually exclusive.
#[cfg(all(feature = "seam-force-copy", feature = "seam-force-pin"))]
compile_error!("features `seam-force-copy` and `seam-force-pin` are mutually exclusive");

#[cfg(all(feature = "seam-force-copy", not(feature = "seam-force-pin")))]
#[allow(dead_code)]
pub(crate) const SEAM_POLICY: SeamPolicy = SeamPolicy::ForceCopy;

#[cfg(all(feature = "seam-force-pin", not(feature = "seam-force-copy")))]
#[allow(dead_code)]
pub(crate) const SEAM_POLICY: SeamPolicy = SeamPolicy::ForcePin;

#[cfg(not(any(feature = "seam-force-copy", feature = "seam-force-pin")))]
#[allow(dead_code)]
pub(crate) const SEAM_POLICY: SeamPolicy = SeamPolicy::CostDriven;

/// Runtime value: the universal type that `KFunction`s consume and produce.
///
/// Every composite payload is a region-resident substrate the value borrows (`&'a`), immutable
/// after construction; a future mutable-list builtin would need a fresh substrate at the mutation
/// site. `Struct.fields` uses `IndexMap` so iteration matches declaration order.
///
/// A `KFunction` is a bare borrow into its defining region; the regions an escaping
/// closure reaches are pinned by its carrier's witness set ([`FrameSet`](crate::machine::FrameSet)),
/// not a per-value anchor. See [per-call-region/lifecycle.md § Carriers](../../../../design/per-call-region/lifecycle.md#carriers).
pub enum KObject<'a> {
    Number(f64),
    KString(String),
    Bool(bool),
    /// List value. The first field is a region borrow of the list's [`ListSubstrate`] — the element
    /// slice plus its three construction memos, contains-borrows / copy-cost / borrows-home
    /// (positional, index-ordered). The second is the value's **own type handle** — the interned
    /// `List<element>` — memoized at construction from the join (LUB) of the contents, or re-stamped
    /// at an annotated boundary to the declared list type (coarsening included). Construct via
    /// [`KObject::list`] / [`KObject::list_with_type`] — never the tuple directly outside this
    /// module, and never `Rc::new`: the substrate is born only through
    /// [`FoldingBrand::alloc_substrate_folded`]. Each element is a [`Held`] (an object or a
    /// first-class type).
    List(&'a ListSubstrate<'a>, KType),
    /// Dict value. The first field is a region borrow of the dict's [`DictSubstrate`] — the frozen
    /// entry table plus its three construction memos, contains-borrows / copy-cost / borrows-home.
    /// Each value cell is a [`Held`] (an object or a first-class type); keys are the concrete scalar
    /// [`KKey`]. The second field is the value's **own type handle** — the interned `Dict<key, value>`
    /// over the join of the keys / values, or the declared dict type after a stamp. Construct via
    /// [`KObject::dict`] / [`KObject::dict_with_type`] — never the tuple directly outside this module,
    /// and never `Rc::new`: the substrate is born only through
    /// [`FoldingBrand::alloc_substrate_folded`].
    Dict(&'a DictSubstrate<'a>, KType),
    KExpression(KExpression<'a>),
    KFunction(&'a KFunction<'a>),
    /// Tagged-union value. The `value` field is a region borrow of the payload's
    /// [`PayloadSubstrate`] — the single payload cell plus its three construction memos,
    /// contains-borrows / copy-cost / borrows-home. `identity` is the value's own type handle: the
    /// union member's `SetMember` handle when the carrier's type arguments are erased, or the
    /// `ConstructorApply` over that member when an ascription stamped a parameterized union's
    /// arguments in. One handle carries what the member reference and the runtime type
    /// arguments used to carry separately, so `ktype()` is a copy and identity comparison is
    /// one `u128`. Construct via [`KObject::tagged`] — never the struct literal directly outside this
    /// module, and never `Rc::new`: the substrate is born only through
    /// [`FoldingBrand::alloc_substrate_folded`].
    Tagged {
        tag: String,
        value: &'a PayloadSubstrate<'a>,
        identity: KType,
    },
    /// Anonymous structural record value (`{x = 1, y = "a"}`). The first field is a region
    /// borrow of the record's [`RecordSubstrate`] — the field record plus its three construction
    /// memos, contains-borrows / copy-cost / borrows-home (identifier-keyed, declaration-ordered,
    /// order-blind equality); the
    /// second is the value's own type handle — the interned `Record` over each field's
    /// `ktype()` at fresh construction, re-stamped to a declared record type at an annotated
    /// boundary (mirrors `List` / `Dict`). Construct via [`KObject::record`] /
    /// [`KObject::record_with_type`] — never the tuple directly, and never `Rc::new`: the
    /// substrate is born only through [`FoldingBrand::alloc_substrate_folded`]. Distinct from the
    /// nominal `Struct`: a record carries no `(name, scope_id)` identity, only its structure.
    /// Each field value is a [`Held`] (an object or a first-class type).
    Record(&'a RecordSubstrate<'a>, KType),
    /// NEWTYPE identity-wrapper carrier (and the ATTR abstract-type re-tag carrier): tags a
    /// representation value with a type identity. (A user-`UNION` variant value is a
    /// [`Self::Tagged`], not a `Wrapped` — ruling 13.) The `inner` field is a region borrow of the
    /// payload's [`PayloadSubstrate`] — the single payload cell plus its three construction memos.
    /// A re-tag collapses one wrapper layer ([`KObject::wrapped_peel`]); a genuine construction
    /// preserves the payload verbatim ([`KObject::wrapped_hold`]), so a newtype nesting another
    /// keeps every layer. `type_id` is the declaration-stable identity handle — for a standalone
    /// newtype the sealed member's `SetMember` handle, for an identity-wrapper (`NEWTYPE (T AS W)`)
    /// construction a `ConstructorApply` over it, and for an opaque-ascription abstract-type re-tag
    /// the per-call `AbstractType` identity. Construct via [`KObject::wrapped_peel`] /
    /// [`KObject::wrapped_hold`] — never the struct literal directly outside this module, and never
    /// `Rc::new`: the substrate is born only through [`FoldingBrand::alloc_substrate_folded`].
    ///
    /// `ktype()` copies `type_id` — the per-declaration identity. ATTR over a `Wrapped` falls
    /// through to `inner`, so wrapping a struct in a NEWTYPE doesn't force every field accessor
    /// to redo.
    Wrapped {
        inner: &'a PayloadSubstrate<'a>,
        type_id: KType,
    },
    /// First-class module value. A bare borrow into the region the module was minted in,
    /// pinned by the value carrier's witness set — the same contract as [`Self::KFunction`].
    /// `ktype()` reports the module's principal signature (the handle its self-sig seal
    /// interned), so a module in expression position dispatches and satisfies signature slots
    /// on the value channel.
    Module(&'a Module<'a>),
    Null,
}

impl<'a> KObject<'a> {
    /// Fresh `List` carrier: memoizes the element type as the join (LUB) of contents.
    /// Empty list memoizes `Any` (the join's identity); the empty-container *error*
    /// rule lives at the untyped-resolution boundary, not here. `door` is the fold brand the
    /// substrate is born through — see [`Self::list_of_held`].
    pub fn list(
        door: FoldingBrand<'a>,
        items: Vec<KObject<'a>>,
        types: &TypeRegistry,
    ) -> KObject<'a> {
        KObject::list_of_held(door, items.into_iter().map(Held::Object).collect(), types)
    }

    /// Fresh `List` carrier over [`Held`] cells — the type-aware path (a list element may be
    /// a first-class type). One pass over `items` computes the memoized element-type join (this
    /// carrier's own `ktype()`) and the substrate's three memos — contains-borrows, copy-cost,
    /// borrows-home (see [`ListSubstrate`]'s doc for the per-cell rules; the leaf checks read
    /// `door`'s own region as home) — then allocates the substrate through `door` — the list door's
    /// sole construction site.
    pub fn list_of_held(
        door: FoldingBrand<'a>,
        items: Vec<Held<'a>>,
        types: &TypeRegistry,
    ) -> KObject<'a> {
        let element = types.join_iter(items.iter().map(|i| i.ktype(types)));
        let home = door.region();
        let memos = SubstrateMemos::compute(items.iter(), home);
        let substrate = door.alloc_substrate_folded::<ListSubstrate<'static>>(
            ContainerSubstrate::new(items, memos),
        );
        KObject::List(substrate, types.list(element))
    }

    /// `List` carrier with an explicitly supplied **list type** — for ascription stamping (re-tag to
    /// the declared list type, coarsening included). Shares the substrate borrow verbatim — the
    /// substrate is immutable after construction, so retype never touches cells. `list_type` is the
    /// whole `List<element>` handle, already interned, not the element type. See
    /// [`Self::record_with_type`].
    pub fn list_with_type(substrate: &'a ListSubstrate<'a>, list_type: KType) -> KObject<'a> {
        KObject::List(substrate, list_type)
    }

    /// Re-home an already-relocated element slice into `door`'s region under the value's existing
    /// memoized list type — the seam copy verb's list arm ([`copy_object_into`]). Relocation
    /// preserves every element's `ktype()`, so the element-type join is unchanged and `list_type`
    /// rides verbatim; the three memos recompute relative to the new home (`door`'s own region). The
    /// substrate is born through `door` — the list door's sole construction site. See
    /// [`Self::record_rehomed`].
    pub fn list_rehomed(
        door: FoldingBrand<'a>,
        items: Vec<Held<'a>>,
        list_type: KType,
    ) -> KObject<'a> {
        let home = door.region();
        let memos = SubstrateMemos::compute(items.iter(), home);
        let substrate = door.alloc_substrate_folded::<ListSubstrate<'static>>(
            ContainerSubstrate::new(items, memos),
        );
        KObject::List(substrate, list_type)
    }

    /// Fresh `Dict` carrier: memoizes key + value types as the join of the keys / values. `door` is
    /// the fold brand the substrate is born through — see [`Self::dict_of_held`].
    pub fn dict(
        door: FoldingBrand<'a>,
        map: HashMap<KKey, KObject<'a>>,
        types: &TypeRegistry,
    ) -> KObject<'a> {
        KObject::dict_of_held(
            door,
            map.into_iter().map(|(k, v)| (k, Held::Object(v))).collect(),
            types,
        )
    }

    /// Fresh `Dict` carrier over [`Held`] value cells — the type-aware path (a dict value may be a
    /// first-class type; keys stay scalar). One pass over `map` computes the memoized key/value type
    /// join (this carrier's own `ktype()`) and the substrate's three memos — contains-borrows,
    /// copy-cost, borrows-home (the leaf checks read `door`'s own region as home; only the value
    /// cells carry region borrows, so the memos fold over `map.values()`). The frozen table is
    /// materialized into `hashbrown` (last-wins dedup already happened in the transient input map)
    /// and allocated through `door` — the dict door's sole construction site.
    pub fn dict_of_held(
        door: FoldingBrand<'a>,
        map: HashMap<KKey, Held<'a>>,
        types: &TypeRegistry,
    ) -> KObject<'a> {
        let key = types.join_iter(map.keys().map(|k| k.ktype()));
        let value = types.join_iter(map.values().map(|v| v.ktype(types)));
        let home = door.region();
        let memos = SubstrateMemos::compute(map.values(), home);
        let frozen: hashbrown::HashMap<KKey, Held<'a>> = map.into_iter().collect();
        let substrate = door.alloc_substrate_folded::<DictSubstrate<'static>>(
            ContainerSubstrate::new(frozen, memos),
        );
        KObject::Dict(substrate, types.dict(key, value))
    }

    /// `Dict` carrier with an explicitly supplied **dict type** — for ascription stamping (re-tag to
    /// the declared dict type, coarsening included). Shares the substrate borrow verbatim — the
    /// substrate is immutable after construction, so retype never touches cells. `dict_type` is the
    /// whole `Dict<key, value>` handle, already interned. See [`Self::list_with_type`].
    pub fn dict_with_type(substrate: &'a DictSubstrate<'a>, dict_type: KType) -> KObject<'a> {
        KObject::Dict(substrate, dict_type)
    }

    /// Re-home an already-relocated entry table into `door`'s region under the value's existing
    /// memoized dict type — the seam copy verb's dict arm ([`copy_object_into`]). Relocation
    /// preserves every value cell's `ktype()`, so the key/value-type join is unchanged and
    /// `dict_type` rides verbatim; the three memos recompute relative to the new home (`door`'s own
    /// region). The substrate is born through `door` — the dict door's sole construction site. See
    /// [`Self::list_rehomed`].
    pub fn dict_rehomed(
        door: FoldingBrand<'a>,
        map: hashbrown::HashMap<KKey, Held<'a>>,
        dict_type: KType,
    ) -> KObject<'a> {
        let home = door.region();
        let memos = SubstrateMemos::compute(map.values(), home);
        let substrate = door
            .alloc_substrate_folded::<DictSubstrate<'static>>(ContainerSubstrate::new(map, memos));
        KObject::Dict(substrate, dict_type)
    }

    /// Fresh `Record` carrier: memoizes the per-field type record as each field's
    /// `ktype()`. Field order follows declaration; equality is order-blind per the
    /// `Record` substrate. `door` is the fold brand the substrate is born through — see
    /// [`Self::record_of_held`].
    pub fn record(
        door: FoldingBrand<'a>,
        fields: Record<KObject<'a>>,
        types: &TypeRegistry,
    ) -> KObject<'a> {
        KObject::record_of_held(
            door,
            Record::from_pairs(fields.into_pairs().map(|(k, v)| (k, Held::Object(v)))),
            types,
        )
    }

    /// Fresh `Record` carrier over [`Held`] field cells — the type-aware path (a field
    /// value may be a first-class type). One pass over `fields` computes the memoized
    /// field-type join (this carrier's own `ktype()`) and the substrate's three memos —
    /// contains-borrows, copy-cost, borrows-home (see [`RecordSubstrate`]'s doc for the per-cell
    /// rules; the leaf checks read `door`'s own region as home) — then allocates the substrate
    /// through `door` — the record door's sole construction site.
    pub fn record_of_held(
        door: FoldingBrand<'a>,
        fields: Record<Held<'a>>,
        types: &TypeRegistry,
    ) -> KObject<'a> {
        let field_types = fields.map(|v| v.ktype(types));
        let home = door.region();
        let memos = SubstrateMemos::compute(fields.values(), home);
        let substrate = door.alloc_substrate_folded::<RecordSubstrate<'static>>(
            ContainerSubstrate::new(fields, memos),
        );
        KObject::Record(substrate, types.record(field_types))
    }

    /// `Record` carrier with an explicitly supplied **record type** — the whole interned
    /// record-type handle — for ascription stamping (re-tag to the declared field types,
    /// coarsening included). Shares the substrate borrow verbatim — the substrate is immutable
    /// after construction, so retype never touches cells. See [`Self::list_with_type`].
    pub fn record_with_type(substrate: &'a RecordSubstrate<'a>, record_type: KType) -> KObject<'a> {
        KObject::Record(substrate, record_type)
    }

    /// Re-home an already-relocated field record into `door`'s region under the value's existing
    /// memoized record type — the seam copy verb's record arm (`copy_object_into`, in
    /// `machine::execute::lift`). Relocation preserves every field's `ktype()`, so the field-type
    /// join is unchanged and `record_type` rides verbatim; the three memos — contains-borrows,
    /// copy-cost, borrows-home — recompute relative to the new home (`door`'s own region): a bit may
    /// set, stay, or clear there. The substrate is born through `door` — the record door's sole
    /// construction site.
    pub fn record_rehomed(
        door: FoldingBrand<'a>,
        fields: Record<Held<'a>>,
        record_type: KType,
    ) -> KObject<'a> {
        let home = door.region();
        let memos = SubstrateMemos::compute(fields.values(), home);
        let substrate = door.alloc_substrate_folded::<RecordSubstrate<'static>>(
            ContainerSubstrate::new(fields, memos),
        );
        KObject::Record(substrate, record_type)
    }

    /// Fresh `Tagged` carrier over one payload value. Allocates the payload substrate — the single
    /// cell plus its three construction memos (computed relative to `door`'s own region as home) —
    /// through `door`, the tagged door's sole construction site, then names the carrier `tag` /
    /// `identity`. `value` is deep-cloned into the substrate (a pointer copy for a substrate-carrier
    /// payload); the caller keeps its borrow.
    pub fn tagged(
        door: FoldingBrand<'a>,
        tag: String,
        value: &KObject<'a>,
        identity: KType,
    ) -> KObject<'a> {
        let substrate = alloc_payload(door, value.deep_clone());
        KObject::Tagged {
            tag,
            value: substrate,
            identity,
        }
    }

    /// Fresh `Wrapped` carrier for a **construction**, preserving the payload verbatim — including a
    /// nested `Wrapped`, so a newtype over another keeps every layer. Allocates the payload substrate
    /// (the single cell plus its memos, home = `door`'s region) through `door`. See
    /// [`Self::wrapped_peel`] for the re-tag verb.
    pub fn wrapped_hold(door: FoldingBrand<'a>, value: &KObject<'a>, type_id: KType) -> KObject<'a> {
        let substrate = alloc_payload(door, value.deep_clone());
        KObject::Wrapped {
            inner: substrate,
            type_id,
        }
    }

    /// Fresh `Wrapped` carrier for a **re-tag**, collapsing one `Wrapped` layer so `Wrapped.inner`'s
    /// payload is never itself `Wrapped` (the single-layer invariant): the new identity replaces the
    /// old. When `value` is already a `Wrapped`, its inner substrate borrow rides verbatim (O(1), no
    /// copy — the payload keeps its home, pinned by the carrier's reach); anything else allocates a
    /// fresh payload substrate over an independent `deep_clone` through `door`.
    pub fn wrapped_peel(door: FoldingBrand<'a>, value: &KObject<'a>, type_id: KType) -> KObject<'a> {
        let inner = match value {
            KObject::Wrapped { inner, .. } => *inner,
            _ => alloc_payload(door, value.deep_clone()),
        };
        KObject::Wrapped { inner, type_id }
    }

    /// Ascription stamping at an annotated boundary (FN return type, argument slot,
    /// LET ascription). Callers have already checked the value satisfies `declared`;
    /// this re-tags the carrier to *exactly* the declared parameter types — a
    /// `List<Number>` returned through `:(LIST OF Any)` re-tags to `List<Any>`, so
    /// downstream dispatch sees the contract rather than the implementation's
    /// incidental precision.
    ///
    /// Only the four parameterized carriers re-tag, and each re-tags to `declared` itself —
    /// the declared type IS the carrier's new identity handle. Every other shape passes through
    /// (its `ktype()` is already its nominal identity). For a `Tagged` stamped against a
    /// `ConstructorApply`, the constructor identity must already match, so adopting `declared`
    /// wholesale supplies exactly the declared arguments.
    pub fn stamp_type(self, declared: KType, types: &TypeRegistry) -> KObject<'a> {
        match (self, types.node(declared)) {
            (KObject::List(substrate, _), TypeNode::List { .. }) => {
                KObject::List(substrate, declared)
            }
            (KObject::Dict(substrate, _), TypeNode::Dict { .. }) => {
                KObject::Dict(substrate, declared)
            }
            (KObject::Record(substrate, _), TypeNode::Record { .. }) => {
                KObject::Record(substrate, declared)
            }
            (KObject::Tagged { tag, value, .. }, TypeNode::ConstructorApply { .. }) => {
                // Share the payload substrate borrow verbatim — immutable after construction, so the
                // retype never touches the payload; only the identity handle changes.
                KObject::Tagged {
                    tag,
                    value,
                    identity: declared,
                }
            }
            (other, _) => other,
        }
    }

    /// True iff this is an empty container carrying no usable element-type information —
    /// an empty `List` whose memoized element type is `Any`, or an empty `Dict` whose
    /// key and value types are both `Any`. Reaching an *untyped* resolution boundary
    /// (untyped `LET` binding, bare top-level expression result) with this shape is an
    /// error (see [ktype/parameterization-and-variance.md § Runtime type-parameter carriers](../../../../design/typing/ktype/parameterization-and-variance.md#runtime-type-parameter-carriers)).
    ///
    /// A stamped empty container is not flagged (its carrier carries a non-`Any`
    /// element type), nor is a non-empty heterogeneous literal `List<Any>` (it carries
    /// information and is legal where `:(LIST OF Any)` is declared).
    pub fn is_unstamped_empty_container(&self) -> bool {
        match self {
            KObject::List(substrate, list_type) => {
                substrate.elements().is_empty() && *list_type == KType::LIST_OF_ANY
            }
            KObject::Dict(substrate, dict_type) => {
                substrate.entries().is_empty() && *dict_type == KType::DICT_ANY_ANY
            }
            _ => false,
        }
    }

    /// Whether this is a **shallow scalar** — a fully-owned leaf (`Number`, `KString`, `Bool`,
    /// `Null`) whose representation embeds no `&'a` region borrow and no [`Held`] cell. Such a value
    /// cannot reference any dep the construction fold was handed, so the dep-witness union is pure
    /// over-retention: the combinator gate ([`alloc_object_scalar`](crate::machine::core::StepAllocator::alloc_object_scalar))
    /// routes it to the no-fold path so it seals with an empty reach. Every other variant borrows
    /// (`KFunction`, `Module`) or holds cells that transitively might (`List`/`Dict`/`Record`/
    /// `Tagged`/`Wrapped`/`KExpression`), so it keeps the fold.
    pub fn is_shallow_scalar(&self) -> bool {
        matches!(
            self,
            KObject::Number(_) | KObject::KString(_) | KObject::Bool(_) | KObject::Null
        )
    }

    /// True when every region borrow in `self` points into `dest`. Only value-channel borrows
    /// are walked: `KFunction`, `Module`, `KExpression` splices, and a substrate carrier's
    /// (`Record`/`List`/`Dict`/`Tagged`/`Wrapped`) substrate address (O(1), never its cells). The
    /// `KType` tags (`List`/`Dict`/`Record` memos, `Tagged { identity }`, `Wrapped { type_id }`) are
    /// not walked — a handle is one `u128` naming registry-owned content, so it borrows no region at
    /// all.
    pub(crate) fn resident_in(&self, dest: &KoanRegion) -> bool {
        self.resident_in_visiting(&Residence::dest_only(dest))
    }

    /// The evidence-widened twin of [`Self::resident_in`], for a value built from (or embedding a
    /// projection of) one or more delivered carriers: every walked borrow must point into
    /// `dest` or be covered by one of `sets` — the object delivered tier's coverage predicate,
    /// over the same borrows as [`Self::resident_in`]. The `StoredReach` tokens holding the
    /// reach are opaque to this layer; core extracts the sets before calling.
    pub(crate) fn resident_in_delivered(&self, dest: &KoanRegion, sets: &[&FrameSet]) -> bool {
        self.resident_in_visiting(&Residence::with_reach(dest, sets))
    }

    pub(crate) fn resident_in_visiting(&self, residence: &Residence<'_>) -> bool {
        match self {
            KObject::Number(_) | KObject::KString(_) | KObject::Bool(_) | KObject::Null => true,
            KObject::KFunction(f) => residence.owns_function(f),
            KObject::KExpression(e) => e.is_splice_free(),
            // O(1) address-membership check on the substrate borrow — never a cell walk. Every
            // substrate carrier answers residence by its own address, whether it is a bare top-level
            // value (born resident through the fold door) or rides inside another carrier.
            KObject::List(substrate, _) => residence.owns_substrate(substrate),
            KObject::Dict(substrate, _) => residence.owns_substrate(substrate),
            KObject::Record(substrate, _) => residence.owns_substrate(substrate),
            KObject::Tagged { value, .. } => residence.owns_substrate(value),
            KObject::Wrapped { inner, .. } => residence.owns_substrate(inner),
            KObject::Module(m) => residence.owns_module(m),
        }
    }

    /// Runtime type tag — context-free by construction (ruling 4). Every value memoizes its
    /// full interned type handle where it is built, at a site that holds the registry, so this
    /// only ever copies a stored handle or names a pre-seeded constant. It builds nothing and
    /// needs no registry.
    pub fn ktype(&self) -> KType {
        match self {
            KObject::Number(_) => KType::NUMBER,
            KObject::KString(_) => KType::STR,
            KObject::Bool(_) => KType::BOOL,
            KObject::Null => KType::NULL,
            KObject::KExpression(_) => KType::KEXPRESSION,
            KObject::List(_, list_type) => *list_type,
            KObject::Dict(_, dict_type) => *dict_type,
            KObject::Record(_, record_type) => *record_type,
            KObject::KFunction(f) => f.value_ktype(),
            KObject::Tagged { identity, .. } => *identity,
            KObject::Wrapped { type_id, .. } => *type_id,
            KObject::Module(m) => m.ktype(),
        }
    }

    /// Independent-but-cheap clone: every composite pointer-copies its region-resident substrate
    /// borrow (`Copy`) under the immutable-value contract, never rebuilding cells; a `KFunction`
    /// copies its bare defining-region borrow. Lift-stability across a dying frame comes from the
    /// carrier's witness/reach, not a refcount bump.
    pub fn deep_clone(&self) -> KObject<'a> {
        match self {
            KObject::Number(n) => KObject::Number(*n),
            KObject::KString(s) => KObject::KString(s.clone()),
            KObject::Bool(b) => KObject::Bool(*b),
            KObject::Null => KObject::Null,
            // A pointer copy: the substrate borrow copies (`Copy`), never rebuilding the cells.
            KObject::List(substrate, list_type) => KObject::List(substrate, *list_type),
            KObject::Dict(substrate, dict_type) => KObject::Dict(substrate, *dict_type),
            KObject::KExpression(e) => KObject::KExpression(e.clone()),
            KObject::KFunction(f) => KObject::KFunction(f),
            // A pointer copy: the payload substrate borrow copies (`Copy`), never rebuilding the
            // payload.
            KObject::Tagged {
                tag,
                value,
                identity,
            } => KObject::Tagged {
                tag: tag.clone(),
                value,
                identity: *identity,
            },
            // A pointer copy: the substrate borrow copies (`Copy`), never rebuilding the fields.
            KObject::Record(substrate, record_type) => KObject::Record(substrate, *record_type),
            KObject::Wrapped { inner, type_id } => KObject::Wrapped {
                inner,
                type_id: *type_id,
            },
            KObject::Module(m) => KObject::Module(m),
        }
    }

    pub fn as_kexpression(&self) -> Option<&KExpression<'a>> {
        match self {
            KObject::KExpression(e) => Some(e),
            _ => None,
        }
    }

    pub fn as_function(&self) -> Option<&'a KFunction<'a>> {
        match self {
            KObject::KFunction(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_module(&self) -> Option<&'a Module<'a>> {
        match self {
            KObject::Module(m) => Some(*m),
            _ => None,
        }
    }

    /// Whether `self` is a substrate carrier — a `Record`, `List`, `Dict`, `Tagged`, or `Wrapped`,
    /// each of which directly borrows a region-resident substrate. Purely structural — unlike
    /// [`Self::resident_in_visiting`], no residence is checked here. A substrate is always a genuine
    /// region borrow into its own home (Ruling 5, design/value-substrates.md), which the fold engines
    /// that build a fresh carrier around one cannot see (composing a witness only ever consults the
    /// fold's *other* operands, never the value its own closure just built) — so a step-terminal seal
    /// uses this predicate to force the carrier's `borrows_host` bit conservatively true rather than
    /// under-reporting it.
    pub(crate) fn embeds_substrate(&self) -> bool {
        matches!(
            self,
            KObject::Record(..)
                | KObject::List(..)
                | KObject::Dict(..)
                | KObject::Tagged { .. }
                | KObject::Wrapped { .. }
        )
    }
}

/// Allocate a [`PayloadSubstrate`] over one owned payload `value` through `door` — the single
/// construction site every `Tagged` / `Wrapped` door verb ([`KObject::tagged`],
/// [`KObject::wrapped_hold`], the non-`Wrapped` arm of [`KObject::wrapped_peel`], and the seam copy
/// verb's tagged/wrapped arms) funnels through. Computes the payload's three memos relative to
/// `door`'s own region as home, then stores the substrate and hands back a co-located borrow.
fn alloc_payload<'a>(door: FoldingBrand<'a>, value: KObject<'a>) -> &'a PayloadSubstrate<'a> {
    let home = door.region();
    let memos = SubstrateMemos::compute_object(&value, home);
    door.alloc_substrate_folded::<PayloadSubstrate<'static>>(ContainerSubstrate::new(value, memos))
}

/// The seam copy verb's total rebuild: reconstruct `value`'s entire reachable structure at `dest`'s
/// brand. A substrate carrier (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`) rebuilds each
/// cell recursively and allocates a fresh substrate at `dest` through its door (the contains-borrows
/// memo recomputes — leaves ride the rebuild, so the bit can only stay or clear); a scalar rebuilds
/// owned; a `KFunction` / `Module` borrow rides verbatim (its own reach rides the transfer witness);
/// a `KExpression` clones.
/// Total or not at all — a partial spine copy would pay the copy *and* keep the pin. See
/// [design/value-substrates.md § Escape](../../../../design/value-substrates.md#escape-pin-by-default).
pub(crate) fn copy_object_into<'b>(value: &KObject<'b>, dest: FoldingBrand<'b>) -> KObject<'b> {
    match value {
        KObject::Number(n) => KObject::Number(*n),
        KObject::KString(s) => KObject::KString(s.clone()),
        KObject::Bool(b) => KObject::Bool(*b),
        KObject::Null => KObject::Null,
        KObject::KExpression(e) => KObject::KExpression(e.clone()),
        KObject::KFunction(f) => KObject::KFunction(f),
        KObject::Module(m) => KObject::Module(m),
        KObject::Record(substrate, record_type) => {
            let fields: Record<Held<'b>> =
                substrate.fields().map(|cell| copy_held_into(cell, dest));
            KObject::record_rehomed(dest, fields, *record_type)
        }
        KObject::List(substrate, list_type) => {
            let rebuilt: Vec<Held<'b>> = substrate
                .elements()
                .iter()
                .map(|cell| copy_held_into(cell, dest))
                .collect();
            KObject::list_rehomed(dest, rebuilt, *list_type)
        }
        KObject::Dict(substrate, dict_type) => {
            let rebuilt: hashbrown::HashMap<KKey, Held<'b>> = substrate
                .entries()
                .iter()
                .map(|(key, cell)| (key.clone(), copy_held_into(cell, dest)))
                .collect();
            KObject::dict_rehomed(dest, rebuilt, *dict_type)
        }
        KObject::Tagged {
            tag,
            value,
            identity,
        } => KObject::Tagged {
            tag: tag.clone(),
            value: alloc_payload(dest, copy_object_into(value.payload(), dest)),
            identity: *identity,
        },
        KObject::Wrapped { inner, type_id } => KObject::Wrapped {
            inner: alloc_payload(dest, copy_object_into(inner.payload(), dest)),
            type_id: *type_id,
        },
    }
}

/// [`copy_object_into`]'s per-cell dispatch for a [`Held`] field / element: an object rebuilds
/// recursively, a type-channel cell is owned data copied verbatim.
fn copy_held_into<'b>(cell: &Held<'b>, dest: FoldingBrand<'b>) -> Held<'b> {
    match cell {
        Held::Object(o) => Held::Object(copy_object_into(o, dest)),
        Held::Type(t) => Held::Type(*t),
        Held::UnresolvedType(ti) => Held::UnresolvedType(ti.clone()),
    }
}

/// Exact host-release probe, run only when a container's contains-borrows memo is set: does any
/// surviving borrow leaf of `value` point into `host`? A `KFunction` / `Module` leaf checks `host`'s
/// own address tables; a non-splice-free `KExpression` answers conservatively `true`; a substrate
/// carrier (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`) short-circuits on its own clear memo
/// (the copy releases it), else recurses its cells. A memo-clear carrier answers `false` outright —
/// the copy releases every host it retired. Read-only; borrows nothing.
pub(crate) fn still_borrows_host(value: &KObject<'_>, host: &KoanRegion) -> bool {
    match value {
        KObject::Number(_) | KObject::KString(_) | KObject::Bool(_) | KObject::Null => false,
        KObject::KFunction(f) => host.owns_function(*f as *const _),
        KObject::Module(m) => host.owns_module(*m as *const _),
        KObject::KExpression(e) => !e.is_splice_free(),
        KObject::Record(substrate, _) => {
            substrate.contains_borrows()
                && substrate
                    .fields()
                    .values()
                    .any(|cell| held_borrows_host(cell, host))
        }
        KObject::List(substrate, _) => {
            substrate.contains_borrows()
                && substrate
                    .elements()
                    .iter()
                    .any(|cell| held_borrows_host(cell, host))
        }
        KObject::Dict(substrate, _) => {
            substrate.contains_borrows()
                && substrate
                    .entries()
                    .values()
                    .any(|cell| held_borrows_host(cell, host))
        }
        KObject::Tagged { value, .. } => {
            value.contains_borrows() && still_borrows_host(value.payload(), host)
        }
        KObject::Wrapped { inner, .. } => {
            inner.contains_borrows() && still_borrows_host(inner.payload(), host)
        }
    }
}

/// [`still_borrows_host`]'s per-cell dispatch: an object recurses, a type-channel cell owns
/// its data and borrows no region.
fn held_borrows_host(cell: &Held<'_>, host: &KoanRegion) -> bool {
    match cell {
        Held::Object(o) => still_borrows_host(o, host),
        Held::Type(_) | Held::UnresolvedType(_) => false,
    }
}

/// The [`RegionEscape`] verb for a top-level record, chosen per value in O(1) from its memos and the
/// producer host's allocated total. Non-record values never reach this — they always copy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RegionEscape {
    /// Borrow rides, the producer region transfers by hold (`Residence::Kept`); the relocate hook
    /// pointer-copies the record (its substrate borrow rides, covered by the Kept-minted reach).
    Pin,
    /// Total rebuild of the value's reachable structure at the destination brand. `released`: the
    /// rebuild provably frees the retiring producer host (`Residence::Released` vs
    /// `Residence::Copied`).
    Copy { released: bool },
}

impl RegionEscape {
    /// The residence mode this verb transfers under: `Pin` keeps the producer region (the substrate
    /// borrow rides its unconditionally-minted reach); a released copy frees the retiring host; an
    /// unreleased copy leaves the host to its conservative materialization.
    pub(crate) fn residence(self) -> crate::witnessed::Residence {
        use crate::witnessed::Residence as SeamResidence;
        match self {
            RegionEscape::Pin => SeamResidence::Kept,
            RegionEscape::Copy { released: true } => SeamResidence::Released,
            RegionEscape::Copy { released: false } => SeamResidence::Copied,
        }
    }
}

/// A seam tuning constant: copy a priceable home-crossing record only when its exact rebuild cost
/// is under 1/`ALPHA_DIVISOR` of what the pin would retain (the host's allocated total). Not
/// observable in language semantics; provisional pending measurement.
const ALPHA_DIVISOR: u64 = 4;

/// The escape-seam copy-vs-pin decision for a top-level container `value` (whose cell substrate is
/// `substrate`) crossing out of producer `host`. O(1) but for the one address-table membership scan
/// (`owns_substrate`) and, on the unpriceable path only, the exact host-release probe. Generic over
/// the substrate's cell payload `C`; only records instantiate it today. See
/// design/value-substrates.md § Cost-driven copy.
pub(crate) fn copy_or_pin<C>(
    substrate: &ContainerSubstrate<C>,
    value: &KObject<'_>,
    host: &KoanRegion,
) -> RegionEscape {
    // Forced verification builds override the table for top-level records; `released` is
    // probe-derived so a forced copy is sound at either crossing.
    match SEAM_POLICY {
        SeamPolicy::ForcePin => return RegionEscape::Pin,
        SeamPolicy::ForceCopy => {
            return RegionEscape::Copy {
                released: !still_borrows_host(value, host),
            }
        }
        SeamPolicy::CostDriven => {}
    }

    // Unpriceable: keep today's unconditional total copy, `released` from the exact probe.
    if substrate.copy_cost() == u64::MAX {
        return RegionEscape::Copy {
            released: !still_borrows_host(value, host),
        };
    }

    let home_crossing = host.owns_substrate(substrate);
    if !home_crossing {
        // Foreign crossing: pricing a copy-out at an intermediate host is region evacuation's job.
        return RegionEscape::Pin;
    }

    // Priceable home crossing.
    if substrate.borrows_home() {
        // A leaf provably points into the home region: a copy would pay the rebuild AND keep the
        // pin, so pin outright (exact, no probe).
        return RegionEscape::Pin;
    }
    // Clear borrows-home bit is exact for a priceable record: no leaf borrows home, so the rebuild
    // frees the host. Copy when the value is a small fraction of what the pin would retain.
    if substrate.copy_cost() < host.allocated_total() / ALPHA_DIVISOR {
        RegionEscape::Copy { released: true }
    } else {
        RegionEscape::Pin
    }
}

impl<'a> Parseable for KObject<'a> {
    fn ktype(&self) -> KType {
        KObject::ktype(self)
    }
}

impl<'a> KObject<'a> {
    /// Canonical surface rendering of a value. Carried types render through the registry.
    pub fn summarize(&self, types: &TypeRegistry) -> String {
        match self {
            KObject::Number(n) => n.to_string(),
            KObject::KString(s) => s.clone(),
            KObject::Bool(b) => b.to_string(),
            KObject::List(substrate, _) => {
                let parts: Vec<String> = substrate
                    .elements()
                    .iter()
                    .map(|i| i.summarize(types))
                    .collect();
                format!("[{}]", parts.join(", "))
            }
            KObject::Dict(substrate, _) => {
                let parts: Vec<String> = substrate
                    .entries()
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.summarize(), v.summarize(types)))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            KObject::KExpression(e) => e.summarize(),
            KObject::KFunction(f) => f.summarize(),
            KObject::Tagged { tag, value, .. } => {
                format!("{}({})", tag, value.payload().summarize(types))
            }
            KObject::Record(substrate, _) => {
                let parts: Vec<String> = substrate
                    .fields()
                    .iter()
                    .map(|(field, value)| format!("{} = {}", field, value.summarize(types)))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            KObject::Null => "null".to_string(),
            KObject::Wrapped { inner, type_id } => {
                format!(
                    "{}({})",
                    type_id.name(types),
                    inner.payload().summarize(types)
                )
            }
            KObject::Module(m) => m.path.clone(),
        }
    }
}
