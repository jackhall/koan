//! Surface-name and `TypeIdentifier` → `KType` elaboration, plus join (LUB) for inferring
//! container element types from heterogeneous values.

use super::kkind::KKind;
use super::ktype::KType;
use super::record::Record;
use crate::machine::model::ast::TypeIdentifier;

impl KType {
    /// Look up a `KType` by the textual name a user can write in source (e.g. `Number`,
    /// `List`).
    pub fn from_name(name: &str) -> Option<KType> {
        match name {
            "Number" => Some(KType::Number),
            "Str" => Some(KType::Str),
            "Bool" => Some(KType::Bool),
            "Null" => Some(KType::Null),
            "List" => Some(KType::list(Box::new(KType::Any))),
            "Dict" => Some(KType::dict(Box::new(KType::Any), Box::new(KType::Any))),
            "KExpression" => Some(KType::KExpression),
            "Type" => Some(KType::OfKind(KKind::AnyType)),
            "Module" => Some(KType::empty_signature()),
            "Signature" => Some(KType::OfKind(KKind::Signature)),
            "Any" => Some(KType::Any),
            _ => None,
        }
    }

    /// Lower a parser `TypeIdentifier` into a `KType` against the builtin table only — no
    /// scope-aware resolver. The single entry point onto the [`KType::from_name`]
    /// builtin-table fallback: both the bind-time scopeless caller and the scope-aware
    /// [`elaborate_type_identifier`](crate::machine::model::types::elaborate_type_identifier)
    /// route their builtin fallback through here. Unknown names surface as `Err(_)`.
    pub fn from_type_identifier(t: &TypeIdentifier) -> Result<KType, String> {
        KType::from_name(t.as_str()).ok_or_else(|| format!("unknown type name `{}`", t.as_str()))
    }

    /// Canonicalizing constructor for [`KType::Union`] — the single entry point that builds a
    /// union. Flattens any nested `Union` member into its members, deduplicates by `PartialEq`
    /// (O(n²) scan; member counts are small), and collapses a single surviving member to that
    /// member (`:(A | A)` is `:A`). Callers guarantee at least one member.
    pub fn union_of(members: Vec<KType>) -> KType {
        debug_assert!(!members.is_empty(), "union_of requires at least one member");
        let mut flat: Vec<KType> = Vec::with_capacity(members.len());
        let push_unique = |m: KType, flat: &mut Vec<KType>| {
            if !flat.contains(&m) {
                flat.push(m);
            }
        };
        for m in members {
            match m {
                KType::Union { members: inner, .. } => {
                    for i in inner {
                        push_unique(i, &mut flat);
                    }
                }
                other => push_unique(other, &mut flat),
            }
        }
        if flat.len() == 1 {
            return flat.pop().unwrap();
        }
        let digest = super::type_digest::union_digest(&flat);
        KType::Union {
            members: flat,
            digest,
        }
    }

    /// Least-upper-bound of two types. `[1, 2]` → `List<Number>`, `[1, "x"]` →
    /// `List<Any>`; nested containers join element-wise.
    pub fn join(a: &KType, b: &KType) -> KType {
        if a == b {
            return a.clone();
        }
        match (a, b) {
            (KType::List { element: x, .. }, KType::List { element: y, .. }) => {
                KType::list(Box::new(KType::join(x, y)))
            }
            (
                KType::Dict {
                    key: xk, value: xv, ..
                },
                KType::Dict {
                    key: yk, value: yv, ..
                },
            ) => KType::dict(Box::new(KType::join(xk, yk)), Box::new(KType::join(xv, yv))),
            (
                KType::KFunction {
                    params: xa,
                    ret: xr,
                    ..
                },
                KType::KFunction {
                    params: ya,
                    ret: yr,
                    ..
                },
            ) => match join_param_record(xa, ya) {
                Some(params) => KType::function_type(params, Box::new(KType::join(xr, yr))),
                None => KType::Any,
            },
            _ => KType::Any,
        }
    }

    /// Reduce an iterator of types to their least upper bound. Empty iterator → `Any`.
    pub fn join_iter<I: IntoIterator<Item = KType>>(iter: I) -> KType {
        iter.into_iter()
            .reduce(|a, b| KType::join(&a, &b))
            .unwrap_or(KType::Any)
    }
}

/// Name-keyed join of two parameter records. `Some(joined)` when the records have equal
/// length and the same key set; `None` on differing key sets, which callers coarsen to
/// `KType::Any`.
fn join_param_record(xa: &Record<KType>, ya: &Record<KType>) -> Option<Record<KType>> {
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
