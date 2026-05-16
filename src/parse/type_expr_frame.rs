//! Folds the parts of a `:(...)` type-expression group into a structured `TypeExpr`.
//!
//! Mirrors [`super::type_frame::TypeFrame`] but reads from S-expression paren shape
//! instead of `<>` shape. Three surface shapes share the frame:
//! - Bare with no params: `:(Number)` — single Type part, builds as `TypeParams::None`.
//! - List-style: `:(List Number)`, `:(Dict K V)` — head Type plus 1+ Type args,
//!   folded into `TypeParams::List`.
//! - Function-style: `:(Function (Number Str) -> Bool)` — head must be `Function`,
//!   one parenthesized arg group, one `->`, one return type, folded into
//!   `TypeParams::Function`.

use crate::runtime::machine::model::ast::{ExpressionPart, TypeExpr, TypeParams};

pub(super) struct TypeExprFrame<'a> {
    pub(super) parts: Vec<ExpressionPart<'a>>,
}

impl<'a> TypeExprFrame<'a> {
    pub(super) fn new() -> Self {
        Self { parts: Vec::new() }
    }

    pub(super) fn build(self) -> Result<TypeExpr, String> {
        let TypeExprFrame { parts } = self;
        if parts.is_empty() {
            return Err(
                "empty `:(...)` type expression — write `:(<TypeName>)` or `:(<TypeName> <args...>)`"
                    .to_string(),
            );
        }
        let head_name = match &parts[0] {
            ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => t.name.clone(),
            ExpressionPart::Type(t) => {
                return Err(format!(
                    "type-expression head must be a bare type name, got `{}`",
                    t.render(),
                ));
            }
            other => {
                return Err(format!(
                    "type-expression head must be a type name, got `{}`",
                    other.summarize(),
                ));
            }
        };
        let rest = &parts[1..];
        let arrow_idx = find_arrow(&head_name, rest)?;
        let is_function = head_name == "Function";

        match (arrow_idx, is_function) {
            (Some(_), false) => Err(format!(
                "type `:({head_name} ...)` cannot contain `->` — \
                 the arrow is reserved for `:(Function (args) -> return)`",
            )),
            (None, true) => Err(
                "type `:(Function ...)` requires `->` to separate args from the return type \
                 (e.g. `:(Function (Number) -> Str)`, or `:(Function () -> Str)` for nullary)"
                    .to_string(),
            ),
            (None, false) => build_list_params(head_name, rest.to_vec()),
            (Some(idx), true) => build_function_params(head_name, rest.to_vec(), idx),
        }
    }
}

/// Locate the single `Keyword("->")` and reject any non-type, non-paren parts up front so
/// the builder paths don't have to re-walk.
fn find_arrow(head: &str, rest: &[ExpressionPart<'_>]) -> Result<Option<usize>, String> {
    let mut idx: Option<usize> = None;
    for (i, p) in rest.iter().enumerate() {
        match p {
            ExpressionPart::Type(_) | ExpressionPart::Expression(_) => {}
            ExpressionPart::Keyword(s) if s == "->" => {
                if idx.is_some() {
                    return Err(format!(
                        "type `:({head} ...)` has more than one `->` arrow",
                    ));
                }
                idx = Some(i);
            }
            other => {
                return Err(format!(
                    "type `:({head} ...)` parameter must be a type name, got `{}`",
                    other.summarize(),
                ));
            }
        }
    }
    Ok(idx)
}

fn build_list_params<'a>(
    name: String,
    rest: Vec<ExpressionPart<'a>>,
) -> Result<TypeExpr, String> {
    if rest.is_empty() {
        return Ok(TypeExpr { name, params: TypeParams::None, builtin_cache: std::cell::OnceCell::new() });
    }
    let params = rest
        .into_iter()
        .map(|p| match p {
            ExpressionPart::Type(t) => Ok(t),
            // Inside a TypeExpr frame, nested parens are themselves type expressions
            // without the sigil prefix: `:(Dict Str (List Number))`'s inner
            // `(List Number)` lands as an `Expression` part with type-named contents.
            // Recursively fold those contents through the same TypeExprFrame builder so
            // arbitrary nesting depth is supported.
            ExpressionPart::Expression(boxed) => {
                let frame = TypeExprFrame { parts: boxed.parts };
                frame.build()
            }
            other => Err(format!(
                "type `:({name} ...)` parameter must be a type name, got `{}`",
                other.summarize(),
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TypeExpr { name, params: TypeParams::List(params), builtin_cache: std::cell::OnceCell::new() })
}

fn build_function_params<'a>(
    name: String,
    rest: Vec<ExpressionPart<'a>>,
    arrow_idx: usize,
) -> Result<TypeExpr, String> {
    let mut iter = rest.into_iter();
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
        builtin_cache: std::cell::OnceCell::new(),
    })
}

fn extract_function_args<'a>(
    before: Vec<ExpressionPart<'a>>,
) -> Result<Vec<TypeExpr>, String> {
    const MISSING_PARENS: &str =
        "type `:(Function ...)` args must be parenthesized = \
         `:(Function (arg1 arg2 ...) -> R)` (use `:(Function () -> R)` for nullary)";

    let [only] = <[ExpressionPart<'a>; 1]>::try_from(before)
        .map_err(|_| MISSING_PARENS.to_string())?;
    let arg_parts = match only {
        ExpressionPart::Expression(boxed) => boxed.parts,
        _ => return Err(MISSING_PARENS.to_string()),
    };
    arg_parts
        .into_iter()
        .map(|p| match p {
            ExpressionPart::Type(t) => Ok(t),
            // Args themselves can be parameterized types: `:(Function ((List Number)) -> R)`
            // wraps the args list in `(...)` and each arg may itself be a sigil-less
            // nested type expression. Recurse through TypeExprFrame to fold the nested
            // shape.
            ExpressionPart::Expression(boxed) => {
                let frame = TypeExprFrame { parts: boxed.parts };
                frame.build()
            }
            other => Err(format!(
                "type `:(Function (...))` arg must be a type name, got `{}`",
                other.summarize()
            )),
        })
        .collect()
}

fn extract_function_return<'a>(
    after: Vec<ExpressionPart<'a>>,
) -> Result<TypeExpr, String> {
    let [only] = <[ExpressionPart<'a>; 1]>::try_from(after).map_err(|after| format!(
        "type `:(Function ... -> R)` needs exactly one return type after the arrow, got {}",
        after.len()
    ))?;
    match only {
        ExpressionPart::Type(t) => Ok(t),
        // The return slot can itself be a parameterized type:
        // `:(Function (Number) -> (List Str))` wraps the return in parens.
        ExpressionPart::Expression(boxed) => {
            let frame = TypeExprFrame { parts: boxed.parts };
            frame.build()
        }
        other => Err(format!(
            "type `:(Function ... -> R)` return type must be a type name, got `{}`",
            other.summarize()
        )),
    }
}
