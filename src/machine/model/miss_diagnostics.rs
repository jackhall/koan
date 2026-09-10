//! Dispatch-miss diagnosis: the targeted message a *shape* mistake earns, kept out of the
//! success-path registry.
//!
//! Some mistakes are shapes no overload should ever succeed at — a module named with a Type token, a
//! `UNARY OP` with no result segment, a return slot naming a value. Registering a body for each
//! would put an always-erroring overload in the very bucket a reader consults to learn what a form
//! *does*. They live here instead: a static table ([`MISS_DIAGNOSTICS`]) probed only once dispatch
//! has already failed, each entry pairing a full untyped key with a render fn that confirms the
//! mistake from the raw parts. No hit means the generic miss reason stands.
//!
//! Recognition is by the [`FormId`] the node resolved at construction — a full untyped bucket key,
//! sound because builtin buckets are unshadowable, so a node whose key matches a
//! [`FORMS`](crate::parse::forms::FORMS) entry can only ever resolve to that builtin's
//! overloads. A form whose key has *no* registration at all — the missing-result `UNARY OP` forms,
//! whose only shape is the mistake — carries that argument itself: its entry is marked
//! [`reserved`](crate::parse::forms::Form::reserved), and the overload write door
//! refuses a user registration under a reserved key, so the shape stays unshadowable and the
//! diagnosis stays sound.

use crate::machine::model::registries::RunRegistries;
use crate::machine::model::{WorkingExpression, render_label};
use crate::parse::forms::{FormId, form_for};
use crate::parse::snake_case_identifier;
use crate::parse::{ExpressionPart, UntypedKey};

/// The targeted message a miss under one form earns, when the parts confirm the mistake the entry
/// names (the name slot really is a Type token, say); `None` leaves the generic dispatch-miss reason
/// standing.
pub type MissRender = for<'a> fn(&WorkingExpression<'a>, &RunRegistries) -> Option<String>;

/// The targeted message `expr`'s miss earns, or `None` for a miss no entry names. Entries may share
/// a form — two different mistakes are spellable under one `FN` shape — so the walk takes the first
/// entry for the node's form whose render confirms.
pub(crate) fn diagnose_miss(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    let id = expr.cache().form()?.id;
    MISS_DIAGNOSTICS
        .iter()
        .filter(|(tag, _)| *tag == id)
        .find_map(|(_, render)| render(expr, registries))
}

/// True iff `key` names a reserved form — a shape whose only reading is the mistake it diagnoses,
/// which therefore admits no registration at all.
pub(crate) fn key_is_reserved(key: &UntypedKey) -> bool {
    form_for(key.iter().copied()).is_some_and(|form| form.reserved)
}

// ---------- part reads ----------
//
// Every builder reads raw parts and answers `None` for a part shape it does not recognize, so an
// entry speaks only for the mistake it names and every other miss under the same key falls through
// to the generic reason.

/// The Type token at `index`, for the entries whose mistake is a value named Type-side.
fn type_name_at(
    expr: &WorkingExpression<'_>,
    index: usize,
    registries: &RunRegistries,
) -> Option<String> {
    match expr.parts.get(index)?.value.as_ast()? {
        ExpressionPart::Type(t) => Some(render_label(t.symbol(), registries)),
        _ => None,
    }
}

/// The identifier at `index`, for the entries whose mistake is a type named value-side.
fn identifier_at(
    expr: &WorkingExpression<'_>,
    index: usize,
    registries: &RunRegistries,
) -> Option<String> {
    match expr.parts.get(index)?.value.as_ast()? {
        ExpressionPart::Identifier(v) => Some(render_label(v.symbol(), registries)),
        _ => None,
    }
}

/// The operator glyph the declaration quotes — the first `#(…)` part of the run, read exactly as
/// [`symbol_from_parts`](crate::parse::symbol_from_parts) reads it off a statement.
fn quoted_symbol(expr: &WorkingExpression<'_>, registries: &RunRegistries) -> Option<String> {
    let quoted = expr
        .parts
        .iter()
        .find_map(|part| match part.value.as_ast()? {
            ExpressionPart::QuotedExpression(inner) => Some(inner.reference()),
            _ => None,
        })?;
    let [only] = quoted.parts else { return None };
    match only.value {
        ExpressionPart::Keyword(symbol) => Some(render_label(symbol.symbol(), registries)),
        _ => None,
    }
}

// ---------- render fns ----------

/// `UNARY OP #(<sym>) OVER <Operand> = (<body>)` — the result segment is mandatory: a unary body
/// consumes a whole list of operands, so its result type is not its operand type and there is
/// nothing to default it to.
fn unary_missing_result(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    let sym = quoted_symbol(expr, registries)?;
    Some(format!(
        "`UNARY OP #({sym})` must declare its result type: \
         `UNARY OP #({sym}) OVER <Operand> -> <Result> = (…)`",
    ))
}

/// The bodyless twin of [`unary_missing_result`]: a SIG operator member's head, whose result is no
/// more optional than a definition's — the run reaches a unary body as one list, so nothing feeds
/// a result back for the declaration to default to.
fn unary_head_missing_result(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    let sym = quoted_symbol(expr, registries)?;
    Some(format!(
        "`UNARY OP #({sym})` must declare its result type: \
         `UNARY OP #({sym}) OVER <Operand> -> <Result>`",
    ))
}

