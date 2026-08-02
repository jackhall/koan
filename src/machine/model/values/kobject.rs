use std::collections::HashMap;
use std::rc::Weak;

use crate::machine::core::KFunction;
use crate::machine::core::{
    FrameCoverage, FrameReach, FrameStorage, KoanRegion, KoanRegionExt, Residence, SubstrateDoor,
};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{KType, Parseable, Record, TypeNode, TypeRegistry};
use crate::witnessed::{CellInput, CellReach, Sectioned};

use super::container_substrate::{
    held_copy_cost, HeldCells, ListLayout, PayloadLayout, RecordLayout,
};
use super::{
    ContainerSubstrate, DictSubstrate, Held, KKey, ListSubstrate, Module, PayloadSubstrate,
    RecordSubstrate,
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
/// site.
///
/// A `KFunction` is a bare borrow into its defining region; the regions an escaping
/// closure reaches are named by its carrier's reach description
/// ([`FrameReach`](crate::machine::core::FrameReach)) and pinned by the holder's owned
/// [`FrameCoverage`](crate::machine::core::FrameCoverage) coverage, not a per-value anchor. See [per-call-region/lifecycle.md § Carriers](../../../../design/per-call-region/lifecycle.md#carriers).
pub enum KObject<'a> {
    Number(f64),
    /// String value: a region-hosted `&'a str`, bumped into the region the value lives in
    /// ([`RegionBrand::alloc_text`](crate::machine::core::RegionBrand::alloc_text)). The slot owns
    /// no allocation, so it runs no `Drop` at region death and [`Self::deep_clone`] is a pointer
    /// copy; the bytes are freed as bump chunks with the rest of the region. Every door that claims
    /// a region's *release* re-bumps the bytes at its destination, so a stored string cell is always
    /// home-resident — see [`section_cells`] and [`copy_object_into`].
    KString(&'a str),
    Bool(bool),
    /// List value. The first field is a region borrow of the list's [`ListSubstrate`] — the elements
    /// in sectioned storage (positional, index-ordered, partitioned into reach runs) plus the value's
    /// stored reach union and copy cost. The second is the value's **own type handle** — the interned
    /// `List<element>` — memoized at construction from the join (LUB) of the contents, or re-stamped
    /// at an annotated boundary to the declared list type (coarsening included). Construct via
    /// [`KObject::list`] / [`KObject::list_with_type`] — never the tuple directly outside this
    /// module, and never `Rc::new`: the substrate is born only through
    /// [`FoldingBrand::alloc_substrate_folded`]. Each element is a [`Held`] (an object or a
    /// first-class type).
    List(&'a ListSubstrate<'a>, KType),
    /// Dict value. The first field is a region borrow of the dict's [`DictSubstrate`] — the value
    /// cells in sectioned storage plus the frozen key→index table, the stored reach union and the copy
    /// cost. Each value cell is a [`Held`] (an object or a first-class type); keys are the concrete scalar
    /// [`KKey`]. The second field is the value's **own type handle** — the interned `Dict<key, value>`
    /// over the join of the keys / values, or the declared dict type after a stamp. Construct via
    /// [`KObject::dict`] / [`KObject::dict_with_type`] — never the tuple directly outside this module,
    /// and never `Rc::new`: the substrate is born only through
    /// [`FoldingBrand::alloc_substrate_folded`].
    Dict(&'a DictSubstrate<'a>, KType),
    KExpression(KExpression<'a>),
    KFunction(&'a KFunction<'a>),
    /// Tagged-union value. The `value` field is a region borrow of the payload's
    /// [`PayloadSubstrate`] — the single payload cell in sectioned storage plus its stored reach and
    /// copy cost. `identity` is the value's own type handle: the
    /// union member's `SetMember` handle when the carrier's type arguments are erased, or the
    /// `ConstructorApply` over that member when an ascription stamped a parameterized union's
    /// arguments in. One handle carries what the member reference and the runtime type
    /// arguments used to carry separately, so `ktype()` is a copy and identity comparison is
    /// one `u128`. Construct via [`KObject::tagged`] — never the struct literal directly outside this
    /// module, and never `Rc::new`: the substrate is born only through
    /// [`FoldingBrand::alloc_substrate_folded`].
    Tagged {
        tag: &'a str,
        value: &'a PayloadSubstrate<'a>,
        identity: KType,
    },
    /// Anonymous structural record value (`{x = 1, y = "a"}`). The first field is a region
    /// borrow of the record's [`RecordSubstrate`] — the field cells in sectioned storage plus the
    /// sorted name slice that indexes them, the stored reach union and the copy cost
    /// (identifier-keyed, name-ordered, order-blind equality); the
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
    /// payload's [`PayloadSubstrate`] — the single payload cell plus its stored reach and copy cost.
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
    /// rule lives at the untyped-resolution boundary, not here. `door` is the substrate door the
    /// cells are sectioned through — see [`Self::list_of_held`].
    pub fn list(
        door: SubstrateDoor<'a, '_>,
        items: Vec<KObject<'a>>,
        types: &TypeRegistry,
    ) -> KObject<'a> {
        KObject::list_of_held(door, items.into_iter().map(Held::Object).collect(), types)
    }

    /// Fresh `List` carrier over [`Held`] cells — the type-aware path (a list element may be
    /// a first-class type). One pass over `items` computes the memoized element-type join (this
    /// carrier's own `ktype()`); the substrate door then sections the cells, folding their stored
    /// reach verdicts into the runs and the value-level union ([`section_cells`]) — the list door's
    /// sole construction site.
    pub fn list_of_held(
        door: SubstrateDoor<'a, '_>,
        items: Vec<Held<'a>>,
        types: &TypeRegistry,
    ) -> KObject<'a> {
        let element = types.join_iter(items.iter().map(|i| i.ktype(types)));
        KObject::List(alloc_list(door, items), types.list(element))
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
    /// rides verbatim; the cells are re-sectioned at `door`, so each rebuilt cell's run reads its own
    /// fresh stored facts. See [`Self::record_rehomed`].
    pub fn list_rehomed(
        door: SubstrateDoor<'a, '_>,
        items: Vec<Held<'a>>,
        list_type: KType,
    ) -> KObject<'a> {
        KObject::List(alloc_list(door, items), list_type)
    }

    /// Fresh `Dict` carrier: memoizes key + value types as the join of the keys / values. `door` is
    /// the substrate door the cells are sectioned through — see [`Self::dict_of_held`].
    pub fn dict(
        door: SubstrateDoor<'a, '_>,
        map: HashMap<KKey<'_>, KObject<'a>>,
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
    /// join (this carrier's own `ktype()`); the value cells are then sectioned through `door` and the
    /// key→index table frozen into `hashbrown` (last-wins dedup already happened in the transient
    /// input map) — the dict door's sole construction site.
    ///
    /// **Keys carry no reach** ([`KKey`] admits only `String` / `Number` / `Bool`, so a key naming a
    /// substrate or a closure is unrepresentable here, and a string key's bytes are re-bumped into
    /// this dict's own region by the door below); the O(1) stored-envelope rejection of a key whose
    /// carrier names any reach member runs at the one site that produces a key from a carrier
    /// (`dispatch::literal`'s `scalar_key`).
    pub fn dict_of_held(
        door: SubstrateDoor<'a, '_>,
        map: HashMap<KKey<'_>, Held<'a>>,
        types: &TypeRegistry,
    ) -> KObject<'a> {
        let key = types.join_iter(map.keys().map(|k| k.ktype()));
        let value = types.join_iter(map.values().map(|v| v.ktype(types)));
        KObject::Dict(alloc_dict(door, map), types.dict(key, value))
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
    /// `dict_type` rides verbatim; the cells are re-sectioned at `door`. See [`Self::list_rehomed`].
    pub fn dict_rehomed(
        door: SubstrateDoor<'a, '_>,
        map: HashMap<KKey<'_>, Held<'a>>,
        dict_type: KType,
    ) -> KObject<'a> {
        KObject::Dict(alloc_dict(door, map), dict_type)
    }

    /// Fresh `Record` carrier: memoizes the per-field type record as each field's
    /// `ktype()`. Field order follows declaration; equality is order-blind per the
    /// `Record` substrate. `door` is the substrate door the cells are sectioned through — see
    /// [`Self::record_of_held`].
    pub fn record(
        door: SubstrateDoor<'a, '_>,
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
    /// field-type join (this carrier's own `ktype()`); the cells are then sectioned through `door` in
    /// declaration order, with the name→index table as the substrate's layout — the record door's
    /// sole construction site.
    pub fn record_of_held(
        door: SubstrateDoor<'a, '_>,
        fields: Record<Held<'a>>,
        types: &TypeRegistry,
    ) -> KObject<'a> {
        let field_types = fields.map(|v| v.ktype(types));
        KObject::Record(alloc_record(door, fields), types.record(field_types))
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
    /// join is unchanged and `record_type` rides verbatim; the cells are re-sectioned at `door`, so
    /// each rebuilt cell's run reads its own fresh stored facts — a run that named the source region
    /// names it no longer once the cell rebuilt owned.
    pub fn record_rehomed(
        door: SubstrateDoor<'a, '_>,
        fields: Record<Held<'a>>,
        record_type: KType,
    ) -> KObject<'a> {
        KObject::Record(alloc_record(door, fields), record_type)
    }

    /// Fresh `Tagged` carrier over one payload value. Sections the payload cell through `door` — the
    /// tagged door's sole construction site — then names the carrier `tag` / `identity`. `value` is
    /// deep-cloned into the substrate (a pointer copy for a substrate-carrier payload, whose own
    /// stored reach then becomes the payload run's); the caller keeps its borrow.
    ///
    /// The discriminant's bytes are re-bumped into `door`'s region, so the tag is home-resident like
    /// every other string a substrate door stores — a carrier whose tag still borrowed the caller's
    /// region would reach a region its own run never names.
    pub fn tagged(
        door: SubstrateDoor<'a, '_>,
        tag: &str,
        value: &KObject<'a>,
        identity: KType,
    ) -> KObject<'a> {
        let substrate = alloc_payload(door, value.deep_clone());
        KObject::Tagged {
            tag: door.alloc_text(tag),
            value: substrate,
            identity,
        }
    }

    /// Fresh `Wrapped` carrier for a **construction**, preserving the payload verbatim — including a
    /// nested `Wrapped`, so a newtype over another keeps every layer. Sections the payload cell
    /// through `door`. See [`Self::wrapped_peel`] for the re-tag verb.
    pub fn wrapped_hold(
        door: SubstrateDoor<'a, '_>,
        value: &KObject<'a>,
        type_id: KType,
    ) -> KObject<'a> {
        let substrate = alloc_payload(door, value.deep_clone());
        KObject::Wrapped {
            inner: substrate,
            type_id,
        }
    }

    /// Fresh `Wrapped` carrier for a **re-tag**, collapsing one `Wrapped` layer so `Wrapped.inner`'s
    /// payload is never itself `Wrapped` (the single-layer invariant): the new identity replaces the
    /// old. When `value` is already a `Wrapped`, its inner substrate borrow rides verbatim (O(1), no
    /// copy — the payload keeps its home, pinned by the carrier's reach); anything else sections a
    /// fresh payload cell over an independent `deep_clone` through `door`.
    pub fn wrapped_peel(
        door: SubstrateDoor<'a, '_>,
        value: &KObject<'a>,
        type_id: KType,
    ) -> KObject<'a> {
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
                substrate.is_empty() && *list_type == KType::LIST_OF_ANY
            }
            KObject::Dict(substrate, dict_type) => {
                substrate.is_empty() && *dict_type == KType::DICT_ANY_ANY
            }
            _ => false,
        }
    }

    /// Whether this is a **shallow scalar** — a fully-owned leaf (`Number`, `Bool`, `Null`) whose
    /// representation embeds no `&'a` region borrow and no [`Held`] cell. Such a value cannot
    /// reference any dep the construction fold was handed, so the dep-witness union is pure
    /// over-retention: the combinator gate ([`alloc_object_scalar`](crate::machine::core::StepAllocator::alloc_object_scalar))
    /// routes it to the no-fold path so it seals with an empty reach, rebuilding owned at `'static`.
    ///
    /// A `KString` is **not** one: its bytes live in a region's bump, so there is no `'static`
    /// rebuild for that no-fold path to make, and a string producer takes a fold door instead. Every
    /// other variant borrows (`KFunction`, `Module`) or holds cells that transitively might
    /// (`List`/`Dict`/`Record`/`Tagged`/`Wrapped`/`KExpression`), so it keeps the fold too.
    pub fn is_shallow_scalar(&self) -> bool {
        matches!(self, KObject::Number(_) | KObject::Bool(_) | KObject::Null)
    }

    /// True when every region borrow in `self` points into the walk's destination region. Only
    /// value-channel borrows are walked: `KFunction`, `Module`, `KExpression` splices, and a
    /// substrate carrier's (`Record`/`List`/`Dict`/`Tagged`/`Wrapped`) substrate address (O(1),
    /// never its cells). The `KType` tags (`List`/`Dict`/`Record` memos, `Tagged { identity }`,
    /// `Wrapped { type_id }`) are not walked — a handle is one `u128` naming registry-owned content,
    /// so it borrows no region at all.
    pub(crate) fn resident_in_visiting(&self, residence: &Residence<'_>) -> bool {
        match self {
            KObject::Number(_) | KObject::Bool(_) | KObject::Null => true,
            // A string's bytes live in some region's bump, and the bump keeps no address table — so
            // no audit can say which. Answering `false` is what keeps that unanswerable question off
            // every runtime-audited move-in: a bare string never crosses one, and a copying adoption
            // rebuilds it through the destination's own text door instead
            // ([`needs_destination_door`](Self::needs_destination_door)).
            KObject::KString(_) => false,
            KObject::KFunction(f) => residence.owns_function(f),
            // An expression is raw AST: its parts run, its keyword text and its structural cache
            // all live in the program storage that parsed them, which sits at the eternal tier and
            // outlives every region a holder could have. It names no producer region either — a
            // resolved sub-result lives only on the scheduler's `WorkingExpression`, which no value
            // cell can hold — so pointing at one pins nothing and can dangle nowhere. The
            // `KString` arm's sibling: a borrow no address table can place, settled structurally
            // rather than by a probe. Which half of that is typed and which is flow-cleared is
            // [`ProgramBrand`](crate::machine::core::ProgramBrand)'s doc.
            KObject::KExpression(_) => true,
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
            // A pointer copy: the string's region-hosted bytes are shared, never re-bumped. The
            // clone rides whatever pin already covers the original — a pin path, by construction:
            // a path claiming the region's release re-bumps at its destination instead.
            KObject::KString(s) => KObject::KString(s),
            KObject::Bool(b) => KObject::Bool(*b),
            KObject::Null => KObject::Null,
            // A pointer copy: the substrate borrow copies (`Copy`), never rebuilding the cells.
            KObject::List(substrate, list_type) => KObject::List(substrate, *list_type),
            KObject::Dict(substrate, dict_type) => KObject::Dict(substrate, *dict_type),
            KObject::KExpression(e) => KObject::KExpression(*e),
            KObject::KFunction(f) => KObject::KFunction(f),
            // A pointer copy: the payload substrate borrow copies (`Copy`), never rebuilding the
            // payload.
            KObject::Tagged {
                tag,
                value,
                identity,
            } => KObject::Tagged {
                tag,
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
    /// region borrow into its own home (Ruling 5, design/value-substrates.md), which is what makes
    /// this the shape question the adoption rules turn on: a substrate carrier cannot cross a
    /// checked move-in by a pointer copy, so a copying seam rebuilds it through the fold door.
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

    /// Whether a **copying** adoption of `self` has to rebuild it through a destination door rather
    /// than pointer-copy the top node. True for a substrate carrier ([`Self::embeds_substrate`]) and
    /// for a bare `KString`: both keep region storage the pointer copy would leave behind in a
    /// producer the copy's own reach then releases. A string's is its bump-hosted bytes, whose region
    /// no audit can name, so the copy has to re-bump them at the destination
    /// ([`copy_object_into`]) exactly as a substrate has to be re-sectioned there.
    ///
    /// Everything else — the scalars, a `KExpression` whose parts run stays in the eternal-tier
    /// storage that parsed it, a `KFunction` / `Module` borrow leaf that rides verbatim — copies as
    /// a top node under the fused mint.
    pub(crate) fn needs_destination_door(&self) -> bool {
        self.embeds_substrate() || matches!(self, KObject::KString(_))
    }
}

/// The sectioned alloc door's reach verdict for one cell — read off **stored facts**, one O(1) read
/// per cell, never a walk over the cell's contents:
///
/// - **Owned data** — a scalar, a type-channel cell, a quoted expression — lands in an
///   empty-reach run, as does a `KString`, whose bytes [`section_cells`] re-bumped into this
///   door's own region before the verdict is read.
/// - A **substrate carrier** hands in its own nested substrate's stored union, which is exact by
///   construction. The run-level self rule at the door
///   ([`Sectioned::build`](crate::witnessed::Sectioned::build)) drops that substrate's home when it
///   *is* the destination, so a co-resident sub-container contributes nothing of its own residence
///   while a foreign one contributes the region it lives in — which is what keeps the borrows-home
///   memo answering the question it exists for: does a borrow *leaf* point home.
/// - A `KFunction` / `Module` is a **born-borrowing seed** naming the scope it captures: the `FN`
///   door naming a closure's captured scope, the module door naming its child scope.
fn cell_reach<'a>(cell: &Held<'a>, door: SubstrateDoor<'a, '_>) -> CellReach<'a, FrameStorage> {
    match cell {
        Held::Type(_) | Held::UnresolvedType(_) => CellReach::Owned,
        Held::Object(o) => object_cell_reach(o, door),
    }
}

