//! Type-parameter sub-state factored out of `expression_tree::build_tree`. Owns the inner
//! parts of a `<...>` group and folds them into a structured `TypeExpr` on close. Mirrors
//! the `dict_literal::DictFrame` split: `build_tree` opens / closes a `Frame::Type` on `<`
//! and `>`, and delegates the validation+folding work here so the main char loop stays
//! focused on framing.
//!
//! Two surface shapes share the frame:
//! - List-style: `List<X>`, `Dict<K, V>` — bare type names separated by whitespace or
//!   commas, folded into `TypeParams::List`.
//! - Function-style: `Function<(args...) -> R>` — a single parenthesized arg group plus
//!   exactly one `->` arrow plus a return type, folded into `TypeParams::Function`. The
//!   arrow is exclusive to `Function`; every other type rejects it.
//!
//! The `>`-after-`-` rule in `build_tree` keeps `->` contiguous so we never see a stray
//! `-` or `>` as separate parts here.

use crate::parse::kexpression::{ExpressionPart, TypeExpr, TypeParams};

/// In-progress `<...>` group. Holds the type name (popped off the parent frame when `<`
/// opens) and the parts collected until the matching `>`. The character loop in
/// `expression_tree::build_tree` pushes parts via `Frame::push`; `build()` consumes the
/// frame to produce a structured `TypeExpr`.
pub(super) struct TypeFrame<'a> {
    pub(super) name: String,
    pub(super) parts: Vec<ExpressionPart<'a>>,
}

impl<'a> TypeFrame<'a> {
    pub(super) fn new(name: String) -> Self {
        Self { name, parts: Vec::new() }
    }

    /// Fold the collected parts into a structured `TypeExpr`. The dispatch is a 4-way
    /// case on `(arrow_present, is_function)`; each case routes to a focused helper. The
    /// arrow is exclusive to `Function`, so the two diagonal cases (Function-without-arrow
    /// and non-Function-with-arrow) are errors with hint text.
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

/// Walk the parts and locate the single `Keyword("->")`. Validates that every part is one
/// of the shapes the two builder paths expect: bare type names, parenthesized expressions
/// (only meaningful in the `Function` arg group), and at most one arrow. Anything else is
/// a shape error reported here so the builder paths don't have to re-walk.
fn find_arrow(name: &str, parts: &[ExpressionPart<'_>]) -> Result<Option<usize>, String> {
    let mut idx: Option<usize> = None;
    for (i, p) in parts.iter().enumerate() {
        match p {
            // Bare type or parenthesized group — both are legitimate here; per-shape
            // validation happens in build_list_params / build_function_params.
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

/// Build the no-arrow case: `List<X>`, `Dict<K, V>`, etc. Every part must be a bare
/// `Type(_)` — parenthesized groups are rejected here so users can't sneak a function-
/// style arg list into a non-function type.
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

/// Build the arrow case for `Function`: split parts at the arrow, extract args from the
/// parenthesized group on the left, extract the single return type on the right.
fn build_function_params<'a>(
    name: String,
    parts: Vec<ExpressionPart<'a>>,
    arrow_idx: usize,
) -> Result<TypeExpr, String> {
    let mut iter = parts.into_iter();
    let before: Vec<ExpressionPart<'a>> = (&mut iter).take(arrow_idx).collect();
    iter.next(); // consume the arrow
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

/// Pull the arg types out of the pre-arrow section. `Function`'s args ALWAYS live inside
/// a single parenthesized group — `Function<(arg1, arg2) -> R>`, with `Function<() -> R>`
/// for nullary. Any other shape (loose Type tokens, multiple expression groups, missing
/// parens) hits the same error so the user gets one consistent fix-it message.
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
        ExpressionPart::Expression(boxed) => (*boxed).parts,
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

/// Pull the return type out of the post-arrow section. Exactly one `Type` part is
/// expected; zero (a stray trailing arrow) or more than one (a second sneaky type) is an
/// error.
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
