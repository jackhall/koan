//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at `from_name` time. There's no bare `KFunction` — "any function" with
//! no signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates live in `ktype_predicates.rs`; elaboration lives in `ktype_resolution.rs`.
//!
//! Lifetime parameter `'a`: the module/signature variants (`Module`, `Signature`,
//! `AbstractType`, `AnyModule`, `AnySignature`) hold `&'a Module<'a>` / `&'a Signature<'a>`
//! arena pointers; every other variant is owned data and ignores the parameter.

use super::record::Record;
use super::signature::DeferredReturnSurface;
use crate::machine::core::{CallArena, ScopeId};
use crate::machine::model::values::{Module, Signature};
use std::collections::HashMap;
use std::rc::Rc;

/// Surface-keyword classifier shared by `KType::UserType` and `KType::AnyUserType`.
/// See [per-declaration type identity](../../../../design/typing/user-types.md).
///
/// Each arm carries the declared type's schema as its payload (fields for `Struct`,
/// tag→type for `Tagged`/`TypeConstructor`, transparent repr for `Newtype`).
/// Construction reads that payload straight from the identity stored in
/// `bindings.types`, so there is no separate value-side schema carrier.
///
/// The manual `PartialEq` / `Eq` impl below *ignores* the inner payload — identity
/// equality is by variant tag only. Load-bearing two ways: wildcard admissibility
/// (`AnyUserType { kind: Newtype { repr: <sentinel> } }` admits any concrete
/// `UserType { kind: Newtype { repr: <real> }, .. }`, same for the others), and the
/// SCC cycle-close upsert — a payload-empty pre-installed identity compares equal to
/// the schema-bearing final identity, so finalize can overwrite the payload in place.
#[derive(Clone, Debug)]
pub enum UserTypeKind<'a> {
    /// Record schema in declaration order. The empty-`Record` sentinel is the payload
    /// an instance's `.ktype()` synthesizes and the cycle-close pre-install holds;
    /// construction reads the real record from the `bindings.types` identity.
    Struct { fields: Rc<Record<KType<'a>>> },
    /// Tagged-union schema keyed by tag. Same empty-`Rc` sentinel convention as `Struct`.
    Tagged {
        schema: Rc<HashMap<String, KType<'a>>>,
    },
    /// Fresh nominal identity over a transparent representation. `repr` is NOT part of
    /// identity equality.
    Newtype { repr: Box<KType<'a>> },
    /// Higher-kinded type-constructor slot. `schema` carries the erased-parameter
    /// variant schema (e.g. `Result`'s `{ok: Any, error: Any}`); neither `schema` nor
    /// `param_names` is part of identity equality.
    TypeConstructor {
        schema: Rc<HashMap<String, KType<'a>>>,
        param_names: Vec<String>,
    },
}

impl<'a> PartialEq for UserTypeKind<'a> {
    fn eq(&self, other: &Self) -> bool {
        use UserTypeKind::*;
        matches!(
            (self, other),
            (Struct { .. }, Struct { .. })
                | (Tagged { .. }, Tagged { .. })
                | (Newtype { .. }, Newtype { .. })
                | (TypeConstructor { .. }, TypeConstructor { .. }),
        )
    }
}
impl<'a> Eq for UserTypeKind<'a> {}

/// Mirror the tag-only `PartialEq`: identity is the variant tag, the payload is
/// ignored. Two `Struct`s with different `fields` compare equal, so they must hash
/// equal — hashing the discriminant alone keeps `Hash` consistent with `Eq`.
impl<'a> std::hash::Hash for UserTypeKind<'a> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
    }
}

