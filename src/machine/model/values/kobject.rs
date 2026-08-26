use std::collections::HashMap;
use std::rc::Weak;

use crate::machine::core::KFunction;
use crate::machine::core::{
    FrameCoverage, FrameReach, FrameStorage, KoanRegion, KoanRegionExt, SubstrateDoor,
};
use crate::machine::model::ast::{KExpression, ProgramExpression};
use crate::machine::model::labels::{BinderSymbol, Symbol, TypeSymbol};
use crate::machine::model::registries::RunRegistries;
use crate::machine::model::types::render_label;
use crate::machine::model::types::{KType, Parseable, Record, TypeNode, TypeRegistry};
use crate::witnessed::{BumpVec, CellInput, CellReach, Sectioned};

use super::container_substrate::{
    HeldCells, ListLayout, PayloadLayout, RecordLayout, held_copy_cost,
};
use super::rehomed::Rehomed;
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
///
/// `Copy` because every arm is a scalar or a region borrow — the value owns no allocation, so it
/// runs no `Drop` at region death. That is what lets a cell ride the `T: Copy` bump doors
/// ([`RegionBrand::allocator`](crate::machine::core::RegionBrand::allocator)), where the bound
/// is the `Drop`-freedom proof. The derived `Clone` is that same shallow copy; **duplicating** a
/// value — rebuilding its substrates in a destination region — is [`Self::deep_clone`].
#[derive(Clone, Copy)]
pub enum KObject<'a> {
    Number(f64),
    /// String value: a region-hosted `&'a str`, bumped into the region the value lives in
    /// ([`RegionBrand::allocator`](crate::machine::core::RegionBrand::allocator)). The slot owns
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
    /// Quoted / captured AST as data. The payload is the marked [`ProgramExpression`], not a bare
    /// node: the cell's `Owned` reach verdict and its `false` [`retains_home`] answer are exactly
    /// the claim that marker carries, so the arm takes its proof as an operand.
    KExpression(ProgramExpression<'a>),
    KFunction(&'a KFunction<'a>),
    /// Tagged-union value. The `value` field is a region borrow of the payload's
    /// [`PayloadSubstrate`] — the single payload cell in sectioned storage plus its stored reach and
    /// copy cost. `identity` is the value's own type handle: the
    /// union member's `SetMember` handle when the carrier's type arguments are erased, or the
    /// `ConstructorApply` over that member when an ascription stamped a parameterized union's
    /// arguments in. One handle carries what the member reference and the runtime type
    /// arguments used to carry separately, so `ktype()` is a copy and identity comparison is
    /// one `u128`. The tag is the variant's classified name symbol — a fixed-width `Copy` key, so a
    /// construction stores no discriminant bytes and a tag comparison is a symbol compare. Construct
    /// via [`KObject::tagged`] — never the struct literal directly outside this
    /// module, and never `Rc::new`: the substrate is born only through
    /// [`FoldingBrand::alloc_substrate_folded`].
    Tagged {
        tag: TypeSymbol,
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

/// A [`KObject`]'s **owned leaf** arms, with no lifetime: `Number`, `Bool`, `Null`. The shape a value
/// takes when it borrows no region at all — not even for string bytes, which is why `KString` is not
/// here and takes the region brand's `alloc_string` door instead.
///
/// It exists so "region-free" is a *type* rather than a predicate a caller re-checks. The store door
/// (`RegionBrand::alloc_scalar`) takes one of these, so a value that borrows a region cannot reach
/// it — where a `KObject<'static>` parameter would say the same thing but leave the door with
/// unbuildable arms to rebuild, since `KObject` is lifetime-invariant and a `'static` one has no
/// coercion to the destination's `'a`.
#[derive(Clone, Copy)]
pub enum Scalar {
    Number(f64),
    Bool(bool),
    Null,
}

impl Scalar {
    /// This leaf as a value at any region's lifetime — the rebuild the store door writes. Total, and
    /// free of a lifetime retype: each arm is reconstructed from its owned payload.
    pub fn into_object<'a>(self) -> KObject<'a> {
        match self {
            Scalar::Number(n) => KObject::Number(n),
            Scalar::Bool(b) => KObject::Bool(b),
            Scalar::Null => KObject::Null,
        }
    }
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
        let held: Vec<Held<'a>> = items.into_iter().map(Held::Object).collect();
        KObject::list_of_held(door, &held, types)
    }

    /// Fresh `List` carrier over [`Held`] cells — the type-aware path (a list element may be
    /// a first-class type). One pass over `items` computes the memoized element-type join (this
    /// carrier's own `ktype()`); the substrate door then sections the cells, folding their stored
    /// reach verdicts into the runs and the value-level union ([`section_cells`]).
    pub fn list_of_held(
        door: SubstrateDoor<'a, '_>,
        items: &[Held<'a>],
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
        KObject::List(alloc_list(door, &items), list_type)
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
    /// key→index table frozen into the region's bump (last-wins dedup already happened in the
    /// transient input map).
    ///
    /// **Keys carry no reach** ([`KKey`] admits only `String` / `Number` / `Bool`, so a key naming a
    /// substrate or a closure is unrepresentable here, and a string key's bytes are re-bumped into
    /// this dict's own region by the door below); a key produced from a carrier is rejected in O(1)
    /// off the stored envelope when that carrier names any reach member.
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
    /// `ktype()`. Field order follows the names, not the literal; equality is order-blind per the
    /// `Record` substrate. `door` is the substrate door the cells are sectioned through — see
    /// [`Self::record_of_held`].
    pub fn record(
        door: SubstrateDoor<'a, '_>,
        fields: &[(BinderSymbol, KObject<'a>)],
        types: &TypeRegistry,
    ) -> KObject<'a> {
        let mut held: BumpVec<'a, (BinderSymbol, Held<'a>)> =
            BumpVec::with_capacity_in(fields.len(), door.allocator());
        held.extend(
            fields
                .iter()
                .map(|(name, value)| (*name, Held::Object(*value))),
        );
        KObject::record_of_held(door, &held, types)
    }

    /// Fresh `Record` carrier over [`Held`] field cells — the type-aware path (a field
    /// value may be a first-class type). One pass over `fields` computes the memoized
    /// field-type join (this carrier's own `ktype()`); the cells are then sectioned through `door`
    /// name-sorted, aligned with the bump-hosted name slice that is the substrate's whole layout.
    pub fn record_of_held(
        door: SubstrateDoor<'a, '_>,
        fields: &[(BinderSymbol, Held<'a>)],
        types: &TypeRegistry,
    ) -> KObject<'a> {
        let field_types =
            Record::from_pairs(fields.iter().map(|(name, cell)| (*name, cell.ktype(types))));
        KObject::Record(
            alloc_record(
                door,
                fields.iter().map(|(name, cell)| (name.symbol(), *cell)),
            ),
            types.record(field_types),
        )
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
        fields: &[(Symbol, Held<'a>)],
        record_type: KType,
    ) -> KObject<'a> {
        KObject::Record(alloc_record(door, fields.iter().copied()), record_type)
    }

    /// Fresh `Tagged` carrier over one payload value. Sections the payload cell through `door`, then
    /// names the carrier `tag` / `identity`. `value` is deep-cloned into the substrate (a pointer
    /// copy for a substrate-carrier payload, whose own stored reach then becomes the payload run's);
    /// the caller keeps its borrow.
    ///
    /// The discriminant is the variant's name symbol, which points into no region: it rides the
    /// carrier as fixed-width `Copy` data, so a construction bumps the payload cell and nothing
    /// else, and the tag can never reach a region the carrier's own run does not name.
    pub fn tagged(
        door: SubstrateDoor<'a, '_>,
        tag: TypeSymbol,
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
    /// Only parameterized carriers re-tag, and each re-tags to `declared` itself —
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

    /// This value's owned leaf form, or `None` if it is not one — the **shallow scalar** test and the
    /// rebuild in a single verb, so no caller can pair the predicate with a rebuild that disagrees
    /// with it.
    ///
    /// A [`Scalar`] embeds no `&'a` region borrow and no [`Held`] cell, so it cannot reference any dep
    /// the construction fold was handed and the dep-witness union over it would be pure
    /// over-retention: the combinator gate
    /// ([`alloc_object_scalar`](crate::machine::core::StepAllocator::alloc_object_scalar)) routes a
    /// hit to the no-fold path, where it seals with an empty reach.
    ///
    /// A `KString` is **not** one: its bytes live in a region's bump, so a rebuild has to re-bump them
    /// at its destination and a string producer takes a fold door instead. Every other variant borrows
    /// (`KFunction`, `Module`) or holds cells that transitively might
    /// (`List`/`Dict`/`Record`/`Tagged`/`Wrapped`/`KExpression`), so it keeps the fold too.
    pub fn as_scalar(&self) -> Option<Scalar> {
        match *self {
            KObject::Number(n) => Some(Scalar::Number(n)),
            KObject::Bool(b) => Some(Scalar::Bool(b)),
            KObject::Null => Some(Scalar::Null),
            _ => None,
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
                tag: *tag,
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
    /// each of which directly borrows a region-resident substrate. Purely structural: no residence
    /// is read here. A substrate is always a genuine region borrow into its own home (Ruling 5,
    /// design/value-substrates.md), which is what makes this the shape question the adoption rules
    /// turn on: a substrate carrier cannot move regions by a pointer copy, so a copying seam
    /// rebuilds it through the fold door.
    pub(crate) fn embeds_substrate(&self) -> bool {
        // Exhaustive on purpose: a new variant must declare its shape here, because defaulting to
        // `false` would let a copying seam pointer-copy it under a release claim.
        match self {
            KObject::Record(..)
            | KObject::List(..)
            | KObject::Dict(..)
            | KObject::Tagged { .. }
            | KObject::Wrapped { .. } => true,
            KObject::Number(_)
            | KObject::KString(_)
            | KObject::Bool(_)
            | KObject::Null
            | KObject::KExpression(_)
            | KObject::KFunction(_)
            | KObject::Module(_) => false,
        }
    }

    /// Whether a **copying** adoption of `self` has to rebuild it through a destination door rather
    /// than pointer-copy the top node. True for a substrate carrier ([`Self::embeds_substrate`]) and
    /// for a bare `KString`: both keep region storage the pointer copy would leave behind in a
    /// producer the copy's own reach then releases. A string's is its bump-hosted bytes, whose region
    /// no audit can name, so the copy has to re-bump them at the destination
    /// ([`copy_object_into`]) exactly as a substrate has to be re-sectioned there.
    ///
    /// Everything else — the scalars, a `KExpression` whose [`ProgramExpression`] payload keeps its
    /// parts run in program storage, a `KFunction` / `Module` borrow leaf that rides verbatim — copies as
    /// a top node under the fused mint.
    pub(crate) fn needs_destination_door(&self) -> bool {
        // Exhaustive on purpose, like [`Self::embeds_substrate`]: a new variant defaulting into the
        // pointer-copy arm under a release claim is the dangerous direction.
        match self {
            KObject::Record(..)
            | KObject::List(..)
            | KObject::Dict(..)
            | KObject::Tagged { .. }
            | KObject::Wrapped { .. }
            | KObject::KString(_) => true,
            KObject::Number(_)
            | KObject::Bool(_)
            | KObject::Null
            | KObject::KExpression(_)
            | KObject::KFunction(_)
            | KObject::Module(_) => false,
        }
    }
}

/// The sectioned alloc door's reach verdict for one cell — read off **stored facts**, one O(1) read
/// per cell, never a walk over the cell's contents:
///
/// - **Owned data** — a scalar, a type-channel cell, a quoted expression — lands in an
///   empty-reach run, as does a `KString`, whose bytes the [`Rehomed`] mint re-bumped into this
///   door's own region. That is why this reads a token rather than a bare `Held`: the `Owned`
///   verdict is exact only after the re-home, and taking the token makes running it first a
///   signature obligation instead of an ordering comment.
/// - A **substrate carrier** hands in its own nested substrate's stored union, which is exact by
///   construction. The run-level self rule at the door
///   ([`Sectioned::build`](crate::witnessed::Sectioned::build)) drops that substrate's home when it
///   *is* the destination, so a co-resident sub-container contributes nothing of its own residence
///   while a foreign one contributes the region it lives in — which is what keeps the borrows-home
///   memo answering the question it exists for: does a borrow *leaf* point home.
/// - A `KFunction` / `Module` is a **born-borrowing seed** naming the scope it captures: the `FN`
///   door naming a closure's captured scope, the module door naming its child scope.
fn cell_reach<'a>(cell: &Rehomed<'a>, door: SubstrateDoor<'a, '_>) -> CellReach<'a, FrameStorage> {
    match cell.cell() {
        Held::Type(_) | Held::UnresolvedType(_) => CellReach::Owned,
        // A name carrier lives in a bound-argument slot, never in a container substrate.
        Held::Identifier(_) => unreachable!("a captured identifier is never a substrate cell"),
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
        // A `KFunction` is born into the very region that owns its captured scope — the
        // destination is derived from that scope at `KFunction::alloc_captured`, so it cannot be
        // otherwise — and naming that scope therefore names its residence too.
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
        // The cell's payload is a [`ProgramExpression`], so its parts run is program-storage hosted
        // by type. Program storage is no region a holder can outlive, so a cell holding one reaches
        // nothing this door has to pin.
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

/// Section `cells` through `door`: re-home each top-node string ([`Rehomed::mint`]), store the
/// cell resident (so the door receives a `&'a Held<'a>` anchored to the container's own region, and
/// one pin covers a projected cell and its reach together), and pair it with the two facts the
/// alloc door folds — the reach verdict [`cell_reach`] reads off its stored facts, and the copy
/// weight [`held_copy_cost`] prices. Returns the sectioned storage and the value-level union the
/// door mints — the shared body of every container door.
///
/// One pass and no staging buffer — the input chain streams straight into the alloc door — and no
/// fold of its own: both facts ride the same [`CellInput`], so the container's copy cost is the
/// sectioned storage's own [`Sectioned::weight`] rather than a total this door re-derives.
///
/// The re-home/verdict ordering is carried by the [`Rehomed`] token rather than by this function's
/// statement order: `cell_reach` takes one, and only the mint produces one.
fn section_cells<'a>(
    door: SubstrateDoor<'a, '_>,
    cells: &[Held<'a>],
) -> (HeldCells<'a>, &'a FrameReach) {
    let inputs = cells
        .iter()
        .copied()
        .map(|cell| Rehomed::mint(door, cell))
        .map(|cell| {
            // The verdict is read before the cell moves into storage: it names the cell's *stored*
            // reach, whose `&'a` lifetime is the region's, not this borrow's.
            let reach = cell_reach(&cell, door);
            let weight = held_copy_cost(cell.cell());
            CellInput {
                payload: door.alloc_cell_folded(cell.into_cell()),
                reach,
                weight,
            }
        });
    Sectioned::build(door.handle(), inputs)
}

/// Section a list's elements and store the [`ListSubstrate`] — positional, so the layout is implicit
/// and a cell's index is its position.
fn alloc_list<'a>(door: SubstrateDoor<'a, '_>, items: &[Held<'a>]) -> &'a ListSubstrate<'a> {
    let (cells, reach) = section_cells(door, items);
    door.alloc_substrate_folded(ContainerSubstrate::new(ListLayout, cells, reach))
}

/// Sort a record's fields into canonical symbol order, section the cells in that order, and store
/// the [`RecordSubstrate`] under the region-hosted symbol slice that order makes an index. Sorting
/// happens **before** sectioning: the run partition is computed over the cell order handed to
/// [`section_cells`], so a later sort would mispair runs with cells. Names cannot repeat — the
/// incoming [`Record`] deduplicates last-wins upstream — so the sort is a total order over them and
/// binary search resolves a field exactly.
fn alloc_record<'a>(
    door: SubstrateDoor<'a, '_>,
    fields: impl ExactSizeIterator<Item = (Symbol, Held<'a>)>,
) -> &'a RecordSubstrate<'a> {
    // Both working buffers are bumped in the destination region itself — the construction storage
    // the door already writes the name slice and the cells into — so building a record takes no
    // heap container at any size.
    let mut pairs: BumpVec<'a, (Symbol, Held<'a>)> =
        BumpVec::with_capacity_in(fields.len(), door.allocator());
    pairs.extend(fields);
    pairs.sort_unstable_by_key(|pair| pair.0);
    let names = door
        .allocator()
        .slice_from_iter(pairs.iter().map(|(symbol, _)| *symbol));
    let mut cells: BumpVec<'a, Held<'a>> = BumpVec::with_capacity_in(pairs.len(), door.allocator());
    cells.extend(pairs.iter().map(|(_, cell)| *cell));
    let (cells, reach) = section_cells(door, &cells);
    door.alloc_substrate_folded(ContainerSubstrate::new(
        RecordLayout::new(names),
        cells,
        reach,
    ))
}

/// Section a dict's value cells and store the [`DictSubstrate`] under the region-hosted key→index
/// table the door bumps beside them.
/// Cell order follows the input map's iteration order, which is what makes dict entry order
/// unspecified. Incoming keys may borrow **anywhere** — a producer's region, a caller's staging
/// buffer — because they are re-homed here, so no dict door verb has to state a residence rule of
/// its own.
fn alloc_dict<'a>(
    door: SubstrateDoor<'a, '_>,
    map: HashMap<KKey<'_>, Held<'a>>,
) -> &'a DictSubstrate<'a> {
    let mut entries: Vec<(KKey<'a>, usize)> = Vec::with_capacity(map.len());
    let mut cells: Vec<Held<'a>> = Vec::with_capacity(map.len());
    for (key, cell) in map {
        // A string key is re-bumped into the dict's own region as the table freezes, so the key
        // block is home-resident and the dict's run never has to name where a key came from.
        entries.push((key.rehomed(door), cells.len()));
        cells.push(cell);
    }
    let index = door.allocator().frozen_table(entries);
    let (cells, reach) = section_cells(door, &cells);
    door.alloc_substrate_folded(ContainerSubstrate::new(index, cells, reach))
}

/// Section one owned payload `value` as a [`PayloadSubstrate`]'s single cell through `door` — the
/// construction site every `Tagged` / `Wrapped` door verb ([`KObject::tagged`],
/// [`KObject::wrapped_hold`], the non-`Wrapped` arm of [`KObject::wrapped_peel`], and the seam copy
/// verb's tagged/wrapped arms) funnels through.
fn alloc_payload<'a>(door: SubstrateDoor<'a, '_>, value: KObject<'a>) -> &'a PayloadSubstrate<'a> {
    let (cells, reach) = section_cells(door, &[Held::Object(value)]);
    door.alloc_substrate_folded(ContainerSubstrate::new(PayloadLayout, cells, reach))
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
        KObject::KString(s) => KObject::KString(dest.allocator().text(s)),
        KObject::Bool(b) => KObject::Bool(*b),
        KObject::Null => KObject::Null,
        // A pointer copy: the payload's marker puts its parts run in program storage, which the
        // copy verb's release claim never covers, so there is nothing to re-bump here.
        KObject::KExpression(e) => KObject::KExpression(*e),
        KObject::KFunction(f) => KObject::KFunction(f),
        KObject::Module(m) => KObject::Module(m),
        KObject::Record(substrate, record_type) => {
            let mut fields: BumpVec<'b, (Symbol, Held<'b>)> =
                BumpVec::with_capacity_in(substrate.len(), dest.allocator());
            fields.extend(
                substrate
                    .fields()
                    .map(|(symbol, cell)| (symbol, copy_held_into(cell, dest))),
            );
            KObject::record_rehomed(dest, &fields, *record_type)
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
            tag: *tag,
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
        RegionEscape::Copy if value.needs_destination_door() => copy_object_into(value, dest),
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
        // A name carrier lives in a bound-argument slot, never in a container substrate.
        Held::Identifier(_) => unreachable!("a captured identifier is never a substrate cell"),
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
///   **`KExpression`** answers `false` on the proof its own payload carries: a
///   [`ProgramExpression`] borrows program storage and nothing else, which no relocation releases
///   and `home` is never.
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

/// The escape verb for a top-level container value, chosen per value in O(1) from its memos and the
/// producer host's allocated total. Values with no container substrate never reach this — they
/// always copy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RegionEscape {
    /// Borrow rides, the producer region transfers by hold; the relocate hook pointer-copies the
    /// record (its substrate borrow rides, covered by the reach the transfer mints).
    Pin,
    /// Total rebuild of the value's reachable structure at the destination brand. Whether the
    /// rebuild frees the retiring producer region is not stored here: the fold's retention claim
    /// derives it from the **product** the rebuild built ([`product_reaches_region`]), so the
    /// verdict and the act cannot disagree.
    ///
    /// [`product_reaches_region`]: crate::machine::core::product_reaches_region
    Copy,
}

/// A seam tuning constant: copy a priceable home-crossing record only when its exact rebuild cost
/// is under 1/`ALPHA_DIVISOR` of what the pin would retain (the host's allocated total). Not
/// observable in language semantics; provisional pending measurement.
const ALPHA_DIVISOR: u64 = 4;

/// The escape-seam copy-vs-pin decision for a top-level container value (whose cell substrate is
/// `substrate`) crossing out of producer `host`. O(1), every read a stored fact: the home-crossing
/// test compares the home the substrate's own reach description records against `host` by region
/// identity. Generic over the substrate's cell payload `C`. See
/// [Cost-driven copy](../../../../design/value-substrates.md#cost-driven-copy-the-optimization).
pub(crate) fn copy_or_pin<C>(
    substrate: &ContainerSubstrate<'_, C>,
    host: &KoanRegion,
) -> RegionEscape {
    // Forced verification builds override the table for top-level records; the retention claim is
    // derived from the rebuilt product, so a forced copy is sound at either crossing.
    match SEAM_POLICY {
        SeamPolicy::ForcePin => return RegionEscape::Pin,
        SeamPolicy::ForceCopy => return RegionEscape::Copy,
        SeamPolicy::CostDriven => {}
    }

    // The bit read below is home-relative to the *substrate's* own home, so it only prices a
    // crossing out of that region — see [`ContainerSubstrate::homed_in`].
    if !substrate.homed_in(host) {
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
        RegionEscape::Copy
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
    /// Canonical surface rendering of a value. Carried types render through the registry, and
    /// record field labels resolve through the interner beside it.
    pub fn summarize(&self, registries: &RunRegistries) -> String {
        match self {
            KObject::Number(n) => n.to_string(),
            KObject::KString(s) => (*s).to_string(),
            KObject::Bool(b) => b.to_string(),
            KObject::List(substrate, _) => {
                let parts: Vec<String> = substrate
                    .elements()
                    .iter()
                    .map(|i| i.summarize(registries))
                    .collect();
                format!("[{}]", parts.join(", "))
            }
            KObject::Dict(substrate, _) => {
                let parts: Vec<String> = substrate
                    .entries()
                    .map(|(k, v)| format!("{}: {}", k.summarize(), v.summarize(registries)))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            KObject::KExpression(e) => e.summarize(&registries.labels),
            KObject::KFunction(f) => f.value_ktype().name(registries),
            KObject::Tagged { tag, value, .. } => {
                format!(
                    "{}({})",
                    render_label(tag.symbol(), registries),
                    value.payload().summarize(registries)
                )
            }
            KObject::Record(substrate, _) => {
                // The substrate lays cells out in symbol order, which carries no meaning to a
                // reader; rendering re-sorts by the resolved text so a printed record reads in
                // field-name order. A render-path sort only — the layout itself stays symbol-keyed.
                let mut fields: Vec<(String, String)> = substrate
                    .fields()
                    .map(|(field, value)| {
                        (render_label(field, registries), value.summarize(registries))
                    })
                    .collect();
                fields.sort_by(|left, right| left.0.cmp(&right.0));
                let parts: Vec<String> = fields
                    .into_iter()
                    .map(|(name, value)| format!("{name} = {value}"))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            KObject::Null => "null".to_string(),
            KObject::Wrapped { inner, type_id } => {
                format!(
                    "{}({})",
                    type_id.name(registries),
                    inner.payload().summarize(registries)
                )
            }
            KObject::Module(m) => m.path.to_string(),
        }
    }
}
