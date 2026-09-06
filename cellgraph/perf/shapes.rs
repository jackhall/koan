//! The benchmark shapes: six traversals of the substrate, each sized to the smallest `n` that
//! shows its trend.
//!
//! Every shape builds its own table, drives public doors only, and ends with an `is_empty` assert
//! — a shape that leaks a record is wrong, not slow. Values are read back wherever a redeem
//! happens, so a wrong forward or a wrong reach shows up as a panic rather than as a cheap row.
//!
//! Two rules keep the rows meaning what the schema says. Every door call is wrapped in
//! [`measure`], including `read`. Bookkeeping the harness owns but must do inside a step — a
//! `Vec` of carriers, an operand list — is wrapped as [`Verb::Harness`] and reported as its own
//! row, so it neither hides nor inflates a real verb.

use cellgraph::{
    Absorption, CellTable, Crossed, Crossing, DropFree, Handle, Operand, Reattachable, Resident,
    Sealed, Verdict, Writer, reattachable,
};

use crate::meter::{Verb, measure};

/// The slab width every shape builds at. One cap for the whole set, so a row's `cap` column is a
/// constant today and a dimension once the compile-time slab width lands.
pub const CAP: u32 = 128;

/// The widest operand list any shape passes, and so the stack buffer a slice build writes into: a
/// build closure that allocated would charge the placement for the harness's own work.
const MAX_OPERANDS: usize = 32;

/// The continuation family: an owned string, holding nothing a region owns.
pub struct Work;

/// The two value families, borrowing their cell's region storage.
struct Number;
struct Numbers;

reattachable!(
    Work => String,
    Number => &'r u32,
    Numbers => &'r [u32],
);

impl DropFree for Number {}
impl DropFree for Numbers {}

/// The always-pin embedder the roadmap names: every operand crosses pinned, so every placement
/// over operands pays the pricing walk. This is the shape that makes `pin_price` a per-verb cost.
fn always_pin(_: Crossing) -> Verdict {
    Verdict::Pin
}

fn table() -> CellTable<Work> {
    CellTable::new(CAP, always_pin)
}

/// An operand priced above anything a pin can cost, so [`always_pin`] pins it whatever the slab is
/// doing.
fn pinned<'a, 'b, V: Reattachable + DropFree>(carrier: &'a Sealed<'b, V>) -> Operand<'a, 'b, V> {
    Operand {
        carrier,
        copy_bytes: usize::MAX,
    }
}

