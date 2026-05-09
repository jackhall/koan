//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Used by `Argument::matches` at dispatch time, by user-facing return-type annotations on
//! functions, and by the scheduler's runtime return-type check.
//!
//! `KExpression` is the lazy slot: it accepts an unevaluated `ExpressionPart::Expression`
//! so the receiving builtin can choose when (or whether) to run it. `TypeExprRef` is the
//! single meta-type for slots that capture a parsed type-name token (`ExpressionPart::Type(_)`):
//! used by FN's return-type slot and by STRUCT/UNION/type-call's name slots. Resolved values
//! are `KObject::TypeExprValue(t)` so callers see the full `TypeExpr` — name plus any
//! nested params — rather than a flattened name string.
//!
//! `Type` is the meta-type for any first-class type-value: a tagged-union schema produced by
//! `UNION` or a struct schema produced by `STRUCT` are both `KType::Type` at runtime, so
//! builtins that consume "a type" (construction primitives, future trait checks) can declare
//! a single slot and accept either form.
//!
//! Future work: let users define duck types instead of an enum.
//!
//! Container types are always parameterized: `List(Box<KType>)` carries the element type;
//! `Dict(Box<KType>, Box<KType>)` carries key and value types; `KFunction { args, ret }`
//! carries the full function signature. The bare names `List` / `Dict` lower to the `Any`-
//! elemented forms (`List<Any>`, `Dict<Any, Any>`) at `from_name` time. There's no bare
//! `KFunction` — users write `Function<(args) -> R>` for a typed function or `Any` for an
//! unconstrained value, since "any function" with no signature has nothing to dispatch on.
//!
//! This file holds the core enum and its surface-name `name()` rendering. The predicate
//! impls (`is_more_specific_than`, `matches_value`, `accepts_part`, `function_compat`) live
//! in `ktype_predicates.rs`; the elaboration impls (`from_name`, `from_type_expr`, `join`,
//! `join_iter`) live in `ktype_resolution.rs`.

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum KType {
    Number,
    Str,
    Bool,
    Null,
    /// `List<T>` — element type. Bare `List` lowers to `List<Any>`.
    List(Box<KType>),
    /// `Dict<K, V>` — key type and value type. Bare `Dict` lowers to `Dict<Any, Any>`.
    Dict(Box<KType>, Box<KType>),
    /// `Function<(args) -> ret>` — structural function type. `args.len()` is the arity;
    /// each `args[i]` is the declared type of that parameter slot.
    KFunction {
        args: Vec<KType>,
        ret: Box<KType>,
    },
    Identifier,
    KExpression,
    /// Meta-type for any slot that captures a parsed type-name token (`ExpressionPart::Type`).
    /// Resolves to `KObject::TypeExprValue(t.clone())` — the full structured `TypeExpr`,
    /// preserving any nested parameters (`List<Number>`, `Function<(N) -> S>`). Slots that
    /// want only the bare name (e.g. STRUCT/UNION's name slots) check `TypeParams::None` on
    /// the inner expr and read `t.name`.
    TypeExprRef,
    /// Meta-type for first-class type-values: `KObject::TaggedUnionType` and
    /// `KObject::StructType` both report this. Consumed by construction primitives and any
    /// builtin that takes "a type" as an argument.
    Type,
    /// Flat singleton tag: every tagged-union value reports the same `KType::Tagged` regardless
    /// of which UNION schema declared it. Per-declaration identity for tagged-union values is
    /// not yet carried here — the analogous identity story for opaquely-ascribed module
    /// abstract types lives in `ModuleType` below. Folding declaring-scope identity into
    /// `Tagged` is tracked as
    /// [per-declaration type identity](../../../roadmap/per-declaration-type-identity.md).
    Tagged,
    /// Flat singleton tag: every user struct reports the same `KType::Struct` regardless of
    /// declaration. Per-declaration identity is not yet carried here; see the `Tagged` note
    /// above for the analogous module-identity discussion and
    /// [per-declaration type identity](../../../roadmap/per-declaration-type-identity.md)
    /// for the tracking item.
    Struct,
    /// Per-module abstract type (`Foo.Type` after opaque ascription). `scope_id` is the
    /// declaring module's child-scope address cast to `usize` — stable for the run because
    /// `Scope`s are arena-allocated and never moved, distinct across modules because the
    /// arena hands out fresh addresses, and equal between two `KType::ModuleType` values iff
    /// they were minted by the same opaque-ascription event. `name` is the abstract type name
    /// (typically `"Type"`); it's the textual disambiguator within a module that declares
    /// multiple abstract types. Equality on `KType::ModuleType` is the dispatch identity
    /// check that makes opaquely-ascribed `IntOrd.Type` distinct from `Number` even when
    /// the underlying definition is `Number`.
    ModuleType { scope_id: usize, name: String },
    /// Meta-type for `KObject::KModule` values: a first-class module value. Reported by
    /// `KObject::ktype()` so any "expects a module" slot — ATTR's lhs, the ascription
    /// operators' lhs — can declare a single slot type.
    Module,
    /// Meta-type for `KObject::KSignature` values: a first-class module signature. The
    /// ascription operators' RHS slot uses this to require a signature on the right-hand
    /// side of `:|` / `:!`.
    Signature,
    Any,
}

impl KType {
    /// Surface-syntax rendering of this type — used by error formatters. Mirrors the parser's
    /// `Function<(args) -> R>` / `List<T>` / `Dict<K, V>` syntax so a round-trip through the
    /// parser produces the same `KType`.
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
            KType::Signature => "Signature".into(),
            KType::Any => "Any".into(),
        }
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
}
