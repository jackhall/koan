//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at `from_name` time. There's no bare `KFunction` — "any function" with
//! no signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates live in `ktype_predicates.rs`; elaboration lives in `ktype_resolution.rs`.
//!
//! Lifetime parameter `'a`: the module/signature variants (`Module`, `Signature`,
//! `AbstractType`) hold `&'a Module<'a>` / `&'a Signature<'a>` arena pointers; every other
//! variant is owned data and ignores the parameter.

use super::kkind::KKind;
use super::record::Record;
use super::recursive_set::RecursiveSet;
use super::signature::DeferredReturnSurface;
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::{CallArena, ScopeId};
use crate::machine::model::ast::TypeName;
use crate::machine::model::values::{Module, Signature};
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
    /// Structural record type (`:{x :Number, y :Str}`) — an identifier-keyed field
    /// schema with width/depth subtyping. Anonymous: a record-repr `Newtype` `SetRef`
    /// (an ex-struct) wraps this with a nominal identity, but the bare record type is
    /// structural and order-blind.
    /// The inner `Record<KType>` is declaration-ordered for
    /// rendering and order-blind by `(name, type)` for identity. A record *value*
    /// (`KObject::Record`) memoizes this as its carried type. Subtyping is the dual of
    /// the function-parameter relation — width-*superset* is more specific, covariant
    /// depth — see `record_value_more_specific`.
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
    /// [ktype.md § Variance](../../../../design/typing/ktype.md#variance).
    DeferredReturn(DeferredReturnSurface),
    Identifier,
    /// Lazy slot: accepts an unevaluated `ExpressionPart::Expression` so the builtin chooses
    /// when (or whether) to run it.
    KExpression,
    /// Lazy slot for a `:(...)` type expression — the sibling of [`KType::KExpression`] for a
    /// `SigiledTypeExpr` part. Captures it raw (via `resolve_for`, as the inner
    /// `KObject::KExpression`) instead of eager-sub-dispatching, so a builtin can defer a
    /// param-referencing dotted/sigil return (`-> Er.Type`) to per-call elaboration. More
    /// specific than [`KType::OfKind(KKind::Proper)`], so it wins the overload when both admit.
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
        sig: &'a Signature<'a>,
        pinned_slots: Vec<(String, KType<'a>)>,
    },
    /// First-class module value's type. `frame` carries the per-call `Rc<CallArena>`
    /// anchor for functor-built modules.
    Module {
        module: &'a Module<'a>,
        frame: Option<Rc<CallArena>>,
    },
    /// Abstract type member named by a SIG slot or minted by opaque ascription. `source`
    /// distinguishes the two roots: `Sig(scope_id)` is the SIG-decl-time member (bound when a
    /// SIG-local `LET Type = ...` would otherwise collapse to the underlying type), `Module`
    /// is the per-call mint `:|` produces (`Foo.Type`). Identity keys on
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
    /// park-capable [`Scope::resolve_type_expr`](crate::machine::core::Scope::resolve_type_expr).
    /// Carries the structured `TypeName` so the surface form survives the bind.
    Unresolved(TypeName),
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

    /// Classify a *type* into its shallow dispatch [`KKind`] — the value-side direction of
    /// `OfKind`. A module is `Module`, a signature is `Signature`, a user-declared nominal is
    /// its family (`Tagged` / `Newtype` / `TypeConstructor`, read off the set member it
    /// references), and every other type is `Proper`. Never returns `KKind::Any` (a slot-only
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
            _ => KKind::Proper,
        }
    }
}

/// Render an FN/FUNCTOR parameter record as the comma-free `name <:type>` group the
/// `:(FN (...) -> _)` surface re-parses. Each field is `name` then the type surface:
/// `kt.name()` prefixed with `:` for a leaf (`:Number`), left as-is when it already opens
/// a sigil (`:(LIST OF Number)` — no `::`). Declaration order is preserved.
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

/// Manual `PartialEq` — `Module`, `Signature`, and `AbstractType` carry arena pointers
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
            // `frame` is a lifecycle anchor, not part of identity.
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

