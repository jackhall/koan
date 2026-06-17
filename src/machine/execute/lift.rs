use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::values::{ArgValue, Held};
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::{CallArena, KFuture, RuntimeArena};

use super::runtime::KoanRuntime;

/// The workload's value-relocation hook: copy a [`Carried`] out of `src`'s arena into `dst`,
/// rebinding the copy's lifetime to `dst` so it dies with the destination. The scheduler decides
/// *when and where* (which frame, which arena); this hook owns the `KObject`-invariant *how* — the
/// arena→arena copy plus the escaping-closure anchor decision. Contract enforcement is a separate
/// layer (see [the run loop](super::run_loop)), never folded in here, so the hook is
/// reusable for any delivery edge.
///
/// Concrete in Koan types for this item; [Workload-independent DAG runtime] genericizes the
/// scheduler over this trait.
///
/// [Workload-independent DAG runtime]: ../../../roadmap/refactor/workload-independent-dag-runtime.md
pub(in crate::machine::execute) trait NodeLift<'run> {
    /// Copy `value` (alive in `src`'s arena) into `dst`; the result dies with `dst`.
    fn lift(
        &self,
        value: Carried<'run>,
        src: &Rc<CallArena>,
        dst: &'run RuntimeArena,
    ) -> Carried<'run>;
}

impl<'run> NodeLift<'run> for KoanRuntime<'run> {
    fn lift(
        &self,
        value: Carried<'run>,
        src: &Rc<CallArena>,
        dst: &'run RuntimeArena,
    ) -> Carried<'run> {
        match value {
            Carried::Object(v) => Carried::Object(dst.alloc_object(lift_kobject(v, src))),
            Carried::Type(t) => Carried::Type(dst.alloc_ktype(lift_ktype(t, src))),
        }
    }
}

/// Lift a KObject out of `dying_frame`'s arena into the destination arena, attaching
/// an `Rc<CallArena>` to anchor any descendant that borrows into the dying arena.
/// See [per-call-arena-protocol.md § Lift-time anchor decision](../../../design/per-call-arena-protocol.md#lift-time-anchor-decision).
/// Test seam for the type-channel lift: a first-class type (e.g. a per-call `Module`
/// carrier) lifts via [`lift_ktype`], re-anchoring its frame onto the dying arena.
#[cfg(test)]
pub fn lift_ktype_for_test<'run>(t: &KType<'run>, dying_frame: &Rc<CallArena>) -> KType<'run> {
    lift_ktype(t, dying_frame)
}

pub(super) fn lift_kobject<'run>(v: &KObject<'run>, dying_frame: &Rc<CallArena>) -> KObject<'run> {
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
        // Carrier type (`elem` / `k` / `v`) is preserved across rebuild: lifting only
        // attaches arena anchors, never changes a descendant's `ktype()`.
        KObject::List(items, elem) => {
            if items.iter().any(|x| held_needs_lift(x, dying_frame)) {
                let lifted: Vec<Held<'run>> =
                    items.iter().map(|x| lift_held(x, dying_frame)).collect();
                KObject::list_with_type(Rc::new(lifted), (**elem).clone())
            } else {
                KObject::list_with_type(Rc::clone(items), (**elem).clone())
            }
        }
        KObject::Dict(entries, k, v) => {
            if entries.values().any(|x| held_needs_lift(x, dying_frame)) {
                let lifted: HashMap<_, _> = entries
                    .iter()
                    .map(|(k, val)| (k.clone_box(), lift_held(val, dying_frame)))
                    .collect();
                KObject::dict_with_type(Rc::new(lifted), (**k).clone(), (**v).clone())
            } else {
                KObject::dict_with_type(Rc::clone(entries), (**k).clone(), (**v).clone())
            }
        }
        // The union's `RecursiveSet` is `Rc`-owned (not arena-owned), so it travels by
        // `Rc::clone` — no copy, no anchor. Only the carried `value` may borrow the dying
        // arena and need lifting.
        KObject::Tagged {
            tag,
            value,
            set,
            index,
            type_args,
        } => {
            let lifted_value = if needs_lift(value, dying_frame) {
                Rc::new(lift_kobject(value, dying_frame))
            } else {
                Rc::clone(value)
            };
            KObject::Tagged {
                tag: tag.clone(),
                value: lifted_value,
                set: Rc::clone(set),
                index: *index,
                type_args: Rc::clone(type_args),
            }
        }
        // A `Struct` / `Wrapped` carrying a `SetRef` shares its set by `Rc::clone` (via
        // `deep_clone`); the recursive group travels as one unit with no anchor. A schema's
        // `&'run Module` / `Signature` refs ride their own existing anchors.
        other => other.deep_clone(),
    }
}

