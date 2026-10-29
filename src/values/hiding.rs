//! A hidden field never shows: the copy a value takes where a type would show a field it stores
//! that the type it was held at hides.
//!
//! A part **hides** a field when it stores a cell for the field and its **view** — the type the
//! door reads it at through its holder ([surface.rs](super::surface)) — does not name it, or names
//! it at exactly `Null`. Each
//! container and tagged value carries a `hides` bit in its weight ([weight.rs](super::weight)): set
//! whenever it, or a part below it, may hide a field. Types alone decide it, so it may be set where
//! nothing is hidden, which costs only a visit. Construction ORs its parts' bits, a restamp from one
//! type to another ORs in [`hides_more`](super::fill::hides_more), a walk of the two types alone,
//! and a holder rebuilt over a copy sets it where a cell it keeps still hides a field.
//!
//! Where a value with the bit set comes to be held at a new view — built into a container, or
//! restamped by a binding, a handing on or `:!` — [`unhidden`] walks it, descending only into parts
//! whose bit is set, and reads each part's old view and new view as the door would. A record whose
//! new view names a field its old view hides — or names at exactly `Null` a field its old view
//! names at another type, which the door reads as `null` — is copied shallowly without that field,
//! over the same cells; each holder on the path above it is rebuilt over the copy. Each takes its
//! own type [`narrowed`] by its old view, less the fields it drops: what it holds, and no `Null` a
//! holder's fill gave it, so a type never names a field a copy below it dropped, and a copy reads
//! as a part that was never copied. The crossing's deep copy lays each list, dict and record at the
//! same type. So no record stores a value under a field its type has at exactly `Null`, and
//! [`Value::along`] reads what the door reads. A value without the bit, and no such field at its
//! top, is never visited. A knot member is never copied: the door reads it at its holder's type.

use crate::memory::{BumpAllocator, BumpVec, Writer, collect};
use crate::symbols::Symbol;
use crate::type_lattice::{KType, TypeNode};

use super::fill::hides_more;
use super::settled::part_hides;
use super::surface::taken_at;
use super::{Chain, Dict, Knotted, List, Record, Seen, Tagged, Value, representation};

/// Why a composite's view is concrete.
const SEEN: &str = "a composite is seen at a type";

/// One frame of the walk: a part to enter at its old and new views, or a part to rebuild over its
/// entered children's results.
enum Frame<'cell, 'x, X> {
    Enter {
        part: Value<'cell, X>,
        old: KType,
        new: KType,
    },
    Exit {
        part: Value<'cell, X>,
        old: KType,
        new: KType,
        /// Whether the part is a record to copy without the fields it [drops](self::dropped).
        copied: bool,
        /// The cell index of each entered child.
        children: &'x [usize],
    },
}

/// `value`, held so far at its own type, as it is to be held at `view`: each part whose view there
/// would show a field the part's view so far hides copied without that field, and each holder
/// above it rebuilt. `value` itself where its bit is clear and it drops no field at its top.
pub(super) fn unhidden<'cell, X: Knotted + 'cell>(
    writer: Writer<'cell, '_>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
    value: Value<'cell, X>,
    view: KType,
) -> Value<'cell, X> {
    let drops = || match value {
        Value::Record(record) => match (types.node(record.ktype()), types.node(view)) {
            (TypeNode::Record { fields: old }, TypeNode::Record { fields: new }) => (record
                .names())
            .iter()
            .any(|name| dropped(old.get(*name), old.get(*name), new.get(*name))),
            _ => false,
        },
        _ => false,
    };
    if !part_hides(&value) && !drops() {
        return value;
    }
    let mut stack = BumpVec::new_in(scratch);
    stack.push(Frame::Enter {
        part: value,
        old: value.concrete_ktype(),
        new: view,
    });
    let mut results: BumpVec<'_, Option<Value<'cell, X>>> = BumpVec::new_in(scratch);
    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Enter { part, old, new } => {
                let mut entered = BumpVec::new_in(scratch);
                let mut children = BumpVec::new_in(scratch);
                let copied = opened(
                    types,
                    scratch,
                    part,
                    old,
                    new,
                    &mut |at, child, old, new| {
                        entered.push(at);
                        children.push(Frame::Enter {
                            part: child,
                            old,
                            new,
                        });
                    },
                );
                stack.push(Frame::Exit {
                    part,
                    old,
                    new,
                    copied,
                    children: entered.leak(),
                });
                // Entered last to first, so each child's result lands in order.
                stack.extend(children.into_iter().rev());
            }
            Frame::Exit {
                part,
                old,
                new,
                copied,
                children,
            } => {
                let from = results.len() - children.len();
                let mut replaced = BumpVec::with_capacity_in(children.len(), scratch);
                replaced.extend(results.drain(from..));
                let changed = replaced.iter().any(Option::is_some);
                results.push((copied || changed).then(|| {
                    let at = Rebuilt {
                        old,
                        new,
                        copied,
                        children,
                        replaced: &replaced,
                    };
                    rebuilt(writer, types, scratch, part, &at)
                }));
            }
        }
    }
    results.pop().flatten().unwrap_or(value)
}