/// The [`Held::Object`] arm of [`cell_reach`] — see its doc for the per-shape rules.
fn object_cell_reach<'a>(
    o: &KObject<'a>,
    door: SubstrateDoor<'a, '_>,
) -> CellReach<'a, FrameStorage> {
    match o {
        // A string cell is `Owned` because [`section_cells`] re-bumped its bytes into this door's
        // own region first, so it borrows nothing outside the container it is landing in.
        KObject::Number(_) | KObject::KString(_) | KObject::Bool(_) | KObject::Null => {
            CellReach::Owned
        }
        // A `KFunction` is allocated into the very region that owns its captured scope — release-
        // enforced at `RegionBrand::alloc_function` — so naming that scope names its residence too.
        KObject::KFunction(f) => CellReach::Seed(scope_coverage(f.captured_scope().region_owner())),
        // A `Module` carries no such invariant: a transparent-ascription view re-tags a foreign
        // module into the viewing scope's own region, so its residence and its child scope's region
        // differ, and the residence is not recoverable from the value. The seed therefore names the
        // child scope — the module door of the design — *and* the door's holder, which pins wherever
        // the cell was read from.
        KObject::Module(m) => {
            let mut declared = scope_coverage(m.child_scope().region_owner());
            declared.absorb(door.holder());
            CellReach::Seed(declared)
        }
        // An expression borrows only the eternal-tier storage that parsed it, and it has no arm
        // that can name a producer region, so a cell holding one reaches nothing this door has to
        // pin.
        KObject::KExpression(_) => CellReach::Owned,
        KObject::Record(substrate, _) => pinned_cell(substrate.reach(), door),
        KObject::List(substrate, _) => pinned_cell(substrate.reach(), door),
        KObject::Dict(substrate, _) => pinned_cell(substrate.reach(), door),
        KObject::Tagged { value, .. } => pinned_cell(value.reach(), door),
        KObject::Wrapped { inner, .. } => pinned_cell(inner.reach(), door),
    }
}