/// Lift a `KType` out of `dying_frame`'s arena into the destination arena — the `Type`-arm
/// dual of [`lift_kobject`]. A `KType::Module { frame }` re-anchors on the dying frame if its
/// child scope was alloc'd there (mirroring the module arm of `lift_kobject`); every other
/// `KType` is `Rc`-owned (recursive sets) or owned data, so it travels by `clone`.
pub(super) fn lift_ktype<'run>(t: &KType<'run>, dying_frame: &Rc<CallArena>) -> KType<'run> {
    match t {
        KType::Module {
            module: m,
            frame: existing,
        } => {
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
            KType::Module {
                module: m,
                frame: new_frame,
            }
        }
        other => other.clone(),
    }
}

/// True iff some descendant of `v` satisfies `predicate`. The predicate returns
/// `Some(true)` to short-circuit, `Some(false)` to bottom out the current subtree
/// without recursing, or `None` to let the walker recurse into composite payloads.
///
/// Single source of composite-variant coverage for `needs_lift` and
/// `kobject_borrows_arena`; they differ only in the per-leaf decision.
fn any_descendant<'run, F>(v: &KObject<'run>, predicate: &F) -> bool
where
    F: Fn(&KObject<'run>) -> Option<bool>,
{
    if let Some(decision) = predicate(v) {
        return decision;
    }
    match v {
        // Only the `Object` arm of an aggregate cell is a `KObject` descendant; a `Type`
        // cell's module rides its own frame anchor (see `lift_held`), so it bottoms out here
        // just like a type-arm `Future` in `part_borrows_arena`.
        KObject::List(items, _) => items
            .iter()
            .filter_map(|x| x.as_object())
            .any(|o| any_descendant(o, predicate)),
        KObject::Dict(entries, _, _) => entries
            .values()
            .filter_map(|x| x.as_object())
            .any(|o| any_descendant(o, predicate)),
        KObject::Tagged { value, .. } => any_descendant(value, predicate),
        // A `Wrapped` carrier holds its repr by `Rc` (lift-stable), but the repr may itself
        // hold a descendant (a record field) that borrows the dying arena — recurse into it.
        KObject::Wrapped { inner, .. } => any_descendant(inner.get(), predicate),
        // A record's fields are the ex-struct field walk: a field may borrow the dying arena.
        KObject::Record(values, _) => values
            .iter()
            .filter_map(|(_, x)| x.as_object())
            .any(|o| any_descendant(o, predicate)),
        KObject::KExpression(e) => e.parts.iter().any(|p| match &p.value {
            ExpressionPart::Future(Carried::Object(obj)) => any_descendant(obj, predicate),
            ExpressionPart::Expression(inner) | ExpressionPart::SigiledTypeExpr(inner) => {
                inner.parts.iter().any(|p2| match &p2.value {
                    ExpressionPart::Future(Carried::Object(obj)) => any_descendant(obj, predicate),
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
/// Bottoms out on `Wrapped`/`KExpression`: a `Wrapped` holds its repr by `Rc` (lift-stable
/// by `Rc::clone`, like the retired `Struct`'s `Rc<IndexMap>` fields), and a bare
/// `KExpression` isn't reachable as a value inside a List/Dict/Tagged at lift time in current
/// Koan, so neither needs an arena anchor of its own.
fn needs_lift<'run>(v: &KObject<'run>, dying_frame: &Rc<CallArena>) -> bool {
    let dying_runtime: *const RuntimeArena = dying_frame.arena();
    any_descendant(v, &|obj: &KObject<'run>| match obj {
        KObject::KFunction(_, Some(_)) => Some(false),
        KObject::KFunction(f, None) => {
            let captured_runtime: *const RuntimeArena = f.captured_scope().arena;
            Some(std::ptr::eq(captured_runtime, dying_runtime))
        }
        KObject::KFuture(_, Some(_)) => Some(false),
        KObject::KFuture(t, None) => Some(kfuture_borrows_dying_arena(t, dying_frame.arena())),
        KObject::KExpression(_) => Some(false),
        KObject::List(..) | KObject::Dict(..) | KObject::Tagged { .. } => None,
        _ => Some(false),
    })
}

/// Lift an aggregate cell: the `Object` arm rides [`lift_kobject`], the `Type` arm rides
/// [`lift_ktype`] (re-anchoring a per-call `Module`'s frame).
fn lift_held<'run>(cell: &Held<'run>, dying_frame: &Rc<CallArena>) -> Held<'run> {
    match cell {
        Held::Object(o) => Held::Object(lift_kobject(o, dying_frame)),
        Held::Type(t) => Held::Type(lift_ktype(t, dying_frame)),
    }
}

/// True iff lifting `cell` would attach an anchor: an `Object` arm via [`needs_lift`], or a
/// `Type` arm holding an unanchored `Module` whose child scope rides the dying arena.
fn held_needs_lift<'run>(cell: &Held<'run>, dying_frame: &Rc<CallArena>) -> bool {
    match cell {
        Held::Object(o) => needs_lift(o, dying_frame),
        Held::Type(KType::Module {
            module: m,
            frame: None,
        }) => std::ptr::eq(m.child_scope().arena, dying_frame.arena()),
        Held::Type(_) => false,
    }
}

/// True iff any descendant of an unanchored `KFuture` borrows into `arena`. Three
/// borrow sites: the function ref's captured arena, the parsed expression's
/// `Future(Carried)` parts, and the bundle args.
fn kfuture_borrows_dying_arena<'run>(t: &KFuture<'run>, arena: &RuntimeArena) -> bool {
    if std::ptr::eq(
        t.function.captured_scope().arena,
        arena as *const RuntimeArena,
    ) {
        return true;
    }
    if expression_borrows_arena(&t.parsed, arena) {
        return true;
    }
    t.args.values().any(|v| argvalue_borrows_arena(v, arena))
}

/// An [`ArgValue`] borrows the dying arena iff its object arm has an arena-borrowing
/// descendant, or its type arm is a `Module` whose child scope rides the dying arena.
fn argvalue_borrows_arena<'run>(v: &ArgValue<'run>, arena: &RuntimeArena) -> bool {
    match v {
        ArgValue::Object(obj) => kobject_borrows_arena(obj, arena),
        ArgValue::Type(kt) => ktype_borrows_arena(kt, arena),
    }
}

/// True iff `kt` is a `Module` whose child scope borrows the dying arena. Other type
/// carriers are declaration-stable and never anchor into a per-call arena.
fn ktype_borrows_arena(kt: &KType<'_>, arena: &RuntimeArena) -> bool {
    matches!(kt, KType::Module { module: m, .. }
        if std::ptr::eq(m.child_scope().arena, arena as *const RuntimeArena))
}

fn expression_borrows_arena<'run>(expr: &KExpression<'run>, arena: &RuntimeArena) -> bool {
    expr.parts
        .iter()
        .any(|p| part_borrows_arena(&p.value, arena))
}