/// One step of [`narrowed`]: a pair to enter, or a pair to build over its entered children's
/// results.
enum Narrowing {
    Enter(KType, KType),
    Exit(KType, KType),
}

/// `own`, a part's own type, narrowed by `view`, the type it is held at: what a copy of the part is
/// laid at, so a copy reads as its source does under every later type. A record keeps each field
/// `view` names, narrowed by the field there, and each it has at exactly `Null`; it drops a field
/// `view` lacks, or names at exactly `Null` over another type, and gains none `view` adds. A list's
/// element and a dict's value narrow; a union narrows each member by what the door takes it at in
/// `view`; anything else is `own`. `own` itself where `view` may hide nothing of it
/// ([`hides_more`]).
pub(super) fn narrowed(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    own: KType,
    view: KType,
) -> KType {
    let unchanged = |own: KType, view: KType| own == view || !hides_more(types, own, view);
    if unchanged(own, view) {
        return own;
    }
    // Each field of a record `own` that `view` narrows: the type `view` names it at, if any.
    let field = |own: KType, held: Option<KType>| match held {
        None => (own == KType::NULL).then_some(None),
        Some(held) if held == KType::NULL && own != KType::NULL => None,
        Some(held) => Some(Some(held)),
    };
    let mut stack = BumpVec::new_in(scratch);
    stack.push(Narrowing::Enter(own, view));
    let mut results: BumpVec<'_, KType> = BumpVec::new_in(scratch);
    while let Some(step) = stack.pop() {
        match step {
            Narrowing::Enter(own, view) if unchanged(own, view) => results.push(own),
            Narrowing::Enter(own, view) => {
                let mut children = BumpVec::new_in(scratch);
                match (types.node(own), types.node(view)) {
                    (TypeNode::Union { members }, _) => children.extend(
                        members
                            .iter()
                            .map(|member| (member, taken_at(types, scratch, member, view))),
                    ),
                    (_, TypeNode::Union { .. }) => {
                        let taken = taken_at(types, scratch, own, view);
                        match types.node(taken) {
                            TypeNode::Union { .. } => results.push(own),
                            _ => stack.push(Narrowing::Enter(own, taken)),
                        }
                        continue;
                    }
                    (TypeNode::Record { fields }, TypeNode::Record { fields: held }) => children
                        .extend(fields.iter().filter_map(|(name, own)| {
                            field(own, held.get(name.symbol()))?.map(|held| (own, held))
                        })),
                    (TypeNode::List { element }, TypeNode::List { element: held }) => {
                        children.push((element, held))
                    }
                    (TypeNode::Dict { value, .. }, TypeNode::Dict { value: held, .. }) => {
                        children.push((value, held))
                    }
                    _ => {
                        results.push(own);
                        continue;
                    }
                }
                stack.push(Narrowing::Exit(own, view));
                // Entered last to first, so each child's result lands in order.
                stack.extend(
                    (children.iter().rev()).map(|(own, view)| Narrowing::Enter(*own, *view)),
                );
            }
            Narrowing::Exit(own, view) => {
                let built = match (types.node(own), types.node(view)) {
                    (TypeNode::Union { members }, _) => {
                        let from = results.len() - members.len();
                        let narrowed = types.union_of(scratch, &results[from..]);
                        results.truncate(from);
                        narrowed
                    }
                    (TypeNode::Record { fields }, TypeNode::Record { fields: held }) => {
                        let mut kept = BumpVec::with_capacity_in(fields.len(), scratch);
                        kept.extend(fields.iter().filter_map(|(name, own)| {
                            Some((name, own, field(own, held.get(name.symbol()))?))
                        }));
                        let entered = kept.iter().filter(|(.., held)| held.is_some()).count();
                        let mut narrowed = results[results.len() - entered..].iter();
                        let mut rebuilt = BumpVec::with_capacity_in(kept.len(), scratch);
                        rebuilt.extend(kept.iter().map(|(name, own, held)| match held {
                            Some(_) => (*name, *narrowed.next().expect("an entered field")),
                            None => (*name, *own),
                        }));
                        results.truncate(results.len() - entered);
                        types.record(scratch, &rebuilt)
                    }
                    (TypeNode::List { .. }, _) => {
                        let element = results.pop().expect("a list narrows its element");
                        types.list(element)
                    }
                    (TypeNode::Dict { key, .. }, _) => {
                        let value = results.pop().expect("a dict narrows its value");
                        types.dict(key, value)
                    }
                    _ => unreachable!("only a composite or a union is left"),
                };
                results.push(built);
            }
        }
    }
    results.pop().expect("a narrowing answers its pair")
}

