//! `RECURSIVE TYPES <name:ProperType> = (<body>)` — co-declare a group of
//! mutually-recursive nominal types as one [`RecursiveSet`].
//!
//! The block is the one cross-order type-name resolution that survives strict lexical
//! lookup. Its body is a newline-separated sequence of ordinary `UNION` /
//! `NEWTYPE` declarations; every member name is in scope for every body inside the block,
//! so a cross-reference lowers to a transient `RecursiveRef` and seals to a `SetLocal`
//! index into the shared set. See
//! [user-types.md](../../design/typing/user-types.md).
//!
//! Mechanism: discover the members (name + kind) from the body declarations, mint one
//! shared `RecursiveSet` (members `pending`), and dispatch the declarations against a child
//! scope that carries the set via
//! [`await_body_in_scope`](super::await_body::await_body_in_scope) — so each declaration's
//! elaborator threads the group. Each member's own finalize fills its slot in the shared set
//! (the pre-installed `SetRef` routes it there rather than minting a singleton). The finish
//! mirrors the sealed members into the enclosing scope and binds the group handle: exiting
//! the block guarantees every forward reference resolved.

use crate::machine::model::types::KKind;
use std::collections::HashSet;
use std::rc::Rc;

use crate::machine::model::types::{NominalMember, RecursiveSet};
use crate::machine::model::KType;
use crate::machine::{BindingIndex, KError, KErrorKind, Scope, TraceFrame};

use crate::machine::model::ast::{ExpressionPart, KExpression};

use super::{arg, kw, sig};

/// Discover each member declaration's `(name, kind)` from the block body, using the same
/// multi-statement split `split_body_statements` applies. Rejects a body with no declarations, a
/// non-`UNION`/`NEWTYPE` statement, or a duplicate member name.
fn discover_members(body: &KExpression<'_>) -> Result<Vec<(String, KKind)>, KError> {
    let is_multi = !body.parts.is_empty()
        && body
            .parts
            .iter()
            .all(|p| matches!(p.value, ExpressionPart::Expression(_)));
    let decls: Vec<&KExpression<'_>> = if is_multi {
        body.parts
            .iter()
            .filter_map(|p| match &p.value {
                ExpressionPart::Expression(e) => Some(e.as_ref()),
                _ => None,
            })
            .collect()
    } else {
        vec![body]
    };
    if decls.is_empty() {
        return Err(KError::new(KErrorKind::ShapeError(
            "RECURSIVE TYPES needs at least one UNION / NEWTYPE declaration".to_string(),
        )));
    }
    let mut members: Vec<(String, KKind)> = Vec::with_capacity(decls.len());
    let mut seen: HashSet<String> = HashSet::new();
    for decl in decls {
        let kind = match leading_keyword(decl) {
            Some("UNION") | Some("NEWTYPE") => KKind::NewType,
            other => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "RECURSIVE TYPES body admits only UNION / NEWTYPE declarations, \
                     got `{}`",
                    other.unwrap_or("<non-declaration>"),
                ))));
            }
        };
        let name = decl.binder_name_from_type_part().ok_or_else(|| {
            KError::new(KErrorKind::ShapeError(
                "RECURSIVE TYPES member declaration is missing a type name".to_string(),
            ))
        })?;
        if !seen.insert(name.clone()) {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "RECURSIVE TYPES has a duplicate member `{name}`",
            ))));
        }
        members.push((name, kind));
    }
    Ok(members)
}

/// The first keyword token of a declaration expression (`UNION` / `NEWTYPE`).
fn leading_keyword<'b>(decl: &'b KExpression<'_>) -> Option<&'b str> {
    decl.parts.iter().find_map(|p| match &p.value {
        ExpressionPart::Keyword(s) => Some(s.as_str()),
        _ => None,
    })
}

/// The RECURSIVE TYPES body: discovers the members, mints the set + carrying child
/// scope, pre-installs each member's `SetRef`, dispatches the body block via
/// [`await_body_in_scope`](super::await_body::await_body_in_scope) (which fans out per
/// declaration), and the finish mirrors the sealed members + binds the group handle into
/// the enclosing scope.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::await_body::{await_body_in_scope, ChildScopeSeal};
    use crate::machine::core::kfunction::action::{
        require_bare_type_name, require_kexpression, Action,
    };

    let group_name =
        crate::try_action!(require_bare_type_name(ctx.args, "name", "RECURSIVE TYPES"));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "RECURSIVE TYPES", "body"));
    let members = crate::try_action!(discover_members(&body_expr));

    let scope_id = ctx.scope.id;
    let set = Rc::new(RecursiveSet::new(
        members
            .iter()
            .map(|(name, kind)| NominalMember::pending(name.clone(), scope_id, *kind))
            .collect(),
    ));
    let child = ctx
        .scope
        .brand()
        .alloc_scope(Scope::child_recursive_group(ctx.scope, Rc::clone(&set)));
    for (index, (name, _)) in members.iter().enumerate() {
        child.preinstall_identity(
            name.clone(),
            KType::SetRef {
                set: Rc::clone(&set),
                index,
            },
            BindingIndex::value(0),
        );
    }

    let bind_index = ctx.bind_index();
    await_body_in_scope(child, body_expr, ChildScopeSeal::LeaveOpen, move |fctx| {
        let frame =
            || TraceFrame::bare("<recursive-types>", format!("RECURSIVE TYPES {group_name}"));
        for (index, (name, _)) in members.iter().enumerate() {
            if !set.member(index).is_filled() {
                return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "RECURSIVE TYPES `{group_name}`: member `{name}` did not seal — a \
                         declaration referenced a name outside the group",
                )))
                .with_frame(frame())));
            }
        }
        for (index, (name, _)) in members.iter().enumerate() {
            let member_ref = KType::SetRef {
                set: Rc::clone(&set),
                index,
            };
            if let Err(e) = fctx
                .scope
                .register_nominal_upsert(name.clone(), member_ref, bind_index)
            {
                return Action::Done(Err(e.with_frame(frame())));
            }
        }
        let handle = KType::RecursiveGroup(Rc::clone(&set));
        match fctx
            .scope
            .register_nominal_upsert(group_name.clone(), handle, bind_index)
        {
            Ok(kt_ref) => Action::Done(Ok(crate::try_action!(fctx
                .ctx
                .alloc_type_checked(kt_ref.clone())))),
            Err(e) => Action::Done(Err(e.with_frame(frame()))),
        }
    })
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::OfKind(KKind::AnyType),
        vec![
            kw("RECURSIVE"),
            kw("TYPES"),
            arg("name", KType::OfKind(KKind::ProperType)),
            kw("="),
            arg("body", KType::KExpression),
        ],
    );
    crate::builtins::register_builtin_full(
        scope,
        "RECURSIVE TYPES",
        signature,
        body,
        Some((super::type_part_binder_name, crate::machine::BindKind::Type)),
        None,
        false,
    );
}

#[cfg(test)]
mod tests;