/// The [`CellReach::Pinned`] verdict for a substrate-carrier cell: its nested substrate's stored
/// union, under the door's holder-rule proof. The door folds in the nested substrate's own home
/// region unless that is the destination itself.
fn pinned_cell<'a>(
    reach: &'a FrameReach,
    door: SubstrateDoor<'a, '_>,
) -> CellReach<'a, FrameStorage> {
    CellReach::Pinned {
        reach,
        coverage: door.holder(),
    }
}

/// The born-borrowing seed's declared pins: coverage of the scope's own region, from the owner the
/// scope names. An owner that fails to upgrade covers nothing — the storage is already gone, so
/// there is no region left to pin.
fn scope_coverage(owner: Weak<FrameStorage>) -> FrameCoverage {
    match owner.upgrade() {
        Some(owner) => FrameCoverage::of(owner),
        None => FrameCoverage::empty(),
    }
}

/// Section `cells` through `door`: re-home each top-node string ([`rehome_cell_text`]), store the
/// cell resident (so the door receives a `&'a Held<'a>` anchored to the container's own region, and
/// one pin covers a projected cell and its reach together), pair it with the verdict [`cell_reach`]
/// reads off its stored facts, and hand the whole batch to the alloc door. Returns the sectioned
/// storage, the value-level union the door mints, and the copy-cost fold — the shared body of every
/// container door.
fn section_cells<'a>(
    door: SubstrateDoor<'a, '_>,
    cells: Vec<Held<'a>>,
) -> (HeldCells<'a>, &'a FrameReach, u64) {
    let copy_cost = cells.iter().fold(0u64, |total, cell| {
        total.saturating_add(held_copy_cost(cell))
    });
    let inputs: Vec<CellInput<'a, 'a, Held<'static>, FrameStorage>> = cells
        .into_iter()
        .map(rehome_cell_text(door))
        .map(|cell| {
            // The verdict is read before the cell moves into storage: it names the cell's *stored*
            // reach, whose `&'a` lifetime is the region's, not this borrow's.
            let reach = cell_reach(&cell, door);
            CellInput {
                payload: door.alloc_cell_folded(cell),
                reach,
            }
        })
        .collect();
    let (cells, union) = Sectioned::build(door.handle(), inputs);
    (cells, union, copy_cost)
}

