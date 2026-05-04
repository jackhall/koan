//! `KType` — the type tag attached to argument slots, function return-types, and runtime values.
//!
//! Used by `Argument::matches` at dispatch time, by user-facing return-type annotations on
//! functions, and by the scheduler's runtime return-type check.
//!
//! `KExpression` is the lazy slot: it accepts an unevaluated `ExpressionPart::Expression`
//! so the receiving builtin can choose when (or whether) to run it. `TypeRef` is a meta-type
//! for argument slots that capture a parsed type-name token (`ExpressionPart::Type(_)`) —
//! used by `FN`'s return-type annotation slot, not declarable in user code.
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

use crate::parse::kexpression::{ExpressionPart, KLiteral, TypeExpr, TypeParams};

use crate::dispatch::values::KObject;
use super::signature::{ExpressionSignature, SignatureElement};

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
    /// Meta-type for an FN-style type-name slot that receives a flat `Type(_)` token. Resolves
    /// to `KString(name)`. Phase 2 leaves this in for any caller that genuinely wants the
    /// name string only — currently no remaining uses, but kept as a vestigial slot kind in
    /// case a builtin needs it.
    TypeRef,
    /// Meta-type for an FN-style type-name slot that wants the full structured `TypeExpr`,
    /// not just the name. Used by FN's return-type slot so parameterized types like
    /// `List<Number>` survive the parser → dispatch boundary intact. Resolves to
    /// `KObject::TypeExprValue(t.clone())`.
    TypeExprRef,
    /// Meta-type for first-class type-values: `KObject::TaggedUnionType` and
    /// `KObject::StructType` both report this. Consumed by construction primitives and any
    /// builtin that takes "a type" as an argument.
    Type,
    /// A tagged value — one variant of a tagged union, carrying its tag and inner payload.
    /// Produced by `TAG`, consumed by `MATCH` to branch by tag.
    Tagged,
    /// A struct value — a record of named fields produced by a struct-type constructor.
    Struct,
    Any,
}