/// Whether a record storing a field its own type has at `own` and that it holds at `old` drops it
/// where held at `new`: `new` names it, and `old` does not, or names it at a type other than the
/// exactly `Null` `new` names it at. A field its own type has at exactly `Null` holds no value to
/// drop.
fn dropped(own: Option<KType>, old: Option<KType>, new: Option<KType>) -> bool {
    own != Some(KType::NULL)
        && match (old, new) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(old), Some(new)) => old != KType::NULL && new == KType::NULL,
        }
}

/// Hand `child` each of `part`'s cells that may hide a field, at its index, beside its view under
/// `old` and under `new`: one whose bit is set, or whose view under `old` hides a field its own
/// type names. Answer whether `part` is a record that [drops](self::dropped) a field it stores.
fn opened<'cell, X: Knotted + 'cell>(
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
    part: Value<'cell, X>,
    old: KType,
    new: KType,
    child: &mut impl FnMut(usize, Value<'cell, X>, KType, KType),
) -> bool {
    let views = |cell: Value<'cell, X>, old: KType, new: KType| {
        let view = |declared: KType| Seen::of(cell).seen_at(declared, types, scratch).ktype();
        (view(old), view(new))
    };
    let mut enter = |at: usize, cell: Value<'cell, X>, old: KType, new: KType| {
        if !matches!(
            cell,
            Value::List(_) | Value::Dict(_) | Value::Record(_) | Value::Tagged(_)
        ) {
            return;
        }
        let (old, new) = views(cell, old, new);
        let (old, new) = (old.as_type().expect(SEEN), new.as_type().expect(SEEN));
        if part_hides(&cell) || hides_more(types, cell.concrete_ktype(), old) {
            child(at, cell, old, new);
        }
    };
    match part {
        Value::List(list) => {
            let (TypeNode::List { element: old }, TypeNode::List { element: new }) =
                (types.node(old), types.node(new))
            else {
                return false;
            };
            for (at, cell) in list.cells().iter().enumerate() {
                enter(at, *cell, old, new);
            }
            false
        }
        Value::Dict(dict) => {
            let (TypeNode::Dict { value: old, .. }, TypeNode::Dict { value: new, .. }) =
                (types.node(old), types.node(new))
            else {
                return false;
            };
            for (at, cell) in dict.cells().iter().enumerate() {
                enter(at, *cell, old, new);
            }
            false
        }
        Value::Record(record) => {
            let (TypeNode::Record { fields: old }, TypeNode::Record { fields: new }) =
                (types.node(old), types.node(new))
            else {
                return false;
            };
            let own = |name: Symbol| match types.node(record.ktype()) {
                TypeNode::Record { fields } => fields.get(name),
                _ => unreachable!("a record's own type is a record type"),
            };
            let mut copied = false;
            for (at, (name, cell)) in record.names().iter().zip(record.cells()).enumerate() {
                match (old.get(*name), new.get(*name)) {
                    (old, new) if dropped(own(*name), old, new) => copied = true,
                    (Some(old), Some(new)) if old != KType::NULL => enter(at, *cell, old, new),
                    _ => {}
                }
            }
            copied
        }
        Value::Tagged(tagged) => {
            if let (Some(old), Some(new)) = (
                representation(types, scratch, old),
                representation(types, scratch, new),
            ) {
                enter(0, *tagged.payload(), old, new);
            }
            false
        }
        _ => false,
    }
}

/// `cells`, laid as the cells of a container of type `ktype`, each held at its view there:
/// [`unhidden`] where it may hide a field. `cells` itself where none does.
pub(super) fn unhidden_cells<'cell, X: Knotted + 'cell>(
    writer: Writer<'cell, '_>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
    cells: &'cell [Value<'cell, X>],
    ktype: KType,
) -> &'cell [Value<'cell, X>] {
    if !cells.iter().any(part_hides) {
        return cells;
    }
    collect(
        writer,
        (cells.iter()).map(|cell| held_in(writer, types, scratch, *cell, ktype)),
    )
}

/// `cell`, a cell of a list or dict of type `ktype` — a knot's data node's value cell among them —
/// held at its view there: [`unhidden`] where it may hide a field.
pub fn held_in<'cell, X: Knotted + 'cell>(
    writer: Writer<'cell, '_>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
    cell: Value<'cell, X>,
    ktype: KType,
) -> Value<'cell, X> {
    if !part_hides(&cell) {
        return cell;
    }
    let element = match types.node(ktype) {
        TypeNode::List { element } => element,
        TypeNode::Dict { value, .. } => value,
        _ => unreachable!("a container's cells are its elements or values"),
    };
    let view = Seen::of(cell).seen_at(element, types, scratch).ktype();
    unhidden(writer, types, scratch, cell, view.as_type().expect(SEEN))
}