fn build_number<'r, 'v>(writer: Writer<'r>, views: &[Crossed<'r, 'v, Number>]) -> &'r u32 {
    match views[0] {
        Crossed::Pinned(value) | Crossed::Copied(value) => writer.value(*value),
    }
}

fn build_slice<'r, 'v>(writer: Writer<'r>, views: &[Crossed<'r, 'v, Number>]) -> &'r [u32] {
    let mut buffer = [0u32; MAX_OPERANDS];
    for (cell, view) in buffer.iter_mut().zip(views) {
        *cell = match view {
            Crossed::Pinned(value) | Crossed::Copied(value) => **value,
        };
    }
    writer.slice(&buffer[..views.len()])
}

/// Per-step value cost in one cell: a value kept at the end of every step and redeemed at the head
/// of the next, `n` times over, with nothing else in the table.
fn keep_redeem(n: u32) {
    let mut table = table();
    let cell = measure(Verb::Create, || table.create(None, None)).unwrap();

    let first = measure(Verb::Enter, || {
        table.enter(cell, |context| {
            let value = measure(Verb::Alloc, || {
                context.alloc::<Number>(|writer| writer.value(0))
            });
            measure(Verb::Keep, || context.keep(value))
        })
    })
    .unwrap();
    let mut resting = Some(first);

    for step in 1..=n {
        let held = resting.take().expect("the previous step put one to rest");
        let next = measure(Verb::Enter, || {
            table.enter(cell, move |context| {
                let carrier = measure(Verb::Redeem, || context.redeem(held).unwrap());
                let value = measure(Verb::Read, || *context.read(&carrier).value());
                assert_eq!(value, step - 1);
                let next = measure(Verb::Alloc, || {
                    context.alloc::<Number>(|writer| writer.value(value + 1))
                });
                measure(Verb::Keep, || context.keep(next))
            })
        })
        .unwrap();
        resting = Some(next);
    }

    measure(Verb::Release, || {
        table.release(cell, Absorption::IntoHolder)
    })
    .unwrap();
    assert!(table.is_empty());
}

/// Death-time absorption: `n` single-consumer producers, each building straight into the consumer
/// and then dying, so every release finds a unique holder and folds into it.
fn push_chain(n: u32) {
    let mut table = table();
    let consumer = measure(Verb::Create, || table.create(None, None)).unwrap();
    let mut residents: Vec<Resident<Number>> = Vec::with_capacity(n as usize);

    for i in 0..n {
        let producer = measure(Verb::Create, || table.create(None, None)).unwrap();
        let resting = measure(Verb::Enter, || {
            table.enter(producer, |context| {
                let value = measure(Verb::Alloc, || {
                    context.alloc::<Number>(|writer| writer.value(i))
                });
                let pushed = measure(Verb::AllocInto, || {
                    context
                        .alloc_into::<Number, Number>(consumer, &[pinned(&value)], build_number)
                        .unwrap()
                });
                measure(Verb::Keep, || context.keep(pushed))
            })
        })
        .unwrap();
        residents.push(resting);
        measure(Verb::Release, || {
            table.release(producer, Absorption::IntoHolder)
        })
        .unwrap();
    }

    measure(Verb::Enter, || {
        table.enter(consumer, |context| {
            for (i, resting) in residents.drain(..).enumerate() {
                let carrier = measure(Verb::Redeem, || context.redeem(resting).unwrap());
                let value = measure(Verb::Read, || *context.read(&carrier).value());
                assert_eq!(value, i as u32);
            }
        })
    })
    .unwrap();

    measure(Verb::Release, || {
        table.release(consumer, Absorption::IntoHolder)
    })
    .unwrap();
    assert!(table.is_empty());
}

/// The seal transition and record retirement: `n` producers the consumer holds bare, each refusing
/// the merge on death so it seals, then read out of their records and wound down together.
fn pull_chain(n: u32) {
    let mut table = table();
    let consumer = measure(Verb::Create, || table.create(None, None)).unwrap();
    let mut residents: Vec<Resident<Number>> = Vec::with_capacity(n as usize);

    for i in 0..n {
        let producer = measure(Verb::Create, || table.create(None, None)).unwrap();
        let resting = measure(Verb::Enter, || {
            table.enter(producer, |context| {
                let value = measure(Verb::Alloc, || {
                    context.alloc::<Number>(|writer| writer.value(i))
                });
                measure(Verb::Keep, || context.keep(value))
            })
        })
        .unwrap();
        residents.push(resting);

        measure(Verb::Enter, || {
            table.enter(consumer, |context| {
                measure(Verb::Hold, || context.hold(producer).unwrap());
            })
        })
        .unwrap();
        measure(Verb::Release, || {
            table.release(producer, Absorption::Refused)
        })
        .unwrap();
    }

    measure(Verb::Enter, || {
        table.enter(consumer, |context| {
            for (i, resting) in residents.drain(..).enumerate() {
                let carrier = measure(Verb::Redeem, || context.redeem(resting).unwrap());
                let value = measure(Verb::Read, || *context.read(&carrier).value());
                assert_eq!(value, i as u32);
            }
        })
    })
    .unwrap();

    measure(Verb::Release, || {
        table.release(consumer, Absorption::IntoHolder)
    })
    .unwrap();
    assert!(table.is_empty());
}

/// Dead ancestors waiting on a descendant: a chain of `n` cells each born under the last,
/// released outermost-first so every one of them stays dead-resident under the leaf's birth row,
/// and then the leaf — the single release that frees the whole chain in one walk up it.
fn birth_chain(n: u32) {
    let mut table = table();
    let mut handles: Vec<Handle> = Vec::with_capacity(n as usize);

    let root = measure(Verb::Create, || table.create(None, None)).unwrap();
    handles.push(root);
    for depth in 1..n {
        let parent = handles[depth as usize - 1];
        let child = measure(Verb::Create, || table.create(Some(parent), None)).unwrap();
        handles.push(child);
    }

    // Every release but the last leaves its cell dead-resident: the leaf is still live, and its
    // birth row names the whole chain above it.
    for handle in &handles {
        measure(Verb::Release, || {
            table.release(*handle, Absorption::IntoHolder)
        })
        .unwrap();
    }
    assert!(table.is_empty());
}

/// One round of the fan-out: `m` values built in `source` and placed as one slice into `dest`.
fn fan_out_round(
    table: &mut CellTable<Work>,
    source: Handle,
    dest: Handle,
    m: u32,
) -> Resident<Numbers> {
    measure(Verb::Enter, || {
        table.enter(source, |context| {
            let mut values = measure(Verb::Harness, || Vec::with_capacity(m as usize));
            for i in 0..m {
                let value = measure(Verb::Alloc, || {
                    context.alloc::<Number>(|writer| writer.value(i))
                });
                values.push(value);
            }
            let operands: Vec<Operand<'_, '_, Number>> =
                measure(Verb::Harness, || values.iter().map(pinned).collect());
            let placed = measure(Verb::AllocInto, || {
                context
                    .alloc_into::<Numbers, Number>(dest, &operands, build_slice)
                    .unwrap()
            });
            measure(Verb::Keep, || context.keep(placed))
        })
    })
    .unwrap()
}

/// The pricing walk, twice: the first round pays it on every operand, the second finds `dest`
/// already naming everything the operands reach — the covered case a placement into a cell that
/// already holds the home is, and the one an always-pin embedder pays per operand.
fn fan_out(m: u32) {
    let mut table = table();
    let source = measure(Verb::Create, || table.create(None, None)).unwrap();
    let dest = measure(Verb::Create, || table.create(None, None)).unwrap();

    let first = fan_out_round(&mut table, source, dest, m);
    let second = fan_out_round(&mut table, source, dest, m);

    measure(Verb::Enter, || {
        table.enter(dest, |context| {
            for resting in [first, second] {
                let carrier = measure(Verb::Redeem, || context.redeem(resting).unwrap());
                // The view is handed straight out of the measured window: copying it into a
                // `Vec` first would charge `read` for the harness's own allocation.
                let values = measure(Verb::Read, || context.read(&carrier).value());
                assert_eq!(values.len(), m as usize);
                assert!(values.iter().copied().eq(0..m));
            }
        })
    })
    .unwrap();

    measure(Verb::Release, || {
        table.release(source, Absorption::IntoHolder)
    })
    .unwrap();
    measure(Verb::Release, || {
        table.release(dest, Absorption::IntoHolder)
    })
    .unwrap();
    assert!(table.is_empty());
}

/// The sealed-tier cascade: `n` bases held from two branches, so each seals rather than merges,
/// their records unioned into one placement, then both branches wound down so every record's
/// holder count falls to zero at once.
fn shared_subtier(n: u32) {
    let mut table = table();
    let left = measure(Verb::Create, || table.create(None, None)).unwrap();
    let right = measure(Verb::Create, || table.create(None, None)).unwrap();

    let mut bases: Vec<Handle> = Vec::with_capacity(n as usize);
    for _ in 0..n {
        bases.push(measure(Verb::Create, || table.create(None, None)).unwrap());
    }

    let mut residents: Vec<Resident<Number>> = Vec::with_capacity(n as usize);
    for (i, base) in bases.iter().enumerate() {
        let resting = measure(Verb::Enter, || {
            table.enter(*base, |context| {
                let value = measure(Verb::Alloc, || {
                    context.alloc::<Number>(|writer| writer.value(i as u32))
                });
                measure(Verb::Keep, || context.keep(value))
            })
        })
        .unwrap();
        residents.push(resting);
    }

    for branch in [left, right] {
        measure(Verb::Enter, || {
            table.enter(branch, |context| {
                for base in &bases {
                    measure(Verb::Hold, || context.hold(*base).unwrap());
                }
            })
        })
        .unwrap();
    }

    for base in &bases {
        measure(Verb::Release, || table.release(*base, Absorption::Refused)).unwrap();
    }

    measure(Verb::Enter, || {
        table.enter(right, |context| {
            let mut carriers = measure(Verb::Harness, || Vec::with_capacity(n as usize));
            for (i, resting) in residents.drain(..).enumerate() {
                let carrier = measure(Verb::Redeem, || context.redeem(resting).unwrap());
                let value = measure(Verb::Read, || *context.read(&carrier).value());
                assert_eq!(value, i as u32);
                carriers.push(carrier);
            }
            let operands: Vec<Operand<'_, '_, Number>> =
                measure(Verb::Harness, || carriers.iter().map(pinned).collect());
            let placed = measure(Verb::AllocInto, || {
                context
                    .alloc_into::<Numbers, Number>(right, &operands, build_slice)
                    .unwrap()
            });
            measure(Verb::Keep, || context.keep(placed));
        })
    })
    .unwrap();

    measure(Verb::Release, || {
        table.release(left, Absorption::IntoHolder)
    })
    .unwrap();
    measure(Verb::Release, || {
        table.release(right, Absorption::IntoHolder)
    })
    .unwrap();
    assert!(table.is_empty());
}

/// One benchmark: the traversal, the name its rows carry, and the two sizes it is swept at.
pub struct Shape {
    pub name: &'static str,
    pub run: fn(u32),
    pub small: u32,
    pub large: u32,
}

/// Every shape. Each is swept at both its sizes, so the per-unit trend is the difference over the
/// difference in `n` — a term needs both readings.
pub const SHAPES: [Shape; 6] = [
    Shape {
        name: "keep_redeem",
        run: keep_redeem,
        small: 16,
        large: 64,
    },
    Shape {
        name: "push_chain",
        run: push_chain,
        small: 8,
        large: 32,
    },
    Shape {
        name: "pull_chain",
        run: pull_chain,
        small: 8,
        large: 32,
    },
    Shape {
        name: "birth_chain",
        run: birth_chain,
        small: 8,
        large: 32,
    },
    Shape {
        name: "fan_out",
        run: fan_out,
        small: 4,
        large: 16,
    },
    Shape {
        name: "shared_subtier",
        run: shared_subtier,
        small: 8,
        large: 32,
    },
];