impl KType {
    /// Specificity ordering for `specificity_vs`. Concrete types outrank `Any`; for parameterized
    /// containers, refinement of any inner slot makes the whole type more specific (covariant in
    /// element / key / value / arg / return positions). Strict — returns `false` for equal types.
    pub fn is_more_specific_than(&self, other: &KType) -> bool {
        use KType::*;
        if matches!(other, Any) && !matches!(self, Any) {
            return true;
        }
        match (self, other) {
            (List(a), List(b)) => a.is_more_specific_than(b),
            (Dict(ka, va), Dict(kb, vb)) => {
                let k_more = ka.is_more_specific_than(kb);
                let v_more = va.is_more_specific_than(vb);
                let k_eq = ka == kb;
                let v_eq = va == vb;
                (k_more && (v_more || v_eq)) || (k_eq && v_more)
            }
            (
                KFunction { args: aa, ret: ar },
                KFunction { args: ba, ret: br },
            ) if aa.len() == ba.len() => {
                let args_more = aa.iter().zip(ba.iter()).any(|(x, y)| x.is_more_specific_than(y));
                let args_eq = aa == ba;
                let ret_more = ar.is_more_specific_than(br);
                let ret_eq = ar == br;
                (args_more && (ret_more || ret_eq)) || (args_eq && ret_more)
            }
            _ => false,
        }
    }

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
            KType::TypeRef => "TypeRef".into(),
            KType::TypeExprRef => "TypeExprRef".into(),
            KType::Type => "Type".into(),
            KType::Tagged => "Tagged".into(),
            KType::Struct => "Struct".into(),
            KType::Any => "Any".into(),
        }
    }

    /// Look up a `KType` by the textual name a user can write in source (e.g. `Number`,
    /// `List`). Returns `None` for unknown names. `Identifier`, `TypeRef`, `TypeExprRef` are
    /// dispatch-time meta-types — not surface-declarable. `KFunction` is no longer a surface
    /// name; users write `Function<(...)-> R>` for typed functions or `Any` for unconstrained.
    pub fn from_name(name: &str) -> Option<KType> {
        match name {
            "Number" => Some(KType::Number),
            "Str" => Some(KType::Str),
            "Bool" => Some(KType::Bool),
            "Null" => Some(KType::Null),
            "List" => Some(KType::List(Box::new(KType::Any))),
            "Dict" => Some(KType::Dict(Box::new(KType::Any), Box::new(KType::Any))),
            "KExpression" => Some(KType::KExpression),
            "Type" => Some(KType::Type),
            "Tagged" => Some(KType::Tagged),
            "Struct" => Some(KType::Struct),
            "Any" => Some(KType::Any),
            _ => None,
        }
    }

    /// Convert a parser `TypeExpr` into a `KType`. This is the surface-level type-parsing
    /// boundary used by FN signatures, FN return-type slots, and UNION/STRUCT field types.
    /// Recurses on nested type parameters; arity for the known containers is enforced here so
    /// errors surface at FN-definition time rather than at first call.
    pub fn from_type_expr(t: &TypeExpr) -> Result<KType, String> {
        match (t.name.as_str(), &t.params) {
            (_, TypeParams::None) => KType::from_name(&t.name)
                .ok_or_else(|| format!("unknown type name `{}`", t.name)),
            ("List", TypeParams::List(items)) if items.len() == 1 => {
                Ok(KType::List(Box::new(KType::from_type_expr(&items[0])?)))
            }
            ("List", TypeParams::List(items)) => Err(format!(
                "List<...> expects exactly 1 type parameter, got {}",
                items.len()
            )),
            ("Dict", TypeParams::List(items)) if items.len() == 2 => Ok(KType::Dict(
                Box::new(KType::from_type_expr(&items[0])?),
                Box::new(KType::from_type_expr(&items[1])?),
            )),
            ("Dict", TypeParams::List(items)) => Err(format!(
                "Dict<...> expects exactly 2 type parameters, got {}",
                items.len()
            )),
            ("Function", TypeParams::Function { args, ret }) => {
                let args = args
                    .iter()
                    .map(KType::from_type_expr)
                    .collect::<Result<Vec<_>, _>>()?;
                let ret = Box::new(KType::from_type_expr(ret)?);
                Ok(KType::KFunction { args, ret })
            }
            (_, TypeParams::List(_)) => {
                Err(format!("type `{}` does not take type parameters", t.name))
            }
            (_, TypeParams::Function { .. }) => Err(format!(
                "only `Function` accepts a `(args) -> ret` shape; got `{}`",
                t.name
            )),
        }
    }

    /// True iff a runtime `KObject` value satisfies this declared type. `Any` matches
    /// everything; container types recurse into element/key/value positions; function types
    /// require structural signature compatibility (a `KFuture` thunk is accepted because its
    /// result isn't known yet — full check deferred to runtime).
    pub fn matches_value(&self, obj: &KObject<'_>) -> bool {
        match self {
            KType::Any => true,
            KType::List(elem) => match obj {
                KObject::List(items) => items.iter().all(|x| elem.matches_value(x)),
                _ => false,
            },
            KType::Dict(k_ty, v_ty) => match obj {
                KObject::Dict(map) => map.iter().all(|(k_key, v_obj)| {
                    let k_t = k_key.ktype();
                    (matches!(k_ty.as_ref(), KType::Any) || **k_ty == k_t)
                        && v_ty.matches_value(v_obj)
                }),
                _ => false,
            },
            KType::KFunction { args, ret } => match obj {
                KObject::KFunction(f, _) => function_compat(&f.signature, args, ret),
                KObject::KFuture(_, _) => true,
                _ => false,
            },
            _ => *self == obj.ktype(),
        }
    }

    /// Per-`ExpressionPart` admissibility check: can a part of this shape fill an argument
    /// slot of this type? Container slots are shape-only at dispatch time — element-type
    /// validation for `List<Number>` etc. happens post-evaluation in `matches_value`, since
    /// lazy lists at dispatch time may carry unevaluated `Expression` parts. Function slots
    /// with a structural `KFunction { args, ret }` shape DO validate the bound function's
    /// signature here, since `KObject::KFunction` carries the full signature.
    ///
    /// `Argument::matches` is a thin delegate to this; the per-variant table lives here so
    /// it stays next to the `KType` enum.
    pub fn accepts_part(&self, part: &ExpressionPart<'_>) -> bool {
        match self {
            KType::Any => true,
            KType::Number => matches!(
                part,
                ExpressionPart::Literal(KLiteral::Number(_))
                    | ExpressionPart::Future(KObject::Number(_))
            ),
            KType::Str => matches!(
                part,
                ExpressionPart::Literal(KLiteral::String(_))
                    | ExpressionPart::Future(KObject::KString(_))
            ),
            KType::Bool => matches!(
                part,
                ExpressionPart::Literal(KLiteral::Boolean(_))
                    | ExpressionPart::Future(KObject::Bool(_))
            ),
            KType::Null => matches!(
                part,
                ExpressionPart::Literal(KLiteral::Null) | ExpressionPart::Future(KObject::Null)
            ),
            KType::List(_) => matches!(
                part,
                ExpressionPart::ListLiteral(_) | ExpressionPart::Future(KObject::List(_))
            ),
            KType::Dict(_, _) => matches!(
                part,
                ExpressionPart::DictLiteral(_) | ExpressionPart::Future(KObject::Dict(_))
            ),
            KType::KFunction { args, ret } => match part {
                ExpressionPart::Future(KObject::KFunction(f, _)) => {
                    function_compat(&f.signature, args, ret)
                }
                ExpressionPart::Future(KObject::KFuture(_, _)) => true,
                _ => false,
            },
            KType::Identifier => matches!(part, ExpressionPart::Identifier(_)),
            KType::KExpression => matches!(part, ExpressionPart::Expression(_)),
            KType::TypeRef | KType::TypeExprRef => matches!(part, ExpressionPart::Type(_)),
            KType::Type => matches!(
                part,
                ExpressionPart::Future(KObject::TaggedUnionType(_))
                    | ExpressionPart::Future(KObject::StructType { .. })
            ),
            KType::Tagged => matches!(
                part,
                ExpressionPart::Future(KObject::Tagged { .. })
            ),
            KType::Struct => matches!(
                part,
                ExpressionPart::Future(KObject::Struct { .. })
            ),
        }
    }

    /// Least-upper-bound of two types. Used by `KObject::ktype` to infer container element
    /// types from heterogeneous values: `[1, 2]` → `List<Number>`, `[1, "x"]` → `List<Any>`,
    /// nested containers join element-wise.
    pub fn join(a: &KType, b: &KType) -> KType {
        if a == b {
            return a.clone();
        }
        match (a, b) {
            (KType::List(x), KType::List(y)) => KType::List(Box::new(KType::join(x, y))),
            (KType::Dict(xk, xv), KType::Dict(yk, yv)) => {
                KType::Dict(Box::new(KType::join(xk, yk)), Box::new(KType::join(xv, yv)))
            }
            (
                KType::KFunction { args: xa, ret: xr },
                KType::KFunction { args: ya, ret: yr },
            ) if xa.len() == ya.len() => {
                let args = xa.iter().zip(ya.iter()).map(|(x, y)| KType::join(x, y)).collect();
                let ret = Box::new(KType::join(xr, yr));
                KType::KFunction { args, ret }
            }
            _ => KType::Any,
        }
    }

    /// Reduce an iterator of types to their least upper bound. Empty iterator → `Any`.
    pub fn join_iter<I: IntoIterator<Item = KType>>(iter: I) -> KType {
        iter.into_iter().reduce(|a, b| KType::join(&a, &b)).unwrap_or(KType::Any)
    }
}