/// What a part is rebuilt over: its old and new views, whether it is a record to copy, and the
/// result of each entered child, by cell index.
struct Rebuilt<'a, 'cell, X> {
    old: KType,
    new: KType,
    copied: bool,
    children: &'a [usize],
    replaced: &'a [Option<Value<'cell, X>>],
}

/// `part` rebuilt over its entered children's results, at its own type
/// [narrowed](self::narrowed) by its old view: a copied record over the fields it stores that type
/// names and it does not drop, at that type less the fields it drops. Its bit is set where a cell
/// still hides a field at its view there.
fn rebuilt<'cell, X: Knotted + 'cell>(
    writer: Writer<'cell, '_>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
    part: Value<'cell, X>,
    at: &Rebuilt<'_, 'cell, X>,
) -> Value<'cell, X> {
    let (old, copied) = (at.old, at.copied);
    let cell = |index: usize, cell: Value<'cell, X>| match at.children.binary_search(&index) {
        Ok(entered) => at.replaced[entered].unwrap_or(cell),
        Err(_) => cell,
    };
    let rebuilt = match part {
        Value::List(list) => {
            let cells = collect(
                writer,
                list.cells().iter().enumerate().map(|(at, c)| cell(at, *c)),
            );
            let ktype = narrowed(types, scratch, list.ktype(), old);
            Value::List(List::weighed(writer, cells, ktype, types))
        }
        Value::Dict(dict) => {
            let cells = collect(
                writer,
                dict.cells().iter().enumerate().map(|(at, c)| cell(at, *c)),
            );
            Value::Dict(Dict::weighed(
                writer,
                dict.keys(),
                cells,
                narrowed(types, scratch, dict.ktype(), old),
                types,
            ))
        }
        Value::Record(record) => {
            let narrowed = narrowed(types, scratch, record.ktype(), old);
            let (
                TypeNode::Record { fields: own },
                TypeNode::Record { fields: held },
                TypeNode::Record { fields },
                TypeNode::Record { fields: new },
            ) = (
                types.node(record.ktype()),
                types.node(old),
                types.node(narrowed),
                types.node(at.new),
            )
            else {
                unreachable!("a record is seen at a record type")
            };
            let drops = |name: Symbol| dropped(own.get(name), held.get(name), new.get(name));
            let kept = |name: &Symbol| !copied || (fields.get(*name).is_some() && !drops(*name));
            let mut stored = BumpVec::with_capacity_in(record.names().len(), scratch);
            stored.extend(
                (record.names().iter().zip(record.cells()).enumerate())
                    .filter(|(_, (name, _))| kept(name))
                    .map(|(index, (name, c))| (*name, cell(index, *c))),
            );
            let names = collect(writer, stored.iter().map(|(name, _)| *name));
            let cells = collect(writer, stored.iter().map(|(_, c)| *c));
            let ktype = match copied && fields.keys().any(|name| drops(name.symbol())) {
                false => narrowed,
                true => {
                    let mut less = BumpVec::with_capacity_in(fields.len(), scratch);
                    less.extend(fields.iter().filter(|(name, _)| !drops(name.symbol())));
                    types.record(scratch, &less)
                }
            };
            Value::Record(Record::weighed(writer, names, cells, ktype, types))
        }
        // `hold` reads what the identity hides of its payload.
        Value::Tagged(tagged) => {
            return Value::Tagged(Tagged::hold(
                writer,
                cell(0, *tagged.payload()),
                tagged.ktype(),
                types,
            ));
        }
        _ => unreachable!("only a composite is entered"),
    };
    // `weighed` reads the cells' bits alone; a cell it kept may still hide a field at its view
    // under the rebuilt type, as the part did under its own, and a record may store a field its
    // type does not name.
    let own = rebuilt.concrete_ktype();
    let mut hides = match (rebuilt, types.node(own)) {
        (Value::Record(record), TypeNode::Record { fields }) => record
            .names()
            .iter()
            .any(|name| fields.get(*name).is_none()),
        _ => false,
    };
    opened(types, scratch, rebuilt, own, own, &mut |_, _, _, _| {
        hides = true
    });
    if hides {
        match rebuilt {
            Value::List(list) => list.hide(),
            Value::Dict(dict) => dict.hide(),
            Value::Record(record) => record.hide(),
            _ => {}
        }
    }
    rebuilt
}