fn part_borrows_arena<'run>(part: &ExpressionPart<'run>, arena: &RuntimeArena) -> bool {
    match part {
        // Only a value-arm Future borrows an arena `KObject`; a type arm's `Module` rides
        // its own frame anchor, not an arena `KObject`.
        ExpressionPart::Future(Carried::Object(obj)) => {
            arena.owns_object(*obj as *const KObject<'run>)
        }
        ExpressionPart::Expression(e) => expression_borrows_arena(e, arena),
        // Dispatch-time splicing can introduce `Future` parts inside a SigiledTypeExpr;
        // recurse through the type-context marker.
        ExpressionPart::SigiledTypeExpr(e) => expression_borrows_arena(e, arena),
        ExpressionPart::ListLiteral(items) => items.iter().any(|p| part_borrows_arena(p, arena)),
        ExpressionPart::DictLiteral(pairs) => pairs
            .iter()
            .any(|(k, v)| part_borrows_arena(k, arena) || part_borrows_arena(v, arena)),
        ExpressionPart::RecordLiteral(fields) => {
            fields.iter().any(|(_, v)| part_borrows_arena(v, arena))
        }
        _ => false,
    }
}

/// True iff any descendant of `v` borrows into `arena`. KExpression and KFuture
/// settle as predicate leaves (their recursion is not `KObject`-shaped — parts,
/// bundle args, function ref) so the walker doesn't double-traverse via the
/// KExpression arm.
fn kobject_borrows_arena<'run>(v: &KObject<'run>, arena: &RuntimeArena) -> bool {
    any_descendant(v, &|obj: &KObject<'run>| match obj {
        KObject::KExpression(e) => Some(expression_borrows_arena(e, arena)),
        KObject::KFuture(t, _) => Some(kfuture_borrows_dying_arena(t, arena)),
        KObject::KFunction(f, _) => Some(std::ptr::eq(
            f.captured_scope().arena,
            arena as *const RuntimeArena,
        )),
        KObject::List(..)
        | KObject::Dict(..)
        | KObject::Tagged { .. }
        | KObject::Wrapped { .. }
        | KObject::Record(..) => None,
        _ => Some(false),
    })
}

#[cfg(test)]
mod tests;
