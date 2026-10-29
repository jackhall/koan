//! The fill: the type a container gives the records it holds, so no record lies under another by
//! width and a union keeps both.
//!
//! At each **position** below a container — a list's element, a dict's value, a record's field,
//! across union members — each record gains, at `Null`, every field another record at that
//! position names. The fill stops at every other type node: a scalar, `Any`, a tagged value, a
//! function, a type, code, a module; a dict's keys are never filled. The fill writes no cell: the
//! door reads a field typed exactly `Null` that a record does not store as `null`
//! ([surface.rs](super::surface)).
//!
//! The fill is a **merge** over **groups**: a group is the sorted, deduplicated set of types that
//! reach one position, each union flattened into its members. A group of fewer than two types maps
//! each to itself — a container's own type is already filled — so the merge descends only where
//! two distinct types meet. A child group holds `Null` wherever a record of its parent lacks the
//! field, as the fill gives it there. It is a downstream walk over type nodes, building only through
//! the chain's canonical doors (`record`, `list`, `dict`, `union_of`), and memoized through the
//! lattice's memo ([`MemoKind::Fill`]): each group's results, and each result group as mapping to
//! itself, so a container built from a merge's results merges in O(1). It runs over an explicit
//! stack of groups, since a type is as deep as the data a loop nests.
//!
//! [`hides_more`] walks the same positions for the `hides` bit ([hiding.rs](super::hiding)).

#[cfg(test)]
use std::cell::Cell;

use crate::memory::{Bump, BumpAllocator, BumpVec};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, MemoKind, Parametric, TypeNode};

use super::{Chain, representation};

#[cfg(test)]
thread_local! {
    /// `test`-only: how many groups the merge computed, a memo hit computing none.
    pub static FILL_STEPS: Cell<usize> = const { Cell::new(0) };
}

/// Where a child group sits below its parent's.
#[derive(Clone, Copy)]
enum Step {
    Element,
    Value,
    Field(BinderSymbol),
}

impl Step {
    fn is(self, other: Step) -> bool {
        match (self, other) {
            (Step::Element, Step::Element) | (Step::Value, Step::Value) => true,
            (Step::Field(left), Step::Field(right)) => left.symbol() == right.symbol(),
            _ => false,
        }
    }
}

/// One group of the merge: its types, its child groups once entered, and its results once left,
/// aligned with its types.
struct Group<'x> {
    handles: &'x [KType],
    children: &'x [(Step, usize)],
    results: Option<&'x [KType]>,
}

/// One frame of the merge's stack.
enum Frame {
    Enter(usize),
    Exit(usize),
}

/// Each of `members`, a container's cells' types, filled against the rest, aligned with them.
pub fn filled<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    members: &[KType],
) -> BumpVec<'x, KType> {
    let mut flat = BumpVec::new_in(scratch);
    for member in members {
        flatten(types, *member, &mut flat);
    }
    let top = sorted(flat);
    let groups = merge(types, scratch, top);
    let mut out = BumpVec::with_capacity_in(members.len(), scratch);
    out.extend((members.iter()).map(|member| resolved(types, scratch, &groups[0], *member)));
    out
}

/// The canonical union of `members` filled against each other; `Never` for none.
pub fn filled_union(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    members: impl Iterator<Item = KType>,
) -> KType {
    let mut given = BumpVec::new_in(scratch);
    given.extend(members);
    let results = filled(types, scratch, &given);
    types.union_of(scratch, &results)
}

/// `own`'s result in the merge of `own` with `other`: `own` given, at every depth, the `Null`
/// fields `other` names that it lacks. What the door reads a part at when its own type falls
/// short of a member of the type it is read at.
pub(super) fn filled_against(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    own: KType,
    other: KType,
) -> KType {
    filled(types, scratch, &[own, other])[0]
}

/// Whether `ktype` holds a record, or a variable, at a position the fill reaches: itself, a list's
/// element, a dict's value, a record's field or a union's member. What the load reads off a
/// literal's element to know the fill leaves it as it is.
pub fn may_hold_record(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ktype: Parametric,
) -> bool {
    let mut stack = BumpVec::new_in(scratch);
    stack.push(ktype);
    while let Some(at) = stack.pop() {
        match types.node(at) {
            TypeNode::Record { .. } | TypeNode::Quantified { .. } | TypeNode::Lexical { .. } => {
                return true;
            }
            TypeNode::List { element } => stack.push(element),
            TypeNode::Dict { value, .. } => stack.push(value),
            TypeNode::Union { members } => stack.extend(members.iter()),
            _ => {}
        }
    }
    false
}

