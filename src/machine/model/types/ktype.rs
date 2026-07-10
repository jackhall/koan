//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at `from_name` time. There's no bare `KFunction` — "any function" with
//! no signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates live in `ktype_predicates.rs`; elaboration lives in `ktype_resolution.rs`.
//!
//! Lifetime parameter `'a`: the module/signature variants (`Module`, `Signature`,
//! `AbstractType`) hold `&'a Module<'a>` / `&'a ModuleSignature<'a>` region pointers; every other
//! variant is owned data and ignores the parameter.

use super::kkind::KKind;
use super::record::Record;
use super::recursive_set::{NominalSchema, RecursiveSet};
use super::signature::DeferredReturnSurface;
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::ScopeId;
use crate::machine::core::{FrameSet, KoanRegion, Residence, StoredReach};
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::model::values::{Module, ModuleSignature};
use std::rc::Rc;

/// Root of a [`KType::AbstractType`] identity. `Sig` carries the SIG decl_scope's id for a
/// member named at SIG-declaration time (no `&Module` to project members off); `Module`
/// carries the per-call opaque-ascription module so `Foo.Type` can project further members.
/// `scope_id()` is the identity key both variants contribute to `AbstractType` equality.
#[derive(Clone, Copy)]
pub enum AbstractSource<'a> {
    Sig(ScopeId),
    Module(&'a Module<'a>),
}

impl<'a> AbstractSource<'a> {
    pub fn scope_id(&self) -> ScopeId {
        match self {
            AbstractSource::Sig(id) => *id,
            AbstractSource::Module(m) => m.scope_id(),
        }
    }
}

