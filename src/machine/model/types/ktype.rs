//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at `from_name` time. There's no bare `KFunction` — "any function" with
//! no signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates live in `ktype_predicates.rs`; elaboration lives in `ktype_resolution.rs`.
//!
//! Lifetime parameter `'a`: the five module/signature variants (`Module`, `Signature`,
//! `AbstractType`, `AnyModule`, `AnySignature`) hold `&'a Module<'a>` / `&'a Signature<'a>`
//! arena pointers; every other variant is owned data and ignores the parameter.

use crate::machine::core::{CallArena, ScopeId};
use crate::machine::model::values::{Module, Signature};
use std::rc::Rc;

/// Surface-keyword classifier shared by `KType::UserType` and `KType::AnyUserType`.
/// See [per-declaration type identity](../../../../design/typing/user-types.md).
///
/// The manual `PartialEq` / `Eq` impl below *ignores* the inner payload — identity
/// equality is by variant tag only. Load-bearing for wildcard admissibility:
/// `AnyUserType { kind: Newtype { repr: <sentinel> } }` must admit any concrete
/// `UserType { kind: Newtype { repr: <real> }, .. }`, and the same for `TypeConstructor`.
#[derive(Clone, Debug)]
pub enum UserTypeKind<'a> {
    Struct,
    Tagged,
    /// Fresh nominal identity over a transparent representation. `repr` is NOT part of
    /// identity equality.
    Newtype { repr: Box<KType<'a>> },
    /// Higher-kinded type-constructor slot. `param_names` is NOT part of identity equality.
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
    /// `function_compat` refuse both directions).
    KFunctor {
        params: Vec<KType<'a>>,
        ret: Box<KType<'a>>,
    },
    Identifier,
    /// Lazy slot: accepts an unevaluated `ExpressionPart::Expression` so the builtin chooses
    /// when (or whether) to run it.
    KExpression,
    /// Meta-type for slots capturing a parsed type-name token. Carries the full structured
    /// `TypeExpr` rather than flattening to a name string.
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
    /// First-class module value tagged with the signature it satisfies. `sig_id` addresses
    /// are stable for the run (the arena pins the `Signature`) and rebind in the same scope
    /// already errors, so the cast is collision-free. Equality is by `sig_id` plus
    /// `pinned_slots`; `sig_path` is diagnostic-only.
    ///
    /// `pinned_slots` carries abstract-type slots pinned to concrete `KType`s. The vec is
    /// order-preserving (rather than a `HashMap`) so structural equality is deterministic.
    SatisfiesSignature {
        sig_id: ScopeId,
        sig_path: String,
        pinned_slots: Vec<(String, KType<'a>)>,
    },
    /// First-class module value's type. `frame` carries the per-call `Rc<CallArena>`
    /// anchor for functor-built modules.
    Module {
        module: &'a Module<'a>,
        frame: Option<Rc<CallArena>>,
    },
    /// First-class module signature value's type.
    Signature(&'a Signature<'a>),
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
    Mu { binder: String, body: Box<KType<'a>> },
    /// Application of a higher-kinded type constructor to arg types. `ctor` is a
    /// `UserType` with `TypeConstructor` kind; `args` are the elaborated arg types.
    /// Structural equality by `(ctor, args)`.
    ConstructorApply { ctor: Box<KType<'a>>, args: Vec<KType<'a>> },
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
                    // Display-only; does not round-trip through the parser.
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

    /// Stable entry point for diagnostic rendering. Reserved seam for cycle-aware printing.
    pub fn render(&self) -> String {
        self.name()
    }
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
            // `frame` is a lifecycle anchor, not part of identity.
            (Module { module: m1, .. }, Module { module: m2, .. }) => {
                m1.scope_id() == m2.scope_id()
            }
            (Signature(s1), Signature(s2)) => s1.sig_id() == s2.sig_id(),
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
        let am: KType<'_> = KType::AnyModule;
        let asg: KType<'_> = KType::AnySignature;
        assert_eq!(am.name(), "Module");
        assert_eq!(asg.name(), "Signature");
    }

    #[test]
    fn user_type_name_renders_bare_name() {
        // Renders the declared `name`, not the kind keyword: a `Point` struct slot shows
        // `Point`, not `Struct`.
        let t = KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: ScopeId::from_raw(0, 0x1234),
            name: "Point".into(),
        };
        assert_eq!(t.name(), "Point");
    }
}