/// Whether a value held at `from` and restamped at `to` may come to hide a field it stores: at some
/// position, a record `to` holds there lacks, or names at exactly `Null`, a field `from`'s record
/// there names at a type other than exactly `Null`, which is the only kind of field a record that
/// hides nothing stores. A walk of the two types alone, memoized in the lattice
/// ([`MemoKind::Hides`]).
pub fn hides_more(types: &Chain<'_, '_, '_, '_>, from: KType, to: KType) -> bool {
    if from == to {
        return false;
    }
    if let Some(known) = types.memo_flag(MemoKind::Hides, from, to) {
        return known;
    }
    let scratch = Bump::new();
    let mut stack = BumpVec::new_in(&scratch);
    stack.push((from, to));
    let mut hides = false;
    'walk: while let Some((from, to)) = stack.pop() {
        if from == to {
            continue;
        }
        let mut each = BumpVec::new_in(&scratch);
        flatten(types, to, &mut each);
        match types.node(from) {
            TypeNode::Union { members } => stack.extend(members.iter().map(|member| (member, to))),
            TypeNode::Record { fields } => {
                for member in each.iter() {
                    let TypeNode::Record { fields: held } = types.node(*member) else {
                        continue;
                    };
                    for (name, field) in fields.iter() {
                        match held.get(name.symbol()) {
                            _ if field == KType::NULL => {}
                            Some(held) if held != KType::NULL => stack.push((field, held)),
                            _ => {
                                hides = true;
                                break 'walk;
                            }
                        }
                    }
                }
            }
            TypeNode::List { element } => {
                stack.extend(each.iter().filter_map(|member| match types.node(*member) {
                    TypeNode::List { element: held } => Some((element, held)),
                    _ => None,
                }))
            }
            TypeNode::Dict { value, .. } => {
                stack.extend(each.iter().filter_map(|member| match types.node(*member) {
                    TypeNode::Dict { value: held, .. } => Some((value, held)),
                    _ => None,
                }))
            }
            TypeNode::ConstructorApply { .. } | TypeNode::SetMember { .. } => {
                let from = representation(types, &scratch, from);
                for member in each.iter() {
                    if let (Some(from), Some(held)) =
                        (from, representation(types, &scratch, *member))
                    {
                        stack.push((from, held));
                    }
                }
            }
            _ => {}
        }
    }
    types.record_memo_flag(MemoKind::Hides, from, to, hides);
    hides
}

/// The merge of `top`, a sorted group: every group it reached, `top`'s first, each with its
/// results.
fn merge<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    top: &'x [KType],
) -> BumpVec<'x, Group<'x>> {
    let mut groups = BumpVec::new_in(scratch);
    groups.push(Group {
        handles: top,
        children: &[],
        results: None,
    });
    let mut stack = BumpVec::new_in(scratch);
    stack.push(Frame::Enter(0));
    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Enter(at) => {
                let handles = groups[at].handles;
                if let Some(results) = known(types, scratch, handles) {
                    groups[at].results = Some(results);
                    continue;
                }
                #[cfg(test)]
                FILL_STEPS.with(|steps| steps.set(steps.get() + 1));
                let children = children(types, scratch, handles, &mut groups);
                groups[at].children = children;
                stack.push(Frame::Exit(at));
                stack.extend(children.iter().map(|(_, child)| Frame::Enter(*child)));
            }
            Frame::Exit(at) => {
                let group = &groups[at];
                let mut results = BumpVec::with_capacity_in(group.handles.len(), scratch);
                results.extend(
                    (group.handles.iter())
                        .map(|handle| rebuilt(types, scratch, &groups, at, *handle)),
                );
                let results = results.leak();
                remember(types, scratch, group.handles, results);
                groups[at].results = Some(results);
            }
        }
    }
    groups
}

/// The results of `handles` without a merge: each to itself for fewer than two, else as the
/// lattice's memo recorded them.
fn known<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    handles: &'x [KType],
) -> Option<&'x [KType]> {
    if handles.len() < 2 {
        return Some(handles);
    }
    let mut results = BumpVec::with_capacity_in(handles.len(), scratch);
    for handle in handles {
        results.push(types.memo_member(MemoKind::Fill, handles, *handle)?);
    }
    Some(results.leak())
}

/// Record `results` for `handles` in the lattice's memo, and the results' own group as mapping to
/// itself: the fill is idempotent.
fn remember(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    handles: &[KType],
    results: &[KType],
) {
    for (handle, result) in handles.iter().zip(results) {
        types.record_memo_member(MemoKind::Fill, handles, *handle, *result);
    }
    let mut own = BumpVec::with_capacity_in(results.len(), scratch);
    own.extend_from_slice(results);
    let own = sorted(own);
    if own.len() >= 2 && own != handles {
        for result in own {
            types.record_memo_member(MemoKind::Fill, own, *result, *result);
        }
    }
}

