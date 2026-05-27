//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at `from_name` time. There's no bare `KFunction` — "any function" with
//! no signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates (`is_more_specific_than`, `matches_value`, `accepts_part`, `function_compat`)
//! live in `ktype_predicates.rs`; elaboration (`from_name`, `from_type_expr`, `join`,
//! `join_iter`) lives in `ktype_resolution.rs`.
//!
//! Lifetime parameter `'a`: KType carries arena-pinned references for the type-language
//! entities that own a child scope (`Module`, `Signature`). The five "module/signature"
//! variants (`Module`, `Signature`, `AbstractType`, `AnyModule`, `AnySignature`) hold
//! `&'a Module<'a>` / `&'a Signature<'a>` pointers; every other variant is owned data and
//! ignores the parameter. The annotation mirrors `KObject<'a>`, `KFunction<'a>`, and
//! `Module<'a>` — types and values now share one lifetime.

use crate::machine::core::{CallArena, ScopeId};
use crate::machine::model::values::{Module, Signature};
use std::rc::Rc;

/// Surface-keyword classifier shared by `KType::UserType` and `KType::AnyUserType`.
/// Maps each variant to its declaring keyword (`STRUCT`, `UNION` → `Tagged`, `MODULE`,
/// `NEWTYPE`, `TYPE_CONSTRUCTOR`). See
/// [per-declaration type identity](../../../../design/typing/user-types.md).
///
/// The manual `PartialEq` / `Eq` impl below *ignores* the inner payload — identity
/// equality is by variant tag only. Load-bearing for wildcard admissibility:
/// `AnyUserType { kind: Newtype { repr: <sentinel> } }` must admit any concrete
/// `UserType { kind: Newtype { repr: <real> }, .. }`, and the same applies to the
/// `TypeConstructor` variant.
#[derive(Clone, Debug)]
pub enum UserTypeKind<'a> {
    Struct,
    Tagged,
    /// Fresh nominal identity over a transparent representation (`NEWTYPE Distance = Number`
    /// carries `repr: Box<KType::Number>`). `repr` is NOT part of identity equality — the
    /// manual `PartialEq` excludes it.
    Newtype { repr: Box<KType<'a>> },
    /// Higher-kinded type-constructor slot declared via `LET Wrap = (TYPE_CONSTRUCTOR T)`
    /// inside a SIG body. `param_names` is NOT part of identity equality — the manual
    /// `PartialEq` excludes it.
    TypeConstructor { param_names: Vec<String> },
}

impl<'a> PartialEq for UserTypeKind<'a> {
    fn eq(&self, other: &Self) -> bool {
        use UserTypeKind::*;
        matches!(
            (self, other),
            (Struct, Struct)
                | (Tagged, Tagged)
                | (Newtype { .. }, Newtype { .. })
                | (TypeConstructor { .. }, TypeConstructor { .. }),
        )
    }
}
impl<'a> Eq for UserTypeKind<'a> {}