/// Re-bump a cell's **own** string bytes into `door`'s region, so a stored top-node string cell is
/// home-resident and its `Owned` reach verdict is exact. The cost is one memcpy per string per
/// container construction — what the `String::clone` in a cell's `deep_clone` cost at these same
/// sites before strings moved into the region.
///
/// Top-node only, and that is the whole rule: a **nested substrate** cell's own strings are already
/// home-resident in that substrate's region, which its stored reach union names, so the pinned-cell
/// verdict covers them and re-walking would be a deep copy this door does not do. A `Tagged`'s tag
/// rides its own substrate's region for the same reason ([`KObject::tagged`] re-bumped it there).
fn rehome_cell_text<'a: 'h, 'h>(door: SubstrateDoor<'a, 'h>) -> impl Fn(Held<'a>) -> Held<'a> + 'h {
    move |cell| match cell {
        Held::Object(KObject::KString(s)) => Held::Object(KObject::KString(door.alloc_text(s))),
        other => other,
    }
}

/// Section a list's elements and store the [`ListSubstrate`] — positional, so the layout is implicit
/// and a cell's index is its position.
fn alloc_list<'a>(door: SubstrateDoor<'a, '_>, items: Vec<Held<'a>>) -> &'a ListSubstrate<'a> {
    let (cells, reach, copy_cost) = section_cells(door, items);
    door.alloc_substrate_folded::<ListSubstrate<'static>>(ContainerSubstrate::new(
        ListLayout, cells, reach, copy_cost,
    ))
}

