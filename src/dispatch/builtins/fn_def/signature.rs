//! Signature parsing for the `FN` builtin.
//!
//! Two entry points:
//! - [`parse_fn_param_list`] — the full structural parse used by [`super::body`] at FN
//!   construction time.
//! - [`pre_run`] — the dispatch-time placeholder extractor used by `register` to
//!   announce the function's name before its body runs.

use crate::dispatch::{Argument, KType, SignatureElement};
use crate::dispatch::types::TypeResolver;
use crate::parse::kexpression::{ExpressionPart, KExpression};

/// Convert the captured FN-parameter-list `KExpression` into a list of `SignatureElement`s.
/// (Module signatures — `Signature`, declared via `SIG` — are a different concept; this
/// function only handles the FN parameter list.) Walks the parts left-to-right, consuming
/// bare `Keyword` parts as fixed tokens and `Identifier(name) Keyword(":") Type(t)` triples
/// as typed `Argument` slots. Bare identifiers without a `: Type` annotation, unknown type
/// names, stray `:` or `Type` parts, and any other variant (`Literal`, `Expression`,
/// `ListLiteral`, `DictLiteral`, `Future`) yield an `Err(message)` for the caller to wrap
/// in `ShapeError`. The colon keyword is consumed only as part of a triple — a stray `:`
/// outside that shape is a shape error.
///
/// The `resolver` is forwarded into `KType::from_type_expr` for each parameter type so
/// user-defined types in the surrounding scope can shadow builtins. Stage-2 substrate per
/// the [module-system stage 2 plan](../../../../roadmap/module-system-2-scheduler.md).
pub(super) fn parse_fn_param_list<'a>(
    signature: &KExpression<'a>,
    resolver: &dyn TypeResolver,
) -> Result<Vec<SignatureElement>, String> {
    let parts = &signature.parts;
    let mut elements: Vec<SignatureElement> = Vec::with_capacity(parts.len());
    let mut i = 0;
    while i < parts.len() {
        match &parts[i] {
            ExpressionPart::Keyword(s) if s == ":" => {
                return Err(
                    "FN signature has a stray `:` outside a `<name>: <Type>` triple".to_string(),
                );
            }
            ExpressionPart::Keyword(s) => {
                elements.push(SignatureElement::Keyword(s.clone()));
                i += 1;
            }
            ExpressionPart::Identifier(name) => {
                let colon = parts.get(i + 1);
                let ty = parts.get(i + 2);
                match (colon, ty) {
                    (Some(ExpressionPart::Keyword(c)), Some(ExpressionPart::Type(t))) if c == ":" => {
                        let ktype = KType::from_type_expr(t, resolver).map_err(|e| {
                            format!("{e} in FN signature for parameter `{name}`")
                        })?;
                        elements.push(SignatureElement::Argument(Argument {
                            name: name.clone(),
                            ktype,
                        }));
                        i += 3;
                    }
                    _ => {
                        return Err(format!(
                            "FN signature parameter `{name}` requires a `: Type` annotation \
                             (e.g. `{name}: Number`)",
                        ));
                    }
                }
            }
            ExpressionPart::Type(t) => {
                return Err(format!(
                    "FN signature has a stray type `{}` outside a `<name>: <Type>` triple",
                    t.render(),
                ));
            }
            other => {
                return Err(format!(
                    "FN signature part `{}` is not a Keyword, Identifier, or `<name>: <Type>` triple",
                    other.summarize(),
                ));
            }
        }
    }
    Ok(elements)
}

/// Dispatch-time placeholder extractor for FN. The signature slot at `parts[1]` is an
/// `Expression(signature_expr)` whose first `Keyword` is the function's name (the same
/// name used by `body` to register the function — see the `find_map(SignatureElement::
/// Keyword, ...)` call). Walks the signature parts inline rather than re-running the
/// full `parse_fn_param_list`; the body still does the full parse and surfaces any shape
/// errors. Returns `None` if the signature slot is missing or malformed (e.g. no Keyword
/// in the signature) — the body's `ShapeError` reports the real failure.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    let sig_part = expr.parts.get(1)?;
    let signature_expr = match sig_part {
        ExpressionPart::Expression(boxed) => boxed,
        _ => return None,
    };
    for part in &signature_expr.parts {
        if let ExpressionPart::Keyword(s) = part {
            return Some(s.clone());
        }
    }
    None
}
