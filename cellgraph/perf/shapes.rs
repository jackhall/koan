//! The benchmark shapes: eight traversals of the substrate, each sized to the smallest `n` that
//! shows its trend.
//!
//! Every shape builds its own graph, drives public doors only, and ends with an `is_empty` assert —
//! a shape that leaks a sealed cell is wrong, not slow. Values are read back wherever a redeem
//! happens, so a wrong forward or a wrong reach shows up as a panic rather than as a cheap row.
//!
//! Two rules keep the rows meaning what the schema says. Every door call is wrapped in
//! [`measure`], including `read`. Bookkeeping the harness owns but must do inside a step — a
//! `Vec` of carriers, an operand list — is wrapped as [`Verb::Harness`] and reported as its own
//! row, so it neither hides nor inflates a real verb.

use cellgraph::{
    Active, CellGraph, CellHandle, CrossedOperand, Dormant, DropFree, Operand, Prices, Ready,
    Reattachable, ReleaseAbsorption, SlabHandle, StepContext, TreeHandle, Verdict, Writer,
    reattachable,
};

use crate::meter::{Verb, measure};

/// The cap every shape builds at: the full width of a one-word graph, which is the width the crate
/// ships at. One cap for the whole set, so a row's `cap` column is a constant across the record.
pub const CAP: u32 = 64;

/// The continuation family: an owned string, holding nothing a region owns.
pub struct Work;

/// The two value families, borrowing their cell's region storage.
struct Number;
struct Numbers;

reattachable!(
    Work => String,
    Number => &'cell u32,
    Numbers => &'cell [u32],
);

impl DropFree for Number {}
impl DropFree for Numbers {}

/// The always-pin embedder the roadmap names: every operand crosses pinned, so every placement
/// over operands pays the pricing walk. This is the shape that makes `pin_price` a per-verb cost.
fn always_pin(_: Prices) -> Verdict {
    Verdict::Pin
}

fn graph() -> CellGraph<'static, Work> {
    CellGraph::new(CAP, always_pin)
}

/// An operand priced above anything a pin can cost, so [`always_pin`] pins it whatever the slab is
/// doing.
fn pinned<'a, 'step, V: Reattachable<'static> + DropFree>(
    carrier: &'a Ready<'static, 'step, V>,
) -> Operand<'static, 'a, 'step, V> {
    Operand {
        carrier,
        copy_bytes: usize::MAX,
    }
}

/// One value, laid down through the writer's single run verb — the shape every embedder derives
/// its own one-value write from, since the substrate ships no such verb. No allocation of its own:
/// the `Option` is a stack slot the run of one empties.
fn one<'cell, T>(writer: Writer<'cell>, value: T) -> &'cell T {
    let mut value = Some(value);
    &writer.fill(1, |_| value.take().expect("a run of one fills once"))[0]
}

/// A value homed in the executing cell: the own-region write, then the bridge that makes it a
/// carrier. What every shape's `Verb::Alloc` row measures.
fn number_here<'step>(
    context: &StepContext<'static, 'step, '_, Work>,
    value: u32,
) -> Ready<'static, 'step, Number> {
    context.lift::<Number>(one(context.writer(), value))
}

