//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at `from_name` time. There's no bare `KFunction` — "any function" with
//! no signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates (`is_more_specific_than`, `matches_value`, `accepts_part`, `function_compat`)
//! live in `ktype_predicates.rs`; elaboration (`from_name`, `from_type_expr`, `join`,
//! `join_iter`) lives in `ktype_resolution.rs`.

use crate::machine::core::ScopeId;

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
pub enum UserTypeKind {
    Struct,
    Tagged,
    Module,
    /// Fresh nominal identity over a transparent representation (`NEWTYPE Distance = Number`
    /// carries `repr: Box<KType::Number>`). `repr` is NOT part of identity equality — the
    /// manual `PartialEq` excludes it.
    Newtype { repr: Box<KType> },
    /// Higher-kinded type-constructor slot declared via `LET Wrap = (TYPE_CONSTRUCTOR T)`
    /// inside a SIG body. `param_names` is NOT part of identity equality — the manual
    /// `PartialEq` excludes it.
    TypeConstructor { param_names: Vec<String> },
}

impl PartialEq for UserTypeKind {
    fn eq(&self, other: &Self) -> bool {
        use UserTypeKind::*;
        matches!(
            (self, other),
            (Struct, Struct)
                | (Tagged, Tagged)
                | (Module, Module)
                | (Newtype { .. }, Newtype { .. })
                | (TypeConstructor { .. }, TypeConstructor { .. }),
        )
    }
}
impl Eq for UserTypeKind {}

impl UserTypeKind {
    /// Surface keyword rendered in diagnostics and `AnyUserType::name()`.
    pub fn surface_keyword(&self) -> &'static str {
        match self {
            UserTypeKind::Struct => "Struct",
            UserTypeKind::Tagged => "Tagged",
            UserTypeKind::Module => "Module",
            UserTypeKind::Newtype { .. } => "Newtype",
            UserTypeKind::TypeConstructor { .. } => "TypeConstructor",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum KType {
    Number,
    Str,
    Bool,
    Null,
    /// Bare `List` lowers to `List<Any>`.
    List(Box<KType>),
    /// Bare `Dict` lowers to `Dict<Any, Any>`.
    Dict(Box<KType>, Box<KType>),
    KFunction {
        args: Vec<KType>,
        ret: Box<KType>,
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
    ///
    /// Also covers per-module abstract types (`Foo.Type` from opaque ascription) with
    /// `kind: Module` and `name` set to the abstract type's name (typically `"Type"`) —
    /// distinguished from a first-class module value by `name`.
    UserType { kind: UserTypeKind, scope_id: ScopeId, name: String },
    /// Wildcard tag matching any user-declared carrier of the given `kind`. Strictly more
    /// specific than `Any`; specificity to a concrete `UserType { kind: K, .. }` is
    /// one-direction only — `UserType` is more specific than `AnyUserType` of the same
    /// kind, not the reverse.
    AnyUserType { kind: UserTypeKind },
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
    SignatureBound {
        sig_id: ScopeId,
        sig_path: String,
        pinned_slots: Vec<(String, KType)>,
    },
    /// Meta-type for first-class module signatures (`KObject::KSignature`).
    Signature,
    /// Recursive type binder. `body` describes the unfolded shape with `binder` in scope as a
    /// `RecursiveRef` for self-references. `name()` renders as the binder name so diagnostics
    /// stay readable (e.g. `Tree` rather than `Mu Tree. List<Tree>`).
    Mu { binder: String, body: Box<KType> },
    /// Application of a higher-kinded type constructor to arg types. `ctor` is a
    /// `KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. }` carrying the
    /// per-call nominal identity of the constructor slot; `args` are the elaborated arg
    /// types, one per `param_names` entry on the constructor.
    ///
    /// Structural equality by `(ctor, args)` — mirrors `KType::List(_)` / `Dict(_, _)`: an
    /// applied form keyed on inner structure, not a per-declaration nominal identity.
    ConstructorApply { ctor: Box<KType>, args: Vec<KType> },
    /// Back-reference to an enclosing `Mu`'s binder. Equality is by binder name only — the
    /// concrete identity is recovered from the surrounding `Mu` context.
    RecursiveRef(String),
    Any,
}

impl KType {
    /// Surface-syntax rendering. Mirrors the parser's `Function<(args) -> R>` / `List<T>` /
    /// `Dict<K, V>` syntax so a round-trip through the parser produces the same `KType`.
    pub fn name(&self) -> String {
        match self {
            KType::Number => "Number".into(),
            KType::Str => "Str".into(),
            KType::Bool => "Bool".into(),
            KType::Null => "Null".into(),
            KType::List(t) => format!(":(List {})", t.name()),
            KType::Dict(k, v) => format!(":(Dict {} {})", k.name(), v.name()),
            KType::KFunction { args, ret } => {
                let arg_names: Vec<String> = args.iter().map(|a| a.name()).collect();
                format!(":(Function ({}) -> {})", arg_names.join(" "), ret.name())
            }
            KType::Identifier => "Identifier".into(),
            KType::KExpression => "KExpression".into(),
            KType::TypeExprRef => "TypeExprRef".into(),
            KType::Type => "Type".into(),
            KType::UserType { name, .. } => name.clone(),
            KType::AnyUserType { kind } => kind.surface_keyword().into(),
            KType::SignatureBound { sig_path, pinned_slots, .. } => {
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
            KType::Signature => "Signature".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_renders_parameterized_list() {
        let t = KType::List(Box::new(KType::List(Box::new(KType::Number))));
        assert_eq!(t.name(), ":(List :(List Number))");
    }

    #[test]
    fn name_renders_dict() {
        let t = KType::Dict(Box::new(KType::Str), Box::new(KType::Number));
        assert_eq!(t.name(), ":(Dict Str Number)");
    }

    #[test]
    fn name_renders_function() {
        let t = KType::KFunction {
            args: vec![KType::Number, KType::Str],
            ret: Box::new(KType::Bool),
        };
        assert_eq!(t.name(), ":(Function (Number Str) -> Bool)");
    }

    #[test]
    fn name_renders_function_nullary() {
        let t = KType::KFunction {
            args: vec![],
            ret: Box::new(KType::Any),
        };
        assert_eq!(t.name(), ":(Function () -> Any)");
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
        assert_eq!(UserTypeKind::Module.surface_keyword(), "Module");
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
        let a = UserTypeKind::Newtype { repr: Box::new(KType::Number) };
        let b = UserTypeKind::Newtype { repr: Box::new(KType::Str) };
        assert_eq!(a, b);
        assert_ne!(a, UserTypeKind::Struct);
        assert_ne!(UserTypeKind::Struct, a);
    }

    #[test]
    fn user_type_kind_type_constructor_partial_eq_ignores_param_names() {
        let a = UserTypeKind::TypeConstructor { param_names: vec!["T".into()] };
        let b = UserTypeKind::TypeConstructor { param_names: vec!["U".into()] };
        let empty = UserTypeKind::TypeConstructor { param_names: Vec::new() };
        assert_eq!(a, b);
        assert_eq!(a, empty);
        assert_ne!(a, UserTypeKind::Struct);
        assert_ne!(UserTypeKind::Module, a);
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
        assert_eq!(
            KType::AnyUserType { kind: UserTypeKind::Module }.name(),
            "Module"
        );
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
