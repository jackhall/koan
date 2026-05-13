//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at `from_name` time. There's no bare `KFunction` — "any function" with
//! no signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates (`is_more_specific_than`, `matches_value`, `accepts_part`, `function_compat`)
//! live in `ktype_predicates.rs`; elaboration (`from_name`, `from_type_expr`, `join`,
//! `join_iter`) lives in `ktype_resolution.rs`.

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
    /// Singleton tag for every tagged-union value, regardless of declaring schema. Per-declaration
    /// identity is tracked as
    /// [per-declaration type identity](../../../../roadmap/type-identity-3-user-type-and-per-decl.md).
    Tagged,
    /// Singleton tag for every user struct, regardless of declaration. Per-declaration identity
    /// is tracked as
    /// [per-declaration type identity](../../../../roadmap/type-identity-3-user-type-and-per-decl.md).
    Struct,
    /// Per-module abstract type (`Foo.Type` after opaque ascription). `scope_id` is the
    /// declaring module's child-scope address cast to `usize` — stable for the run because
    /// `Scope`s are arena-allocated and never moved, distinct across modules because the
    /// arena hands out fresh addresses, and equal between two `KType::ModuleType` values iff
    /// they were minted by the same opaque-ascription event. `name` is the abstract type name
    /// (typically `"Type"`); it disambiguates when a module declares multiple abstract types.
    /// Equality on `KType::ModuleType` is the dispatch identity check that makes opaquely-
    /// ascribed `IntOrd.Type` distinct from `Number` even when the underlying definition is
    /// `Number`.
    ModuleType { scope_id: usize, name: String },
    /// Meta-type for first-class module values (`KObject::KModule`).
    Module,
    /// First-class module value tagged with the signature it satisfies. `sig_id` is the
    /// declaring `Signature`'s `decl_scope_ptr as usize` — the same identity scheme
    /// `ModuleType` uses for module abstract types: the arena pins the `Signature` for the
    /// run, addresses are stable and unique, and two `SIG Foo = (...)` declarations in the
    /// same scope already error (`Rebind`). Equality (and dispatch admissibility) is by
    /// `sig_id` exclusively; `sig_path` is for diagnostics only. Distinguishing this from
    /// `KType::Module` is what lets the dispatcher reject unascribed modules from a
    /// signature-typed slot — the per-sig admissibility check rides on `Module`'s
    /// `compatible_sigs` set populated by `:|` / `:!`.
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
    /// Bind-time carrier for a bare-leaf type name (`Point`, `IntOrd`, `MyList`) whose
    /// `from_name` lookup failed at [`crate::ast::ExpressionPart::resolve_for`] time —
    /// i.e. anything not in [`KType::from_name`]'s builtin table. Carries the surface name
    /// so downstream consumers (`extract_bare_type_name`, `dispatch_constructor`, ATTR's
    /// TypeExprRef-lhs lookup, FN return-type elaboration) can recover the user's
    /// identifier and re-elaborate against `Scope` via [`super::elaborate_type_expr`].
    ///
    /// **Why this isn't gone yet.** `resolve_for` runs at `KFunction::bind` time — after
    /// the dispatcher's per-slot classification but before any body sees the bundle. For
    /// pre_run-bearing binders (LET, STRUCT, UNION, MODULE, SIG, FN) the dispatcher's
    /// `classify_for_pick` skips `wrap_indices` / `ref_name_indices` for `TypeExprRef`
    /// slots because the binder's own name slot is a *declaration*, not a reference;
    /// FN's return-type slot rides through that same skip and lands here as a non-builtin
    /// `Type(_)` token. Deleting `Unresolved` requires either a per-slot "I am a
    /// reference, please wrap" opt-in on `classify_for_pick` (currently coarse: pre_run
    /// → skip-wrap on all literal-name slots) or a new `KObject` carrier preserving the
    /// surface `TypeExpr` through bind. Both are out of scope for the eager-type-
    /// elaboration roadmap item.
    Unresolved(String),
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
            KType::Tagged => "Tagged".into(),
            KType::Struct => "Struct".into(),
            KType::ModuleType { name, .. } => name.clone(),
            KType::Module => "Module".into(),
            KType::SignatureBound { sig_path, .. } => sig_path.clone(),
            KType::Signature => "Signature".into(),
            KType::Mu { binder, .. } => binder.clone(),
            KType::RecursiveRef(name) => name.clone(),
            KType::Unresolved(name) => name.clone(),
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
}