#[derive(Clone)]
pub enum KType<'a> {
    Number,
    Str,
    Bool,
    Null,
    /// Bare `List` lowers to `List<Any>`.
    List(Box<KType<'a>>),
    /// Bare `Dict` lowers to `Dict<Any, Any>`.
    Dict(Box<KType<'a>>, Box<KType<'a>>),
    /// Structural record type (`:{x :Number, y :Str}`) — an identifier-keyed field schema
    /// with width/depth subtyping, order-blind by `(name, type)` for identity and
    /// declaration-ordered for rendering. A record-repr `NewType` `SetRef` wraps this with a
    /// nominal identity; the bare record type stays structural. A record *value*
    /// (`KObject::Record`) memoizes it as its carried type. Subtyping is the dual of the
    /// function-parameter relation — width-*superset* is more specific, covariant depth —
    /// see `record_value_more_specific`.
    Record(Box<Record<KType<'a>>>),
    /// `params` is the parameter record `(name → type)` — order preserved for rendering,
    /// equality order-blind by `(name, type)`. koan has no positional call syntax, so a
    /// function-typed slot records the names a caller must use to invoke the function it
    /// receives. Field name matches `KFunctor::params` so the two arms share join /
    /// specificity logic; the variant tag still keeps the two families admissibly disjoint.
    KFunction {
        params: Record<KType<'a>>,
        ret: Box<KType<'a>>,
    },
    /// Structural functor type — mirrors `KFunction` storage and rendering, but
    /// carries no admissibility against `KFunction` (the cross-arms in
    /// `function_compat` refuse both directions).
    ///
    /// `body` distinguishes a *bound functor value* from a *type annotation*. A
    /// `LET F = (FUNCTOR …)` name-binding registers the functor type-side carrying
    /// `body: Some(f)` — the callable `&KFunction` so a later `:(F {…})` / `F {…}`
    /// application can invoke it. The `:(FUNCTOR …)` type-position annotation
    /// carries `body: None` (no callable, just a shape). `body` is identity-inert:
    /// it is excluded from `PartialEq`, `Hash`, admissibility, join, and rendering,
    /// so two structurally-identical functor types compare and hash equal
    /// regardless of body.
    KFunctor {
        params: Record<KType<'a>>,
        ret: Box<KType<'a>>,
        body: Option<&'a KFunction<'a>>,
    },
    /// Confined carrier for a synthesized FN/FUNCTOR `ret` slot whose source return is a
    /// `ReturnType::Deferred` — a per-call-elaborated return like `-> Er` or
    /// `-> Er.Type`. Holds only the hashable surface shadow
    /// ([`DeferredReturnSurface`]) so equality/hashing/specificity read the deferred
    /// shape directly instead of coarsening it to `Any`. Valid *only* inside a
    /// `KFunction`/`KFunctor` `ret` box that `function_value_ktype` builds; no runtime
    /// value's `ktype()` returns it, and it admits nothing on its own
    /// (`accepts_part` is `false`). Admission against a precise slot is syntactic
    /// equality of the surface shadow — see
    /// [ktype/parameterization-and-variance.md § Variance](../../../../design/typing/ktype/parameterization-and-variance.md#variance).
    DeferredReturn(DeferredReturnSurface),
    Identifier,
    /// Lazy slot: accepts an unevaluated `ExpressionPart::Expression` so the builtin chooses
    /// when (or whether) to run it.
    KExpression,
    /// Lazy slot for a `:(...)` type expression — the sibling of [`KType::KExpression`] for a
    /// `SigiledTypeExpr` part. Captures it raw (via `resolve_for`, as the inner
    /// `KObject::KExpression`) instead of eager-sub-dispatching, so a builtin can defer a
    /// param-referencing dotted/sigil return (`-> Er.Type`) to per-call elaboration. More
    /// specific than [`KType::OfKind(KKind::ProperType)`], so it wins the overload when both admit.
    SigiledTypeExpr,
    /// Lazy slot for a `:{…}` record type — the sibling of [`KType::SigiledTypeExpr`] for a
    /// [`ExpressionPart::RecordType`](crate::machine::model::ast::ExpressionPart::RecordType)
    /// part. Captures the field list raw (via `resolve_for`, as the inner
    /// `KObject::KExpression`) so the NEWTYPE record-repr declarator owns its elaboration and
    /// threads its own binder name. More specific than [`KType::OfKind`], so it wins the
    /// overload when both admit.
    RecordType,
    /// Type-accepting argument slot, carrying the shallow [`KKind`] it admits — and the
    /// `ktype()` a non-module / non-signature type value reports (`OfKind(Proper)`). A module
    /// or signature *value* reports its exact `Module { .. }` / `Signature { .. }` identity
    /// instead; `kind_of` classifies a type into its `KKind`, matched against the slot's kind.
    OfKind(KKind),
    /// External reference to a member of a [`RecursiveSet`]. The `(set ptr, index)` pair
    /// is the dispatch identity; the member's `kind` (read via `set.member(index).kind`) is
    /// what `kind_of` reports to classify this nominal into its family. The whole set rides
    /// every `SetRef`, so lift shares it by `Rc::clone` — see [`crate::machine::execute::lift`].
    SetRef {
        set: Rc<RecursiveSet<'a>>,
        index: usize,
    },
    /// A single variant of a tagged-union member, reached *through* its union. `(set, index)`
    /// names the `KKind::Tagged` member; `tag` selects one variant within it. A
    /// refinement of the union: `Variant` is strictly more specific than the union's
    /// `SetRef` and than the `OfKind(Tagged)` kind, so a slot typed `:(Maybe Some)`
    /// admits only `Some` values while a `:Maybe` slot admits any variant. A Tagged-kind
    /// `KObject::Tagged` value reports its `Variant` from `ktype()`. Identity is
    /// `(set ptr, index, tag)`; the whole set rides every `Variant`, so lift shares it by
    /// `Rc::clone`, like [`KType::SetRef`].
    Variant {
        set: Rc<RecursiveSet<'a>>,
        index: usize,
        tag: String,
    },
    /// Intra-set sibling reference — a bare index resolved against the ambient set during
    /// deep traversal only. Carries no `Rc`, so a set holds no internal refcount cycle and
    /// frees once its last external handle drops. Never reaches the predicates (matching is
    /// shallow `SetRef` identity that does not descend a member's schema).
    SetLocal(usize),
    /// First-class handle to a whole [`RecursiveSet`], bound by a `RECURSIVE TYPES` group
    /// name. Identity is the set pointer (`Rc::ptr_eq`); lift shares the set by `Rc::clone`
    /// through the derived `Clone`. Inert in value dispatch — it names a group of types, not
    /// a value type — and reserved for value-language cycle construction.
    RecursiveGroup(Rc<RecursiveSet<'a>>),
    /// A module signature: both the introspectable value (`decl_scope` via `sig`) and the
    /// dispatch constraint ("any module satisfying `sig`"). Disambiguated by position — as a
    /// parameter slot it matches a module whose `compatible_sigs` contains `sig.sig_id()`; as
    /// a value (a `Signature { .. }` in the value channel's `Type` arm) it is matched by the
    /// `OfKind(Signature)` wildcard.
    ///
    /// `pinned_slots` carries `WITH` abstract-type specializations (empty for a bare
    /// signature), each an abstract-type slot pinned to a concrete `KType`. The vec is
    /// order-preserving (rather than a `HashMap`) so structural equality is deterministic.
    /// Identity is `sig.sig_id()` + `pinned_slots`; `sig.path` is diagnostic-only.
    Signature {
        sig: &'a ModuleSignature<'a>,
        pinned_slots: Vec<(String, KType<'a>)>,
    },
    /// First-class module value's type. A bare borrow into the region the functor call minted the
    /// module in; that region is pinned by the value carrier's witness set when the module flows down
    /// a dep edge (see [`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into)). A
    /// *concrete* module is rejected as a function's resolved return type (a module value's identity
    /// is not a return type — return a signature or the `:Module` kind), so it never rides the
    /// contract channel.
    Module {
        module: &'a Module<'a>,
    },
    /// Abstract type member named by a SIG slot or minted by opaque ascription. `source`
    /// distinguishes the two roots: `Sig(scope_id)` is the SIG-decl-time member (bound when a
    /// SIG-local `LET Carrier = ...` would otherwise collapse to the underlying type), `Module`
    /// is the per-call mint `:|` produces (`Foo.Carrier`). Identity keys on
    /// `(source.scope_id(), name)`, so two opaque ascriptions of the same source module with
    /// the same abstract name compare equal, and a per-call module mint stays distinct from
    /// the SIG-decl-time member it was threaded from.
    AbstractType {
        source: AbstractSource<'a>,
        name: String,
    },
    /// Application of a higher-kinded type constructor to arg types. `ctor` is a `SetRef`
    /// to a `TypeConstructor`-kind member; `args` are the elaborated arg types. Structural
    /// equality by `(ctor, args)`.
    ConstructorApply {
        ctor: Box<KType<'a>>,
        args: Vec<KType<'a>>,
    },
    /// Definition-time transient: a reference to a not-yet-sealed nominal (self or forward
    /// sibling) while elaborating a type-definition body. Sealed into a [`KType::SetLocal`]
    /// index at the member's finalize, so it never survives into a sealed type and never
    /// reaches the predicates. Equality is by name only.
    RecursiveRef(String),
    /// Bind-time transient for a bare-leaf type name that couldn't be lowered to a concrete
    /// `KType` at the synchronous [`ExpressionPart::resolve_for`](crate::machine::model::ast::ExpressionPart::resolve_for)
    /// seam — a name not in [`KType::from_name`]'s builtin table (`Point`, `IntOrd`, `MyList`).
    /// Sibling to [`RecursiveRef`](KType::RecursiveRef): it rides the value channel's `Type`
    /// arm, never reaches the dispatch predicates, and is consumed + replaced by the
    /// park-capable [`Scope::resolve_type_identifier`](crate::machine::core::Scope::resolve_type_identifier).
    /// Carries the structured `TypeIdentifier` so the surface form survives the bind.
    Unresolved(TypeIdentifier),
    Any,
}

impl<'a> KType<'a> {
    /// Surface-syntax rendering. The rendered form parses back to the same `KType`
    /// through the dispatch-driven type-language path (see
    /// [type-language via dispatch](../../../../design/typing/type-language-via-dispatch.md)).
    pub fn name(&self) -> String {
        match self {
            KType::Number => "Number".into(),
            KType::Str => "Str".into(),
            KType::Bool => "Bool".into(),
            KType::Null => "Null".into(),
            KType::List(t) => format!(":(LIST OF {})", t.name()),
            KType::Dict(k, v) => format!(":(MAP {} -> {})", k.name(), v.name()),
            // `:{x :Number y :Str}` — the braced type-sigil surface. Fields render
            // space-separated like FN params (the field-list parser accepts that).
            KType::Record(r) => format!(":{{{}}}", render_param_record(r)),
            KType::KFunction { params, ret } => {
                format!(":(FN ({}) -> {})", render_param_record(params), ret.name())
            }
            KType::KFunctor { params, ret, .. } => {
                format!(
                    ":(FUNCTOR ({}) -> {})",
                    render_param_record(params),
                    ret.name()
                )
            }
            KType::DeferredReturn(s) => s.render(),
            KType::Identifier => "Identifier".into(),
            KType::KExpression => "KExpression".into(),
            KType::SigiledTypeExpr => "SigiledTypeExpr".into(),
            KType::RecordType => "RecordType".into(),
            KType::OfKind(k) => k.surface_keyword().into(),
            KType::SetRef { set, index } => set.member(*index).name.clone(),
            // `:(Maybe Some)` — the variant reached through its union. Round-trips through
            // the union-qualified type sigil.
            KType::Variant { set, index, tag } => {
                format!(":({} {})", set.member(*index).name, tag)
            }
            // Diagnostic-only: a sibling reference renders against no ambient set here, so
            // report the slot index. Deep traversal resolves it against the set.
            KType::SetLocal(i) => format!("SetLocal({i})"),
            KType::RecursiveGroup(set) => {
                let names: Vec<&str> = set.members().iter().map(|m| m.name.as_str()).collect();
                format!("RECURSIVE TYPES ({})", names.join(" "))
            }
            KType::Signature { sig, pinned_slots } => {
                if pinned_slots.is_empty() {
                    sig.path.clone()
                } else {
                    // Display-only; does not round-trip through the parser.
                    let inner: Vec<String> = pinned_slots
                        .iter()
                        .map(|(name, kt)| format!("{} = {}", name, kt.name()))
                        .collect();
                    format!("({} WITH {{{}}})", sig.path, inner.join(", "))
                }
            }
            KType::Module { module, .. } => module.path.clone(),
            KType::AbstractType { name, .. } => name.clone(),
            KType::RecursiveRef(name) => name.clone(),
            KType::Unresolved(t) => t.render(),
            KType::ConstructorApply { ctor, args } => {
                let arg_names: Vec<String> = args.iter().map(|a| a.name()).collect();
                format!(":({} {})", ctor.name(), arg_names.join(" "))
            }
            KType::Any => "Any".into(),
        }
    }

    /// Stable entry point for diagnostic rendering. Reserved seam for cycle-aware printing.
    pub fn render(&self) -> String {
        self.name()
    }

    /// Variant-wise rebuild at `'static`. `Some` exactly when the rebuild is
    /// possible without a region borrow and without re-minting a shared set:
    /// - `Module` / `Signature` / `AbstractType { source: Module(_) }` /
    ///   `KFunctor { body: Some(_) }` hold region pointers -> `None`.
    /// - `SetRef` / `Variant` / `RecursiveGroup` share their set by `Rc` and
    ///   compare by `Rc::ptr_eq`; a rebuilt set is a different identity -> `None`
    ///   (such values take the runtime-checked resident path instead).
    /// - every other variant rebuilds recursively.
    pub fn to_static(&self) -> Option<KType<'static>> {
        match self {
            KType::Number => Some(KType::Number),
            KType::Str => Some(KType::Str),
            KType::Bool => Some(KType::Bool),
            KType::Null => Some(KType::Null),
            KType::List(t) => Some(KType::List(Box::new(t.to_static()?))),
            KType::Dict(k, v) => Some(KType::Dict(
                Box::new(k.to_static()?),
                Box::new(v.to_static()?),
            )),
            KType::Record(r) => Some(KType::Record(Box::new(record_to_static(r)?))),
            KType::KFunction { params, ret } => Some(KType::KFunction {
                params: record_to_static(params)?,
                ret: Box::new(ret.to_static()?),
            }),
            // A bound functor value's `body` is a live region pointer.
            KType::KFunctor { body: Some(_), .. } => None,
            KType::KFunctor {
                params,
                ret,
                body: None,
            } => Some(KType::KFunctor {
                params: record_to_static(params)?,
                ret: Box::new(ret.to_static()?),
                body: None,
            }),
            KType::DeferredReturn(s) => Some(KType::DeferredReturn(s.clone())),
            KType::Identifier => Some(KType::Identifier),
            KType::KExpression => Some(KType::KExpression),
            KType::SigiledTypeExpr => Some(KType::SigiledTypeExpr),
            KType::RecordType => Some(KType::RecordType),
            KType::OfKind(k) => Some(KType::OfKind(*k)),
            // `Rc`-shared set: rebuilding would mint a new `Rc` and break identity.
            KType::SetRef { .. } => None,
            KType::Variant { .. } => None,
            KType::SetLocal(i) => Some(KType::SetLocal(*i)),
            KType::RecursiveGroup(_) => None,
            // Region pointers.
            KType::Signature { .. } => None,
            KType::Module { .. } => None,
            KType::AbstractType {
                source: AbstractSource::Sig(id),
                name,
            } => Some(KType::AbstractType {
                source: AbstractSource::Sig(*id),
                name: name.clone(),
            }),
            KType::AbstractType {
                source: AbstractSource::Module(_),
                ..
            } => None,
            KType::ConstructorApply { ctor, args } => {
                let ctor = Box::new(ctor.to_static()?);
                let mut static_args = Vec::with_capacity(args.len());
                for a in args {
                    static_args.push(a.to_static()?);
                }
                Some(KType::ConstructorApply {
                    ctor,
                    args: static_args,
                })
            }
            KType::RecursiveRef(s) => Some(KType::RecursiveRef(s.clone())),
            KType::Unresolved(t) => Some(KType::Unresolved(t.clone())),
            KType::Any => Some(KType::Any),
        }
    }

    /// True when every region borrow in `self` points into `dest` and every `Rc`'d set member is
    /// transitively free of foreign borrows. The runtime twin of [`Self::to_static`]:
    /// `to_static().is_some()` implies this for any `dest`, but this also answers for the
    /// `SetRef` / `Variant` / `RecursiveGroup` values `to_static` declines — the whole reason the
    /// checked path exists for them (rebuilding the set would break `Rc` identity, but the set's
    /// members can still be audited by address without rebuilding anything).
    pub(crate) fn resident_in(&self, dest: &KoanRegion) -> bool {
        let residence = Residence::dest_only(dest);
        let mut visited = Vec::new();
        self.resident_in_visiting(&residence, &mut visited)
    }

    /// The evidence-widened twin of [`Self::resident_in`]: every region borrow in `self` must
    /// point into `dest` **or** be covered by `reach`'s foreign reach — the reaching tier's
    /// coverage predicate. Exact for `KType`, whose only region pointers (`&Module` /
    /// `&ModuleSignature` / `&KFunction`) each expose their own region directly, so no
    /// member-enumeration is needed (see [`Residence`]).
    pub(crate) fn resident_in_reach(&self, dest: &KoanRegion, reach: &StoredReach<'_>) -> bool {
        let sets: &[&FrameSet] = match &reach.foreign {
            Some(fs) => std::slice::from_ref(fs),
            None => &[],
        };
        let residence = Residence::with_reach(dest, sets);
        let mut visited = Vec::new();
        self.resident_in_visiting(&residence, &mut visited)
    }

    /// [`Self::resident_in`]/[`Self::resident_in_reach`]'s shared recursive walk, threading a
    /// `visited` list of `RecursiveSet` `Rc` addresses so a set reachable via more than one path
    /// (a shared nominal referenced from two fields) is walked at most once. `pub(crate)` (not
    /// private) so [`KObject::resident_in`](crate::machine::model::values::KObject::resident_in)'s
    /// walk can recurse into a memoized/carried `KType` tag under the same `Residence`.
    pub(crate) fn resident_in_visiting(
        &self,
        residence: &Residence<'_>,
        visited: &mut Vec<*const ()>,
    ) -> bool {
        match self {
            KType::Number
            | KType::Str
            | KType::Bool
            | KType::Null
            | KType::DeferredReturn(_)
            | KType::Identifier
            | KType::KExpression
            | KType::SigiledTypeExpr
            | KType::RecordType
            | KType::OfKind(_)
            | KType::SetLocal(_)
            | KType::RecursiveRef(_)
            | KType::Unresolved(_)
            | KType::Any => true,
            KType::List(t) => t.resident_in_visiting(residence, visited),
            KType::Dict(k, v) => {
                k.resident_in_visiting(residence, visited)
                    && v.resident_in_visiting(residence, visited)
            }
            KType::Record(r) => record_resident_in(r, residence, visited),
            KType::KFunction { params, ret } => {
                record_resident_in(params, residence, visited)
                    && ret.resident_in_visiting(residence, visited)
            }
            KType::KFunctor { params, ret, body } => {
                let body_ok = match body {
                    Some(f) => residence.owns_function(f),
                    None => true,
                };
                body_ok
                    && record_resident_in(params, residence, visited)
                    && ret.resident_in_visiting(residence, visited)
            }
            KType::SetRef { set, .. } | KType::Variant { set, .. } | KType::RecursiveGroup(set) => {
                set_resident_in(set, residence, visited)
            }
            KType::Signature { sig, pinned_slots } => {
                residence.owns_signature(sig)
                    && pinned_slots
                        .iter()
                        .all(|(_, kt)| kt.resident_in_visiting(residence, visited))
            }
            KType::Module { module } => residence.owns_module(module),
            KType::AbstractType {
                source: AbstractSource::Sig(_),
                ..
            } => true,
            KType::AbstractType {
                source: AbstractSource::Module(m),
                ..
            } => residence.owns_module(m),
            KType::ConstructorApply { ctor, args } => {
                ctor.resident_in_visiting(residence, visited)
                    && args
                        .iter()
                        .all(|a| a.resident_in_visiting(residence, visited))
            }
        }
    }

    /// Classify a *type* into its shallow dispatch [`KKind`] — the value-side direction of
    /// `OfKind`. A module is `Module`, a signature is `Signature`, a user-declared nominal is
    /// its family (`Tagged` / `NewType` / `TypeConstructor`, read off the set member it
    /// references), and every other type is `Proper`. Never returns `KKind::AnyType` (a slot-only
    /// expectation). Applied to the type a type value carries — or a runtime value's
    /// `ktype()` — to match it against an `OfKind` slot.
    pub fn kind_of(&self) -> KKind {
        match self {
            KType::Module { .. } => KKind::Module,
            KType::Signature { .. } => KKind::Signature,
            // A nominal carries its family on the set member. A `Variant` is always a
            // `Tagged` member; a `ConstructorApply` defers to its `ctor` (a
            // `TypeConstructor`-kind `SetRef`).
            KType::SetRef { set, index } => set.member(*index).kind,
            KType::Variant { set, index, .. } => set.member(*index).kind,
            KType::ConstructorApply { ctor, .. } => ctor.kind_of(),
            _ => KKind::ProperType,
        }
    }
}

/// Field-wise `'static` rebuild of a parameter/field record for [`KType::to_static`].
/// `Record::map` cannot express a fallible per-field transform, so this walks `iter()`
/// directly and short-circuits on the first region-borrowing field.
fn record_to_static(record: &Record<KType<'_>>) -> Option<Record<KType<'static>>> {
    let mut out = Record::new();
    for (name, kt) in record.iter() {
        out.insert(name.clone(), kt.to_static()?);
    }
    Some(out)
}

/// Field-wise residence audit of a parameter/field record for [`KType::resident_in`] — the
/// checked-path sibling of [`record_to_static`].
fn record_resident_in(
    record: &Record<KType<'_>>,
    residence: &Residence<'_>,
    visited: &mut Vec<*const ()>,
) -> bool {
    record
        .iter()
        .all(|(_, kt)| kt.resident_in_visiting(residence, visited))
}

/// Residence audit of every member schema in a [`RecursiveSet`] shared by [`KType::SetRef`] /
/// [`KType::Variant`] / [`KType::RecursiveGroup`] — the checked path those variants take since
/// [`KType::to_static`] declines them (rebuilding the set would mint a new `Rc` and break
/// identity). `visited` guards a set reachable via more than one member from being walked twice —
/// a `Vec` linear scan is fine since sets are small and this is not a hot path. An unfilled
/// member schema (mid-declaration, before its own finalize) has nothing to check yet, so it's
/// trivially resident.
fn set_resident_in(
    set: &Rc<RecursiveSet<'_>>,
    residence: &Residence<'_>,
    visited: &mut Vec<*const ()>,
) -> bool {
    let ptr = Rc::as_ptr(set) as *const ();
    if visited.contains(&ptr) {
        return true;
    }
    visited.push(ptr);
    set.members()
        .iter()
        .all(|member| match member.schema().as_ref() {
            None => true,
            Some(NominalSchema::Tagged(members)) => members
                .values()
                .all(|kt| kt.resident_in_visiting(residence, visited)),
            Some(NominalSchema::NewType(kt)) => kt.resident_in_visiting(residence, visited),
            Some(NominalSchema::TypeConstructor { schema, .. }) => schema
                .values()
                .all(|kt| kt.resident_in_visiting(residence, visited)),
        })
}

/// Render an FN/FUNCTOR parameter record as the comma-free `name :type` group the
/// `:(FN (...) -> _)` surface re-parses. A leaf type surface gets a `:` prefix; one that
/// already opens a sigil (`:(LIST OF Number)`) is left as-is (no `::`).
fn render_param_record(params: &Record<KType<'_>>) -> String {
    params
        .iter()
        .map(|(name, kt)| {
            let surface = kt.name();
            if surface.starts_with(':') {
                format!("{name} {surface}")
            } else {
                format!("{name} :{surface}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Manual `PartialEq` — `Module`, `Signature`, and `AbstractType` carry region pointers
/// whose identity is the pointee's stable `scope_id()` / `sig_id()` rather than the raw
/// pointer. Two opaque ascriptions of the same source module produce different `&Module`
/// (each allocates a fresh child scope) and must NOT compare equal under `KType::Module`;
/// two `KType::AbstractType` values minted from the same source-and-name MUST compare
/// equal even when their `&Module` pointers differ.
impl<'a> PartialEq for KType<'a> {
    fn eq(&self, other: &Self) -> bool {
        use KType::*;
        match (self, other) {
            (Number, Number)
            | (Str, Str)
            | (Bool, Bool)
            | (Null, Null)
            | (Identifier, Identifier)
            | (KExpression, KExpression)
            | (SigiledTypeExpr, SigiledTypeExpr)
            | (RecordType, RecordType)
            | (Any, Any) => true,
            (OfKind(a), OfKind(b)) => a == b,
            (List(a), List(b)) => a == b,
            (Dict(ka, va), Dict(kb, vb)) => ka == kb && va == vb,
            (Record(a), Record(b)) => a == b,
            (
                KFunction {
                    params: p1,
                    ret: r1,
                },
                KFunction {
                    params: p2,
                    ret: r2,
                },
            ) => p1 == p2 && r1 == r2,
            // `body` is identity-inert: two functor types with different (or no)
            // bodies but the same `params`/`ret` compare equal.
            (
                KFunctor {
                    params: p1,
                    ret: r1,
                    ..
                },
                KFunctor {
                    params: p2,
                    ret: r2,
                    ..
                },
            ) => p1 == p2 && r1 == r2,
            // Identity is `(set ptr, index)` ONLY — never descend the schema, which is
            // cyclic. `Rc::ptr_eq` keys the shared allocation; lift preserves it.
            (SetRef { set: s1, index: i1 }, SetRef { set: s2, index: i2 }) => {
                Rc::ptr_eq(s1, s2) && i1 == i2
            }
            // Variant identity is `(set ptr, index, tag)` — the union member plus the
            // selected tag, never the (cyclic) schema.
            (
                Variant {
                    set: s1,
                    index: i1,
                    tag: t1,
                },
                Variant {
                    set: s2,
                    index: i2,
                    tag: t2,
                },
            ) => Rc::ptr_eq(s1, s2) && i1 == i2 && t1 == t2,
            (SetLocal(a), SetLocal(b)) => a == b,
            (
                Signature {
                    sig: s1,
                    pinned_slots: p1,
                },
                Signature {
                    sig: s2,
                    pinned_slots: p2,
                },
            ) => s1.sig_id() == s2.sig_id() && p1 == p2,
            (Module { module: m1, .. }, Module { module: m2, .. }) => {
                m1.scope_id() == m2.scope_id()
            }
            (
                AbstractType {
                    source: s1,
                    name: n1,
                },
                AbstractType {
                    source: s2,
                    name: n2,
                },
            ) => s1.scope_id() == s2.scope_id() && n1 == n2,
            (ConstructorApply { ctor: c1, args: a1 }, ConstructorApply { ctor: c2, args: a2 }) => {
                c1 == c2 && a1 == a2
            }
            (RecursiveRef(n1), RecursiveRef(n2)) => n1 == n2,
            (Unresolved(a), Unresolved(b)) => a == b,
            // Whole-set handle: identity is the set pointer, never the (cyclic) schema.
            (RecursiveGroup(a), RecursiveGroup(b)) => Rc::ptr_eq(a, b),
            (DeferredReturn(a), DeferredReturn(b)) => a == b,
            _ => false,
        }
    }
}
impl<'a> Eq for KType<'a> {}

/// Manual `Hash`, kept consistent with the hand-written `PartialEq` above
/// (`a == b` ⟹ `hash(a) == hash(b)`): each arm hashes exactly the fields its `PartialEq`
/// arm compares. The region-pointer variants hash their stable identity key
/// (`scope_id()` / `source.scope_id()` / `sig_id()`), never the raw pointer; the set
/// variants hash `Rc::as_ptr` + index, never the cyclic schema.
impl<'a> std::hash::Hash for KType<'a> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        use KType::*;
        std::mem::discriminant(self).hash(state);
        match self {
            Number | Str | Bool | Null | Identifier | KExpression | SigiledTypeExpr
            | RecordType | Any => {}
            OfKind(k) => k.hash(state),
            List(t) => t.hash(state),
            Dict(k, v) => {
                k.hash(state);
                v.hash(state);
            }
            Record(r) => r.hash(state),
            KFunction { params, ret } => {
                params.hash(state);
                ret.hash(state);
            }
            // `body` is excluded to stay consistent with `PartialEq`.
            KFunctor { params, ret, .. } => {
                params.hash(state);
                ret.hash(state);
            }
            SetRef { set, index } => {
                (Rc::as_ptr(set) as *const ()).hash(state);
                index.hash(state);
            }
            // Set-pointer + index + tag — matching `PartialEq`, never the cyclic schema.
            Variant { set, index, tag } => {
                (Rc::as_ptr(set) as *const ()).hash(state);
                index.hash(state);
                tag.hash(state);
            }
            SetLocal(i) => i.hash(state),
            Signature { sig, pinned_slots } => {
                sig.sig_id().hash(state);
                pinned_slots.hash(state);
            }
            Module { module, .. } => module.scope_id().hash(state),
            AbstractType { source, name } => {
                source.scope_id().hash(state);
                name.hash(state);
            }
            ConstructorApply { ctor, args } => {
                ctor.hash(state);
                args.hash(state);
            }
            RecursiveRef(n) => n.hash(state),
            Unresolved(t) => t.hash(state),
            // Set-pointer identity ONLY — never the cyclic schema, matching `PartialEq`.
            RecursiveGroup(set) => (Rc::as_ptr(set) as *const ()).hash(state),
            DeferredReturn(s) => s.hash(state),
        }
    }
}

/// Manual `Debug` — `derive` is blocked because `Module` / `ModuleSignature` / `FrameStorage`
/// don't implement `Debug` and recursing through module-typed KTypes is unbounded.
impl<'a> std::fmt::Debug for KType<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KType({})", self.name())
    }
}

#[cfg(test)]
mod tests;
