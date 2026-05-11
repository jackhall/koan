//! Surface-name and `TypeExpr` → `KType` elaboration, plus join (LUB) for inferring
//! container element types from heterogeneous values. The user-facing entry points for
//! turning parsed type syntax into a `KType` live here, alongside the join used by
//! `KObject::ktype` to infer container element types.

use super::ktype::KType;
use super::resolver::TypeResolver;
use crate::parse::{TypeExpr, TypeParams};

impl KType {
    /// Look up a `KType` by the textual name a user can write in source (e.g. `Number`,
    /// `List`). Returns `None` for unknown names. `Identifier`, `TypeExprRef` are
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
            "Module" => Some(KType::Module),
            "Signature" => Some(KType::Signature),
            "Any" => Some(KType::Any),
            _ => None,
        }
    }

    /// Convert a parser `TypeExpr` into a `KType`. This is the surface-level type-parsing
    /// boundary used by FN signatures, FN return-type slots, and UNION/STRUCT field types.
    /// Recurses on nested type parameters; arity for the known containers is enforced here so
    /// errors surface at FN-definition time rather than at first call.
    ///
    /// Resolution precedence: `resolver.resolve(name)` first (so user-defined / module-local
    /// types can shadow builtins), then `from_name`'s builtin table. The resolver is
    /// consulted only for paramless names — parameterized container types (`List`, `Dict`,
    /// `Function`) are still routed through their structural arms because user code can't
    /// re-define those.
    pub fn from_type_expr(
        t: &TypeExpr,
        resolver: &dyn TypeResolver,
    ) -> Result<KType, String> {
        match (t.name.as_str(), &t.params) {
            (_, TypeParams::None) => {
                if let Some(t) = resolver.resolve(&t.name) {
                    return Ok(t);
                }
                KType::from_name(&t.name)
                    .ok_or_else(|| format!("unknown type name `{}`", t.name))
            }
            ("List", TypeParams::List(items)) if items.len() == 1 => {
                Ok(KType::List(Box::new(KType::from_type_expr(&items[0], resolver)?)))
            }
            ("List", TypeParams::List(items)) => Err(format!(
                "List<...> expects exactly 1 type parameter, got {}",
                items.len()
            )),
            ("Dict", TypeParams::List(items)) if items.len() == 2 => Ok(KType::Dict(
                Box::new(KType::from_type_expr(&items[0], resolver)?),
                Box::new(KType::from_type_expr(&items[1], resolver)?),
            )),
            ("Dict", TypeParams::List(items)) => Err(format!(
                "Dict<...> expects exactly 2 type parameters, got {}",
                items.len()
            )),
            ("Function", TypeParams::Function { args, ret }) => {
                let args = args
                    .iter()
                    .map(|t| KType::from_type_expr(t, resolver))
                    .collect::<Result<Vec<_>, _>>()?;
                let ret = Box::new(KType::from_type_expr(ret, resolver)?);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::NoopResolver;

    fn leaf(n: &str) -> TypeExpr {
        TypeExpr {
            name: n.into(),
            params: TypeParams::None,
        }
    }

    #[test]
    fn from_type_expr_leaf_number() {
        assert_eq!(KType::from_type_expr(&leaf("Number"), &NoopResolver).unwrap(), KType::Number);
    }

    #[test]
    fn from_type_expr_list_of_number() {
        let te = TypeExpr {
            name: "List".into(),
            params: TypeParams::List(vec![leaf("Number")]),
        };
        assert_eq!(
            KType::from_type_expr(&te, &NoopResolver).unwrap(),
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
            KType::from_type_expr(&te, &NoopResolver).unwrap(),
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
            KType::from_type_expr(&te, &NoopResolver).unwrap(),
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
            KType::from_type_expr(&te, &NoopResolver).unwrap(),
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
            KType::from_type_expr(&te, &NoopResolver).unwrap(),
            KType::List(Box::new(KType::List(Box::new(KType::Number))))
        );
    }

    #[test]
    fn from_type_expr_list_wrong_arity_errors() {
        let te = TypeExpr {
            name: "List".into(),
            params: TypeParams::List(vec![leaf("A"), leaf("B")]),
        };
        assert!(KType::from_type_expr(&te, &NoopResolver).is_err());
    }

    #[test]
    fn from_type_expr_dict_wrong_arity_errors() {
        let te = TypeExpr {
            name: "Dict".into(),
            params: TypeParams::List(vec![leaf("Str")]),
        };
        assert!(KType::from_type_expr(&te, &NoopResolver).is_err());
    }

    #[test]
    fn from_type_expr_unknown_paramless_name_errors() {
        // bare unknown leaf → from_name returns None → error
        assert!(KType::from_type_expr(&leaf("Banana"), &NoopResolver).is_err());
    }

    #[test]
    fn from_type_expr_unknown_with_params_errors() {
        let te = TypeExpr {
            name: "Banana".into(),
            params: TypeParams::List(vec![leaf("Number")]),
        };
        assert!(KType::from_type_expr(&te, &NoopResolver).is_err());
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
        assert!(KType::from_type_expr(&te, &NoopResolver).is_err());
    }

    /// Resolver-first precedence: a name registered with the resolver wins over a builtin
    /// of the same name. Stage 1 will rely on this so module-local types can shadow
    /// builtins without needing to thread the choice through every call site.
    #[test]
    fn from_type_expr_resolver_shadows_builtin() {
        struct AlwaysStr;
        impl TypeResolver for AlwaysStr {
            fn resolve(&self, _name: &str) -> Option<KType> {
                Some(KType::Str)
            }
        }
        // `Number` is a builtin; the resolver returning `Str` must win.
        assert_eq!(
            KType::from_type_expr(&leaf("Number"), &AlwaysStr).unwrap(),
            KType::Str,
        );
    }

    /// Resolver returning `None` falls through to the builtin table.
    #[test]
    fn from_type_expr_resolver_none_falls_through_to_builtin() {
        assert_eq!(
            KType::from_type_expr(&leaf("Number"), &NoopResolver).unwrap(),
            KType::Number,
        );
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
