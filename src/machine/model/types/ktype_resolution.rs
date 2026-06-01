//! Surface-name and `TypeName` → `KType` elaboration, plus join (LUB) for inferring
//! container element types from heterogeneous values.

use super::ktype::{KType, UserTypeKind};
use crate::machine::model::ast::TypeName;

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
            "Tagged" => Some(KType::AnyUserType {
                kind: UserTypeKind::tagged_sentinel(),
            }),
            "Struct" => Some(KType::AnyUserType {
                kind: UserTypeKind::struct_sentinel(),
            }),
            "Module" => Some(KType::AnyModule),
            "Signature" => Some(KType::AnySignature),
            "Any" => Some(KType::Any),
            _ => None,
        }
    }

    /// Lower a parser `TypeName` into a `KType` against the builtin table only — no
    /// scope-aware resolver. The leaf name goes through [`KType::from_name`]; unknown
    /// names surface as `Err(_)`, and the caller either falls back to a `TypeNameRef`
    /// carrier or routes through the scheduler-aware
    /// [`crate::machine::model::types::elaborate_type_expr`].
    pub fn from_type_expr(t: &TypeName) -> Result<KType<'a>, String> {
        KType::from_name(t.as_str()).ok_or_else(|| format!("unknown type name `{}`", t.as_str()))
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
            (KType::KFunction { args: xa, ret: xr }, KType::KFunction { args: ya, ret: yr })
                if xa.len() == ya.len() =>
            {
                let args = xa
                    .iter()
                    .zip(ya.iter())
                    .map(|(x, y)| KType::join(x, y))
                    .collect();
                let ret = Box::new(KType::join(xr, yr));
                KType::KFunction { args, ret }
            }
            _ => KType::Any,
        }
    }

    /// Reduce an iterator of types to their least upper bound. Empty iterator → `Any`.
    pub fn join_iter<I: IntoIterator<Item = KType<'a>>>(iter: I) -> KType<'a> {
        iter.into_iter()
            .reduce(|a, b| KType::join(&a, &b))
            .unwrap_or(KType::Any)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(n: &str) -> TypeName {
        TypeName::leaf(n.into())
    }

    #[test]
    fn from_type_expr_leaf_number() {
        assert_eq!(
            KType::from_type_expr(&leaf("Number")).unwrap(),
            KType::Number
        );
    }

    #[test]
    fn from_type_expr_unknown_paramless_name_errors() {
        assert!(KType::from_type_expr(&leaf("Banana")).is_err());
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
        assert_eq!(KType::join(&a, &b), KType::List(Box::new(KType::Any)));
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