/// Sort a record's fields by name, section the cells in that order, and store the
/// [`RecordSubstrate`] under the region-hosted name slice that order makes an index. Sorting
/// happens **before** sectioning: the run partition is computed over the cell order handed to
/// [`section_cells`], so a later sort would mispair runs with cells. Names cannot repeat — the
/// incoming [`Record`] deduplicates last-wins upstream — so the sort is a total order over them and
/// binary search resolves a field exactly.
fn alloc_record<'a>(
    door: SubstrateDoor<'a, '_>,
    fields: Record<Held<'a>>,
) -> &'a RecordSubstrate<'a> {
    let mut pairs: Vec<(String, Held<'a>)> = fields.into_pairs().collect();
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    let mut names: Vec<&'a str> = Vec::with_capacity(pairs.len());
    let mut cells: Vec<Held<'a>> = Vec::with_capacity(pairs.len());
    for (name, cell) in pairs {
        names.push(door.alloc_text(&name));
        cells.push(cell);
    }
    let names = door.alloc_slice(&names);
    let (cells, reach, copy_cost) = section_cells(door, cells);
    door.alloc_substrate_folded::<RecordSubstrate<'static>>(ContainerSubstrate::new(
        RecordLayout::new(names),
        cells,
        reach,
        copy_cost,
    ))
}

