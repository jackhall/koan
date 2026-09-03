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
//! Recognition is by full untyped bucket key, sound for the same reason
//! [`BINDER_SPECS`](crate::machine::model::binder::BINDER_SPECS) and
//! [`LAZY_SLOT_SPECS`](crate::machine::model::lazy_slots::LAZY_SLOT_SPECS) are: builtin buckets are
//! unshadowable, so a node whose key matches an entry can only ever resolve to that builtin's
//! overloads. An entry whose key has *no* registration at all — the missing-result `UNARY OP` forms,
//! whose only shape is the mistake — carries that argument itself: it is marked
//! [`reserved`](MissDiagnostic::reserved), and the overload write door refuses a user registration
//! under a reserved key, so the shape stays unshadowable and the diagnosis stays sound.

use crate::machine::model::key_spec::key_matches_untyped;
use crate::machine::model::key_spec::{KEYWORDS, KeyElementSpec, key_matches_parts};
use crate::machine::model::labels::snake_case_identifier;
use crate::machine::model::registries::RunRegistries;
use crate::machine::model::{ExpressionPart, UntypedKey, WorkingExpression, render_label};

use KeyElementSpec::{Keyword as Kw, Slot};

/// One diagnosable dispatch miss.
pub struct MissDiagnostic {
    /// Full untyped bucket key, every keyword pinned — the same vocabulary and soundness argument as
    /// the sibling spec tables.
    pub key: &'static [KeyElementSpec],
    /// The targeted message, when the parts confirm the mistake this entry names (the name slot
    /// really is a Type token, say); `None` leaves the generic dispatch-miss reason standing.
    pub render: for<'a> fn(&WorkingExpression<'a>, &RunRegistries) -> Option<String>,
    /// True when nothing registers under `key`: the overload write door refuses a user registration
    /// there, so the shape stays diagnosable rather than being claimable by a user form.
    pub reserved: bool,
}

/// The targeted message `expr`'s miss earns, or `None` for a miss no entry names. Entries may share
/// a key — two different mistakes are spellable under one `FN` shape — so the walk takes the first
/// entry whose key matches *and* whose render confirms.
pub(crate) fn diagnose_miss(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    MISS_DIAGNOSTICS
        .iter()
        .filter(|entry| key_matches_parts(entry.key, expr.parts))
        .find_map(|entry| (entry.render)(expr, registries))
}

/// True iff `key` is one this table reserves — a shape whose only reading is the mistake it
/// diagnoses, which therefore admits no registration at all.
pub(crate) fn key_is_reserved(key: &UntypedKey) -> bool {
    MISS_DIAGNOSTICS
        .iter()
        .any(|entry| entry.reserved && key_matches_untyped(entry.key, key))
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
/// [`symbol_from_parts`](crate::machine::model::symbol_from_parts) reads it off a statement.
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

/// A return slot naming a *value*. The mistake is a common one: a module-valued parameter is a
/// value token, so the type it denotes is spelled `:(TYPE OF er)`.
fn value_named_return(name: String) -> String {
    format!(
        "FN return-type slot names a type, but `{name}` is a value. For the type of a value — a \
         module-valued parameter, say — write `-> :(TYPE OF {name})`"
    )
}

fn fn_value_named_return(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    identifier_at(expr, 3, registries).map(value_named_return)
}

fn combined_fn_value_named_return(
    expr: &WorkingExpression<'_>,
    registries: &RunRegistries,
) -> Option<String> {
    identifier_at(expr, 6, registries).map(value_named_return)
}

// ---------- the table ----------

/// The single source of truth for the diagnosable dispatch misses. The two reserved keys are the
/// missing-result `UNARY OP` forms; every other key keeps success-path siblings, and its entry
/// speaks only when its own render confirms the mistake.
pub static MISS_DIAGNOSTICS: &[MissDiagnostic] = &[
    // UNARY OP <symbol> OVER <operand> = <body> — the shape whose only reading is the mistake.
    MissDiagnostic {
        key: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        render: unary_missing_result,
        reserved: true,
    },
    // LET <name> = UNARY OP <symbol> OVER <operand> = <body>.
    MissDiagnostic {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        render: unary_missing_result_combined,
        reserved: true,
    },
    // MODULE <name> = <body>.
    MissDiagnostic {
        key: &[Kw(&KEYWORDS.module), Slot, Kw(&KEYWORDS.equals), Slot],
        render: module_type_named,
        reserved: false,
    },
    // GROUP <name> FOLD LEFT|RIGHT = <body>.
    MissDiagnostic {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        render: module_type_named,
        reserved: false,
    },
    MissDiagnostic {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        render: module_type_named,
        reserved: false,
    },
    // GROUP <name> PAIRWISE FOLD <combiner> LEFT|RIGHT = <body>.
    MissDiagnostic {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            Slot,
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        render: module_type_named,
        reserved: false,
    },
    MissDiagnostic {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            Slot,
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        render: module_type_named,
        reserved: false,
    },
    // FN <signature> -> <return type> = <body>: a value-named return slot.
    MissDiagnostic {
        key: &[
            Kw(&KEYWORDS.fn_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        render: fn_value_named_return,
        reserved: false,
    },
    // LET <name> = FN <signature> -> <return type> = <body>: a Type-classified binder, or a
    // value-named return slot. Two mistakes under one key, each confirmed by its own render.
    MissDiagnostic {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        render: function_bound_type_named,
        reserved: false,
    },
    MissDiagnostic {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        render: combined_fn_value_named_return,
        reserved: false,
    },
];

#[cfg(test)]
mod tests;