/// The combined twin of [`unary_missing_result`], naming the flat spelling in its suggestion.
fn unary_missing_result_combined(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    let sym = quoted_symbol(expr, registries)?;
    let name = identifier_at(expr, 1, registries).unwrap_or_else(|| "op".to_string());
    Some(format!(
        "`UNARY OP #({sym})` must declare its result type: \
         `LET {name} = UNARY OP #({sym}) OVER <Operand> -> <Result> = (…)`",
    ))
}

/// `LET <Name> = FN …` — a function is a value, so it binds under a value-classified identifier.
fn function_bound_type_named(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    let name = type_name_at(expr, 1, registries)?;
    Some(format!(
        "LET binder `{name}` is Type-classified but the bound value is a function (a value); \
         rebind under a value-classified identifier instead (snake_case, e.g. `{suggestion}`)",
        suggestion = snake_case_identifier(&name),
    ))
}

/// `MODULE <Name> = …` / `GROUP <Name> …` — a module is a value, so its name belongs in the value
/// namespace. A group is a module, so its four surfaces take the same message.
fn module_type_named(expr: &WorkingExpression<'_>, registries: &RunRegistries) -> Option<String> {
    let name = type_name_at(expr, 1, registries)?;
    Some(format!(
        "module `{name}` is named with a Type token, but a module is a value — the Type-token \
         namespace names what can type a field. Name it snake_case, e.g. `{suggestion}`",
        suggestion = snake_case_identifier(&name),
    ))
}

/// `LET <name> = FN :{…} -> <Return> = (<body>)` — a lambda in a combined statement. A combined
/// statement exists to install a dispatch bucket alongside the value name, and a lambda has no head
/// to key one on, so the flat spelling reaches no overload; the parenthesized value bind is the one
/// that works. Only the record-schema reading speaks: the key's other reading is a `(<head>)` group
/// the lazy-slot stamp held back raw, which is no lambda and falls through to the generic miss.
fn combined_lambda_has_no_binder(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    if matches!(
        expr.parts.get(4).and_then(|part| part.value.as_ast()),
        Some(ExpressionPart::Expression(_))
    ) {
        return None;
    }
    let name = identifier_at(expr, 1, registries).unwrap_or_else(|| "f".to_string());
    Some(format!(
        "`FN :{{…}}` is a lambda: it registers nothing, so there is no combined statement for it — \
         bind it as an ordinary value, `LET {name} = (FN :{{…}} -> <Return> = (<body>))`",
    ))
}

/// A return slot naming a *value*. The mistake is a common one: a module-valued parameter is a
/// value token, so the type it denotes is spelled `:(TYPE OF er)`.
fn value_named_return(name: String) -> String {
    format!(
        "a return-type slot names a type, but `{name}` is a value. For the type of a value — a \
         module-valued parameter, say — write `-> :(TYPE OF {name})`"
    )
}

fn fn_value_named_return(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    identifier_at(expr, 3, registries).map(value_named_return)
}

fn quantified_value_named_return(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    identifier_at(expr, 6, registries).map(value_named_return)
}

fn combined_quantified_value_named_return(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    identifier_at(expr, 10, registries).map(value_named_return)
}

fn combined_expr_value_named_return(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    identifier_at(expr, 7, registries).map(value_named_return)
}

// ---------- the table ----------

/// The diagnosable dispatch misses, by the form whose shape spells the mistake. The reserved forms
/// are the missing-result `UNARY OP` shapes and the combined `LET … = FN <signature> …` statement;
/// every other form here keeps success-path siblings, and its entry speaks only when its own render
/// confirms the mistake.
pub static MISS_DIAGNOSTICS: &[(FormId, MissRender)] = &[
    // UNARY OP <symbol> OVER <operand> = <body> — the shape whose only reading is the mistake.
    (FormId::UnaryOperatorDefinition, unary_missing_result),
    // UNARY OP <symbol> OVER <operand> — the head form, missing its result.
    (FormId::UnaryOperatorHead, unary_head_missing_result),
    // LET <name> = UNARY OP <symbol> OVER <operand> = <body>.
    (FormId::CombinedUnaryOperator, unary_missing_result_combined),
    // MODULE <name> = <body>.
    (FormId::Module, module_type_named),
    // GROUP <name> FOLD LEFT|RIGHT = <body>.
    (FormId::GroupFoldLeft, module_type_named),
    (FormId::GroupFoldRight, module_type_named),
    // GROUP <name> PAIRWISE FOLD <combiner> LEFT|RIGHT = <body>.
    (FormId::GroupPairwiseFoldLeft, module_type_named),
    (FormId::GroupPairwiseFoldRight, module_type_named),
    // LET <name> = FN <signature> -> <return type> = <body>.
    (FormId::CombinedLambda, combined_lambda_has_no_binder),
    // EXPR <head> -> <return type> = <body>: a value-named return slot.
    (FormId::ExpressionDefinition, fn_value_named_return),
    // LET <name> = FN EXPR <head> -> <return type> = <body>: a Type-classified binder, or a
    // value-named return slot. Two mistakes under one form, each confirmed by its own render.
    (FormId::CombinedExpression, function_bound_type_named),
    (FormId::CombinedExpression, combined_expr_value_named_return),
    // The quantified twins of the two rows above, whose group shifts every slot after it.
    (
        FormId::QuantifiedExpressionDefinition,
        quantified_value_named_return,
    ),
    (
        FormId::CombinedQuantifiedExpression,
        function_bound_type_named,
    ),
    (
        FormId::CombinedQuantifiedExpression,
        combined_quantified_value_named_return,
    ),
];

#[cfg(test)]
mod tests;