fn build_number<'cell, 'severed>(
    writer: Writer<'cell>,
    views: &[CrossedOperand<'static, 'cell, 'severed, Number>],
) -> Active<'static, 'cell, Number> {
    Active::new(match views[0] {
        CrossedOperand::Pinned(value) | CrossedOperand::Copied(value) => one(writer, *value),
    })
}

/// Written straight out of the views: the run is filled by index, so the build needs no buffer of
/// its own and the placement is charged for nothing the harness did.
fn build_slice<'cell, 'severed>(
    writer: Writer<'cell>,
    views: &[CrossedOperand<'static, 'cell, 'severed, Number>],
) -> Active<'static, 'cell, Numbers> {
    Active::new(writer.fill(views.len(), |index| match views[index] {
        CrossedOperand::Pinned(value) | CrossedOperand::Copied(value) => *value,
    }))
}

/// Per-step value cost in one cell: a value kept at the end of every step and redeemed at the head
/// of the next, `n` times over, with nothing else in the graph.
fn keep_redeem(n: u32) {
    let mut graph = graph();
    let cell = measure(Verb::Create, || graph.create(None, None)).unwrap();

    let first = measure(Verb::Enter, || {
        graph.enter(cell, |context| {
            let value = measure(Verb::Alloc, || number_here(context, 0));
            measure(Verb::Keep, || context.keep(value))
        })
    })
    .unwrap();
    let mut resting = Some(first);

    for step in 1..=n {
        let held = resting.take().expect("the previous step put one to rest");
        let next = measure(Verb::Enter, || {
            graph.enter(cell, move |context| {
                let carrier = measure(Verb::Redeem, || context.redeem(held).unwrap());
                let value = measure(Verb::Read, || *context.read(&carrier).value());
                assert_eq!(value, step - 1);
                let next = measure(Verb::Alloc, || number_here(context, value + 1));
                measure(Verb::Keep, || context.keep(next))
            })
        })
        .unwrap();
        resting = Some(next);
    }

    measure(Verb::Release, || {
        graph.release(cell, ReleaseAbsorption::IntoHolder)
    })
    .unwrap();
    assert!(graph.is_empty());
}

/// Dormant interning against many shapes: `n` sources each build one value into `dest`, so every
/// keep in `dest` carries a reach no earlier keep did and the reach table holds `n` entries. Each
/// keep scans the entries before it, which is the linear term interning is priced at.
fn keep_shapes(n: u32) {
    let mut graph = graph();
    let dest = measure(Verb::Create, || graph.create(None, None)).unwrap();
    let mut sources: Vec<SlabHandle> = Vec::with_capacity(n as usize);
    let mut dormant: Vec<Dormant<'static, Number>> = Vec::with_capacity(n as usize);

    for i in 0..n {
        let source = measure(Verb::Create, || graph.create(None, None)).unwrap();
        sources.push(source);
        let resting = measure(Verb::Enter, || {
            graph.enter(source, |context| {
                let value = measure(Verb::Alloc, || number_here(context, i));
                let placed = measure(Verb::AllocInto, || {
                    context
                        .alloc_into::<Number, Number>(dest, &[pinned(&value)], build_number)
                        .unwrap()
                });
                measure(Verb::Keep, || context.keep(placed))
            })
        })
        .unwrap();
        dormant.push(resting);
    }

    measure(Verb::Enter, || {
        graph.enter(dest, |context| {
            for (i, resting) in dormant.drain(..).enumerate() {
                let carrier = measure(Verb::Redeem, || context.redeem(resting).unwrap());
                let value = measure(Verb::Read, || *context.read(&carrier).value());
                assert_eq!(value, i as u32);
            }
        })
    })
    .unwrap();

    for source in &sources {
        measure(Verb::Release, || {
            graph.release(*source, ReleaseAbsorption::IntoHolder)
        })
        .unwrap();
    }
    measure(Verb::Release, || {
        graph.release(dest, ReleaseAbsorption::IntoHolder)
    })
    .unwrap();
    assert!(graph.is_empty());
}

/// Death-time absorption: `n` single-consumer producers, each building straight into the consumer
/// and then dying, so every release finds a unique holder and folds into it.
fn push_chain(n: u32) {
    let mut graph = graph();
    let consumer = measure(Verb::Create, || graph.create(None, None)).unwrap();
    let mut dormant: Vec<Dormant<'static, Number>> = Vec::with_capacity(n as usize);

    for i in 0..n {
        let producer = measure(Verb::Create, || graph.create(None, None)).unwrap();
        let resting = measure(Verb::Enter, || {
            graph.enter(producer, |context| {
                let value = measure(Verb::Alloc, || number_here(context, i));
                let pushed = measure(Verb::AllocInto, || {
                    context
                        .alloc_into::<Number, Number>(consumer, &[pinned(&value)], build_number)
                        .unwrap()
                });
                measure(Verb::Keep, || context.keep(pushed))
            })
        })
        .unwrap();
        dormant.push(resting);
        measure(Verb::Release, || {
            graph.release(producer, ReleaseAbsorption::IntoHolder)
        })
        .unwrap();
    }

    measure(Verb::Enter, || {
        graph.enter(consumer, |context| {
            for (i, resting) in dormant.drain(..).enumerate() {
                let carrier = measure(Verb::Redeem, || context.redeem(resting).unwrap());
                let value = measure(Verb::Read, || *context.read(&carrier).value());
                assert_eq!(value, i as u32);
            }
        })
    })
    .unwrap();

    measure(Verb::Release, || {
        graph.release(consumer, ReleaseAbsorption::IntoHolder)
    })
    .unwrap();
    assert!(graph.is_empty());
}

/// The seal transition and sealed-cell retirement: `n` producers the consumer holds bare, each
/// refusing the merge on death so it seals, then read out of their sealed cells and wound down
/// together.
fn pull_chain(n: u32) {
    let mut graph = graph();
    let consumer = measure(Verb::Create, || graph.create(None, None)).unwrap();
    let mut dormant: Vec<Dormant<'static, Number>> = Vec::with_capacity(n as usize);

    for i in 0..n {
        let producer = measure(Verb::Create, || graph.create(None, None)).unwrap();
        let resting = measure(Verb::Enter, || {
            graph.enter(producer, |context| {
                let value = measure(Verb::Alloc, || number_here(context, i));
                measure(Verb::Keep, || context.keep(value))
            })
        })
        .unwrap();
        dormant.push(resting);

        measure(Verb::Enter, || {
            graph.enter(consumer, |context| {
                measure(Verb::Hold, || context.hold(producer).unwrap());
            })
        })
        .unwrap();
        measure(Verb::Release, || {
            graph.release(producer, ReleaseAbsorption::Refused)
        })
        .unwrap();
    }

    measure(Verb::Enter, || {
        graph.enter(consumer, |context| {
            for (i, resting) in dormant.drain(..).enumerate() {
                let carrier = measure(Verb::Redeem, || context.redeem(resting).unwrap());
                let value = measure(Verb::Read, || *context.read(&carrier).value());
                assert_eq!(value, i as u32);
            }
        })
    })
    .unwrap();

    measure(Verb::Release, || {
        graph.release(consumer, ReleaseAbsorption::IntoHolder)
    })
    .unwrap();
    assert!(graph.is_empty());
}

/// Dead ancestors waiting on a descendant: a chain of `n` cells each born under the last, released
/// outermost-first so every one of them stays dead-but-undisposed under the leaf's birth row, and
/// then the leaf — the single release that frees the whole chain in one walk up it.
fn birth_chain(n: u32) {
    let mut graph = graph();
    let mut handles: Vec<SlabHandle> = Vec::with_capacity(n as usize);

    let root = measure(Verb::Create, || graph.create(None, None)).unwrap();
    handles.push(root);
    for depth in 1..n {
        let parent = handles[depth as usize - 1];
        let child = measure(Verb::Create, || graph.create(Some(parent), None)).unwrap();
        handles.push(child);
    }

    // Every release but the last leaves its cell dead-but-undisposed: the leaf is still live, and
    // its birth row names the whole chain above it.
    for handle in &handles {
        measure(Verb::Release, || {
            graph.release(*handle, ReleaseAbsorption::IntoHolder)
        })
        .unwrap();
    }
    assert!(graph.is_empty());
}

/// One round of the fan-out: `m` values built in `source` and placed as one slice into `dest`.
fn fan_out_round(
    graph: &mut CellGraph<'static, Work>,
    source: SlabHandle,
    dest: SlabHandle,
    m: u32,
) -> Dormant<'static, Numbers> {
    measure(Verb::Enter, || {
        graph.enter(source, |context| {
            let mut values = measure(Verb::Harness, || Vec::with_capacity(m as usize));
            for i in 0..m {
                let value = measure(Verb::Alloc, || number_here(context, i));
                values.push(value);
            }
            let operands: Vec<Operand<'static, '_, '_, Number>> =
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
    let mut graph = graph();
    let source = measure(Verb::Create, || graph.create(None, None)).unwrap();
    let dest = measure(Verb::Create, || graph.create(None, None)).unwrap();

    let first = fan_out_round(&mut graph, source, dest, m);
    let second = fan_out_round(&mut graph, source, dest, m);

    measure(Verb::Enter, || {
        graph.enter(dest, |context| {
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
        graph.release(source, ReleaseAbsorption::IntoHolder)
    })
    .unwrap();
    measure(Verb::Release, || {
        graph.release(dest, ReleaseAbsorption::IntoHolder)
    })
    .unwrap();
    assert!(graph.is_empty());
}

/// The sealed-tier cascade: `n` bases held from two branches, so each seals rather than merges,
/// their sealed cells unioned into one placement, then both branches wound down so every sealed
/// cell's holder count falls to zero at once.
fn shared_subtier(n: u32) {
    let mut graph = graph();
    let left = measure(Verb::Create, || graph.create(None, None)).unwrap();
    let right = measure(Verb::Create, || graph.create(None, None)).unwrap();

    let mut bases: Vec<SlabHandle> = Vec::with_capacity(n as usize);
    for _ in 0..n {
        bases.push(measure(Verb::Create, || graph.create(None, None)).unwrap());
    }

    let mut dormant: Vec<Dormant<'static, Number>> = Vec::with_capacity(n as usize);
    for (i, base) in bases.iter().enumerate() {
        let resting = measure(Verb::Enter, || {
            graph.enter(*base, |context| {
                let value = measure(Verb::Alloc, || number_here(context, i as u32));
                measure(Verb::Keep, || context.keep(value))
            })
        })
        .unwrap();
        dormant.push(resting);
    }

    for branch in [left, right] {
        measure(Verb::Enter, || {
            graph.enter(branch, |context| {
                for base in &bases {
                    measure(Verb::Hold, || context.hold(*base).unwrap());
                }
            })
        })
        .unwrap();
    }

    for base in &bases {
        measure(Verb::Release, || {
            graph.release(*base, ReleaseAbsorption::Refused)
        })
        .unwrap();
    }

    measure(Verb::Enter, || {
        graph.enter(right, |context| {
            let mut carriers = measure(Verb::Harness, || Vec::with_capacity(n as usize));
            for (i, resting) in dormant.drain(..).enumerate() {
                let carrier = measure(Verb::Redeem, || context.redeem(resting).unwrap());
                let value = measure(Verb::Read, || *context.read(&carrier).value());
                assert_eq!(value, i as u32);
                carriers.push(carrier);
            }
            let operands: Vec<Operand<'static, '_, '_, Number>> =
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
        graph.release(left, ReleaseAbsorption::IntoHolder)
    })
    .unwrap();
    measure(Verb::Release, || {
        graph.release(right, ReleaseAbsorption::IntoHolder)
    })
    .unwrap();
    assert!(graph.is_empty());
}

/// One benchmark: the traversal, the name its rows carry, and the two sizes it is swept at.
pub struct Shape {
    pub name: &'static str,
    pub run: fn(u32),
    pub small: u32,
    pub large: u32,
}

/// The tree habitat: a root plus `n` nested tree cells, each building a value, pinning it into its
/// parent, and dying — the call-subtree shape the slab cannot hold, since none of it takes a slot.
///
/// The trend this reads is the one the habitat exists for: creation, entry and death are flat per
/// level whatever the depth, because a tree cell takes no row, no column and no reach table, and
/// its death is a bump splice into the ancestor it pledged.
fn tree_chain(n: u32) {
    let mut graph = graph();
    let root = measure(Verb::Create, || graph.create(None, None)).unwrap();

    let mut chain: Vec<TreeHandle> = Vec::with_capacity(n as usize);
    let mut parent = CellHandle::Slab(root);
    for _ in 0..n {
        let cell = measure(Verb::CreateTree, || graph.create_tree(parent, None)).unwrap();
        chain.push(cell);
        parent = CellHandle::Tree(cell);
    }

    // Innermost outward: each level redeems what its child pinned into it, adds one, pins the
    // result into its own parent, and dies — so every level's bump splices one step up the chain.
    let mut carried: Option<Dormant<'static, Number>> = None;
    for level in (0..n as usize).rev() {
        let cell = chain[level];
        let up = match level {
            0 => CellHandle::Slab(root),
            _ => CellHandle::Tree(chain[level - 1]),
        };
        let taken = carried.take();
        carried = Some(
            measure(Verb::EnterTree, || {
                graph.enter(cell, |context| {
                    let value = match taken {
                        None => measure(Verb::Alloc, || number_here(context, 0)),
                        Some(resting) => {
                            let carrier =
                                measure(Verb::Redeem, || context.redeem(resting).unwrap());
                            let seen = measure(Verb::Read, || *context.read(&carrier).value());
                            measure(Verb::Alloc, || number_here(context, seen + 1))
                        }
                    };
                    let placed = measure(Verb::AllocInto, || {
                        context
                            .alloc_into::<Number, Number>(up, &[pinned(&value)], build_number)
                            .unwrap()
                    });
                    measure(Verb::Keep, || context.keep(placed))
                })
            })
            .unwrap(),
        );
        measure(Verb::ReleaseTree, || graph.release_tree(cell)).unwrap();
    }

    let read = measure(Verb::Enter, || {
        graph.enter(root, |context| {
            let carrier = measure(Verb::Redeem, || {
                context
                    .redeem(carried.take().expect("the chain left a dormant carrier"))
                    .unwrap()
            });
            measure(Verb::Read, || *context.read(&carrier).value())
        })
    })
    .unwrap();
    assert_eq!(read, n - 1);

    measure(Verb::Release, || {
        graph.release(root, ReleaseAbsorption::IntoHolder)
    })
    .unwrap();
    assert!(graph.is_empty());
}

/// Every shape. Each is swept at both its sizes, so the per-unit trend is the difference over the
/// difference in `n` — a term needs both readings.
pub const SHAPES: [Shape; 8] = [
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
    Shape {
        name: "keep_shapes",
        run: keep_shapes,
        small: 8,
        large: 32,
    },
    Shape {
        name: "tree_chain",
        run: tree_chain,
        small: 64,
        large: 256,
    },
];
