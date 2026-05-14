//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at `from_name` time. There's no bare `KFunction` — "any function" with
//! no signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates (`is_more_specific_than`, `matches_value`, `accepts_part`, `function_compat`)
//! live in `ktype_predicates.rs`; elaboration (`from_name`, `from_type_expr`, `join`,
//! `join_iter`) lives in `ktype_resolution.rs`.

/// Surface-keyword classifier shared by `KType::UserType` and `KType::AnyUserType`. Each
/// variant maps to the keyword that declares the carrier (`STRUCT`, anonymous-or-named
/// `UNION` → `Tagged`, `MODULE`). The kind is sourced from the declaration site at finalize
/// time and lives on both the per-declaration identity tag (`UserType`) and the wildcard
/// "any user-declared X" tag (`AnyUserType`). This is the dispatcher's primary kind
/// discriminator for user-declared types. See
/// [per-declaration type identity](../../../../design/type-system.md).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UserTypeKind {
    Struct,
    Tagged,
    Module,
}

impl UserTypeKind {
    /// Surface keyword rendered in diagnostics and `AnyUserType::name()`. Matches the
    /// surface name a user would write for the wildcard slot (`Struct`, `Tagged`,
    /// `Module`).
    pub fn surface_keyword(&self) -> &'static str {
        match self {
            UserTypeKind::Struct => "Struct",
            UserTypeKind::Tagged => "Tagged",
            UserTypeKind::Module => "Module",
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
    UserType { kind: UserTypeKind, scope_id: usize, name: String },
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
    /// error (`Rebind`). Equality (and dispatch admissibility) is by `sig_id`
    /// exclusively; `sig_path` is for diagnostics only. Distinguishing this from
    /// `AnyUserType { kind: Module }` is what lets the dispatcher reject unascribed
    /// modules from a signature-typed slot — the per-sig admissibility check rides on
    /// `Module`'s `compatible_sigs` set populated by `:|` / `:!`.
    SignatureBound { sig_id: usize, sig_path: String },
    /// Meta-type for first-class module signatures (`KObject::KSignature`).
    Signature,
    /// Recursive type binder. `body` describes the unfolded shape with `binder` in scope as a
    /// `RecursiveRef` for self-references. `name()` renders as the binder name so diagnostics
    /// stay readable (e.g. `Tree` rather than `Mu Tree. List<Tree>`). Constructed only by the
    /// scheduler-driven elaborator on top-level type-binding sites where a self-reference
    /// fired during body elaboration.
    Mu { binder: String, body: Box<KType> },
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
            KType::SignatureBound { sig_path, .. } => sig_path.clone(),
            KType::Signature => "Signature".into(),
            KType::Mu { binder, .. } => binder.clone(),
            KType::RecursiveRef(name) => name.clone(),
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
            scope_id: 0x1234,
            name: "Point".into(),
        };
        assert_eq!(t.name(), "Point");
    }
}