/// The child groups of `handles`, laid in `groups`: one per field name its records name, holding
/// `Null` where a record lacks it, one over its lists' elements and one over its dicts' values.
fn children<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    handles: &[KType],
    groups: &mut BumpVec<'x, Group<'x>>,
) -> &'x [(Step, usize)] {
    let mut names: BumpVec<'_, BinderSymbol> = BumpVec::new_in(scratch);
    let mut records = 0;
    let (mut elements, mut values) = (BumpVec::new_in(scratch), BumpVec::new_in(scratch));
    for handle in handles {
        match types.node(*handle) {
            TypeNode::Record { fields } => {
                records += 1;
                names.extend(fields.keys());
            }
            TypeNode::List { element } => flatten(types, element, &mut elements),
            TypeNode::Dict { value, .. } => flatten(types, value, &mut values),
            _ => {}
        }
    }
    names.sort_unstable_by_key(|name| name.symbol());
    names.dedup_by_key(|name| name.symbol());
    let mut children = BumpVec::new_in(scratch);
    let mut lay = |step, flat: BumpVec<'x, KType>, groups: &mut BumpVec<'x, Group<'x>>| {
        groups.push(Group {
            handles: sorted(flat),
            children: &[],
            results: None,
        });
        children.push((step, groups.len() - 1));
    };
    for name in names.iter() {
        let mut flat = BumpVec::new_in(scratch);
        let mut holding = 0;
        for handle in handles {
            if let TypeNode::Record { fields } = types.node(*handle)
                && let Some(field) = fields.get(name.symbol())
            {
                holding += 1;
                flatten(types, field, &mut flat);
            }
        }
        if holding < records {
            flat.push(KType::NULL);
        }
        lay(Step::Field(*name), flat, groups);
    }
    if !elements.is_empty() {
        lay(Step::Element, elements, groups);
    }
    if !values.is_empty() {
        lay(Step::Value, values, groups);
    }
    children.leak()
}

/// `handle`, a type of group `at`, rebuilt over its child groups' results: a record given the
/// group's names it lacks at `Null`, each field its result; a list's element and a dict's value
/// their results; anything else as it is. A composite nothing changed in is returned as it is,
/// interning nothing.
fn rebuilt(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    groups: &[Group<'_>],
    at: usize,
    handle: KType,
) -> KType {
    let child = |step: Step| {
        let (_, index) = (groups[at].children.iter())
            .find(|(at, _)| at.is(step))
            .expect("a merged group holds a child at each of its positions");
        &groups[*index]
    };
    match types.node(handle) {
        TypeNode::Record { fields } => {
            let mut changed = false;
            let mut rebuilt = BumpVec::with_capacity_in(fields.len(), scratch);
            for (name, field) in fields.iter() {
                let result = resolved(types, scratch, child(Step::Field(name)), field);
                changed |= result != field;
                rebuilt.push((name, result));
            }
            for (step, _) in groups[at].children {
                if let Step::Field(name) = step
                    && fields.get(name.symbol()).is_none()
                {
                    changed = true;
                    rebuilt.push((*name, KType::NULL));
                }
            }
            if changed {
                types.record(scratch, &rebuilt)
            } else {
                handle
            }
        }
        TypeNode::List { element } => {
            let result = resolved(types, scratch, child(Step::Element), element);
            if result == element {
                handle
            } else {
                types.list(result)
            }
        }
        TypeNode::Dict { key, value } => {
            let result = resolved(types, scratch, child(Step::Value), value);
            if result == value {
                handle
            } else {
                types.dict(key, result)
            }
        }
        _ => handle,
    }
}

/// `ktype`'s result in `group`, which holds it, or each of its members where it is a union.
fn resolved(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    group: &Group<'_>,
    ktype: KType,
) -> KType {
    let results = group
        .results
        .expect("a child group is left before its parent");
    let result = |member: KType| {
        let at = (group.handles.binary_search(&member))
            .expect("a group holds each type that reaches its position");
        results[at]
    };
    match types.node(ktype) {
        TypeNode::Union { members } => {
            let mut changed = false;
            let mut mapped = BumpVec::with_capacity_in(members.len(), scratch);
            for member in members.iter() {
                let filled = result(member);
                changed |= filled != member;
                mapped.push(filled);
            }
            if changed {
                types.union_of(scratch, &mapped)
            } else {
                ktype
            }
        }
        _ => result(ktype),
    }
}

/// Push `ktype` onto `out`, or each of its members where it is a union.
fn flatten(types: &Chain<'_, '_, '_, '_>, ktype: KType, out: &mut BumpVec<'_, KType>) {
    match types.node(ktype) {
        TypeNode::Union { members } => out.extend(members.iter()),
        _ => out.push(ktype),
    }
}

/// `handles` sorted and deduplicated: a group.
fn sorted<'x>(mut handles: BumpVec<'x, KType>) -> &'x [KType] {
    handles.sort_unstable();
    handles.dedup();
    handles.leak()
}
