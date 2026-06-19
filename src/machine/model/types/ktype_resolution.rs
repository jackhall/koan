//! Surface-name and `TypeIdentifier` → `KType` elaboration, plus join (LUB) for inferring
//! container element types from heterogeneous values.

use super::kkind::KKind;
use super::ktype::KType;
use super::record::Record;
use crate::machine::model::ast::TypeIdentifier;

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
            "Type" => Some(KType::OfKind(KKind::AnyType)),
            "Module" => Some(KType::OfKind(KKind::Module)),
            "Signature" => Some(KType::OfKind(KKind::Signature)),
            "Any" => Some(KType::Any),
            _ => None,
        }
    }

    /// Lower a parser `TypeIdentifier` into a `KType` against the builtin table only — no
    /// scope-aware resolver. The leaf name goes through [`KType::from_name`]; unknown
    /// names surface as `Err(_)`, and the caller either falls back to a `KType::Unresolved`
    /// carrier or routes through the scheduler-aware
    /// [`crate::machine::model::types::elaborate_type_identifier`].
    pub fn from_type_identifier(t: &TypeIdentifier) -> Result<KType<'a>, String> {
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
            // Name-keyed join: equal length and the same key set, then join per name and
            // on the return type. Mismatched key sets fall through to `Any` (the `_` arm).
            // `KFunction` and `KFunctor` share the join shape (`join_param_record`) but
            // stay tag-matched — a function and a functor never join to either family.
            (
                KType::KFunction {
                    params: xa,
                    ret: xr,
                },
                KType::KFunction {
                    params: ya,
                    ret: yr,
                },
            ) => match join_param_record(xa, ya) {
                Some(params) => KType::KFunction {
                    params,
                    ret: Box::new(KType::join(xr, yr)),
                },
                None => KType::Any,
            },
            (
                KType::KFunctor {
                    params: xa,
                    ret: xr,
                    ..
                },
                KType::KFunctor {
                    params: ya,
                    ret: yr,
                    ..
                },
            ) => match join_param_record(xa, ya) {
                // A join is an anonymous type result with no callable body.
                Some(params) => KType::KFunctor {
                    params,
                    ret: Box::new(KType::join(xr, yr)),
                    body: None,
                },
                None => KType::Any,
            },
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

/// Name-keyed join of two parameter records, shared by the `KFunction` / `KFunctor`
/// join arms. Returns `Some(joined)` when the records have equal length and the same key
/// set (joining per name); `None` when the key sets differ, which the callers coarsen to
/// `KType::Any`.
fn join_param_record<'a>(
    xa: &Record<KType<'a>>,
    ya: &Record<KType<'a>>,
) -> Option<Record<KType<'a>>> {
    if xa.len() != ya.len() || !xa.keys().all(|k| ya.get(k).is_some()) {
        return None;
    }
    Some(
        xa.iter()
            .map(|(name, x)| (name.clone(), KType::join(x, ya.get(name).unwrap())))
            .collect(),
    )
}

#[cfg(test)]
mod tests;