/// Section a dict's value cells and store the [`DictSubstrate`] under the frozen key→index table.
/// Cell order follows the input map's iteration order, which is what makes dict entry order
/// unspecified. Incoming keys may borrow **anywhere** — a producer's region, a caller's staging
/// buffer — because this is the one site that re-homes them, so no dict door verb has to state a
/// residence rule of its own.
fn alloc_dict<'a>(
    door: SubstrateDoor<'a, '_>,
    map: HashMap<KKey<'_>, Held<'a>>,
) -> &'a DictSubstrate<'a> {
    let mut index: hashbrown::HashMap<KKey<'a>, usize> =
        hashbrown::HashMap::with_capacity(map.len());
    let mut cells: Vec<Held<'a>> = Vec::with_capacity(map.len());
    for (key, cell) in map {
        // A string key is re-bumped into the dict's own region as the table freezes, so the key
        // block is home-resident and the dict's run never has to name where a key came from.
        index.insert(key.rehomed(door), cells.len());
        cells.push(cell);
    }
    let (cells, reach, copy_cost) = section_cells(door, cells);
    door.alloc_substrate_folded::<DictSubstrate<'static>>(ContainerSubstrate::new(
        index, cells, reach, copy_cost,
    ))
}

/// Section one owned payload `value` as a [`PayloadSubstrate`]'s single cell through `door` — the
/// construction site every `Tagged` / `Wrapped` door verb ([`KObject::tagged`],
/// [`KObject::wrapped_hold`], the non-`Wrapped` arm of [`KObject::wrapped_peel`], and the seam copy
/// verb's tagged/wrapped arms) funnels through.
fn alloc_payload<'a>(door: SubstrateDoor<'a, '_>, value: KObject<'a>) -> &'a PayloadSubstrate<'a> {
    let (cells, reach, copy_cost) = section_cells(door, vec![Held::Object(value)]);
    door.alloc_substrate_folded::<PayloadSubstrate<'static>>(ContainerSubstrate::new(
        PayloadLayout,
        cells,
        reach,
        copy_cost,
    ))
}

