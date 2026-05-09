//! Folds the parts of a `<...>` group into a structured `TypeExpr`.
//!
//! Two surface shapes share the frame:
//! - List-style: `List<X>`, `Dict<K, V>` — bare type names, folded into `TypeParams::List`.
//! - Function-style: `Function<(args...) -> R>` — one parenthesized arg group, one `->`,
//!   one return type, folded into `TypeParams::Function`.
//!
//! Key invariant: `->` is exclusive to `Function`; every other type rejects it. The
//! `>`-after-`-` rule in `build_tree` keeps `->` contiguous, so a stray `-` or `>` never
//! reaches this frame as separate parts.

use crate::parse::kexpression::{ExpressionPart, TypeExpr, TypeParams};

pub(super) struct TypeFrame<'a> {
    pub(super) name: String,
    pub(super) parts: Vec<ExpressionPart<'a>>,
}

impl<'a> TypeFrame<'a> {
    pub(super) fn new(name: String) -> Self {
        Self { name, parts: Vec::new() }
    }

    pub(super) fn build(self) -> Result<TypeExpr, String> {
        let TypeFrame { name, parts } = self;
        let arrow_idx = find_arrow(&name, &parts)?;
        let is_function = name == "Function";

        match (arrow_idx, is_function) {
            (Some(_), false) => Err(format!(
                "type `{name}<...>` cannot contain `->` — \
                 the arrow is reserved for `Function<(args) -> return>`",
            )),
            (None, true) => Err(
                "type `Function<...>` requires `->` to separate args from the return type \
                 (e.g. `Function<(Number) -> Str>`, or `Function<() -> Str>` for nullary)"
                    .to_string(),
            ),
            (None, false) => build_list_params(name, parts),
            (Some(idx), true) => build_function_params(name, parts, idx),
        }
    }
}

/// Locate the single `Keyword("->")` and reject any non-type, non-paren parts up front so
/// the builder paths don't have to re-walk.
fn find_arrow(name: &str, parts: &[ExpressionPart<'_>]) -> Result<Option<usize>, String> {
    let mut idx: Option<usize> = None;
    for (i, p) in parts.iter().enumerate() {
        match p {
            ExpressionPart::Type(_) | ExpressionPart::Expression(_) => {}
            ExpressionPart::Keyword(s) if s == "->" => {
                if idx.is_some() {
                    return Err(format!(
                        "type `{name}<...>` has more than one `->` arrow",
                    ));
                }
                idx = Some(i);
            }
            other => {
                return Err(format!(
                    "type `{name}<...>` parameter must be a type name, got `{}`",
                    other.summarize(),
                ));
            }
        }
    }
    Ok(idx)
}

/// Parenthesized groups are rejected here so users can't sneak a function-style arg list
/// into a non-function type.
fn build_list_params<'a>(
    name: String,
    parts: Vec<ExpressionPart<'a>>,
) -> Result<TypeExpr, String> {
    let params = parts
        .into_iter()
        .map(|p| match p {
            ExpressionPart::Type(t) => Ok(t),
            other => Err(format!(
                "type `{name}<...>` parameter must be a type name, got `{}`",
                other.summarize(),
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TypeExpr {
        name,
        params: TypeParams::List(params),
    })
}

fn build_function_params<'a>(
    name: String,
    parts: Vec<ExpressionPart<'a>>,
    arrow_idx: usize,
) -> Result<TypeExpr, String> {
    let mut iter = parts.into_iter();
    let before: Vec<ExpressionPart<'a>> = (&mut iter).take(arrow_idx).collect();
    iter.next();
    let after: Vec<ExpressionPart<'a>> = iter.collect();

    let args = extract_function_args(before)?;
    let ret = extract_function_return(after)?;
    Ok(TypeExpr {
        name,
        params: TypeParams::Function {
            args,
            ret: Box::new(ret),
        },
    })
}

/// `Function`'s args always live inside a single parenthesized group. Every malformed
/// shape (loose Type tokens, multiple expression groups, missing parens) folds into the
/// same error so the user gets one consistent fix-it message.
fn extract_function_args<'a>(
    before: Vec<ExpressionPart<'a>>,
) -> Result<Vec<TypeExpr>, String> {
    const MISSING_PARENS: &str =
        "type `Function<...>` args must be parenthesized: \
         `Function<(arg1, arg2, ...) -> R>` (use `Function<() -> R>` for nullary)";

    if before.len() != 1 {
        return Err(MISSING_PARENS.to_string());
    }
    let arg_parts = match before.into_iter().next().unwrap() {
        ExpressionPart::Expression(boxed) => boxed.parts,
        _ => return Err(MISSING_PARENS.to_string()),
    };
    arg_parts
        .into_iter()
        .map(|p| match p {
            ExpressionPart::Type(t) => Ok(t),
            other => Err(format!(
                "type `Function<(...)>` arg must be a type name, got `{}`",
                other.summarize()
            )),
        })
        .collect()
}

fn extract_function_return<'a>(
    after: Vec<ExpressionPart<'a>>,
) -> Result<TypeExpr, String> {
    if after.len() != 1 {
        return Err(format!(
            "type `Function<... -> R>` needs exactly one return type after the arrow, got {}",
            after.len()
        ));
    }
    match after.into_iter().next().unwrap() {
        ExpressionPart::Type(t) => Ok(t),
        other => Err(format!(
            "type `Function<... -> R>` return type must be a type name, got `{}`",
            other.summarize()
        )),
    }
}
