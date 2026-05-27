//! Generic-destructuring unifier for per-call type-parameter binding.
//!
//! Walks a declared parameter slot's surface [`TypeExpr`] against the runtime value's
//! carried [`KType`], binding free type-parameter *names* (`T`, `E`) to the concrete
//! subtypes they line up with. The bindings feed [`crate::machine::core::Scope::register_type`]
//! in the per-call child scope, so a body reference to `:T` / a deferred return `-> :T`
//! resolves against the value the call actually carried — enabling
//! `FN head (xs :(List T)) -> :T = ...` and `FN unwrap (r :(Result T E)) -> :T = ...`.
//!
//! There is no `KType::TypeParam` variant (decided in
//! [runtime-type-parameter-carriers](../../../../roadmap/type_language/type-parameter-binding.md)):
//! type parameters stay ordinary scope-resolved names. The unifier identifies a leaf as a
//! parameter by membership in the caller-supplied `params` set rather than by a KType tag.

use std::collections::HashSet;

use crate::machine::model::ast::{TypeExpr, TypeParams};
use super::ktype::KType;

/// Outcome of a unification walk.
#[derive(Debug, PartialEq)]
pub enum UnifyResult<'a> {
    /// Every structural position lined up; `bindings` are the `(param_name, concrete)`
    /// pairs to register into the per-call type scope. A parameter that appears in more
    /// than one position must bind consistently — a conflicting second occurrence is a
    /// `Mismatch`.
    Bound(Vec<(String, KType<'a>)>),
    /// The declared shape and the value's carried type don't line up (different head
    /// constructor, wrong arity, or a parameter bound to two incompatible types).
    Mismatch(String),
}

/// Unify `declared` (surface slot type, e.g. `:(List T)`) against `actual` (the value's
/// carried `KType`, e.g. `List<Number>`), collecting bindings for every name in `params`.
///
/// Non-parameter leaves are checked for structural agreement but produce no binding; a
/// parameter leaf binds to whatever `actual` subtype occupies its position. Container and
/// constructor shapes recurse position-wise. Arity or head-constructor disagreement is a
/// `Mismatch`.
pub fn unify_slot<'a>(
    declared: &TypeExpr,
    actual: &KType<'a>,
    params: &HashSet<String>,
) -> UnifyResult<'a> {
    let mut out: Vec<(String, KType<'a>)> = Vec::new();
    match walk(declared, actual, params, &mut out) {
        Ok(()) => UnifyResult::Bound(out),
        Err(msg) => UnifyResult::Mismatch(msg),
    }
}

fn walk<'a>(
    declared: &TypeExpr,
    actual: &KType<'a>,
    params: &HashSet<String>,
    out: &mut Vec<(String, KType<'a>)>,
) -> Result<(), String> {
    match (&declared.name, &declared.params) {
        // Bare leaf: a parameter name binds to `actual`; any other leaf must already
        // line up structurally (its elaborated form equals `actual`), which the caller
        // has typically checked via `matches_value` — here we only bind params.
        (name, TypeParams::None) => {
            if params.contains(name) {
                bind(name, actual.clone(), out)
            } else {
                // Non-parameter leaf: nothing to bind. Disagreement is the caller's
                // concern (it ran `matches_value`); accept so a concrete-leaf slot like
                // `:(LIST OF Number)` doesn't spuriously fail the unifier.
                Ok(())
            }
        }
        (name, TypeParams::List(items)) if name == "List" && items.len() == 1 => match actual {
            KType::List(elem) => walk(&items[0], elem, params, out),
            other => Err(format!(
                "declared `:(LIST OF _)` but value carries `{}`",
                other.name()
            )),
        },
        (name, TypeParams::List(items)) if name == "Dict" && items.len() == 2 => match actual {
            KType::Dict(k, v) => {
                walk(&items[0], k, params, out)?;
                walk(&items[1], v, params, out)
            }
            other => Err(format!(
                "declared `:(MAP _ -> _)` but value carries `{}`",
                other.name()
            )),
        },
        // Applied constructor (`:(Result T E)`): recurse position-wise into the carried
        // `ConstructorApply` args. Head-constructor identity is checked by name (the
        // elaborated `ctor` renders its declared name); arity must match.
        (name, TypeParams::List(items)) => match actual {
            KType::ConstructorApply { ctor, args } if ctor.name() == *name => {
                if items.len() != args.len() {
                    return Err(format!(
                        "constructor `{name}` applied to {} arg(s) but value carries {}",
                        items.len(),
                        args.len()
                    ));
                }
                for (decl_arg, act_arg) in items.iter().zip(args.iter()) {
                    walk(decl_arg, act_arg, params, out)?;
                }
                Ok(())
            }
            other => Err(format!(
                "declared `:({name} ...)` but value carries `{}`",
                other.name()
            )),
        },
        // Function arrow forms aren't a parameterized *value* carrier today; no binding.
        (_, TypeParams::Function { .. }) => Ok(()),
    }
}

fn bind<'a>(name: &str, ty: KType<'a>, out: &mut Vec<(String, KType<'a>)>) -> Result<(), String> {
    if let Some((_, existing)) = out.iter().find(|(n, _)| n == name) {
        if *existing != ty {
            return Err(format!(
                "type parameter `{name}` bound to both `{}` and `{}`",
                existing.name(),
                ty.name()
            ));
        }
        return Ok(());
    }
    out.push((name.to_string(), ty));
    Ok(())
}

#[cfg(test)]
mod tests;
