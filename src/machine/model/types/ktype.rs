//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at `from_name` time. There's no bare `KFunction` — "any function" with
//! no signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates live in `ktype_predicates.rs`; elaboration lives in `ktype_resolution.rs`.
//!
//! No variant borrows region data — `KType<'a>` holds only owned content, and the `'a` parameter
//! threads exclusively through container variants' own `KType<'a>` children (the empty signature,
//! a `SIG`-declared interface, and a module's self-sig are all one kind of owned-schema
//! `Signature` variant, holding an `Rc<SigContent<'a>>`).

use super::kkind::KKind;
use super::record::Record;
use super::recursive_set::{same_nominal, NominalSchema, RecursiveSet};
use super::sig_schema::SigContent;
use super::signature::DeferredReturnSurface;
use super::type_digest::{self, TypeDigest};
use crate::machine::core::ScopeId;
use crate::machine::core::{FrameSet, KoanRegion, Residence};
use crate::machine::model::ast::TypeIdentifier;
use std::rc::Rc;

#[derive(Clone)]
pub enum KType<'a> {
    Number,
    Str,
    Bool,
    Null,
    /// Bare `List` lowers to `List<Any>`. Build through [`KType::list`], which fills `digest`.
    List {
        element: Box<KType<'a>>,
        digest: TypeDigest,
    },
    /// Bare `Dict` lowers to `Dict<Any, Any>`. Build through [`KType::dict`].
    Dict {
        key: Box<KType<'a>>,
        value: Box<KType<'a>>,
        digest: TypeDigest,
    },
    /// Structural record type (`:{x :Number, y :Str}`) — an identifier-keyed field schema
    /// with width/depth subtyping, order-blind by `(name, type)` for identity and
    /// declaration-ordered for rendering. A record-repr `NewType` `SetRef` wraps this with a
    /// nominal identity; the bare record type stays structural. A record *value*
    /// (`KObject::Record`) memoizes it as its carried type. Subtyping is the dual of the
    /// function-parameter relation — width-*superset* is more specific, covariant depth —
    /// see `record_value_more_specific`. Build through [`KType::record`].
    Record {
        fields: Box<Record<KType<'a>>>,
        digest: TypeDigest,
    },
    /// `params` is the parameter record `(name → type)` — order preserved for rendering,
    /// equality order-blind by `(name, type)`. koan has no positional call syntax, so a
    /// function-typed slot records the names a caller must use to invoke the function it
    /// receives.
    KFunction {
        params: Record<KType<'a>>,
        ret: Box<KType<'a>>,
        digest: TypeDigest,
    },
    /// Confined carrier for a synthesized FN `ret` slot whose source return is a
    /// `ReturnType::Deferred` — a per-call-elaborated return like `-> er` or
    /// `-> er.Carrier`. Holds only the hashable surface shadow
    /// ([`DeferredReturnSurface`]) so equality/hashing/specificity read the deferred
    /// shape directly instead of coarsening it to `Any`. Valid *only* inside a
    /// `KFunction` `ret` box that `function_value_ktype` builds; no runtime
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
    /// param-referencing dotted/sigil return (`-> er.Carrier`) to per-call elaboration. More
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
    /// `ktype()` a non-signature type value reports (`OfKind(Proper)`). A signature *value* —
    /// and a module value, whose `ktype()` is its self-sig — reports its exact
    /// `Signature { .. }` identity instead; `kind_of` classifies a type into its `KKind`,
    /// matched against the slot's kind.
    OfKind(KKind),
    /// External reference to a member of a [`RecursiveSet`]. The `(set ptr, index)` pair
    /// is the dispatch identity; the member's `kind` (read via `set.member(index).kind`) is
    /// what `kind_of` reports to classify this nominal into its family. The whole set rides
    /// every `SetRef`, so lift shares it by `Rc::clone` — see [`crate::machine::execute::lift`].
    SetRef {
        set: Rc<RecursiveSet<'a>>,
        index: usize,
    },
    /// Untagged structural disjunction — the type `:(A | B)`. Members are canonical:
    /// deduplicated, no nested `Union`, always two or more (a single-member union is that
    /// member — [`union_of`](KType::union_of) collapses it). Identity is order-blind, so
    /// equality and hashing are set-based rather than positional. Each member is a subtype
    /// of the union; the union admits any value one of its members admits. Build through
    /// [`KType::union_of`], which canonicalizes then fills `digest`.
    Union {
        members: Vec<KType<'a>>,
        digest: TypeDigest,
    },
    /// Intra-set sibling reference — a bare index resolved against the ambient set during
    /// deep traversal only. Carries no `Rc`, so a set holds no internal refcount cycle and
    /// frees once its last external handle drops. Never reaches the predicates (matching is
    /// shallow `SetRef` identity that does not descend a member's schema).
    SetLocal(usize),
    /// First-class handle to a whole [`RecursiveSet`], bound by a `RECURSIVE TYPES` group
    /// name. Identity is the set's content digest (via `same_nominal`, index-free); lift shares
    /// the set by `Rc::clone` through the derived `Clone`. Inert in value dispatch — it names a
    /// group of types, not a value type — and reserved for value-language cycle construction.
    RecursiveGroup(Rc<RecursiveSet<'a>>),
    /// A module signature — owned interface content: a `SIG`-declared interface, a module's
    /// principal signature (its self-sig), and the empty signature (the lattice top the
    /// `:Module` name lowers to) are all this one kind, distinguished only by `content`'s fields.
    /// Both the introspectable signature value and the dispatch constraint ("any module `content`
    /// admits"). Disambiguated by position — as a parameter slot it matches a module via
    /// [`Module::satisfies_sig_content`](crate::machine::model::values::Module::satisfies_sig_content);
    /// as a signature value (in the value channel's `Type` arm) it is matched by the
    /// `OfKind(Signature)` wildcard. A module value's `ktype()` reports its self-sig content.
    ///
    /// `pinned_slots` carries `WITH` abstract-type specializations (empty for a bare
    /// signature), each an abstract-type slot pinned to a concrete `KType`. The vec is
    /// order-preserving (rather than a `HashMap`) so structural equality is deterministic.
    /// Identity is `content.schema_digest` + `pinned_slots`; `content.path` and `content.sig_id`
    /// are diagnostic/specificity-only, never part of identity.
    Signature {
        content: Rc<SigContent<'a>>,
        pinned_slots: Vec<(String, KType<'a>)>,
        digest: TypeDigest,
    },
    /// Abstract type member named by a SIG slot or minted by opaque ascription. `source` is the
    /// root scope the member is named against: a SIG decl_scope's id for the decl-time member
    /// (bound when a SIG-local `LET Carrier = ...` would otherwise collapse to the underlying
    /// type), or the per-call ascription module's id for the mint `:|` produces (`Foo.Carrier`).
    /// Owned data — the variant holds no region pointer. Identity keys on `(source, name)`, so two
    /// opaque ascriptions of the same source module with the same abstract name compare equal, and
    /// a per-call module mint stays distinct from the SIG-decl-time member it was threaded from.
    AbstractType {
        source: ScopeId,
        name: String,
    },
    /// Application of a higher-kinded type constructor to arg types. `ctor` is a `SetRef`
    /// to a `TypeConstructor`-kind member; `args` are the elaborated arg types. Structural
    /// equality by `(ctor, args)`.
    ConstructorApply {
        ctor: Box<KType<'a>>,
        args: Vec<KType<'a>>,
        digest: TypeDigest,
    },
    /// Definition-time transient: a reference to a not-yet-sealed nominal (self or forward
    /// sibling) while elaborating a type-definition body. Sealed into a [`KType::SetLocal`]
    /// index at the member's finalize, so it never survives into a sealed type and never
    /// reaches the predicates. Equality is by name only.
    RecursiveRef(String),
    /// Bind-time transient for a bare-leaf type name that couldn't be lowered to a concrete
    /// `KType` at the synchronous [`ExpressionPart::resolve_for`](crate::machine::model::ast::ExpressionPart::resolve_for)
    /// seam — a name not in [`KType::from_name`]'s builtin table (`Point`, `Wrapped`, `MyList`).
    /// Sibling to [`RecursiveRef`](KType::RecursiveRef): it rides the value channel's `Type`
    /// arm, never reaches the dispatch predicates, and is consumed + replaced by the
    /// park-capable [`Scope::resolve_type_identifier`](crate::machine::core::Scope::resolve_type_identifier).
    /// Carries the structured `TypeIdentifier` so the surface form survives the bind.
    Unresolved(TypeIdentifier),
    Any,
}

impl<'a> KType<'a> {
    /// The empty signature — top of the module lattice, the type the `:Module` name lowers to.
    /// It constrains nothing, so every module value satisfies it; a builtin module-accepting
    /// slot or return is typed this way. Builds a fresh [`SigContent`] per call — its `HashMap`s
    /// don't allocate empty, and every call site is registration/lowering, not a hot loop.
    pub fn empty_signature() -> KType<'a> {
        KType::signature(Rc::new(SigContent::empty()), Vec::new())
    }

    /// This type's content digest — its identity. Reads the stored field for the composite
    /// variants (`O(1)`) and computes the leaf / id-keyed / member-reference variants on
    /// demand. Keyed on by the run's verdict registry (`registry.rs`; see
    /// [type-identity.md § The memo registry](../../../../design/typing/type-identity.md#the-memo-registry)).
    pub fn digest(&self) -> TypeDigest {
        type_digest::digest_of(self)
    }

    // Smart constructors for the digest-carrying variants: each fills `digest` from its
    // children's stored digests (shallow — never a re-walk). Every construction of these
    // variants routes through one of these so no site can install a stale or absent digest.

    /// `List<element>`.
    pub fn list(element: Box<KType<'a>>) -> KType<'a> {
        let digest = type_digest::list_digest(element.digest());
        KType::List { element, digest }
    }

    /// `Dict<key, value>`.
    pub fn dict(key: Box<KType<'a>>, value: Box<KType<'a>>) -> KType<'a> {
        let digest = type_digest::dict_digest(key.digest(), value.digest());
        KType::Dict { key, value, digest }
    }

    /// A structural record type over `fields`.
    pub fn record(fields: Box<Record<KType<'a>>>) -> KType<'a> {
        let digest = type_digest::record_digest(&fields);
        KType::Record { fields, digest }
    }

    /// A function type `(params) -> ret`.
    pub fn function_type(params: Record<KType<'a>>, ret: Box<KType<'a>>) -> KType<'a> {
        let digest = type_digest::function_digest(&params, ret.digest());
        KType::KFunction {
            params,
            ret,
            digest,
        }
    }

    /// Application of a higher-kinded type constructor `ctor` to `args`.
    pub fn constructor_apply(ctor: Box<KType<'a>>, args: Vec<KType<'a>>) -> KType<'a> {
        let digest = type_digest::constructor_apply_digest(ctor.digest(), &args);
        KType::ConstructorApply { ctor, args, digest }
    }

    /// A module-signature type. Routes `empty_signature` and every `WITH`-pinned build.
    pub fn signature(
        content: Rc<SigContent<'a>>,
        pinned_slots: Vec<(String, KType<'a>)>,
    ) -> KType<'a> {
        let digest = type_digest::signature_digest(content.schema_digest, &pinned_slots);
        KType::Signature {
            content,
            pinned_slots,
            digest,
        }
    }

    /// Surface-syntax rendering. The rendered form parses back to the same `KType`
    /// through the dispatch-driven type-language path (see
    /// [type-language via dispatch](../../../../design/typing/type-language-via-dispatch.md)).
    pub fn name(&self) -> String {
        match self {
            KType::Number => "Number".into(),
            KType::Str => "Str".into(),
            KType::Bool => "Bool".into(),
            KType::Null => "Null".into(),
            KType::List { element, .. } => format!(":(LIST OF {})", element.name()),
            KType::Dict { key, value, .. } => {
                format!(":(MAP {} -> {})", key.name(), value.name())
            }
            // `:{x :Number y :Str}` — the braced type-sigil surface. Fields render
            // space-separated like FN params (the field-list parser accepts that).
            KType::Record { fields, .. } => format!(":{{{}}}", render_param_record(fields)),
            KType::KFunction { params, ret, .. } => {
                format!(":(FN ({}) -> {})", render_param_record(params), ret.name())
            }
            KType::DeferredReturn(s) => s.render(),
            KType::Identifier => "Identifier".into(),
            KType::KExpression => "KExpression".into(),
            KType::SigiledTypeExpr => "SigiledTypeExpr".into(),
            KType::RecordType => "RecordType".into(),
            KType::OfKind(k) => k.surface_keyword().into(),
            // A sealed nominal member renders by its own member name — a bare newtype
            // (`:Wrapper`) or a per-variant member reached through its union (`:(Maybe Some)`
            // yields the `Some` member, printed as `Some`).
            KType::SetRef { set, index } => set.member(*index).name.clone(),
            // `:(A | B)` — members joined by ` | ` and wrapped in the type sigil. A compound
            // member already opens its own sigil (`:(LIST OF Number)`), which nests fine.
            KType::Union { members, .. } => {
                let rendered: Vec<String> = members.iter().map(|m| m.name()).collect();
                format!(":({})", rendered.join(" | "))
            }
            // Diagnostic-only: a sibling reference renders against no ambient set here, so
            // report the slot index. Deep traversal resolves it against the set.
            KType::SetLocal(i) => format!("SetLocal({i})"),
            KType::RecursiveGroup(set) => {
                let names: Vec<&str> = set.members().iter().map(|m| m.name.as_str()).collect();
                format!("RECURSIVE TYPES ({})", names.join(" "))
            }
            KType::Signature {
                content,
                pinned_slots,
                ..
            } => {
                if pinned_slots.is_empty() {
                    content.path.clone()
                } else {
                    // Display-only; does not round-trip through the parser.
                    let inner: Vec<String> = pinned_slots
                        .iter()
                        .map(|(name, kt)| format!("{} = {}", name, kt.name()))
                        .collect();
                    format!("({} WITH {{{}}})", content.path, inner.join(", "))
                }
            }
            KType::AbstractType { name, .. } => name.clone(),
            KType::RecursiveRef(name) => name.clone(),
            KType::Unresolved(t) => t.render(),
            KType::ConstructorApply { ctor, args, .. } => {
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
    /// possible without re-minting a shared allocation:
    /// - `SetRef` / `RecursiveGroup` own their set by `Rc` (its content transport); `to_static`
    ///   declines rather than re-mint that shared allocation -> `None`, so such values take the
    ///   runtime-checked resident path instead. (Identity itself is the content digest, which a
    ///   rebuild would preserve — but rebuilding the set is still out of `to_static`'s remit.)
    /// - `Signature` declines for any non-`:Module` content or non-empty pins: rebuilding an
    ///   `Rc<SigContent<'a>>` at `'static` would mint a fresh allocation, so only the `:Module`
    ///   mint (which `empty_signature()` can always freshly build) rebuilds.
    /// - every other variant rebuilds recursively.
    pub fn to_static(&self) -> Option<KType<'static>> {
        match self {
            KType::Number => Some(KType::Number),
            KType::Str => Some(KType::Str),
            KType::Bool => Some(KType::Bool),
            KType::Null => Some(KType::Null),
            KType::List { element, .. } => Some(KType::list(Box::new(element.to_static()?))),
            KType::Dict { key, value, .. } => Some(KType::dict(
                Box::new(key.to_static()?),
                Box::new(value.to_static()?),
            )),
            KType::Record { fields, .. } => {
                Some(KType::record(Box::new(record_to_static(fields)?)))
            }
            KType::KFunction { params, ret, .. } => Some(KType::function_type(
                record_to_static(params)?,
                Box::new(ret.to_static()?),
            )),
            KType::DeferredReturn(s) => Some(KType::DeferredReturn(s.clone())),
            KType::Identifier => Some(KType::Identifier),
            KType::KExpression => Some(KType::KExpression),
            KType::SigiledTypeExpr => Some(KType::SigiledTypeExpr),
            KType::RecordType => Some(KType::RecordType),
            KType::OfKind(k) => Some(KType::OfKind(*k)),
            // `Rc`-shared set: rebuilding would mint a new `Rc` and break identity.
            KType::SetRef { .. } => None,
            // A union's identity is its owned member set; rebuild each member and rewrap. A
            // member holding a region pointer (e.g. a `SetRef`) declines, and the union with it.
            KType::Union { members, .. } => {
                let mut static_members = Vec::with_capacity(members.len());
                for m in members {
                    static_members.push(m.to_static()?);
                }
                Some(KType::union_of(static_members))
            }
            KType::SetLocal(i) => Some(KType::SetLocal(*i)),
            KType::RecursiveGroup(_) => None,
            // The `:Module` mint (SENTINEL sig-id, no pins) rebuilds `'static` as a fresh empty
            // signature. Every other signature declines: an `Rc<SigContent<'a>>` cannot cross to
            // `'static` without a rebuild (interning removes this walk's need to try).
            KType::Signature {
                content,
                pinned_slots,
                ..
            } if content.sig_id == ScopeId::SENTINEL && pinned_slots.is_empty() => {
                Some(KType::empty_signature())
            }
            KType::Signature { .. } => None,
            KType::AbstractType { source, name } => Some(KType::AbstractType {
                source: *source,
                name: name.clone(),
            }),
            KType::ConstructorApply { ctor, args, .. } => {
                let ctor = Box::new(ctor.to_static()?);
                let mut static_args = Vec::with_capacity(args.len());
                for a in args {
                    static_args.push(a.to_static()?);
                }
                Some(KType::constructor_apply(ctor, static_args))
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

    /// The evidence-widened twin of [`Self::resident_in`]: every region borrow reachable from
    /// `self` must point into `dest` **or** be covered by one of `sets` — the reaching tier's
    /// coverage predicate over a binding's already-extracted foreign reach. The `StoredReach`
    /// token that holds the reach is opaque to this layer; core extracts the sets before
    /// calling. `KType` itself holds no region pointer; the walk exists to audit the `Rc`-shared
    /// `RecursiveSet` members reachable through `SetRef` / `RecursiveGroup` (see [`Residence`]).
    pub(crate) fn resident_in_reach(&self, dest: &KoanRegion, sets: &[&FrameSet]) -> bool {
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
            KType::List { element, .. } => element.resident_in_visiting(residence, visited),
            KType::Dict { key, value, .. } => {
                key.resident_in_visiting(residence, visited)
                    && value.resident_in_visiting(residence, visited)
            }
            KType::Record { fields, .. } => record_resident_in(fields, residence, visited),
            KType::KFunction { params, ret, .. } => {
                record_resident_in(params, residence, visited)
                    && ret.resident_in_visiting(residence, visited)
            }
            KType::SetRef { set, .. } | KType::RecursiveGroup(set) => {
                set_resident_in(set, residence, visited)
            }
            KType::Union { members, .. } => members
                .iter()
                .all(|m| m.resident_in_visiting(residence, visited)),
            // `content` is owned data (no region pointer) — only the pins need auditing.
            KType::Signature { pinned_slots, .. } => pinned_slots
                .iter()
                .all(|(_, kt)| kt.resident_in_visiting(residence, visited)),
            // Owned data (a `ScopeId` plus a name) — no region pointer to audit.
            KType::AbstractType { .. } => true,
            KType::ConstructorApply { ctor, args, .. } => {
                ctor.resident_in_visiting(residence, visited)
                    && args
                        .iter()
                        .all(|a| a.resident_in_visiting(residence, visited))
            }
        }
    }

    /// Classify a *type* into its shallow dispatch [`KKind`] — the value-side direction of
    /// `OfKind`. A signature is `Signature`, a user-declared nominal is its family (`Tagged` /
    /// `NewType` / `TypeConstructor`, read off the set member it references), and every other
    /// type is `Proper`. Never returns `KKind::AnyType` (a slot-only expectation). Applied to
    /// the type a type value carries — or a runtime value's `ktype()` — to match it against an
    /// `OfKind` slot.
    pub fn kind_of(&self) -> KKind {
        match self {
            KType::Signature { .. } => KKind::Signature,
            // A nominal carries its family on the set member; a `ConstructorApply` defers to
            // its `ctor` (a `TypeConstructor`-kind `SetRef`).
            KType::SetRef { set, index } => set.member(*index).kind,
            KType::ConstructorApply { ctor, .. } => ctor.kind_of(),
            // A union is a proper type value — it classifies against `OfKind(Proper)` slots
            // and never against a nominal-family kind.
            KType::Union { .. } => KKind::ProperType,
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
/// [`KType::RecursiveGroup`] — the checked path those variants take, as
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
            Some(NominalSchema::NewType(kt)) => kt.resident_in_visiting(residence, visited),
            Some(NominalSchema::TypeConstructor { schema, .. }) => schema
                .values()
                .all(|kt| kt.resident_in_visiting(residence, visited)),
        })
}

/// Render an FN parameter record as the comma-free `name :type` group the
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

/// Manual `PartialEq`. The eight composite variants compare by their stored content
/// [`TypeDigest`] — one `u128` compare, no structural descent and no fallback: the
/// digest is the truth. A member reference (`SetRef` / `RecursiveGroup`) goes through
/// [`same_nominal`] (set-pointer fast path, else content digest + index). `AbstractType`, the
/// remaining id-keyed variant, compares by its source [`ScopeId`] plus its name, so two
/// `AbstractType` values minted from the same source-and-name compare equal.
impl<'a, 'b> PartialEq<KType<'b>> for KType<'a> {
    fn eq(&self, other: &KType<'b>) -> bool {
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
            // The seven composite variants store their content digest — one `u128` compare is
            // the whole test. The digest is the truth: no structural fallback exists.
            (List { digest: a, .. }, List { digest: b, .. })
            | (Dict { digest: a, .. }, Dict { digest: b, .. })
            | (Record { digest: a, .. }, Record { digest: b, .. })
            | (KFunction { digest: a, .. }, KFunction { digest: b, .. })
            | (Union { digest: a, .. }, Union { digest: b, .. })
            | (Signature { digest: a, .. }, Signature { digest: b, .. })
            | (ConstructorApply { digest: a, .. }, ConstructorApply { digest: b, .. }) => a == b,
            // A member reference: the set-pointer fast path, else the content digest plus
            // index — see [`same_nominal`]. Structurally identical declarations unify.
            (SetRef { set: s1, index: i1 }, SetRef { set: s2, index: i2 }) => {
                same_nominal(s1, *i1, s2, *i2)
            }
            // Whole-set handle: same-set identity, index-free.
            (RecursiveGroup(a), RecursiveGroup(b)) => same_nominal(a, 0, b, 0),
            (SetLocal(a), SetLocal(b)) => a == b,
            (
                AbstractType {
                    source: s1,
                    name: n1,
                },
                AbstractType {
                    source: s2,
                    name: n2,
                },
            ) => s1 == s2 && n1 == n2,
            (RecursiveRef(n1), RecursiveRef(n2)) => n1 == n2,
            (Unresolved(a), Unresolved(b)) => a == b,
            (DeferredReturn(a), DeferredReturn(b)) => a == b,
            _ => false,
        }
    }
}
impl<'a> Eq for KType<'a> {}

/// Manual `Hash`, kept consistent with the hand-written `PartialEq` above
/// (`a == b` ⟹ `hash(a) == hash(b)`). The eight composite variants hash their stored content
/// digest (one `u128`); `AbstractType` hashes its stable source [`ScopeId`] plus its
/// name. A member reference hashes its set's sealed digest (+ index), matching
/// [`same_nominal`]'s digest path — falling back to `Rc::as_ptr` only in the pre-seal window,
/// where the pointer path is also what settles equality. A set's hash therefore changes at
/// seal, which is sound because no `KType`-keyed map exists in the crate.
impl<'a> std::hash::Hash for KType<'a> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        use KType::*;
        std::mem::discriminant(self).hash(state);
        match self {
            Number | Str | Bool | Null | Identifier | KExpression | SigiledTypeExpr
            | RecordType | Any => {}
            OfKind(k) => k.hash(state),
            List { digest, .. }
            | Dict { digest, .. }
            | Record { digest, .. }
            | KFunction { digest, .. }
            | Union { digest, .. }
            | Signature { digest, .. }
            | ConstructorApply { digest, .. } => digest.hash(state),
            SetRef { set, index } => {
                hash_set_identity(set, state);
                index.hash(state);
            }
            RecursiveGroup(set) => hash_set_identity(set, state),
            SetLocal(i) => i.hash(state),
            AbstractType { source, name } => {
                source.hash(state);
                name.hash(state);
            }
            RecursiveRef(n) => n.hash(state),
            Unresolved(t) => t.hash(state),
            DeferredReturn(s) => s.hash(state),
        }
    }
}

/// Hash a set's identity for [`KType`]'s `Hash`: its sealed content digest, or — in the
/// pre-seal window only — its pointer, matching [`same_nominal`]'s two paths.
fn hash_set_identity<H: std::hash::Hasher>(set: &Rc<RecursiveSet<'_>>, state: &mut H) {
    match set.digest() {
        Some(d) => state.write_u128(d.0),
        None => state.write_usize(Rc::as_ptr(set) as *const () as usize),
    }
}

/// Manual `Debug` — a derived impl would recurse unboundedly through a self-referential
/// `RecursiveSet` (`SetRef` / `RecursiveGroup`); rendering through [`Self::name`] is the stable,
/// cycle-safe representation.
impl<'a> std::fmt::Debug for KType<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KType({})", self.name())
    }
}

#[cfg(test)]
mod tests;
