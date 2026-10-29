//! A read: a list, a dict or a string at an application's head, applied to its argument. A list or
//! a string applied to a `Number` reads the element or code point at that zero-based index, and
//! applied to a slice record — one naming `from` or `to` — slices it; a dict applied to a key looks
//! the key up. Every read answers the builtin `Result`: `Ok` of what it read, or `Error` of the
//! index, the first bound or the key that failed.
//!
//! A slice is half-open and zero-based. With `to` below `from`, or at -1, it runs backward from
//! `from` to just past `to`, -1 ≤ `to` ≤ `from` ≤ len - 1; otherwise forward, 0 ≤ `from` ≤ `to` ≤
//! len. An open end defaults `from` to 0 and `to` to the length. Its bounds are checked in order:
//! each an integer, `from` first, then the direction read off both, then `from`'s range for that
//! direction, then `to`'s. A slice is a view ([`List::sliced`], [`Text::anchored`]).
//!
//! The evaluator builds a read in the evaluation's home over its head and argument, as it builds a
//! construction ([evaluate.rs](super::evaluate)); the load types it ([walk/lookups.rs](walk/lookups.rs)).
//! An argument the load could not refuse — a record naming neither bound, a bound or an index of
//! another type, a dict key no dict holds — is an error value here.

use crate::knot::KValue;
use crate::memory::{BumpAllocator, Writer};
use crate::program::Program;
use crate::symbols::{Symbol, TypeSymbol, ValueSymbol};
use crate::values::{Chain, Circular, Key, List, Seen, Tagged, Text, Value, construction};

use super::errors::Raised;

/// The `Result` variant `name` names: `Ok` or `Error`.
fn variant_name(name: &str) -> Symbol {
    TypeSymbol::classify(name).expect("a Type token").symbol()
}

/// The slice bound `name` names: `from` or `to`.
pub(super) fn bound(name: &str) -> Symbol {
    ValueSymbol::classify(name).expect("a value name").symbol()
}

/// What a read's head is.
#[derive(Clone, Copy)]
enum Head<'cell> {
    List,
    Dict,
    Str(Text<'cell>),
}

/// What `head` is read as, if it is a list, a dict or a string, plain or a knot's data node.
fn head<'cell>(head: KValue<'_, 'cell>) -> Option<Head<'cell>> {
    match head {
        Value::List(_) => Some(Head::List),
        Value::Dict(_) => Some(Head::Dict),
        _ => match (head.as_text(), head.as_circular()) {
            (Some(text), _) => Some(Head::Str(text)),
            (_, Some((_, Circular::List(_)))) => Some(Head::List),
            (_, Some((_, Circular::Dict(_)))) => Some(Head::Dict),
            _ => None,
        },
    }
}

/// Whether `value` at an application's head is read by its argument rather than called.
pub(super) fn readable(value: KValue<'_, '_>) -> bool {
    head(value).is_some()
}

/// `head`, a [`readable`] value, read by `argument`, in `writer`'s region: the `Result` of
/// the read, or an error value where the argument reads nothing.
pub(super) fn read<'graph, 'cell>(
    program: &Program<'graph>,
    writer: Writer<'cell, '_>,
    types: &Chain<'_, 'cell, '_, '_>,
    container: KValue<'graph, 'cell>,
    argument: KValue<'graph, 'cell>,
    scratch: BumpAllocator<'_>,
) -> KValue<'graph, 'cell> {
    let variant = |name| {
        types
            .union_member_named(program.result_type(), variant_name(name))
            .expect("the builtin `Result` names both variants")
    };
    let answer = |name, value: KValue<'graph, 'cell>| {
        let identity = construction(types, scratch, variant(name), value.concrete_ktype())
            .expect("a `Result` variant wraps any value");
        Value::Tagged(Tagged::hold(writer, value, identity, types))
    };
    let unreadable = || {
        Raised::Unreadable {
            head: container.concrete_ktype(),
            argument: argument.concrete_ktype(),
        }
        .raise(program, writer, types)
    };
    let shape = head(container).expect("a read's head is readable");
    if let Head::Dict = shape {
        let key = match Key::of(&argument, writer) {
            Ok(key) => key,
            Err(rejected) => return Raised::NotAKey { rejected }.raise(program, writer, types),
        };
        let entries = container.surface(types, scratch).expect("a dict opens");
        return match entries.key_index(&key) {
            Some(at) => answer(
                "Ok",
                entries.child(at, types, scratch).restamped(writer, types),
            ),
            None => answer("Error", argument),
        };
    }
    if let Value::Number(index) = argument {
        let at = whole(index).and_then(|at| usize::try_from(at).ok());
        let read = at.and_then(|at| match shape {
            Head::Str(text) => text.code_point(writer, at),
            _ => {
                let items = container.surface(types, scratch).expect("a list opens");
                (at < items.len()).then(|| items.child(at, types, scratch).restamped(writer, types))
            }
        });
        return match read {
            Some(value) => answer("Ok", value),
            None => answer("Error", argument),
        };
    }
    // A record naming `from` or `to` is a slice record, whatever else it names.
    let named =
        |name| (Seen::of(argument).field(bound(name), types, scratch)).map(|seen| seen.value());
    let (from, to) = (named("from"), named("to"));
    if from.is_none() && to.is_none() {
        return unreadable();
    }
    let number = |bound: Option<KValue<'graph, 'cell>>, open: f64| match bound {
        None => Some(open),
        Some(Value::Number(number)) => Some(number),
        Some(_) => None,
    };
    // A string is counted, and its crumbs laid, in the one scan its slice needs.
    let anchored = match shape {
        Head::Str(text) => Some(text.anchored(writer, scratch)),
        _ => None,
    };
    let len = match anchored {
        Some(anchored) => anchored.len(),
        None => container
            .surface(types, scratch)
            .expect("a list opens")
            .len(),
    };
    let (Some(from), Some(to)) = (number(from, 0.0), number(to, len as f64)) else {
        return unreadable();
    };
    let (from, to) = match (whole(from), whole(to)) {
        (None, _) => return answer("Error", Value::Number(from)),
        (_, None) => return answer("Error", Value::Number(to)),
        (Some(from), Some(to)) => (from, to),
    };
    let len = len as i64;
    let backward = to < from || to == -1;
    let (from_range, to_range) = match backward {
        true => (-1..=len - 1, -1..=from),
        false => (0..=len, from..=len),
    };
    if !from_range.contains(&from) {
        return answer("Error", Value::Number(from as f64));
    }
    if !to_range.contains(&to) {
        return answer("Error", Value::Number(to as f64));
    }
    // A backward slice holds `from` down to just past `to`: the forward run `[to + 1, from + 1)`.
    let range = match backward {
        true => ((to + 1) as usize, (from + 1) as usize),
        false => (from as usize, to as usize),
    };
    answer(
        "Ok",
        match anchored {
            Some(anchored) => anchored.sliced(writer, range, backward),
            None => Value::List(List::sliced(writer, container, range, backward, types)),
        },
    )
}

/// `number` as a whole number, where it is one.
fn whole(number: f64) -> Option<i64> {
    (number.is_finite() && number.fract() == 0.0 && number.abs() < 2f64.powi(53))
        .then_some(number as i64)
}