impl<'a> UserTypeKind<'a> {
    /// Surface keyword rendered in diagnostics and `AnyUserType::name()`.
    pub fn surface_keyword(&self) -> &'static str {
        match self {
            UserTypeKind::Struct { .. } => "Struct",
            UserTypeKind::Tagged { .. } => "Tagged",
            UserTypeKind::Newtype { .. } => "Newtype",
            UserTypeKind::TypeConstructor { .. } => "TypeConstructor",
        }
    }

    /// Payload-empty `Struct` sentinel — the shape an instance's `.ktype()` reports and
    /// the cycle-close pre-install holds. Equality ignores the payload, so this stands in
    /// for any `Struct { fields }` in dispatch.
    pub fn struct_sentinel() -> Self {
        UserTypeKind::Struct {
            fields: Rc::new(Record::new()),
        }
    }

    /// Payload-empty `Tagged` sentinel — companion to [`Self::struct_sentinel`].
    pub fn tagged_sentinel() -> Self {
        UserTypeKind::Tagged {
            schema: Rc::new(HashMap::new()),
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
    /// schema with width/depth subtyping, distinct from the nominal
    /// `UserType { kind: Struct }`. The inner `Record<KType>` is declaration-ordered for
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
    KFunctor {
        params: Record<KType<'a>>,
        ret: Box<KType<'a>>,
    },
    /// Confined carrier for a synthesized FN/FUNCTOR `ret` slot whose source return is a
    /// `ReturnType::Deferred` — a per-call-elaborated return like `-> Er` or
    /// `-> (MODULE_TYPE_OF Er Type)`. Holds only the hashable surface shadow
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
    /// Meta-type for slots capturing a parsed type-name token. Carries the full structured
    /// `TypeName` rather than flattening to a name string.
    TypeExprRef,
    /// Meta-type for first-class type-values; both tagged-union and struct schemas report this.
    Type,
    /// Per-declaration identity tag for a user-declared type. The `(scope_id, name)` pair
    /// is the dispatch identity; `kind` carries the surface keyword so the wildcard
    /// `AnyUserType { kind }` can admit only the matching family.
    UserType {
        kind: UserTypeKind<'a>,
        scope_id: ScopeId,
        name: String,
    },
    /// Wildcard tag matching any user-declared carrier of the given `kind`. Strictly more
    /// specific than `Any`; specificity to a concrete `UserType { kind: K, .. }` is
    /// one-direction only — `UserType` is more specific than `AnyUserType` of the same
    /// kind, not the reverse.
    AnyUserType {
        kind: UserTypeKind<'a>,
    },
    /// A module signature: both the introspectable value (`decl_scope` via `sig`) and the
    /// dispatch constraint ("any module satisfying `sig`"). Disambiguated by position — as a
    /// parameter slot it matches a module whose `compatible_sigs` contains `sig.sig_id()`; as
    /// a value (`KTypeValue(Signature { .. })`) it is matched by the `AnySignature` wildcard.
    ///
    /// `pinned_slots` carries `SIG_WITH` abstract-type specializations (empty for a bare
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
    /// Abstract type member of a module, minted by opaque ascription (`Foo.Type`). Manual
    /// `PartialEq` compares `(source_module.scope_id(), name)` so two opaque-ascriptions of
    /// the same source module with the same abstract name compare equal.
    AbstractType {
        source_module: &'a Module<'a>,
        name: String,
    },
    /// `:Module` slot wildcard — admits first-class modules.
    AnyModule,
    /// `:Signature` slot wildcard — admits first-class signature values.
    AnySignature,
    /// Recursive type binder. `body` describes the unfolded shape with `binder` in scope as a
    /// `RecursiveRef` for self-references. `name()` renders as the binder name so diagnostics
    /// stay readable (e.g. `Tree` rather than `Mu Tree. List<Tree>`).
    Mu {
        binder: String,
        body: Box<KType<'a>>,
    },
    /// Application of a higher-kinded type constructor to arg types. `ctor` is a
    /// `UserType` with `TypeConstructor` kind; `args` are the elaborated arg types.
    /// Structural equality by `(ctor, args)`.
    ConstructorApply {
        ctor: Box<KType<'a>>,
        args: Vec<KType<'a>>,
    },
    /// Back-reference to an enclosing `Mu`'s binder. Equality is by binder name only.
    RecursiveRef(String),
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
            KType::KFunctor { params, ret } => {
                format!(
                    ":(FUNCTOR ({}) -> {})",
                    render_param_record(params),
                    ret.name()
                )
            }
            KType::DeferredReturn(s) => s.render(),
            KType::Identifier => "Identifier".into(),
            KType::KExpression => "KExpression".into(),
            KType::TypeExprRef => "TypeExprRef".into(),
            KType::Type => "Type".into(),
            KType::UserType { name, .. } => name.clone(),
            KType::AnyUserType { kind } => kind.surface_keyword().into(),
            KType::Signature { sig, pinned_slots } => {
                if pinned_slots.is_empty() {
                    sig.path.clone()
                } else {
                    // Display-only; does not round-trip through the parser.
                    let inner: Vec<String> = pinned_slots
                        .iter()
                        .map(|(name, kt)| format!("({}: {})", name, kt.name()))
                        .collect();
                    format!("(SIG_WITH {} ({}))", sig.path, inner.join(" "))
                }
            }
            KType::Module { module, .. } => module.path.clone(),
            KType::AbstractType { name, .. } => name.clone(),
            KType::AnyModule => "Module".into(),
            KType::AnySignature => "Signature".into(),
            KType::Mu { binder, .. } => binder.clone(),
            KType::RecursiveRef(name) => name.clone(),
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
            | (TypeExprRef, TypeExprRef)
            | (Type, Type)
            | (Any, Any)
            | (AnyModule, AnyModule)
            | (AnySignature, AnySignature) => true,
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
            (
                KFunctor {
                    params: p1,
                    ret: r1,
                },
                KFunctor {
                    params: p2,
                    ret: r2,
                },
            ) => p1 == p2 && r1 == r2,
            (
                UserType {
                    kind: k1,
                    scope_id: s1,
                    name: n1,
                },
                UserType {
                    kind: k2,
                    scope_id: s2,
                    name: n2,
                },
            ) => k1 == k2 && s1 == s2 && n1 == n2,
            (AnyUserType { kind: k1 }, AnyUserType { kind: k2 }) => k1 == k2,
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
                    source_module: m1,
                    name: n1,
                },
                AbstractType {
                    source_module: m2,
                    name: n2,
                },
            ) => m1.scope_id() == m2.scope_id() && n1 == n2,
            (
                Mu {
                    binder: b1,
                    body: bd1,
                },
                Mu {
                    binder: b2,
                    body: bd2,
                },
            ) => b1 == b2 && bd1 == bd2,
            (ConstructorApply { ctor: c1, args: a1 }, ConstructorApply { ctor: c2, args: a2 }) => {
                c1 == c2 && a1 == a2
            }
            (RecursiveRef(n1), RecursiveRef(n2)) => n1 == n2,
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
/// The arena-pointer variants hash their stable identity key — `Module` and
/// `AbstractType` hash `scope_id()`, `Signature` hashes `sig_id()` — never the raw
/// pointer, matching how `PartialEq` resolves them. `Module`'s `frame` lifecycle
/// anchor and the payload-only `UserTypeKind` fields stay excluded (the latter via
/// `UserTypeKind`'s discriminant-only `Hash`). Recursion bottoms out at the leaf
/// `RecursiveRef`, so `Mu` / `ConstructorApply` hashing is bounded.
impl<'a> std::hash::Hash for KType<'a> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        use KType::*;
        std::mem::discriminant(self).hash(state);
        match self {
            Number | Str | Bool | Null | Identifier | KExpression | TypeExprRef | Type | Any
            | AnyModule | AnySignature => {}
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
            KFunctor { params, ret } => {
                params.hash(state);
                ret.hash(state);
            }
            UserType {
                kind,
                scope_id,
                name,
            } => {
                kind.hash(state);
                scope_id.hash(state);
                name.hash(state);
            }
            AnyUserType { kind } => kind.hash(state),
            Signature { sig, pinned_slots } => {
                sig.sig_id().hash(state);
                pinned_slots.hash(state);
            }
            Module { module, .. } => module.scope_id().hash(state),
            AbstractType {
                source_module,
                name,
            } => {
                source_module.scope_id().hash(state);
                name.hash(state);
            }
            Mu { binder, body } => {
                binder.hash(state);
                body.hash(state);
            }
            ConstructorApply { ctor, args } => {
                ctor.hash(state);
                args.hash(state);
            }
            RecursiveRef(n) => n.hash(state),
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
    use super::*;

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
        };
        assert_eq!(t.name(), ":(FUNCTOR (x :Number y :Str) -> Bool)");
    }

    #[test]
    fn functor_structural_eq_same_shape() {
        let a = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
            ret: Box::new(KType::Bool),
        };
        let b = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
            ret: Box::new(KType::Bool),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn functor_structural_neq_when_params_or_ret_differ() {
        let base = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number)]),
            ret: Box::new(KType::Bool),
        };
        let diff_params = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Str)]),
            ret: Box::new(KType::Bool),
        };
        let diff_ret = KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number)]),
            ret: Box::new(KType::Null),
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
    fn name_renders_mu_as_binder() {
        let t = KType::Mu {
            binder: "Tree".into(),
            body: Box::new(KType::List(Box::new(KType::RecursiveRef("Tree".into())))),
        };
        assert_eq!(t.name(), "Tree");
    }

    #[test]
    fn name_renders_recursive_ref_as_name() {
        let t = KType::RecursiveRef("Tree".into());
        assert_eq!(t.name(), "Tree");
    }

    #[test]
    fn user_type_kind_surface_keywords() {
        assert_eq!(UserTypeKind::struct_sentinel().surface_keyword(), "Struct");
        assert_eq!(UserTypeKind::tagged_sentinel().surface_keyword(), "Tagged");
        assert_eq!(
            UserTypeKind::Newtype {
                repr: Box::new(KType::Number)
            }
            .surface_keyword(),
            "Newtype",
        );
        assert_eq!(
            UserTypeKind::TypeConstructor {
                schema: Rc::new(HashMap::new()),
                param_names: vec!["T".into()],
            }
            .surface_keyword(),
            "TypeConstructor",
        );
    }

    #[test]
    fn newtype_kind_partial_eq_ignores_repr() {
        let a: UserTypeKind<'_> = UserTypeKind::Newtype {
            repr: Box::new(KType::Number),
        };
        let b: UserTypeKind<'_> = UserTypeKind::Newtype {
            repr: Box::new(KType::Str),
        };
        assert_eq!(a, b);
        assert_ne!(a, UserTypeKind::struct_sentinel());
        assert_ne!(UserTypeKind::struct_sentinel(), a);
    }

    /// `Struct`/`Tagged` equality ignores the fields/schema payload, so a
    /// payload-empty sentinel compares equal to a schema-bearing identity — the
    /// property the SCC cycle-close upsert relies on.
    #[test]
    fn struct_and_tagged_partial_eq_ignore_payload() {
        let empty = UserTypeKind::struct_sentinel();
        let full = UserTypeKind::Struct {
            fields: Rc::new(Record::from_pairs(vec![("x".into(), KType::Number)])),
        };
        assert_eq!(empty, full);

        let empty_t = UserTypeKind::tagged_sentinel();
        let mut schema = HashMap::new();
        schema.insert("some".into(), KType::Number);
        let full_t = UserTypeKind::Tagged {
            schema: Rc::new(schema),
        };
        assert_eq!(empty_t, full_t);

        assert_ne!(empty, empty_t);
    }

    #[test]
    fn user_type_kind_type_constructor_partial_eq_ignores_param_names() {
        let a: UserTypeKind<'_> = UserTypeKind::TypeConstructor {
            schema: Rc::new(HashMap::new()),
            param_names: vec!["T".into()],
        };
        let b: UserTypeKind<'_> = UserTypeKind::TypeConstructor {
            schema: Rc::new(HashMap::new()),
            param_names: vec!["U".into()],
        };
        let empty: UserTypeKind<'_> = UserTypeKind::TypeConstructor {
            schema: Rc::new(HashMap::new()),
            param_names: Vec::new(),
        };
        assert_eq!(a, b);
        assert_eq!(a, empty);
        assert_ne!(a, UserTypeKind::struct_sentinel());
        assert_ne!(
            a,
            UserTypeKind::Newtype {
                repr: Box::new(KType::Number)
            }
        );
    }

    #[test]
    fn any_user_type_name_renders_kind_keyword() {
        assert_eq!(
            KType::AnyUserType {
                kind: UserTypeKind::struct_sentinel()
            }
            .name(),
            "Struct"
        );
        assert_eq!(
            KType::AnyUserType {
                kind: UserTypeKind::tagged_sentinel()
            }
            .name(),
            "Tagged"
        );
    }

    #[test]
    fn any_module_and_any_signature_render_surface_keywords() {
        let am: KType<'_> = KType::AnyModule;
        let asg: KType<'_> = KType::AnySignature;
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
            (KType::TypeExprRef, KType::TypeExprRef),
            (KType::Type, KType::Type),
            (KType::Any, KType::Any),
            (KType::AnyModule, KType::AnyModule),
            (KType::AnySignature, KType::AnySignature),
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
                },
                KType::KFunctor {
                    params: Record::from_pairs(vec![("x".into(), KType::Number)]),
                    ret: Box::new(KType::Bool),
                },
            ),
            (
                KType::UserType {
                    kind: UserTypeKind::struct_sentinel(),
                    scope_id: sid,
                    name: "Point".into(),
                },
                KType::UserType {
                    kind: UserTypeKind::struct_sentinel(),
                    scope_id: sid,
                    name: "Point".into(),
                },
            ),
            (
                KType::AnyUserType {
                    kind: UserTypeKind::tagged_sentinel(),
                },
                KType::AnyUserType {
                    kind: UserTypeKind::tagged_sentinel(),
                },
            ),
            (
                KType::Mu {
                    binder: "Tree".into(),
                    body: Box::new(KType::List(Box::new(KType::RecursiveRef("Tree".into())))),
                },
                KType::Mu {
                    binder: "Tree".into(),
                    body: Box::new(KType::List(Box::new(KType::RecursiveRef("Tree".into())))),
                },
            ),
            (
                KType::RecursiveRef("Tree".into()),
                KType::RecursiveRef("Tree".into()),
            ),
        ];
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

    /// `UserType` identity is `(kind-tag, scope_id, name)` and `Struct`'s `fields`
    /// payload is *not* part of it. Two `UserType`s with different field payloads but
    /// the same identity compare equal — so `Hash` must agree (the property the SCC
    /// cycle-close upsert and `.ktype()` sentinel synthesis depend on).
    #[test]
    fn hash_ignores_struct_payload_like_eq() {
        let sid = ScopeId::from_raw(0, 0x1234);
        let empty = KType::UserType {
            kind: UserTypeKind::struct_sentinel(),
            scope_id: sid,
            name: "Point".into(),
        };
        let full = KType::UserType {
            kind: UserTypeKind::Struct {
                fields: Rc::new(Record::from_pairs(vec![("x".into(), KType::Number)])),
            },
            scope_id: sid,
            name: "Point".into(),
        };
        assert_eq!(empty, full);
        assert_eq!(hash_of(&empty), hash_of(&full));
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
        };
        assert_ne!(f, g);
        assert_ne!(hash_of(&f), hash_of(&g));
    }

    #[test]
    fn user_type_name_renders_bare_name() {
        // Renders the declared `name`, not the kind keyword: a `Point` struct slot shows
        // `Point`, not `Struct`.
        let t = KType::UserType {
            kind: UserTypeKind::struct_sentinel(),
            scope_id: ScopeId::from_raw(0, 0x1234),
            name: "Point".into(),
        };
        assert_eq!(t.name(), "Point");
    }
}
