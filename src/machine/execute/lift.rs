use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::model::{KObject, KType};
use crate::machine::{CallArena, KFuture, RuntimeArena};
use crate::machine::model::ast::{ExpressionPart, KExpression};

/// Lift a KObject out of `dying_frame`'s arena into the destination arena, attaching
/// an `Rc<CallArena>` to anchor any descendant that borrows into the dying arena.
/// See [per-call-arena-protocol.md § Lift-time anchor decision](../../../design/per-call-arena-protocol.md#lift-time-anchor-decision).
#[cfg(test)]
pub fn lift_kobject_for_test<'b>(v: &KObject<'b>, dying_frame: &Rc<CallArena>) -> KObject<'b> {
    lift_kobject(v, dying_frame)
}

pub(super) fn lift_kobject<'b>(v: &KObject<'b>, dying_frame: &Rc<CallArena>) -> KObject<'b> {
    if dying_frame.arena().functions_is_empty() {
        return v.deep_clone();
    }
    match v {
        KObject::KFunction(f, existing) => {
            let new_frame = if existing.is_some() {
                existing.clone()
            } else {
                let dying_runtime: *const RuntimeArena = dying_frame.arena();
                let captured_runtime: *const RuntimeArena = f.captured_scope().arena;
                if std::ptr::eq(captured_runtime, dying_runtime) {
                    Some(Rc::clone(dying_frame))
                } else {
                    None
                }
            };
            KObject::KFunction(f, new_frame)
        }
        KObject::KFuture(t, existing) => {
            let new_frame = if existing.is_some() {
                existing.clone()
            } else if kfuture_borrows_dying_arena(t, dying_frame.arena()) {
                Some(Rc::clone(dying_frame))
            } else {
                None
            };
            KObject::KFuture(t.deep_clone(), new_frame)
        }
        // Mirror of the `KFunction` arm: anchor on the dying frame if the module's
        // child scope was alloc'd there (e.g. a functor body's `MODULE Result = (...)`).
        KObject::KTypeValue(KType::Module { module: m, frame: existing }) => {
            let new_frame = if existing.is_some() {
                existing.clone()
            } else {
                let dying_runtime: *const RuntimeArena = dying_frame.arena();
                let module_runtime: *const RuntimeArena = m.child_scope().arena;
                if std::ptr::eq(module_runtime, dying_runtime) {
                    Some(Rc::clone(dying_frame))
                } else {
                    None
                }
            };
            KObject::KTypeValue(KType::Module { module: m, frame: new_frame })
        }
        // Carrier type (`elem` / `k` / `v`) is preserved across rebuild: lifting only
        // attaches arena anchors, never changes a descendant's `ktype()`.
        KObject::List(items, elem) => {
            if items.iter().any(|x| needs_lift(x, dying_frame)) {
                let lifted: Vec<KObject<'b>> = items
                    .iter()
                    .map(|x| lift_kobject(x, dying_frame))
                    .collect();
                KObject::list_with_type(Rc::new(lifted), (**elem).clone())
            } else {
                KObject::list_with_type(Rc::clone(items), (**elem).clone())
            }
        }
        KObject::Dict(entries, k, v) => {
            if entries.values().any(|x| needs_lift(x, dying_frame)) {
                let lifted: HashMap<_, _> = entries
                    .iter()
                    .map(|(k, val)| (k.clone_box(), lift_kobject(val, dying_frame)))
                    .collect();
                KObject::dict_with_type(Rc::new(lifted), (**k).clone(), (**v).clone())
            } else {
                KObject::dict_with_type(Rc::clone(entries), (**k).clone(), (**v).clone())
            }
        }
        KObject::Tagged { tag, value, scope_id, name, type_args } => {
            if needs_lift(value, dying_frame) {
                KObject::Tagged {
                    tag: tag.clone(),
                    value: Rc::new(lift_kobject(value, dying_frame)),
                    scope_id: *scope_id,
                    name: name.clone(),
                    type_args: Rc::clone(type_args),
                }
            } else {
                KObject::Tagged {
                    tag: tag.clone(),
                    value: Rc::clone(value),
                    scope_id: *scope_id,
                    name: name.clone(),
                    type_args: Rc::clone(type_args),
                }
            }
        }
        other => other.deep_clone(),
    }
}

