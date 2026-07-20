//! `GROUP <name:Identifier> <mode> = (<body:KExpression>)` — declare a set of mutually chainable
//! operators. A group **is** a module: it binds a module value under a snake_case name, its body is
//! an ordinary module body, and `USING <group> SCOPE (…)` opens it. What a group adds is the
//! operator-registry entry a multi-operator run resolves against.
//!
//! ```text
//! GROUP vec_ops FOLD LEFT = (
//!   (OP #(+) OVER :(LIST OF Number) = (…))
//!   (OP #(-) OVER :(LIST OF Number) = (…)))
//!
//! GROUP num_compare PAIRWISE FOLD #(BOTH) LEFT = (
//!   (OP #(BOTH) OVER Bool = (…))          -- the combiner, over the pair-result type
//!   (OP #(≺) OVER Number -> Bool = (…))
//!   (OP #(≼) OVER Number -> Bool = (…)))
//! ```
//!
//! Both quoted slots — the members' `#(<sym>)` and the pairwise `#(<combiner>)` — are parse-static
//! [`QuotedExpression`](crate::machine::model::ExpressionPart::QuotedExpression) parts, so they
//! ride ordinary `:KExpression` slots and every `GROUP` overload keeps a *fixed* untyped key. An
//! unquoted symbol would be a `Keyword` part, which lands in the expression's untyped key and so
//! would key a different bucket per operator — no fixed overload could match it.
//!
//! The combiner is recorded **as a symbol**, never evaluated here: the reducer synthesizes it infix
//! at the chain's use site, where the ordinary scope walk resolves it (see
//! [`ReductionMode::Pairwise`]). Declaring it inside the group body is what carries it through
//! `USING` alongside the operator bodies.
//!
//! Declaration order inside the body does not matter to the registry: the members are read off the
//! **unevaluated** body block (a structural scan of its top-level `OP` statements), the group record
//! is allocated up front, and every nonempty subset of the member set is registered into the child
//! scope at index-0 visibility — before a single body statement runs. A mixed-member run therefore
//! reduces inside the group's own body, not just through a `USING` window. Only top-level `OP`
//! statements are members: an `OP` nested inside an `FN` or a branch declares an operator in *that*
//! scope and joins no group. Members' own registry writes are skipped ([`super::op_def`]) — the
//! group is the sole registrar for its members.
//!
//! Surface design: [design/operators.md](../../design/operators.md).

use crate::machine::model::TypeRegistry;
use std::collections::HashSet;

use crate::machine::body_statement_refs;
use crate::machine::model::KKind;
use crate::machine::model::KType;
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{FoldDirection, OperatorGroup, ReductionMode};
use crate::machine::BindingIndex;
use crate::machine::{require_identifier_name, require_kexpression, Action, BodyCtx};
use crate::machine::{KError, KErrorKind, Scope};

use super::op_def::{symbol_from_parts, symbol_from_slot};
use super::{arg, kw, sig};

/// The reduction a `GROUP` overload declares, before the combiner symbol is read out of the args.
#[derive(Clone, Copy)]
enum GroupMode {
    /// `FOLD <LEFT|RIGHT>`: a run folds through the members' binary bodies, whose result type is
    /// their operand type.
    Fold(FoldDirection),
    /// `PAIRWISE FOLD #(<combiner>) <LEFT|RIGHT>`: each adjacent pair dispatches through its own
    /// member's body — which may be heterogeneous (`OVER Number -> Bool`) — and the pair results
    /// fold through the combiner.
    Pairwise(FoldDirection),
}

/// The `GROUP` body: read the mode, scan the unevaluated body block for the members, allocate the
/// one shared group record, mint the child scope under it with the member powerset pre-registered,
/// then run the body and bind the module value through the tail `MODULE` uses
/// ([`super::module_def::await_module_body`]).
fn build<'a>(ctx: &BodyCtx<'a, '_>, group_mode: GroupMode) -> Action<'a> {
    let name = crate::try_action!(require_identifier_name(
        ctx.args, "name", "GROUP", ctx.types
    ));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "GROUP", "body"));
    let mode = crate::try_action!(reduction_mode(ctx, group_mode));
    let members = crate::try_action!(scan_members(&body_expr, &name));

    let member_set: HashSet<String> = members.iter().cloned().collect();
    let group: &'a OperatorGroup = ctx
        .scope
        .brand()
        .alloc_operator_group(OperatorGroup::new(member_set, mode));
    let child_scope =
        ctx.scope
            .brand()
            .alloc_scope(Scope::child_under_group(ctx.scope, name.clone(), group));

    // Index-0 visibility, like parameters and `USING` imports: the registry entries carry no
    // lexical-ordering relationship to the body statement reading them, so a run anywhere in the
    // body resolves the group — including above the `OP` declarations it names, which park on the
    // still-finalizing declarations through the ordinary pending-overload machinery.
    let member_refs: Vec<&str> = members.iter().map(|s| s.as_str()).collect();
    crate::try_action!(child_scope.register_group_under_all_subsets(
        &member_refs,
        group,
        BindingIndex::value(0),
    ));

    super::module_def::await_module_body(child_scope, name, body_expr, ctx.bind_index(), "GROUP")
}