/// The seam copy verb's total rebuild: reconstruct `value`'s entire reachable structure at `dest`'s
/// brand. A substrate carrier (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`) rebuilds each
/// cell recursively and sections a fresh substrate at `dest` through its door (so every run reads the
/// rebuilt cell's own stored facts — a run that named the source region names it no longer once the
/// cell rebuilt owned); a scalar rebuilds owned; a `KFunction` / `Module` borrow rides verbatim as a
/// born-borrowing seed naming its own scope; a `KExpression` rides verbatim as a pointer copy.
/// Total or not at all — a partial spine copy would pay the copy *and* keep the pin. See
/// [design/value-substrates.md § Escape](../../../../design/value-substrates.md#escape-pin-by-default).
pub(crate) fn copy_object_into<'b>(
    value: &KObject<'b>,
    dest: SubstrateDoor<'b, '_>,
) -> KObject<'b> {
    match value {
        KObject::Number(n) => KObject::Number(*n),
        // Re-bump at the destination: the copy verb claims release of the source region, so the
        // rebuilt string must not keep borrowing bytes that region owns. That is what keeps
        // [`retains_home`]'s `false` for a string exact.
        KObject::KString(s) => KObject::KString(dest.alloc_text(s)),
        KObject::Bool(b) => KObject::Bool(*b),
        KObject::Null => KObject::Null,
        // A pointer copy: the parts run lives in the eternal-tier storage that parsed it, which the
        // copy verb's release claim never covers, so there is nothing to re-bump here.
        KObject::KExpression(e) => KObject::KExpression(*e),
        KObject::KFunction(f) => KObject::KFunction(f),
        KObject::Module(m) => KObject::Module(m),
        KObject::Record(substrate, record_type) => {
            let fields: Record<Held<'b>> = Record::from_pairs(
                substrate
                    .fields()
                    .map(|(name, cell)| (name.to_string(), copy_held_into(cell, dest)))
                    .collect::<Vec<_>>(),
            );
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
            let rebuilt: HashMap<KKey<'b>, Held<'b>> = substrate
                .entries()
                .map(|(key, cell)| (*key, copy_held_into(cell, dest)))
                .collect();
            KObject::dict_rehomed(dest, rebuilt, *dict_type)
        }
        KObject::Tagged {
            tag,
            value,
            identity,
        } => KObject::Tagged {
            tag: dest.alloc_text(tag),
            value: alloc_payload(dest, copy_object_into(value.payload(), dest)),
            identity: *identity,
        },
        KObject::Wrapped { inner, type_id } => KObject::Wrapped {
            inner: alloc_payload(dest, copy_object_into(inner.payload(), dest)),
            type_id: *type_id,
        },
    }
}

/// Relocate one value into `dest` under the chosen [`RegionEscape`].
///
/// A **`Copy`** verb claims the source region's release, so a value that would otherwise leave
/// region storage behind is totally rebuilt at the door ([`copy_object_into`]): a substrate carrier
/// (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`), whose substrate must land in `dest`, and a
/// bare `KString`, whose bytes must be re-bumped there. [`KObject::needs_destination_door`] is that
/// question, and it is the whole gate — a string left as a pointer copy would keep borrowing bump
/// bytes the released region owns, which no audit can catch.
///
/// Everything else is a pointer copy ([`deep_clone`](KObject::deep_clone)): a scalar, a `KFunction`
/// / `Module` leaf riding its borrow verbatim, and — under **`Pin`** — a substrate carrier too,
/// whose region-resident borrow rides covered by the Kept-minted producer reach at the enclosing
/// transfer.
///
/// Shared by the execute-side seam hooks and the core adoption engine
/// ([`Scope::relocate_delivered`](crate::machine::core::Scope)).
pub(crate) fn relocate_object_into<'b>(
    value: &KObject<'b>,
    verb: RegionEscape,
    dest: SubstrateDoor<'b, '_>,
) -> KObject<'b> {
    match verb {
        // Copy: a value keeping region storage behind ([`KObject::needs_destination_door`] — a
        // substrate carrier, or a bare `KString` whose bytes live in the producer's bump) is totally
        // rebuilt at the door, so nothing it points at stays in a region the copy's own retention
        // claim then releases. That predicate is the whole gate: keying on the substrate variants
        // alone would leave a string pointer-copied under a release claim.
        RegionEscape::Copy { .. } if value.needs_destination_door() => {
            copy_object_into(value, dest)
        }
        // Everything else is a pointer copy: a scalar, a `KFunction` / `Module` leaf riding its
        // borrow verbatim, or — under Pin — a substrate carrier whose region-resident borrow rides,
        // covered by the Kept-minted producer reach at the enclosing transfer.
        _ => value.deep_clone(),
    }
}

/// [`copy_object_into`]'s per-cell dispatch for a [`Held`] field / element: an object rebuilds
/// recursively, a type-channel cell is owned data copied verbatim.
fn copy_held_into<'b>(cell: &Held<'b>, dest: SubstrateDoor<'b, '_>) -> Held<'b> {
    match cell {
        Held::Object(o) => Held::Object(copy_object_into(o, dest)),
        Held::Type(t) => Held::Type(*t),
        Held::UnresolvedType(ti) => Held::UnresolvedType(*ti),
    }
}

