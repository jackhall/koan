//! Surface-name and `TypeExpr` → `KType` elaboration, plus join (LUB) for inferring
//! container element types from heterogeneous values.

use super::ktype::{KType, UserTypeKind};
use crate::machine::model::ast::{TypeExpr, TypeParams};

impl<'a> KType<'a> {
    /// Look up a `KType` by the textual name a user can write in source (e.g. `Number`,
    /// `List`). Returns `None` for unknown names.
    ///
    /// Built at the caller's `'a` directly because `KType<'a>` is invariant in `'a`
    /// (the `Module.type_members: RefCell<HashMap<_, KType<'a>>>` field puts `'a` in
    /// invariant position), so covariant coercion from `'static` is unavailable.
    pub fn from_name(name: &str) -> Option<KType<'a>> {
        match name {
            "Number" => Some(KType::Number),
            "Str" => Some(KType::Str),
            "Bool" => Some(KType::Bool),
            "Null" => Some(KType::Null),
            "List" => Some(KType::List(Box::new(KType::Any))),
            "Dict" => Some(KType::Dict(Box::new(KType::Any), Box::new(KType::Any))),
            "KExpression" => Some(KType::KExpression),
            "Type" => Some(KType::Type),
            "Tagged" => Some(KType::AnyUserType { kind: UserTypeKind::Tagged }),
            "Struct" => Some(KType::AnyUserType { kind: UserTypeKind::Struct }),
            "Module" => Some(KType::AnyModule),
            "Signature" => Some(KType::AnySignature),
            "Any" => Some(KType::Any),
            _ => None,
        }
    }

    /// Lower a parser `TypeExpr` into a `KType` against the builtin table only — no
    /// scope-aware resolver. Recurses through container shapes; each leaf goes through
    /// [`KType::from_name`]. Unknown leaves surface as `Err(_)`; the caller either falls
    /// back to a `TypeNameRef` carrier or routes through the scheduler-aware
    /// [`crate::machine::model::types::elaborate_type_expr`].
    pub fn from_type_expr(t: &TypeExpr) -> Result<KType<'a>, String> {
        match (t.name.as_str(), &t.params) {
            (_, TypeParams::None) => KType::from_name(&t.name)
                .ok_or_else(|| format!("unknown type name `{}`", t.name)),
            ("List", TypeParams::List(items)) if items.len() == 1 => {
                Ok(KType::List(Box::new(KType::from_type_expr(&items[0])?)))
            }
            ("List", TypeParams::List(items)) => Err(format!(
                ":(List ...) expects exactly 1 type parameter, got {}",
                items.len()
            )),
            ("Dict", TypeParams::List(items)) if items.len() == 2 => Ok(KType::Dict(
                Box::new(KType::from_type_expr(&items[0])?),
                Box::new(KType::from_type_expr(&items[1])?),
            )),
            ("Dict", TypeParams::List(items)) => Err(format!(
                ":(Dict ...) expects exactly 2 type parameters, got {}",
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
            ("Functor", TypeParams::Function { args, ret }) => {
                let params = args
                    .iter()
                    .map(KType::from_type_expr)
                    .collect::<Result<Vec<_>, _>>()?;
                let ret = Box::new(KType::from_type_expr(ret)?);
                Ok(KType::KFunctor { params, ret })
            }
            (_, TypeParams::List(_)) => {
                Err(format!("type `{}` does not take type parameters", t.name))
            }
            (_, TypeParams::Function { .. }) => Err(format!(
                "only `Function` / `Functor` accept a `(args) -> ret` shape; got `{}`",
                t.name
            )),
        }
    }

    /// Least-upper-bound of two types. `[1, 2]` → `List<Number>`, `[1, "x"]` →
    /// `List<Any>`; nested containers join element-wise.
    pub fn join(a: &KType<'a>, b: &KType<'a>) -> KType<'a> {
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
    pub fn join_iter<I: IntoIterator<Item = KType<'a>>>(iter: I) -> KType<'a> {
        iter.into_iter().reduce(|a, b| KType::join(&a, &b)).unwrap_or(KType::Any)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(n: &str) -> TypeExpr {
        TypeExpr::leaf(n.into())
    }

    fn list_typeexpr(name: &str, items: Vec<TypeExpr>) -> TypeExpr {
        TypeExpr {
            name: name.into(),
            params: TypeParams::List(items),
            builtin_cache: std::cell::OnceCell::new(),
        }
    }

    fn function_typeexpr(args: Vec<TypeExpr>, ret: TypeExpr) -> TypeExpr {
        TypeExpr {
            name: "Function".into(),
            params: TypeParams::Function { args, ret: Box::new(ret) },
            builtin_cache: std::cell::OnceCell::new(),
        }
    }

    #[test]
    fn from_type_expr_leaf_number() {
        assert_eq!(KType::from_type_expr(&leaf("Number")).unwrap(), KType::Number);
    }

    #[test]
    fn from_type_expr_list_of_number() {
        let te = list_typeexpr("List", vec![leaf("Number")]);
        assert_eq!(
            KType::from_type_expr(&te).unwrap(),
            KType::List(Box::new(KType::Number))
        );
    }

    #[test]
    fn from_type_expr_dict_string_number() {
        let te = list_typeexpr("Dict", vec![leaf("Str"), leaf("Number")]);
        assert_eq!(
            KType::from_type_expr(&te).unwrap(),
            KType::Dict(Box::new(KType::Str), Box::new(KType::Number))
        );
    }

    #[test]
    fn from_type_expr_function_unary() {
        let te = function_typeexpr(vec![leaf("Number")], leaf("Str"));
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
        let te = function_typeexpr(vec![], leaf("Number"));
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
        let inner = list_typeexpr("List", vec![leaf("Number")]);
        let te = list_typeexpr("List", vec![inner]);
        assert_eq!(
            KType::from_type_expr(&te).unwrap(),
            KType::List(Box::new(KType::List(Box::new(KType::Number))))
        );
    }

    #[test]
    fn from_type_expr_list_wrong_arity_errors() {
        let te = list_typeexpr("List", vec![leaf("A"), leaf("B")]);
        assert!(KType::from_type_expr(&te).is_err());
    }

    #[test]
    fn from_type_expr_dict_wrong_arity_errors() {
        let te = list_typeexpr("Dict", vec![leaf("Str")]);
        assert!(KType::from_type_expr(&te).is_err());
    }

    #[test]
    fn from_type_expr_unknown_paramless_name_errors() {
        assert!(KType::from_type_expr(&leaf("Banana")).is_err());
    }

    #[test]
    fn from_type_expr_unknown_with_params_errors() {
        let te = list_typeexpr("Banana", vec![leaf("Number")]);
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
            builtin_cache: std::cell::OnceCell::new(),
        };
        assert!(KType::from_type_expr(&te).is_err());
    }

    #[test]
    fn from_type_expr_leaf_falls_through_to_builtin() {
        assert_eq!(
            KType::from_type_expr(&leaf("Number")).unwrap(),
            KType::Number,
        );
    }

    #[test]
    fn from_name_kfunction_no_longer_resolves() {
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