impl<'a> UserTypeKind<'a> {
    /// Surface keyword rendered in diagnostics and `AnyUserType::name()`.
    pub fn surface_keyword(&self) -> &'static str {
        match self {
            UserTypeKind::Struct => "Struct",
            UserTypeKind::Tagged => "Tagged",
            UserTypeKind::Newtype { .. } => "Newtype",
            UserTypeKind::TypeConstructor { .. } => "TypeConstructor",
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
    KFunction {
        args: Vec<KType<'a>>,
        ret: Box<KType<'a>>,
    },
    /// Structural functor type — mirrors `KFunction` storage and rendering, but
    /// carries no admissibility against `KFunction` (the cross-arms in
    /// `function_compat` refuse both directions). Minted by the `FUNCTOR` binder
    /// when `is_functor: true` flips on the underlying `KFunctionValue`, and by
    /// the `Functor` arm of `elaborate_type_expr` for the `:(Functor (params) -> R)`
    /// surface-form sigil. `params` mirror `KFunction::args` (positional, same
    /// shape); `ret` mirrors `KFunction::ret`.
    KFunctor {
        params: Vec<KType<'a>>,
        ret: Box<KType<'a>>,
    },
    Identifier,
    /// Lazy slot: accepts an unevaluated `ExpressionPart::Expression` so the builtin chooses
    /// when (or whether) to run it.
    KExpression,
    /// Meta-type for slots capturing a parsed type-name token (`ExpressionPart::Type`).
    /// Resolves to `KObject::TypeExprValue(t)` — the full structured `TypeExpr`, preserving
    /// nested parameters rather than flattening to a name string.
    TypeExprRef,
    /// Meta-type for first-class type-values; both tagged-union and struct schemas report this.
    Type,
    /// Per-declaration identity tag for a user-declared type. The `(scope_id, name)` pair
    /// is the dispatch identity; `kind` carries the surface keyword so the wildcard
    /// `AnyUserType { kind }` can admit only the matching family.
    UserType { kind: UserTypeKind<'a>, scope_id: ScopeId, name: String },
    /// Wildcard tag matching any user-declared carrier of the given `kind`. Strictly more
    /// specific than `Any`; specificity to a concrete `UserType { kind: K, .. }` is
    /// one-direction only — `UserType` is more specific than `AnyUserType` of the same
    /// kind, not the reverse.
    AnyUserType { kind: UserTypeKind<'a> },
    /// First-class module value tagged with the signature it satisfies. `sig_id` is the
    /// declaring `Signature`'s `decl_scope_ptr as usize` — addresses are stable for the
    /// run (the arena pins the `Signature`) and two `SIG Foo = (...)` declarations in
    /// the same scope already error (`Rebind`), so the cast is collision-free. Equality
    /// is by `sig_id` plus the `pinned_slots` constraint vector; `sig_path` is for
    /// diagnostics only.
    ///
    /// `pinned_slots` carries sharing constraints — abstract-type slots of the signature
    /// pinned to specific concrete `KType`s. Empty for the unconstrained `OrderedSig`
    /// form. The vec is order-preserving (rather than a `HashMap`) so structural equality
    /// is deterministic and the diagnostic surface stays stable.
    SatisfiesSignature {
        sig_id: ScopeId,
        sig_path: String,
        pinned_slots: Vec<(String, KType<'a>)>,
    },
    /// First-class module value's type. Replaces `KObject::KModule` for type-language
    /// purposes — the module carrier moves into KType so dispatch and ATTR project from
    /// the same shape. `frame` carries the per-call `Rc<CallArena>` anchor for
    /// functor-built modules (mirrors the old `KModule` payload verbatim).
    Module {
        module: &'a Module<'a>,
        frame: Option<Rc<CallArena>>,
    },
    /// First-class module signature value's type. Replaces `KObject::KSignature` for
    /// type-language purposes; the arena-pinned `&'a Signature<'a>` pointer parallels
    /// `KType::Module`'s carrier shape.
    Signature(&'a Signature<'a>),
    /// Abstract type member of a module, minted by opaque ascription (`Foo.Type`). The
    /// arena-pinned `source_module` pointer mirrors `KType::Module`'s carrier; manual
    /// `PartialEq` compares `(source_module.scope_id(), name)` rather than pointer
    /// equality so two opaque-ascriptions of the same source module with the same
    /// abstract name compare equal. Replaces the former `UserType { kind:
    /// UserTypeKind::Module, .. }` shape for abstract-type members.
    AbstractType {
        source_module: &'a Module<'a>,
        name: String,
    },
    /// `:Module` slot wildcard. Replaces `AnyUserType { kind: Module }` — `:Module`
    /// admits first-class modules, not "abstract-types-from-some-module."
    AnyModule,
    /// `:Signature` slot wildcard. Replaces `MetaSignature` — `:Signature` admits
    /// first-class signature values.
    AnySignature,
    /// Recursive type binder. `body` describes the unfolded shape with `binder` in scope as a
    /// `RecursiveRef` for self-references. `name()` renders as the binder name so diagnostics
    /// stay readable (e.g. `Tree` rather than `Mu Tree. List<Tree>`).
    Mu { binder: String, body: Box<KType<'a>> },
    /// Application of a higher-kinded type constructor to arg types. `ctor` is a
    /// `KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. }` carrying the
    /// per-call nominal identity of the constructor slot; `args` are the elaborated arg
    /// types, one per `param_names` entry on the constructor.
    ///
    /// Structural equality by `(ctor, args)` — mirrors `KType::List(_)` / `Dict(_, _)`: an
    /// applied form keyed on inner structure, not a per-declaration nominal identity.
    ConstructorApply { ctor: Box<KType<'a>>, args: Vec<KType<'a>> },
    /// Back-reference to an enclosing `Mu`'s binder. Equality is by binder name only — the
    /// concrete identity is recovered from the surrounding `Mu` context.
    RecursiveRef(String),
    Any,
}

impl<'a> KType<'a> {
    /// Surface-syntax rendering. Mirrors the keyworded type-language overloads
    /// (`LIST OF T`, `MAP K -> V`, `FN <args> -> R`, `FUNCTOR <params> -> R`) so the
    /// rendered form parses back to the same `KType` through the dispatch-driven
    /// type-language path (see `design/typing/type-language-via-dispatch.md`).
    pub fn name(&self) -> String {
        match self {
            KType::Number => "Number".into(),
            KType::Str => "Str".into(),
            KType::Bool => "Bool".into(),
            KType::Null => "Null".into(),
            KType::List(t) => format!(":(LIST OF {})", t.name()),
            KType::Dict(k, v) => format!(":(MAP {} -> {})", k.name(), v.name()),
            KType::KFunction { args, ret } => {
                let arg_names: Vec<String> = args.iter().map(|a| a.name()).collect();
                format!(":(FN ({}) -> {})", arg_names.join(" "), ret.name())
            }
            KType::KFunctor { params, ret } => {
                let param_names: Vec<String> = params.iter().map(|p| p.name()).collect();
                format!(":(FUNCTOR ({}) -> {})", param_names.join(" "), ret.name())
            }
            KType::Identifier => "Identifier".into(),
            KType::KExpression => "KExpression".into(),
            KType::TypeExprRef => "TypeExprRef".into(),
            KType::Type => "Type".into(),
            KType::UserType { name, .. } => name.clone(),
            KType::AnyUserType { kind } => kind.surface_keyword().into(),
            KType::SatisfiesSignature { sig_path, pinned_slots, .. } => {
                if pinned_slots.is_empty() {
                    sig_path.clone()
                } else {
                    // Display-only — does not round-trip through the parser.
                    let inner: Vec<String> = pinned_slots
                        .iter()
                        .map(|(name, kt)| format!("({}: {})", name, kt.name()))
                        .collect();
                    format!("(SIG_WITH {} ({}))", sig_path, inner.join(" "))
                }
            }
            KType::Module { module, .. } => module.path.clone(),
            KType::Signature(s) => s.path.clone(),
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

    /// Stable entry point for diagnostic rendering. Reserved seam for cycle-aware printing;
    /// currently delegates to `name()`.
    pub fn render(&self) -> String {
        self.name()
    }
}

/// Manual `PartialEq` — needed because three of the new variants carry arena pointers
/// (`Module`, `Signature`, `AbstractType`) whose identity is the pointee's stable
/// `scope_id()` / `sig_id()` rather than the raw pointer. Two opaque ascriptions of the
/// same source module producing different `&Module` (each ascription allocates a fresh
/// child scope) must NOT compare equal under `KType::Module`; conversely, two
/// `KType::AbstractType` values minted from the same source-and-name must compare equal
/// even when their `&Module` pointers differ. The remaining variants follow the prior
/// derived-equality shape verbatim.
impl<'a> PartialEq for KType<'a> {
    fn eq(&self, other: &Self) -> bool {
        use KType::*;
        match (self, other) {
            (Number, Number) | (Str, Str) | (Bool, Bool) | (Null, Null)
            | (Identifier, Identifier) | (KExpression, KExpression)
            | (TypeExprRef, TypeExprRef) | (Type, Type) | (Any, Any)
            | (AnyModule, AnyModule) | (AnySignature, AnySignature) => true,
            (List(a), List(b)) => a == b,
            (Dict(ka, va), Dict(kb, vb)) => ka == kb && va == vb,
            (KFunction { args: a1, ret: r1 }, KFunction { args: a2, ret: r2 }) => {
                a1 == a2 && r1 == r2
            }
            (KFunctor { params: p1, ret: r1 }, KFunctor { params: p2, ret: r2 }) => {
                p1 == p2 && r1 == r2
            }
            (
                UserType { kind: k1, scope_id: s1, name: n1 },
                UserType { kind: k2, scope_id: s2, name: n2 },
            ) => k1 == k2 && s1 == s2 && n1 == n2,
            (AnyUserType { kind: k1 }, AnyUserType { kind: k2 }) => k1 == k2,
            (
                SatisfiesSignature { sig_id: i1, pinned_slots: p1, .. },
                SatisfiesSignature { sig_id: i2, pinned_slots: p2, .. },
            ) => i1 == i2 && p1 == p2,
            // Identity by the module's child-scope id — two opaque ascriptions allocate
            // distinct child scopes (and ScopeIds) so they remain distinct as types.
            // `frame` is a lifecycle anchor, not part of identity.
            (Module { module: m1, .. }, Module { module: m2, .. }) => {
                m1.scope_id() == m2.scope_id()
            }
            // Identity by sig_id — same scheme as `KType::Module`.
            (Signature(s1), Signature(s2)) => s1.sig_id() == s2.sig_id(),
            // Identity by `(source_module.scope_id(), name)` — two ascriptions of the
            // same source module with the same abstract-type name are the same type even
            // though their `&Module` pointers may differ.
            (
                AbstractType { source_module: m1, name: n1 },
                AbstractType { source_module: m2, name: n2 },
            ) => m1.scope_id() == m2.scope_id() && n1 == n2,
            (Mu { binder: b1, body: bd1 }, Mu { binder: b2, body: bd2 }) => {
                b1 == b2 && bd1 == bd2
            }
            (
                ConstructorApply { ctor: c1, args: a1 },
                ConstructorApply { ctor: c2, args: a2 },
            ) => c1 == c2 && a1 == a2,
            (RecursiveRef(n1), RecursiveRef(n2)) => n1 == n2,
            _ => false,
        }
    }
}
impl<'a> Eq for KType<'a> {}

/// Manual `Debug` — `derive` is blocked because `Module<'_>` / `Signature<'_>` /
/// `CallArena` don't (and can't easily) implement `Debug`: `Module` carries a `*const
/// Scope<'static>` lifetime-erasure pointer plus a `RefCell<HashMap<String, KType>>` whose
/// values can recurse into module-typed KTypes; pretty-printing those is unbounded. The
/// surface-name rendering `KType::name()` is the right level of detail for diagnostics
/// and matches what error messages show users.
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
            args: vec![KType::Number, KType::Str],
            ret: Box::new(KType::Bool),
        };
        assert_eq!(t.name(), ":(FN (Number Str) -> Bool)");
    }

    #[test]
    fn name_renders_functor() {
        let t = KType::KFunctor {
            params: vec![KType::Number, KType::Str],
            ret: Box::new(KType::Bool),
        };
        assert_eq!(t.name(), ":(FUNCTOR (Number Str) -> Bool)");
    }

    #[test]
    fn functor_structural_eq_same_shape() {
        let a = KType::KFunctor {
            params: vec![KType::Number, KType::Str],
            ret: Box::new(KType::Bool),
        };
        let b = KType::KFunctor {
            params: vec![KType::Number, KType::Str],
            ret: Box::new(KType::Bool),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn functor_structural_neq_when_params_or_ret_differ() {
        let base = KType::KFunctor {
            params: vec![KType::Number],
            ret: Box::new(KType::Bool),
        };
        let diff_params = KType::KFunctor {
            params: vec![KType::Str],
            ret: Box::new(KType::Bool),
        };
        let diff_ret = KType::KFunctor {
            params: vec![KType::Number],
            ret: Box::new(KType::Null),
        };
        assert_ne!(base, diff_params);
        assert_ne!(base, diff_ret);
    }

    #[test]
    fn functor_and_function_are_disjoint_types() {
        // Stage 0: structural identity is shape-disjoint even when params and ret align.
        // The cross-arm wall in `function_compat` enforces the same disjointness at the
        // admissibility layer (Stage 4).
        let f = KType::KFunction {
            args: vec![KType::Number],
            ret: Box::new(KType::Bool),
        };
        let g = KType::KFunctor {
            params: vec![KType::Number],
            ret: Box::new(KType::Bool),
        };
        assert_ne!(f, g);
    }

    #[test]
    fn name_renders_function_nullary() {
        let t = KType::KFunction {
            args: vec![],
            ret: Box::new(KType::Any),
        };
        assert_eq!(t.name(), ":(FN () -> Any)");
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
        assert_eq!(UserTypeKind::Struct.surface_keyword(), "Struct");
        assert_eq!(UserTypeKind::Tagged.surface_keyword(), "Tagged");
        assert_eq!(
            UserTypeKind::Newtype { repr: Box::new(KType::Number) }.surface_keyword(),
            "Newtype",
        );
        assert_eq!(
            UserTypeKind::TypeConstructor { param_names: vec!["T".into()] }.surface_keyword(),
            "TypeConstructor",
        );
    }

    #[test]
    fn newtype_kind_partial_eq_ignores_repr() {
        let a: UserTypeKind<'_> = UserTypeKind::Newtype { repr: Box::new(KType::Number) };
        let b: UserTypeKind<'_> = UserTypeKind::Newtype { repr: Box::new(KType::Str) };
        assert_eq!(a, b);
        assert_ne!(a, UserTypeKind::Struct);
        assert_ne!(UserTypeKind::Struct, a);
    }

    #[test]
    fn user_type_kind_type_constructor_partial_eq_ignores_param_names() {
        let a: UserTypeKind<'_> = UserTypeKind::TypeConstructor { param_names: vec!["T".into()] };
        let b: UserTypeKind<'_> = UserTypeKind::TypeConstructor { param_names: vec!["U".into()] };
        let empty: UserTypeKind<'_> = UserTypeKind::TypeConstructor { param_names: Vec::new() };
        assert_eq!(a, b);
        assert_eq!(a, empty);
        assert_ne!(a, UserTypeKind::Struct);
        assert_ne!(a, UserTypeKind::Newtype { repr: Box::new(KType::Number) });
    }

    #[test]
    fn any_user_type_name_renders_kind_keyword() {
        assert_eq!(
            KType::AnyUserType { kind: UserTypeKind::Struct }.name(),
            "Struct"
        );
        assert_eq!(
            KType::AnyUserType { kind: UserTypeKind::Tagged }.name(),
            "Tagged"
        );
    }

    #[test]
    fn any_module_and_any_signature_render_surface_keywords() {
        // `:Module` / `:Signature` slot wildcards render as their surface keyword
        // (parser round-trip) — replaces the `AnyUserType { kind: Module }` /
        // `MetaSignature` rendering after the carrier collapse.
        let am: KType<'_> = KType::AnyModule;
        let asg: KType<'_> = KType::AnySignature;
        assert_eq!(am.name(), "Module");
        assert_eq!(asg.name(), "Signature");
    }

    #[test]
    fn user_type_name_renders_bare_name() {
        // Per-declaration tag renders the declared `name`, not the kind keyword — pins the
        // diagnostic surface so a `Point` struct slot shows `Point`, not `Struct`.
        let t = KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: ScopeId::from_raw(0, 0x1234),
            name: "Point".into(),
        };
        assert_eq!(t.name(), "Point");
    }
}