/// The group's [`ReductionMode`]: a pairwise overload reads its combiner out of the quoted
/// `combiner` slot as a **symbol** (the same extraction an `OP` declaration's `#(<sym>)` takes). The
/// combiner is checked at neither declaration nor registration — it is a name the chain's use site
/// resolves, so a missing, non-callable, or wrong-arity combiner is an ordinary error there.
fn reduction_mode<'a>(
    ctx: &BodyCtx<'a, '_>,
    group_mode: GroupMode,
) -> Result<ReductionMode, KError> {
    Ok(match group_mode {
        GroupMode::Fold(FoldDirection::Left) => ReductionMode::FoldLeft,
        GroupMode::Fold(FoldDirection::Right) => ReductionMode::FoldRight,
        GroupMode::Pairwise(direction) => ReductionMode::Pairwise {
            combiner: symbol_from_slot(ctx.args, "GROUP", "combiner")?,
            direction,
        },
    })
}

/// The group's members: the symbol of every top-level `OP` statement of the unevaluated body block,
/// deduped in declaration order. Any other statement (a `LET`, an `FN`, the combiner's own `OP`) is
/// ordinary body content — the scan is structural and reads no value.
///
/// A `UNARY OP` is refused here rather than at the member's own dispatch: a unary operator takes the
/// whole run as one list, so it chains with nothing and can be no group's member.
fn scan_members(body: &KExpression<'_>, name: &str) -> Result<Vec<String>, KError> {
    let mut members: Vec<String> = Vec::new();
    for statement in body_statement_refs(body) {
        match (lead_keyword(statement, 0), lead_keyword(statement, 1)) {
            (Some("UNARY"), Some("OP")) => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "`GROUP {name}` declares a `UNARY OP`: a unary operator takes the whole run as \
                     one list, so it chains with nothing and cannot be a group member",
                ))))
            }
            (Some("OP"), _) => {
                let symbol = symbol_from_parts(statement)?;
                if !members.contains(&symbol) {
                    members.push(symbol);
                }
            }
            _ => {}
        }
    }
    if members.is_empty() {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "`GROUP {name}` declares no operator: a GROUP body holds at least one top-level `OP`",
        ))));
    }
    Ok(members)
}

/// The keyword at part `index` of `expr`, if that part is one — the structural read the member scan
/// leads with.
fn lead_keyword<'x>(expr: &'x KExpression<'_>, index: usize) -> Option<&'x str> {
    match &expr.parts.get(index)?.value {
        ExpressionPart::Keyword(k) => Some(k.as_str()),
        _ => None,
    }
}

fn body_fold_left<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    build(ctx, GroupMode::Fold(FoldDirection::Left))
}

fn body_fold_right<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    build(ctx, GroupMode::Fold(FoldDirection::Right))
}

fn body_pairwise_left<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    build(ctx, GroupMode::Pairwise(FoldDirection::Left))
}

fn body_pairwise_right<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    build(ctx, GroupMode::Pairwise(FoldDirection::Right))
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    use crate::builtins::register_builtin_full;

    // `FOLD <LEFT|RIGHT>` and `PAIRWISE FOLD #(<combiner>) <LEFT|RIGHT>`, each over the two name
    // carriers: an `Identifier` name binds the group's module value, a Type-token name takes the
    // respelling diagnostic MODULE's second overload produces (a group is a module, and a module is
    // a value).
    let fold = |name_kt: KType, direction: &str| {
        sig(
            KType::empty_signature(),
            vec![
                kw("GROUP"),
                arg("name", name_kt),
                kw("FOLD"),
                kw(direction),
                kw("="),
                arg("body", KType::KExpression),
            ],
        )
    };
    let pairwise = |name_kt: KType, direction: &str| {
        sig(
            KType::empty_signature(),
            vec![
                kw("GROUP"),
                arg("name", name_kt),
                kw("PAIRWISE"),
                kw("FOLD"),
                arg("combiner", KType::KExpression),
                kw(direction),
                kw("="),
                arg("body", KType::KExpression),
            ],
        )
    };

    // The group's module value binds value-side, so the submit-time placeholder a forward reference
    // parks on is tagged `Value` — the same hook MODULE installs.
    let value_binder = || {
        Some((
            super::identifier_part_binder_name as crate::machine::BinderNameFn,
            crate::machine::BindKind::Value,
        ))
    };

    for (direction, fold_body, pairwise_body) in [
        (
            "LEFT",
            body_fold_left as crate::machine::ActionFn,
            body_pairwise_left as crate::machine::ActionFn,
        ),
        ("RIGHT", body_fold_right, body_pairwise_right),
    ] {
        register_builtin_full(
            scope,
            "GROUP",
            fold(KType::Identifier, direction),
            fold_body,
            value_binder(),
            None,
            types,
        );
        register_builtin_full(
            scope,
            "GROUP",
            pairwise(KType::Identifier, direction),
            pairwise_body,
            value_binder(),
            None,
            types,
        );
        register_builtin_full(
            scope,
            "GROUP",
            fold(KType::OfKind(KKind::ProperType), direction),
            super::module_def::body_type_named,
            None,
            None,
            types,
        );
        register_builtin_full(
            scope,
            "GROUP",
            pairwise(KType::OfKind(KKind::ProperType), direction),
            super::module_def::body_type_named,
            None,
            None,
            types,
        );
    }
}

#[cfg(test)]
mod tests;