/// True iff some descendant of `v` satisfies `predicate`. The predicate returns
/// `Some(true)` to short-circuit, `Some(false)` to bottom out the current subtree
/// without recursing, or `None` to let the walker recurse into composite payloads.
///
/// Single source of composite-variant coverage for `needs_lift` and
/// `kobject_borrows_arena`; they differ only in the per-leaf decision.
fn any_descendant<'b, F>(v: &KObject<'b>, predicate: &F) -> bool
where
    F: Fn(&KObject<'b>) -> Option<bool>,
{
    if let Some(decision) = predicate(v) {
        return decision;
    }
    match v {
        KObject::List(items, _) => items.iter().any(|x| any_descendant(x, predicate)),
        KObject::Dict(entries, _, _) => entries.values().any(|x| any_descendant(x, predicate)),
        KObject::Tagged { value, .. } => any_descendant(value, predicate),
        KObject::Struct { fields, .. } => fields
            .values()
            .any(|x| any_descendant(x, predicate)),
        KObject::KExpression(e) => e.parts.iter().any(|p| match &p.value {
            ExpressionPart::Future(obj) => any_descendant(obj, predicate),
            ExpressionPart::Expression(inner) | ExpressionPart::SigiledTypeExpr(inner) => {
                inner.parts.iter().any(|p2| match &p2.value {
                    ExpressionPart::Future(obj) => any_descendant(obj, predicate),
                    _ => false,
                })
            }
            _ => false,
        }),
        // None on a non-composite leaf bottoms out as `false`; predicates must
        // classify every leaf they care about.
        _ => false,
    }
}

/// True iff lifting `v` against `dying_frame` would attach an `Rc` to some descendant.
///
/// Bottoms out on `Struct`/`KExpression`: those variants aren't reachable as values
/// inside a List/Dict/Tagged at lift time in current Koan, so `any_descendant`'s
/// recursion through them is left forward-compatible without changing the answer.
fn needs_lift<'b>(v: &KObject<'b>, dying_frame: &Rc<CallArena>) -> bool {
    let dying_runtime: *const RuntimeArena = dying_frame.arena();
    any_descendant(v, &|obj: &KObject<'b>| match obj {
        KObject::KFunction(_, Some(_)) => Some(false),
        KObject::KFunction(f, None) => {
            let captured_runtime: *const RuntimeArena = f.captured_scope().arena;
            Some(std::ptr::eq(captured_runtime, dying_runtime))
        }
        KObject::KFuture(_, Some(_)) => Some(false),
        KObject::KFuture(t, None) => Some(kfuture_borrows_dying_arena(t, dying_frame.arena())),
        KObject::KTypeValue(KType::Module { frame: Some(_), .. }) => Some(false),
        KObject::KTypeValue(KType::Module { module: m, frame: None }) => {
            let module_runtime: *const RuntimeArena = m.child_scope().arena;
            Some(std::ptr::eq(module_runtime, dying_runtime))
        }
        KObject::Struct { .. } | KObject::KExpression(_) => Some(false),
        KObject::List(..) | KObject::Dict(..) | KObject::Tagged { .. } => None,
        _ => Some(false),
    })
}

/// True iff any descendant of an unanchored `KFuture` borrows into `arena`. Three
/// borrow sites: the function ref's captured arena, the parsed expression's
/// `Future(&KObject)` parts, and the bundle args.
fn kfuture_borrows_dying_arena<'b>(t: &KFuture<'b>, arena: &RuntimeArena) -> bool {
    if std::ptr::eq(t.function.captured_scope().arena, arena as *const RuntimeArena) {
        return true;
    }
    if expression_borrows_arena(&t.parsed, arena) {
        return true;
    }
    t.bundle
        .args
        .values()
        .any(|v| kobject_borrows_arena(v, arena))
}

fn expression_borrows_arena<'b>(expr: &KExpression<'b>, arena: &RuntimeArena) -> bool {
    expr.parts.iter().any(|p| part_borrows_arena(&p.value, arena))
}

fn part_borrows_arena<'b>(part: &ExpressionPart<'b>, arena: &RuntimeArena) -> bool {
    match part {
        ExpressionPart::Future(obj) => arena.owns_object(*obj as *const KObject<'b>),
        ExpressionPart::Expression(e) => expression_borrows_arena(e, arena),
        // Dispatch-time splicing can introduce `Future` parts inside a SigiledTypeExpr;
        // recurse through the type-context marker.
        ExpressionPart::SigiledTypeExpr(e) => expression_borrows_arena(e, arena),
        ExpressionPart::ListLiteral(items) => items.iter().any(|p| part_borrows_arena(p, arena)),
        ExpressionPart::DictLiteral(pairs) => pairs.iter().any(|(k, v)| {
            part_borrows_arena(k, arena) || part_borrows_arena(v, arena)
        }),
        _ => false,
    }
}

/// True iff any descendant of `v` borrows into `arena`. KExpression and KFuture
/// settle as predicate leaves (their recursion is not `KObject`-shaped — parts,
/// bundle args, function ref) so the walker doesn't double-traverse via the
/// KExpression arm.
fn kobject_borrows_arena<'b>(v: &KObject<'b>, arena: &RuntimeArena) -> bool {
    any_descendant(v, &|obj: &KObject<'b>| match obj {
        KObject::KExpression(e) => Some(expression_borrows_arena(e, arena)),
        KObject::KFuture(t, _) => Some(kfuture_borrows_dying_arena(t, arena)),
        KObject::KFunction(f, _) => Some(std::ptr::eq(
            f.captured_scope().arena,
            arena as *const RuntimeArena,
        )),
        KObject::KTypeValue(KType::Module { module: m, .. }) => Some(std::ptr::eq(
            m.child_scope().arena,
            arena as *const RuntimeArena,
        )),
        KObject::List(..)
        | KObject::Dict(..)
        | KObject::Tagged { .. }
        | KObject::Struct { .. } => None,
        _ => Some(false),
    })
}


#[cfg(test)]
mod tests;
