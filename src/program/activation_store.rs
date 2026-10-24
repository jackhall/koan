//! A frame's **activation store**: what a frame decides in its own activation at a binding event
//! whose names its arena node's facts leave unbound
//! ([build/README.md § Resumption](../build/README.md#resumption)).
//!
//! The binding is read off the frame's slots: a type value binds its type, and a module value its
//! carried type over its content, by digest. The walk is the language's, rooted at the frame's
//! body, over a layer whose base is everything the frame read below: the load, the settlements
//! between, and the events its activation decided before. It walks at the step's borrow in a
//! scratch bump; the [records copier](crate::scope::Activated) lays what it wrote in the frame's
//! region, and it interns through the frame's own chain: the region's, or a caching frame's
//! scratch, whose exit adopts what the records name as it adopts what its values reach. No arena
//! byte is involved, and nothing reaches the agent tier.

use crate::build::Layer;
use std::fmt::Write as _;

use crate::memory::{Bump, BumpAllocator, BumpVec, Lays, Writer, collect, resident};
use crate::scope::{Activated, Binding, Bound, Facts, RecordSource, Segment, Tree, Unknown};
use crate::values::{Chain, Knotted, Value};

use super::record::{Language, Program};
use super::settle::Text;

/// What a frame's slot holding `value` binds its unknown to: a type value its type, and a module
/// value its carried type over its content, by digest. `None` for any other value.
pub fn bound_of<'o, X: Knotted>(
    lay: impl Lays<'o>,
    value: Value<'_, X>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Option<Bound<'o>> {
    if let Some(named) = value.as_type() {
        return Some(Bound::Type(named.handle()));
    }
    value.as_module()?;
    Some(Bound::Module {
        carried: value.concrete_ktype(),
        tree: resident(lay, Tree::Digest(value.digest(types, scratch))),
    })
}

#[cfg(test)]
thread_local! {
    /// How many binding events walked: what a test reads to see a frame decide, or not.
    pub static DECIDED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The walk a frame's binding event decides through, the language's monomorphized in:
/// [`activated`].
pub(super) type Activate<'graph> =
    for<'here, 'w, 'a, 'bind> fn(
        &Program<'graph>,
        Writer<'here, 'w>,
        Facts<'a>,
        &Chain<'_, '_, '_, '_>,
        Option<&'here Activated<'here>>,
        &[(Unknown, Bound<'bind>)],
        Segment,
    ) -> Result<Activated<'here>, &'here str>;

/// [`activated`] laying through a frame's region writer: what the program holds.
pub(super) fn activated_in<'here, L: Language>(
    program: &Program<'_>,
    writer: Writer<'here, '_>,
    facts: Facts<'_>,
    types: &Chain<'_, '_, '_, '_>,
    below: Option<&'here Activated<'here>>,
    own: &[(Unknown, Bound<'_>)],
    at: Segment,
) -> Result<Activated<'here>, &'here str> {
    activated::<L>(program, writer, facts, types, below, own, at)
}

/// What the binding event binding `own` decides for the frame body `at` over `facts`, which read
/// `below`, the frame's earlier events: `L`'s walk rooted at the body, laid through `lay` and
/// interned through `types`, the frame's chain. A refusal the walk reaches is the message the
/// frame faults with, worded as the load words it.
pub(crate) fn activated<'here, L: Language>(
    program: &Program<'_>,
    lay: impl Lays<'here>,
    facts: Facts<'_>,
    types: &Chain<'_, '_, '_, '_>,
    below: Option<&'here Activated<'here>>,
    own: &[(Unknown, Bound<'_>)],
    at: Segment,
) -> Result<Activated<'here>, &'here str> {
    #[cfg(test)]
    DECIDED.with(|decided| decided.set(decided.get() + 1));
    let (bump, scratch) = (Bump::new(), Bump::new());
    let mut staged = BumpVec::new_in(&scratch);
    facts.each_bound(&mut |unknown, bound| staged.push((unknown, bound)));
    staged.extend(own.iter().copied());
    let binding = Binding::laid(&bump, &mut staged);
    let layer = Layer::new(&bump, &scratch, facts, binding);
    let walked = L::facts(
        program.shapes(),
        program.builtins(),
        &layer,
        types,
        &scratch,
        at,
    );
    match walked {
        Ok(()) => Ok(Activated::of(
            lay,
            &scratch,
            program.shapes(),
            below,
            binding,
            own,
            layer.writes(),
        )),
        Err(error) => {
            let mut text = Text(BumpVec::new_in(&scratch));
            write!(text, "{}", error.display(program.symbols(), types)).expect("a refusal renders");
            let laid = collect(lay, text.0.iter().copied());
            Err(std::str::from_utf8(laid).expect("rendered as UTF-8"))
        }
    }
}