/// Manual `Hash`, kept consistent with the hand-written `PartialEq` above:
/// `a == b` ⟹ `hash(a) == hash(b)`. The discriminant goes in first so distinct
/// variants never alias and the unit variants need no further mixing; each
/// compound arm then hashes exactly the fields its `PartialEq` arm compares.
///
/// The arena-pointer variants hash their stable identity key — `Module` hashes
/// `scope_id()`, `AbstractType` hashes its `source.scope_id()`, `Signature` hashes
/// `sig_id()` — never the raw pointer, matching how `PartialEq` resolves them. `Module`'s
/// `frame` lifecycle anchor stays excluded. A `SetRef` hashes `(Rc::as_ptr(set), index)`
/// ONLY — never the schema, which is cyclic. Recursion bottoms out at the leaf
/// `RecursiveRef` / `SetLocal`, so `ConstructorApply` hashing is bounded.
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

/// Manual `Debug` — `derive` is blocked because `Module` / `Signature` / `CallArena`
/// don't implement `Debug` and recursing through module-typed KTypes is unbounded.
impl<'a> std::fmt::Debug for KType<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KType({})", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::super::recursive_set::{NominalMember, NominalSchema};
    use super::*;

    /// A singleton `Rc<RecursiveSet>` over a record-repr newtype member named `name`, schema
    /// filled.
    fn record_newtype_set<'a>(name: &str, scope_id: ScopeId) -> Rc<RecursiveSet<'a>> {
        let member = NominalMember::pending(name.into(), scope_id, KKind::Newtype);
        member.fill(NominalSchema::Newtype(Box::new(KType::Record(Box::new(
            Record::new(),
        )))));
        Rc::new(RecursiveSet::new(vec![member]))
    }

    #[test]
    fn name_renders_parameterized_list() {
        let t = KType::List(Box::new(KType::List(Box::new(KType::Number))));
        assert_eq!(t.name(), ":(LIST OF :(LIST OF Number))");
    }

    #[test]
    fn name_renders_dict() {
        let t = KType::Dict(Box::new(KType::Str), Box::new(KType::Number));
        assert_eq!(t.name(), ":(MAP Str -> Number)");
    }

    #[test]
    fn name_renders_function() {
        let t = KType::KFunction {
            params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
            ret: Box::new(KType::Bool),
        };
        assert_eq!(t.name(), ":(FN (x :Number y :Str) -> Bool)");
    }

    /// A nested sigiled parameter type already opens with `:`, so the renderer must not
    /// prefix a second colon (`xs :(LIST OF Number)`, not `xs ::(LIST OF Number)`).
    #[test]
    fn name_renders_function_with_sigiled_param() {
        let t = KType::KFunction {
            params: Record::from_pairs(vec![("xs".into(), KType::List(Box::new(KType::Number)))]),
            ret: Box::new(KType::Bool),
        };
        assert_eq!(t.name(), ":(FN (xs :(LIST OF Number)) -> Bool)");
    }

    #[test]
    fn name_renders_functor() {
        let t = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
            ret: Box::new(KType::Bool),
            body: None,
        };
        assert_eq!(t.name(), ":(FUNCTOR (x :Number y :Str) -> Bool)");
    }

    #[test]
    fn functor_structural_eq_same_shape() {
        let a = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
            ret: Box::new(KType::Bool),
            body: None,
        };
        let b = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
            ret: Box::new(KType::Bool),
            body: None,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn functor_structural_neq_when_params_or_ret_differ() {
        let base = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number)]),
            ret: Box::new(KType::Bool),
            body: None,
        };
        let diff_params = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Str)]),
            ret: Box::new(KType::Bool),
            body: None,
        };
        let diff_ret = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number)]),
            ret: Box::new(KType::Null),
            body: None,
        };
        assert_ne!(base, diff_params);
        assert_ne!(base, diff_ret);
    }

    #[test]
    fn functor_and_function_are_disjoint_types() {
        let f = KType::KFunction {
            params: Record::from_pairs(vec![("x".into(), KType::Number)]),
            ret: Box::new(KType::Bool),
        };
        let g = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number)]),
            ret: Box::new(KType::Bool),
            body: None,
        };
        assert_ne!(f, g);
    }

    #[test]
    fn name_renders_function_nullary() {
        let t = KType::KFunction {
            params: Record::new(),
            ret: Box::new(KType::Any),
        };
        assert_eq!(t.name(), ":(FN () -> Any)");
    }

    /// Function-slot identity is the record substrate's order-blind equality: the same
    /// parameters by `(name, type)` in a different declaration order compare equal and
    /// hash equal.
    #[test]
    fn function_params_order_blind_equality() {
        let xy = KType::KFunction {
            params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
            ret: Box::new(KType::Bool),
        };
        let yx = KType::KFunction {
            params: Record::from_pairs(vec![("y".into(), KType::Str), ("x".into(), KType::Number)]),
            ret: Box::new(KType::Bool),
        };
        assert_eq!(xy, yx);
        assert_eq!(hash_of(&xy), hash_of(&yx));
    }

    /// Identity is name-sensitive: same type, different parameter name is a different
    /// function type.
    #[test]
    fn function_params_name_sensitive_inequality() {
        let x = KType::KFunction {
            params: Record::from_pairs(vec![("x".into(), KType::Number)]),
            ret: Box::new(KType::Bool),
        };
        let a = KType::KFunction {
            params: Record::from_pairs(vec![("a".into(), KType::Number)]),
            ret: Box::new(KType::Bool),
        };
        assert_ne!(x, a);
    }

    #[test]
    fn name_renders_recursive_ref_as_name() {
        let t = KType::RecursiveRef("Tree".into());
        assert_eq!(t.name(), "Tree");
    }

    #[test]
    fn nominal_kind_surface_keywords() {
        assert_eq!(KKind::Tagged.surface_keyword(), "Tagged");
        assert_eq!(KKind::Newtype.surface_keyword(), "Newtype");
        assert_eq!(KKind::TypeConstructor.surface_keyword(), "TypeConstructor",);
    }

    #[test]
    fn nominal_of_kind_name_renders_family_keyword() {
        assert_eq!(KType::OfKind(KKind::Newtype).name(), "Newtype");
        assert_eq!(KType::OfKind(KKind::Tagged).name(), "Tagged");
        assert_eq!(
            KType::OfKind(KKind::TypeConstructor).name(),
            "TypeConstructor"
        );
    }

    #[test]
    fn any_module_and_any_signature_render_surface_keywords() {
        let am: KType<'_> = KType::OfKind(KKind::Module);
        let asg: KType<'_> = KType::OfKind(KKind::Signature);
        assert_eq!(am.name(), "Module");
        assert_eq!(asg.name(), "Signature");
    }

    fn hash_of(t: &KType<'_>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    }

    /// `a == b` ⟹ `hash(a) == hash(b)` across every arena-free variant. Each pair is
    /// built independently so a stray identity-from-pointer bug would surface.
    #[test]
    fn hash_agrees_with_eq_for_arena_free_variants() {
        let sid = ScopeId::from_raw(0, 0xBEEF);
        let pairs: Vec<(KType<'_>, KType<'_>)> = vec![
            (KType::Number, KType::Number),
            (KType::Str, KType::Str),
            (KType::Bool, KType::Bool),
            (KType::Null, KType::Null),
            (KType::Identifier, KType::Identifier),
            (KType::KExpression, KType::KExpression),
            (KType::OfKind(KKind::Proper), KType::OfKind(KKind::Proper)),
            (KType::OfKind(KKind::Any), KType::OfKind(KKind::Any)),
            (KType::Any, KType::Any),
            (KType::OfKind(KKind::Module), KType::OfKind(KKind::Module)),
            (
                KType::OfKind(KKind::Signature),
                KType::OfKind(KKind::Signature),
            ),
            (
                KType::List(Box::new(KType::Number)),
                KType::List(Box::new(KType::Number)),
            ),
            (
                KType::Dict(Box::new(KType::Str), Box::new(KType::Number)),
                KType::Dict(Box::new(KType::Str), Box::new(KType::Number)),
            ),
            (
                KType::KFunction {
                    params: Record::from_pairs(vec![("x".into(), KType::Number)]),
                    ret: Box::new(KType::Bool),
                },
                KType::KFunction {
                    params: Record::from_pairs(vec![("x".into(), KType::Number)]),
                    ret: Box::new(KType::Bool),
                },
            ),
            (
                KType::KFunctor {
                    params: Record::from_pairs(vec![("x".into(), KType::Number)]),
                    ret: Box::new(KType::Bool),
                    body: None,
                },
                KType::KFunctor {
                    params: Record::from_pairs(vec![("x".into(), KType::Number)]),
                    ret: Box::new(KType::Bool),
                    body: None,
                },
            ),
            (KType::OfKind(KKind::Tagged), KType::OfKind(KKind::Tagged)),
            (
                KType::RecursiveRef("Tree".into()),
                KType::RecursiveRef("Tree".into()),
            ),
            (KType::SetLocal(2), KType::SetLocal(2)),
        ];
        // A `SetRef` pair sharing one `Rc` — identity is `(set ptr, index)`, so the same
        // allocation must hash and compare equal.
        let shared = record_newtype_set("Point", sid);
        let set_ref_a = KType::SetRef {
            set: Rc::clone(&shared),
            index: 0,
        };
        let set_ref_b = KType::SetRef {
            set: Rc::clone(&shared),
            index: 0,
        };
        let pairs: Vec<(KType<'_>, KType<'_>)> = pairs
            .into_iter()
            .chain(std::iter::once((set_ref_a, set_ref_b)))
            .collect();
        for (a, b) in &pairs {
            assert_eq!(a, b, "values must be equal: {:?}", a);
            assert_eq!(
                hash_of(a),
                hash_of(b),
                "equal values must hash equal: {:?}",
                a
            );
        }
    }

    /// `SetRef` identity is `(set ptr, index)` and never descends the (cyclic) schema. Two
    /// `SetRef`s over the same `Rc` allocation and index compare equal — so `Hash` must
    /// agree. Two over *distinct* allocations of the same name compare unequal.
    #[test]
    fn hash_keys_set_ref_on_pointer_and_index() {
        let sid = ScopeId::from_raw(0, 0x1234);
        let set = record_newtype_set("Point", sid);
        let a = KType::SetRef {
            set: Rc::clone(&set),
            index: 0,
        };
        let b = KType::SetRef {
            set: Rc::clone(&set),
            index: 0,
        };
        assert_eq!(a, b);
        assert_eq!(hash_of(&a), hash_of(&b));

        // A separate allocation with the same name is a distinct identity.
        let other = record_newtype_set("Point", sid);
        let c = KType::SetRef {
            set: other,
            index: 0,
        };
        assert_ne!(a, c);
    }

    /// Distinct variants must not collide structurally — the leading discriminant
    /// keeps e.g. `KFunction` and `KFunctor` of the same shape apart in both `Eq`
    /// and `Hash`.
    #[test]
    fn hash_distinguishes_function_from_functor() {
        let f = KType::KFunction {
            params: Record::from_pairs(vec![("x".into(), KType::Number)]),
            ret: Box::new(KType::Bool),
        };
        let g = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number)]),
            ret: Box::new(KType::Bool),
            body: None,
        };
        assert_ne!(f, g);
        assert_ne!(hash_of(&f), hash_of(&g));
    }

    #[test]
    fn set_ref_name_renders_member_name() {
        // Renders the member's declared `name`, not the kind keyword: a `Point` struct
        // slot shows `Point`, not `Struct`.
        let set = record_newtype_set("Point", ScopeId::from_raw(0, 0x1234));
        let t = KType::SetRef { set, index: 0 };
        assert_eq!(t.name(), "Point");
    }
}
