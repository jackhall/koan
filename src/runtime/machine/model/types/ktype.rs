//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at `from_name` time. There's no bare `KFunction` — "any function" with
//! no signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates (`is_more_specific_than`, `matches_value`, `accepts_part`, `function_compat`)
//! live in `ktype_predicates.rs`; elaboration (`from_name`, `from_type_expr`, `join`,
//! `join_iter`) lives in `ktype_resolution.rs`.

use crate::runtime::machine::core::ScopeId;

/// Surface-keyword classifier shared by `KType::UserType` and `KType::AnyUserType`. Each
/// variant maps to the keyword that declares the carrier (`STRUCT`, anonymous-or-named
/// `UNION` → `Tagged`, `MODULE`, `NEWTYPE`). The kind is sourced from the declaration site
/// at finalize time and lives on both the per-declaration identity tag (`UserType`) and
/// the wildcard "any user-declared X" tag (`AnyUserType`). This is the dispatcher's
/// primary kind discriminator for user-declared types. See
/// [per-declaration type identity](../../../../design/type-system.md).
///
/// Stage 4 added the `Newtype { repr }` variant which carries a `Box<KType>`, so the enum
/// is no longer `Copy`. Module-system stage 2 added the `TypeConstructor { param_names }`
/// variant on the same pattern. The manual `PartialEq` / `Eq` impl below *ignores* the
/// inner payload — identity equality is by variant tag only, since the per-declaration
/// `(scope_id, name)` pair on `KType::UserType` already separates two carriers of the
/// same kind. Ignoring the payload is load-bearing for wildcard admissibility:
/// `AnyUserType { kind: Newtype { repr: <sentinel> } }` admits any concrete
/// `UserType { kind: Newtype { repr: <real> }, .. }`, and the same applies to the
/// `TypeConstructor` variant.
#[derive(Clone, Debug)]
pub enum UserTypeKind {
    Struct,
    Tagged,
    Module,
    /// Stage 4: fresh nominal identity over a transparent representation.
    /// `repr` is the declared representation type (`NEWTYPE Distance = Number`
    /// carries `repr: Box<KType::Number>`). The variant-internal `repr` is NOT
    /// part of identity equality (the manual `PartialEq` excludes it); two
    /// `KType::UserType` values with the same `(scope_id, name)` but technically
    /// different `repr` boxes (e.g. arena-allocated identity vs. a freshly cloned
    /// one, or wildcard-sentinel vs. concrete) still compare equal.
    Newtype { repr: Box<KType> },
    /// Module-system stage 2: higher-kinded type-constructor slot in a SIG. Declared
    /// via `LET Wrap = (TYPE_CONSTRUCTOR T)` inside a SIG body; minted at opaque
    /// ascription with a fresh `scope_id` per call (mirror of the `kind: Module`
    /// abstract-type slot path in `ascribe.rs:body_opaque`). The variant-internal
    /// `param_names` is NOT part of identity equality (the manual `PartialEq`
    /// excludes it): two `UserType { kind: TypeConstructor { param_names: a }, .. }`
    /// values with the same `(scope_id, name)` but different `param_names` lists
    /// (e.g. wildcard-sentinel vs. concrete) still compare equal.
    ///
    /// Stage 2 ships arity-1 only — the param-name list is always a single entry.
    /// Higher arity (`Functor F G`) is deferred per the roadmap.
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
    /// Surface keyword rendered in diagnostics and `AnyUserType::name()`. Matches the
    /// surface name a user would write for the wildcard slot (`Struct`, `Tagged`,
    /// `Module`, `Newtype`, `TypeConstructor`). `Newtype` and `TypeConstructor` are
    /// not registered as writable surface names in `from_name` / `default_scope` —
    /// deferred per the roadmap — but the keyword is still pinned here for
    /// diagnostic rendering.
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
    /// Per-declaration identity tag for a user-declared type (STRUCT, UNION, MODULE). The
    /// `(scope_id, name)` pair is the dispatch identity: distinct declarations in the same
    /// scope have distinct `name`s; same-named declarations in different scopes have
    /// distinct `scope_id`s. `kind` carries the surface keyword so the wildcard
    /// `AnyUserType { kind }` can admit only the matching family.
    ///
    /// Synthesized by `KObject::ktype()` for `Struct`, `Tagged`, and `KModule` carriers
    /// from their `(scope_id, name)` identity fields. Also covers per-module abstract
    /// types (`Foo.Type` from opaque ascription) with `kind: Module` and `name` set to
    /// the abstract type's name (typically `"Type"`) — distinguished from a first-class
    /// module value by `name`.
    UserType { kind: UserTypeKind, scope_id: ScopeId, name: String },
    /// Wildcard tag matching any user-declared carrier of the given `kind`. The surface
    /// names `"Struct"` / `"Tagged"` / `"Module"` resolve to this; a slot typed `Struct`
    /// accepts any `KObject::Struct{..}` regardless of declaring schema. Strictly more
    /// specific than `Any`; incomparable with other `AnyUserType`s of a different kind
    /// and with concrete `UserType`s of the same kind (matching specificity only one
    /// direction: `UserType { kind: K, .. }` is more specific than `AnyUserType { kind: K }`).
    AnyUserType { kind: UserTypeKind },
    /// First-class module value tagged with the signature it satisfies. `sig_id` is the
    /// declaring `Signature`'s `decl_scope_ptr as usize` — same `*const _ as usize`
    /// identity scheme `UserType { kind: Module, scope_id, .. }` uses for first-class
    /// module values: the arena pins the `Signature` for the run, addresses are stable
    /// and unique, and two `SIG Foo = (...)` declarations in the same scope already
    /// error (`Rebind`). Equality (and dispatch admissibility) is by `sig_id` plus the
    /// `pinned_slots` constraint vector; `sig_path` is for diagnostics only.
    /// Distinguishing this from `AnyUserType { kind: Module }` is what lets the
    /// dispatcher reject unascribed modules from a signature-typed slot — the per-sig
    /// admissibility check rides on `Module`'s `compatible_sigs` set populated by
    /// `:|` / `:!`.
    ///
    /// `pinned_slots` carries sharing constraints — abstract-type slots of the signature
    /// pinned to specific concrete `KType`s. Empty for the unconstrained `OrderedSig`
    /// form. Two `SignatureBound`s with the same `sig_id` but different `pinned_slots`
    /// vectors are distinct slot types; admissibility (`matches_value`, `accepts_part`)
    /// also checks each pin against the candidate module's `type_members`. The vec is
    /// order-preserving — rather than a `HashMap` — so structural equality is
    /// deterministic and the diagnostic surface stays stable.
    SignatureBound {
        sig_id: ScopeId,
        sig_path: String,
        pinned_slots: Vec<(String, KType)>,
    },
    /// Meta-type for first-class module signatures (`KObject::KSignature`).
    Signature,
    /// Recursive type binder. `body` describes the unfolded shape with `binder` in scope as a
    /// `RecursiveRef` for self-references. `name()` renders as the binder name so diagnostics
    /// stay readable (e.g. `Tree` rather than `Mu Tree. List<Tree>`). Constructed only by the
    /// scheduler-driven elaborator on top-level type-binding sites where a self-reference
    /// fired during body elaboration.
    Mu { binder: String, body: Box<KType> },
    /// Application of a higher-kinded type constructor to arg types. `ctor` is a
    /// `KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. }` carrying the
    /// per-call nominal identity of the constructor slot (minted by opaque ascription).
    /// `args` are the structurally-elaborated arg types (one per `param_names` entry on
    /// the constructor).
    ///
    /// Structural equality by `(ctor, args)` — two applications of the same constructor
    /// with the same args are interchangeable. Mirrors `KType::List(_)` / `Dict(_, _)`:
    /// an applied form keyed on inner structure, not a per-declaration nominal identity.
    /// Stage 2 emits this for `Wrap<T>` and `M.Wrap<T>` via `elaborate_type_expr`.
    ConstructorApply { ctor: Box<KType>, args: Vec<KType> },
    /// Back-reference to an enclosing `Mu`'s binder. Equality is by binder name only — the
    /// concrete identity is recovered from the surrounding `Mu` context. Never constructed
    /// from user source directly; only the elaborator emits it.
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
            KType::List(t) => format!("List<{}>", t.name()),
            KType::Dict(k, v) => format!("Dict<{}, {}>", k.name(), v.name()),
            KType::KFunction { args, ret } => {
                let arg_names: Vec<String> = args.iter().map(|a| a.name()).collect();
                format!("Function<({}) -> {}>", arg_names.join(", "), ret.name())
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
                    // Pinned-form rendering mirrors the parens-form surface that
                    // `SIG_WITH` accepts at slot positions. Pure display surface — does
                    // not round-trip through the parser.
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
                format!("{}<{}>", ctor.name(), arg_names.join(", "))
            }
            KType::Any => "Any".into(),
        }
    }

    /// Stable entry point for diagnostic rendering. Currently delegates to `name()`; reserved
    /// for cycle-aware printing without churning call sites when the renderer is upgraded.
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
        assert_eq!(t.name(), "List<List<Number>>");
    }

    #[test]
    fn name_renders_dict() {
        let t = KType::Dict(Box::new(KType::Str), Box::new(KType::Number));
        assert_eq!(t.name(), "Dict<Str, Number>");
    }

    #[test]
    fn name_renders_function() {
        let t = KType::KFunction {
            args: vec![KType::Number, KType::Str],
            ret: Box::new(KType::Bool),
        };
        assert_eq!(t.name(), "Function<(Number, Str) -> Bool>");
    }

    #[test]
    fn name_renders_function_nullary() {
        let t = KType::KFunction {
            args: vec![],
            ret: Box::new(KType::Any),
        };
        assert_eq!(t.name(), "Function<() -> Any>");
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

    /// Manual `PartialEq` on `UserTypeKind` ignores the `Newtype` variant's `repr`.
    /// Load-bearing for the wildcard `AnyUserType { kind: Newtype { repr: <sentinel> } }`
    /// to compare equal to a concrete `UserType { kind: Newtype { repr: <real> }, .. }`.
    #[test]
    fn newtype_kind_partial_eq_ignores_repr() {
        let a = UserTypeKind::Newtype { repr: Box::new(KType::Number) };
        let b = UserTypeKind::Newtype { repr: Box::new(KType::Str) };
        assert_eq!(a, b);
        assert_ne!(a, UserTypeKind::Struct);
        assert_ne!(UserTypeKind::Struct, a);
    }

    /// Module-system stage 2: manual `PartialEq` on the `TypeConstructor` variant ignores
    /// `param_names`. Load-bearing for the wildcard
    /// `AnyUserType { kind: TypeConstructor { param_names: <sentinel> } }` to compare
    /// equal to a concrete `UserType { kind: TypeConstructor { param_names: <real> }, .. }`.
    /// Mirror of `newtype_kind_partial_eq_ignores_repr`.
    #[test]
    fn user_type_kind_type_constructor_partial_eq_ignores_param_names() {
        let a = UserTypeKind::TypeConstructor { param_names: vec!["T".into()] };
        let b = UserTypeKind::TypeConstructor { param_names: vec!["U".into()] };
        let empty = UserTypeKind::TypeConstructor { param_names: Vec::new() };
        assert_eq!(a, b);
        assert_eq!(a, empty);
        assert_ne!(a, UserTypeKind::Struct);
        assert_ne!(UserTypeKind::Module, a);
        // Cross-kind: TypeConstructor must not compare equal to Newtype (both carry
        // payloads but distinct variant tags).
        assert_ne!(a, UserTypeKind::Newtype { repr: Box::new(KType::Number) });
    }

    #[test]
    fn any_user_type_name_renders_kind_keyword() {
        // Wildcard tag renders the surface keyword for the kind.
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
        // Per-declaration tag renders the declared `name`, not the kind keyword. Pins the
        // diagnostic surface: a `Point` struct slot shows `Point`, not `Struct`.
        let t = KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: ScopeId::from_raw(0, 0x1234),
            name: "Point".into(),
        };
        assert_eq!(t.name(), "Point");
    }
}