/// Whether `value` still borrows `home` — the region it lives in — after a copying relocation. The
/// release question every relocation asks, and the only region a relocation may release; answered off
/// **stored facts**, with no walk over the value's shape and no address-table probe:
///
/// - A **borrow leaf** — a `KFunction`, a `Module` — is never rebuilt: a copying relocation carries
///   its reference verbatim, so it still borrows wherever it lives, which is `home`. That is the
///   answer regardless of which scope it borrows: a transparent-ascription view re-tags a foreign
///   module into the viewing scope's own region, so a module's residence and its child scope's
///   region genuinely differ, and only residence decides whether the reference survives a release.
///   Answering structurally rather than by reading residence is exact wherever a leaf is resident in
///   its own carrier's home, and conservatively retains where it is not — a carrier deep-cloned into
///   a binding scope while its leaf stays where it was. Retaining more can never dangle.
/// - A **composite** reads its substrate's stored reach union, which the sectioned alloc door
///   composed from its cells' own runs. Exact for the question asked: a cell resident in the
///   substrate's own region contributes no residence of its own (the run-level self rule), so a set
///   answer means a genuine borrow leaf reaches `home`.
/// - **Owned data** — scalars — borrows nothing. A **string** answers `false` for the same reason
///   though its bytes do live in a region: the copy verb re-bumps them at the destination
///   ([`copy_object_into`]), so the relocation genuinely releases whichever region held them. A
///   **`KExpression`** answers `false` because the only storage it borrows is the eternal-tier
///   program storage that parsed it, which no relocation releases and `home` is never.
pub(crate) fn retains_home(value: &KObject<'_>, home: &KoanRegion) -> bool {
    match value {
        KObject::Number(_)
        | KObject::KString(_)
        | KObject::Bool(_)
        | KObject::Null
        | KObject::KExpression(_) => false,
        KObject::KFunction(_) | KObject::Module(_) => true,
        KObject::Record(substrate, _) => substrate.reach().pins_region(home),
        KObject::List(substrate, _) => substrate.reach().pins_region(home),
        KObject::Dict(substrate, _) => substrate.reach().pins_region(home),
        KObject::Tagged { value, .. } => value.reach().pins_region(home),
        KObject::Wrapped { inner, .. } => inner.reach().pins_region(home),
    }
}

/// The [`RegionEscape`] verb for a top-level record, chosen per value in O(1) from its memos and the
/// producer host's allocated total. Non-record values never reach this — they always copy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RegionEscape {
    /// Borrow rides, the producer region transfers by hold; the relocate hook pointer-copies the
    /// record (its substrate borrow rides, covered by the reach the transfer mints).
    Pin,
    /// Total rebuild of the value's reachable structure at the destination brand. `released`: the
    /// rebuild provably frees the retiring producer region, so the transfer claims the empty
    /// source bundle.
    Copy { released: bool },
}

/// A seam tuning constant: copy a priceable home-crossing record only when its exact rebuild cost
/// is under 1/`ALPHA_DIVISOR` of what the pin would retain (the host's allocated total). Not
/// observable in language semantics; provisional pending measurement.
const ALPHA_DIVISOR: u64 = 4;

/// The escape-seam copy-vs-pin decision for a top-level container `value` (whose cell substrate is
/// `substrate`) crossing out of producer `host`. O(1), every read a stored fact: the home-crossing
/// test compares the home the substrate's own reach description records against `host` by region
/// identity, and the release claim on every copying arm is a stored read ([`retains_home`]).
/// Generic over the substrate's cell payload `C`; only records instantiate it today. See
/// design/value-substrates.md § Cost-driven copy.
pub(crate) fn copy_or_pin<C>(
    substrate: &ContainerSubstrate<'_, C>,
    value: &KObject<'_>,
    host: &KoanRegion,
) -> RegionEscape {
    // Forced verification builds override the table for top-level records; `released` is a stored
    // read, so a forced copy is sound at either crossing.
    match SEAM_POLICY {
        SeamPolicy::ForcePin => return RegionEscape::Pin,
        SeamPolicy::ForceCopy => {
            return RegionEscape::Copy {
                released: !retains_home(value, host),
            }
        }
        SeamPolicy::CostDriven => {}
    }

    // The substrate's description records the region its storage lives in — which a pin-bind can
    // separate from the residence of a wrapper value sharing the substrate. The bit read below is
    // home-relative to the *substrate's* home, so it only prices a crossing out of that region.
    let home_crossing = substrate
        .reach()
        .with_home_region(|home| std::ptr::eq(home, host));
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
            KObject::KString(s) => (*s).to_string(),
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