/// Structural function-type compatibility check. Returns true iff `sig`'s declared parameter
/// types and return type are equal (by KType structural equality) to the slot's expectations.
/// Strict equality, not subtyping — a function declared `(x: Number) -> Str` only fills a slot
/// typed `Function<(Number) -> Str>`, not `Function<(Any) -> Str>`. Subtype-aware function
/// matching (contravariant in args, covariant in ret) is a future refinement.
pub(super) fn function_compat(
    sig: &ExpressionSignature,
    args: &[KType],
    ret: &KType,
) -> bool {
    if sig.return_type != *ret {
        return false;
    }
    let mut i = 0;
    for el in &sig.elements {
        if let SignatureElement::Argument(a) = el {
            if i >= args.len() || a.ktype != args[i] {
                return false;
            }
            i += 1;
        }
    }
    i == args.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::kexpression::{TypeExpr, TypeParams};

    fn leaf(n: &str) -> TypeExpr {
        TypeExpr {
            name: n.into(),
            params: TypeParams::None,
        }
    }

    #[test]
    fn from_type_expr_leaf_number() {
        assert_eq!(KType::from_type_expr(&leaf("Number")).unwrap(), KType::Number);
    }

    #[test]
    fn from_type_expr_list_of_number() {
        let te = TypeExpr {
            name: "List".into(),
            params: TypeParams::List(vec![leaf("Number")]),
        };
        assert_eq!(
            KType::from_type_expr(&te).unwrap(),
            KType::List(Box::new(KType::Number))
        );
    }

    #[test]
    fn from_type_expr_dict_string_number() {
        let te = TypeExpr {
            name: "Dict".into(),
            params: TypeParams::List(vec![leaf("Str"), leaf("Number")]),
        };
        assert_eq!(
            KType::from_type_expr(&te).unwrap(),
            KType::Dict(Box::new(KType::Str), Box::new(KType::Number))
        );
    }

    #[test]
    fn from_type_expr_function_unary() {
        let te = TypeExpr {
            name: "Function".into(),
            params: TypeParams::Function {
                args: vec![leaf("Number")],
                ret: Box::new(leaf("Str")),
            },
        };
        assert_eq!(
            KType::from_type_expr(&te).unwrap(),
            KType::KFunction {
                args: vec![KType::Number],
                ret: Box::new(KType::Str),
            }
        );
    }

    #[test]
    fn from_type_expr_function_nullary() {
        let te = TypeExpr {
            name: "Function".into(),
            params: TypeParams::Function {
                args: vec![],
                ret: Box::new(leaf("Number")),
            },
        };
        assert_eq!(
            KType::from_type_expr(&te).unwrap(),
            KType::KFunction {
                args: vec![],
                ret: Box::new(KType::Number),
            }
        );
    }

    #[test]
    fn from_type_expr_nested_list() {
        let inner = TypeExpr {
            name: "List".into(),
            params: TypeParams::List(vec![leaf("Number")]),
        };
        let te = TypeExpr {
            name: "List".into(),
            params: TypeParams::List(vec![inner]),
        };
        assert_eq!(
            KType::from_type_expr(&te).unwrap(),
            KType::List(Box::new(KType::List(Box::new(KType::Number))))
        );
    }

    #[test]
    fn from_type_expr_list_wrong_arity_errors() {
        let te = TypeExpr {
            name: "List".into(),
            params: TypeParams::List(vec![leaf("A"), leaf("B")]),
        };
        assert!(KType::from_type_expr(&te).is_err());
    }

    #[test]
    fn from_type_expr_dict_wrong_arity_errors() {
        let te = TypeExpr {
            name: "Dict".into(),
            params: TypeParams::List(vec![leaf("Str")]),
        };
        assert!(KType::from_type_expr(&te).is_err());
    }

    #[test]
    fn from_type_expr_unknown_paramless_name_errors() {
        // bare unknown leaf → from_name returns None → error
        assert!(KType::from_type_expr(&leaf("Banana")).is_err());
    }

    #[test]
    fn from_type_expr_unknown_with_params_errors() {
        let te = TypeExpr {
            name: "Banana".into(),
            params: TypeParams::List(vec![leaf("Number")]),
        };
        assert!(KType::from_type_expr(&te).is_err());
    }

    #[test]
    fn from_type_expr_function_arrow_on_non_function_errors() {
        let te = TypeExpr {
            name: "List".into(),
            params: TypeParams::Function {
                args: vec![],
                ret: Box::new(leaf("Number")),
            },
        };
        assert!(KType::from_type_expr(&te).is_err());
    }

    #[test]
    fn from_name_kfunction_no_longer_resolves() {
        // KFunction is no longer surface-declarable — users write Function<(...)-> R> or Any.
        assert_eq!(KType::from_name("KFunction"), None);
    }

    #[test]
    fn from_name_list_lowers_to_list_any() {
        assert_eq!(
            KType::from_name("List"),
            Some(KType::List(Box::new(KType::Any)))
        );
    }

    #[test]
    fn from_name_dict_lowers_to_dict_any_any() {
        assert_eq!(
            KType::from_name("Dict"),
            Some(KType::Dict(Box::new(KType::Any), Box::new(KType::Any)))
        );
    }

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
    fn is_more_specific_concrete_beats_any() {
        assert!(KType::Number.is_more_specific_than(&KType::Any));
        assert!(!KType::Any.is_more_specific_than(&KType::Number));
    }

    #[test]
    fn is_more_specific_list_number_beats_list_any() {
        let n = KType::List(Box::new(KType::Number));
        let a = KType::List(Box::new(KType::Any));
        assert!(n.is_more_specific_than(&a));
        assert!(!a.is_more_specific_than(&n));
    }

    #[test]
    fn is_more_specific_disjoint_lists_incomparable() {
        let n = KType::List(Box::new(KType::Number));
        let s = KType::List(Box::new(KType::Str));
        assert!(!n.is_more_specific_than(&s));
        assert!(!s.is_more_specific_than(&n));
    }

    #[test]
    fn is_more_specific_dict_refines_value() {
        let strict = KType::Dict(Box::new(KType::Str), Box::new(KType::Number));
        let loose = KType::Dict(Box::new(KType::Str), Box::new(KType::Any));
        assert!(strict.is_more_specific_than(&loose));
        assert!(!loose.is_more_specific_than(&strict));
    }

    #[test]
    fn is_more_specific_function_arity_mismatch_incomparable() {
        let unary = KType::KFunction {
            args: vec![KType::Number],
            ret: Box::new(KType::Number),
        };
        let nullary = KType::KFunction {
            args: vec![],
            ret: Box::new(KType::Number),
        };
        assert!(!unary.is_more_specific_than(&nullary));
        assert!(!nullary.is_more_specific_than(&unary));
    }

    #[test]
    fn join_distinct_concretes_yields_any() {
        assert_eq!(KType::join(&KType::Number, &KType::Str), KType::Any);
    }

    #[test]
    fn join_same_yields_same() {
        assert_eq!(KType::join(&KType::Number, &KType::Number), KType::Number);
    }

    #[test]
    fn join_lists_recurses_on_element() {
        let a = KType::List(Box::new(KType::Number));
        let b = KType::List(Box::new(KType::Str));
        assert_eq!(
            KType::join(&a, &b),
            KType::List(Box::new(KType::Any))
        );
    }

    #[test]
    fn join_iter_empty_is_any() {
        let v: Vec<KType> = vec![];
        assert_eq!(KType::join_iter(v), KType::Any);
    }

    #[test]
    fn join_iter_homogeneous() {
        let v = vec![KType::Number, KType::Number, KType::Number];
        assert_eq!(KType::join_iter(v), KType::Number);
    }

    #[test]
    fn join_iter_mixed_yields_any() {
        let v = vec![KType::Number, KType::Str, KType::Bool];
        assert_eq!(KType::join_iter(v), KType::Any);
    }
}
